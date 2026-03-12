pub mod params;

mod agent_tools;
mod message_tools;
mod mind_tools;
mod misc_tools;
mod queue_tools;
mod task_tools;

#[cfg(test)]
mod tests;

use crate::state::HiveState;
use crate::types::{AgentRole, AgentStatus, Message, MessageType};
use chrono::Utc;
use rmcp::handler::server::ServerHandler;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::ServiceExt;

#[derive(Clone)]
pub struct HiveMcp {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) repo_root: String,
    tool_router: ToolRouter<Self>,
}

impl HiveMcp {
    pub fn new(run_id: String, agent_id: String, repo_root: String) -> Self {
        Self {
            run_id,
            agent_id,
            repo_root,
            tool_router: Self::combined_tool_router(),
        }
    }

    fn combined_tool_router() -> ToolRouter<Self> {
        let mut router = Self::agent_tool_router();
        router.merge(Self::task_tool_router());
        router.merge(Self::message_tool_router());
        router.merge(Self::queue_tool_router());
        router.merge(Self::mind_tool_router());
        router.merge(Self::misc_tool_router());
        router
    }

    pub(crate) fn state(&self) -> HiveState {
        HiveState::new(self.repo_root.clone().into())
    }

    pub(crate) fn agent_role(&self) -> AgentRole {
        self.state()
            .load_agent(&self.run_id, &self.agent_id)
            .map(|a| a.role)
            .unwrap_or(AgentRole::Worker) // default to most restricted
    }

    pub(crate) fn require_role(&self, allowed: &[AgentRole]) -> Result<(), CallToolResult> {
        let role = self.agent_role();
        if allowed.contains(&role) {
            Ok(())
        } else {
            Err(CallToolResult::error(vec![Content::text(format!(
                "Permission denied: {:?} cannot use this tool.",
                role
            ))]))
        }
    }

    pub(crate) fn auto_commit_worktree(worktree: &str) -> Option<String> {
        let wt_path = std::path::Path::new(worktree);
        if !wt_path.exists() {
            return None;
        }
        let status = crate::git::Git::status_porcelain(wt_path).ok()?;
        if status.is_empty() {
            return None;
        }
        crate::git::Git::add_all(wt_path).ok()?;
        let _ = crate::git::Git::commit(wt_path, "wip: auto-commit on agent exit");
        Some(status)
    }

    /// Resume a crashed/failed agent using its existing session_id, preserving conversation context.
    pub(crate) fn resume_agent(
        &self,
        agent: &mut crate::types::Agent,
        prompt: &str,
    ) -> Result<u32, String> {
        let state = self.state();
        let session_id = agent
            .session_id
            .as_ref()
            .ok_or("No session_id for resume")?;
        let worktree = agent.worktree.as_ref().ok_or("No worktree for resume")?;
        let agent_output_dir = state.agents_dir(&self.run_id).join(&agent.id);
        let output_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(agent_output_dir.join("output.jsonl"))
            .map_err(|e| format!("Failed to open output file: {e}"))?;
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(agent_output_dir.join("stderr.log"))
            .map_err(|e| format!("Failed to open stderr file: {e}"))?;
        let config = state.load_config();
        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p")
            .arg(prompt)
            .arg("--resume")
            .arg(session_id)
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--dangerously-skip-permissions");
        if let Some(ref m) = agent.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(ref fb) = config.fallback_model {
            cmd.arg("--fallback-model").arg(fb);
        }
        let child = cmd
            .env_remove("CLAUDECODE")
            .current_dir(worktree)
            .stdin(std::process::Stdio::null())
            .stdout(output_file)
            .stderr(std::process::Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| format!("Failed to resume claude: {e}"))?;
        let pid = child.id();
        agent.status = AgentStatus::Running;
        agent.pid = Some(pid);
        agent.heartbeat = Some(Utc::now());
        agent.session_id = None; // Will be re-captured from new output
        let _ = state.save_agent(&self.run_id, agent);
        Ok(pid)
    }

    /// If the target agent is idle with a session_id, resume it by spawning
    /// `claude --resume`. Returns a human-readable status string.
    pub(crate) fn try_wake_agent(&self, to: &str, body: &str) -> Option<String> {
        let state = self.state();
        let mut target_agent = state.load_agent(&self.run_id, to).ok()?;
        if target_agent.status != AgentStatus::Idle {
            return None;
        }
        let session_id = target_agent.session_id.as_ref()?;

        let agent_output_dir = state.agents_dir(&self.run_id).join(&target_agent.id);
        let output_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(agent_output_dir.join("output.jsonl"));
        let mut output_file = match output_file {
            Ok(f) => f,
            Err(_) => return None,
        };
        {
            use std::io::Write;
            let _ = writeln!(
                output_file,
                r#"{{"type":"session_boundary","timestamp":"{}","reason":"wake"}}"#,
                chrono::Utc::now().to_rfc3339()
            );
        }
        let worktree = target_agent.worktree.clone().unwrap_or_default();
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(agent_output_dir.join("stderr.log"))
            .ok()
            .map(|mut f| {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    r#"{{"type":"session_boundary","timestamp":"{}","reason":"wake"}}"#,
                    chrono::Utc::now().to_rfc3339()
                );
                f
            });
        let config = state.load_config();
        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p")
            .arg(body)
            .arg("--resume")
            .arg(session_id)
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--dangerously-skip-permissions");
        if let Some(ref m) = target_agent.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(ref fb) = config.fallback_model {
            cmd.arg("--fallback-model").arg(fb);
        }
        cmd.env_remove("CLAUDECODE")
            .current_dir(&worktree)
            .stdin(std::process::Stdio::null())
            .stdout(output_file);
        if let Some(f) = stderr_file {
            cmd.stderr(std::process::Stdio::from(f));
        }
        let result = cmd.spawn();
        match result {
            Ok(child) => {
                target_agent.status = AgentStatus::Running;
                target_agent.pid = Some(child.id());
                target_agent.heartbeat = Some(Utc::now());
                let _ = state.save_agent(&self.run_id, &target_agent);
                Some(format!(" (woke agent '{to}', pid {})", child.id()))
            }
            Err(e) => Some(format!(" (failed to wake agent '{to}': {e})")),
        }
    }

    /// Send a status notification to an agent's parent when the agent transitions
    /// from Running to Idle/Failed/Stalled, and wake the parent if idle.
    pub(crate) fn notify_parent_of_transition(
        &self,
        state: &HiveState,
        agent: &crate::types::Agent,
    ) {
        let parent_id = match agent.parent {
            Some(ref id) => id,
            None => return,
        };
        let body = format!(
            "Agent {} transitioned to {:?} (task: {})",
            agent.id,
            agent.status,
            agent.task_id.as_deref().unwrap_or("none")
        );
        let msg_id = format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let message = Message {
            id: msg_id,
            from: agent.id.clone(),
            to: parent_id.clone(),
            timestamp: Utc::now(),
            message_type: MessageType::Status,
            body: body.clone(),
            refs: agent.task_id.clone().into_iter().collect(),
        };
        let _ = state.save_message(&self.run_id, &message);
        let _ = self.try_wake_agent(parent_id, &body);
    }

    pub(crate) fn notify_submitter(state: &HiveState, run_id: &str, to: &str, body: &str) {
        let msg_id = format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let message = Message {
            id: msg_id,
            from: "coordinator".to_string(),
            to: to.to_string(),
            timestamp: Utc::now(),
            message_type: MessageType::Info,
            body: body.to_string(),
            refs: vec![],
        };
        let _ = state.save_message(run_id, &message);
    }

    pub(crate) fn worktree_status(worktree: &str) -> Option<String> {
        let wt_path = std::path::Path::new(worktree);
        if !wt_path.exists() {
            return None;
        }
        let status = crate::git::Git::status_porcelain(wt_path).ok()?;
        if status.is_empty() {
            return None;
        }
        Some(status)
    }
}

#[rmcp::tool_handler]
impl ServerHandler for HiveMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("hive", env!("CARGO_PKG_VERSION")))
            .with_instructions("Hive MCP server: orchestrates a swarm of Claude Code agents for autonomous software development.")
    }
}

/// Start the MCP server on stdio
pub async fn run_mcp_server(run_id: &str, agent_id: &str) -> Result<(), String> {
    let repo_root = HiveState::discover()?
        .repo_root()
        .to_string_lossy()
        .to_string();
    let server = HiveMcp::new(run_id.to_string(), agent_id.to_string(), repo_root);

    let transport = rmcp::transport::io::stdio();
    let service = server
        .serve(transport)
        .await
        .map_err(|e| format!("MCP server error: {e}"))?;

    service
        .waiting()
        .await
        .map_err(|e| format!("MCP server error: {e}"))?;

    Ok(())
}
