use super::HiveMcp;
use super::params::{LogToolParams, ReadMessagesParams, SendMessageParams};
use crate::logging::LogDb;
use crate::types::{AgentRole, Message, MessageType};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = message_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(description = "Send a message to another agent")]
    pub(crate) async fn hive_send_message(
        &self,
        params: Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let msg_type = match p.message_type.as_str() {
            "info" => MessageType::Info,
            "request" => MessageType::Request,
            "status" => MessageType::Status,
            "task-suggestion" => MessageType::TaskSuggestion,
            _ => MessageType::Info,
        };

        let msg_id = format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let message = Message {
            id: msg_id.clone(),
            from: self.agent_id.clone(),
            to: p.to.clone(),
            timestamp: Utc::now(),
            message_type: msg_type,
            body: p.body.clone(),
            refs: p.refs.clone(),
        };

        // Validate routing
        let sender_role = self.agent_role();
        match sender_role {
            AgentRole::Worker => {
                // Workers can only message their parent lead
                let sender = self.state().load_agent(&self.run_id, &self.agent_id).ok();
                let parent = sender.and_then(|a| a.parent);
                if parent.as_deref() != Some(p.to.as_str()) {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Workers can only send messages to their lead.",
                    )]));
                }
            }
            AgentRole::Lead => {
                // Leads can message their workers or the coordinator
                let target = self.state().load_agent(&self.run_id, &p.to).ok();
                let valid = match target {
                    Some(ref t) if t.parent.as_deref() == Some(self.agent_id.as_str()) => true,
                    _ if p.to == "coordinator" => true,
                    _ => false,
                };
                if !valid {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Leads can only message their workers or the coordinator.",
                    )]));
                }
            }
            AgentRole::Coordinator => {
                // Coordinator can only message leads
                let target = self.state().load_agent(&self.run_id, &p.to).ok();
                if !matches!(target, Some(ref t) if matches!(t.role, AgentRole::Lead | AgentRole::Explorer | AgentRole::Evaluator))
                {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Coordinator can only send messages to leads, explorers, and evaluators.",
                    )]));
                }
            }
            AgentRole::Reviewer
            | AgentRole::Planner
            | AgentRole::Postmortem
            | AgentRole::Explorer
            | AgentRole::Evaluator => {
                // Reviewers/Planner/Postmortem/Explorer/Evaluator can only message the coordinator
                if p.to != "coordinator" {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "This role can only send messages to the coordinator.",
                    )]));
                }
            }
        }

        let state = self.state();
        if let Err(e) = state.save_message(&self.run_id, &message) {
            return Ok(CallToolResult::error(vec![Content::text(e)]));
        }

        self.append_event(
            "message_created",
            &msg_id,
            &format!("from {} to {}", self.agent_id, p.to),
        );

        // Auto-wake: if target agent is idle with a session_id, resume it
        let wake_info = self.try_wake_agent(&p.to, &p.body);

        let wake_suffix = wake_info.unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Sent message '{msg_id}' to '{}'{wake_suffix}",
            p.to
        ))]))
    }

    #[tool(
        description = "Read messages sent to this agent. Updates the read cursor so subsequent calls only return new messages."
    )]
    pub(crate) async fn hive_read_messages(
        &self,
        params: Parameters<ReadMessagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let mut agent = match state.load_agent(&self.run_id, &self.agent_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        // Determine "since" cutoff
        let since =
            if let Some(ref since_str) = params.0.since {
                Some(since_str.parse::<chrono::DateTime<Utc>>().map_err(|e| {
                    McpError::invalid_params(format!("Invalid timestamp: {e}"), None)
                })?)
            } else {
                // Use max(messages_read_at, last_completed_at)
                match (agent.messages_read_at, agent.last_completed_at) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                }
            };

        let messages = match state.load_messages_for_agent(&self.run_id, &self.agent_id, since) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        // Update read cursor
        agent.messages_read_at = Some(Utc::now());
        if let Err(e) = state.save_agent(&self.run_id, &agent) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to update read cursor: {e}"
            ))]));
        }

        let count = messages.len();
        let msg_data: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "from": m.from,
                    "timestamp": m.timestamp,
                    "message_type": m.message_type,
                    "body": m.body,
                })
            })
            .collect();

        let result = serde_json::json!({
            "messages": msg_data,
            "count": count,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )]))
    }

    #[tool(description = "Record a tool call event for observability")]
    pub(crate) async fn hive_log_tool(
        &self,
        params: Parameters<LogToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let state = self.state();
        let log_path = state.run_dir(&self.run_id).join("log.db");

        match LogDb::open(&log_path) {
            Ok(db) => {
                let agent = state.load_agent(&self.run_id, &self.agent_id).ok();
                let role = agent
                    .map(|a| format!("{:?}", a.role).to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string());

                match db.log_tool_call(
                    &self.run_id,
                    &self.agent_id,
                    &role,
                    "claude-code",
                    &p.tool,
                    p.args_summary.as_deref(),
                    &p.status,
                    p.duration_ms,
                ) {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text("Logged.")])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}
