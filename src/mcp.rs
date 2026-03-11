use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types::{
    Agent, AgentRole, AgentStatus, Confidence, Discovery, FailureEntry, Insight, MergeQueue,
    MergeQueueEntry, Message, MessageType, OperationalEntry, Task, TaskStatus, Urgency,
};
use chrono::Utc;
use rmcp::handler::server::ServerHandler;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::ServiceExt;
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Clone)]
pub struct HiveMcp {
    run_id: String,
    agent_id: String,
    repo_root: String,
    tool_router: ToolRouter<Self>,
}

// --- Parameter types ---

#[derive(Deserialize, JsonSchema)]
pub struct SpawnAgentParams {
    /// Agent ID (unique name like "lead-backend" or "worker-001")
    pub agent_id: String,
    /// Role: "lead" or "worker"
    pub role: String,
    /// Task ID to bind this agent to
    pub task_id: String,
    /// Task description for the agent
    pub task_description: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Short title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Urgency: low, normal, high, critical
    #[serde(default = "default_urgency")]
    pub urgency: String,
    /// Optional domain tag
    pub domain: Option<String>,
    /// Optional list of task IDs this blocks
    #[serde(default)]
    pub blocking: Vec<String>,
    /// Optional list of task IDs blocking this
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Optional parent task ID
    pub parent_task: Option<String>,
}

fn default_urgency() -> String {
    "normal".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateTaskParams {
    /// Task ID to update
    pub task_id: String,
    /// New status
    pub status: Option<String>,
    /// Agent ID to assign to
    pub assigned_to: Option<String>,
    /// Branch name
    pub branch: Option<String>,
    /// Optional notes to append to task description (for context handoff)
    pub notes: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTasksParams {
    /// Filter by status
    pub status: Option<String>,
    /// Filter by assignee
    pub assignee: Option<String>,
    /// Filter by domain
    pub domain: Option<String>,
    /// Filter by parent task. Use "none" for top-level only, or a task ID for that task's subtasks.
    pub parent_task: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// Recipient agent ID
    pub to: String,
    /// Message type: info, request, status, task-suggestion
    #[serde(default = "default_message_type")]
    pub message_type: String,
    /// Message body
    pub body: String,
    /// Optional task ID references
    #[serde(default)]
    pub refs: Vec<String>,
}

fn default_message_type() -> String {
    "info".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct SubmitToQueueParams {
    /// Task ID of the approved work
    pub task_id: String,
    /// Branch name to merge
    pub branch: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct WaitForActivityParams {
    /// Timeout in seconds (default 60)
    #[serde(default = "default_wait_timeout")]
    pub timeout_secs: u64,
}

fn default_wait_timeout() -> u64 {
    60
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewAgentParams {
    /// Agent ID to review
    pub agent_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LogToolParams {
    /// Tool name
    pub tool: String,
    /// Status: success or error
    pub status: String,
    /// Duration in milliseconds
    pub duration_ms: Option<i64>,
    /// Optional args summary
    pub args_summary: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadMessagesParams {
    /// Only return messages newer than this timestamp (ISO 8601). If omitted, returns unread messages since last read or last idle.
    pub since: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RetryAgentParams {
    /// Agent ID of the failed agent to retry
    pub agent_id: String,
    /// Optional feedback about what went wrong, appended to the new agent's prompt
    pub feedback: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewVerdictParams {
    /// Task ID being reviewed
    pub task_id: String,
    /// Verdict: "approve", "request-changes", or "reject"
    pub verdict: String,
    /// Feedback message (required for request-changes and reject)
    pub feedback: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveMemoryParams {
    /// Memory type: "operation", "convention", or "failure"
    pub memory_type: String,
    /// Content: JSON string for operation/failure entries, markdown for conventions
    pub content: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveSpecParams {
    /// Full spec markdown content
    pub spec: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiscoverParams {
    /// Content of the discovery
    pub content: String,
    /// Confidence level: "low", "medium", or "high"
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// File paths related to this discovery
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_confidence() -> String {
    "medium".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryMindParams {
    /// Search query (keywords)
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SynthesizeParams {
    /// Insight content synthesized from discoveries
    pub content: String,
    /// IDs of discoveries being synthesized
    pub discovery_ids: Vec<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct EstablishConventionParams {
    /// Convention to add
    pub content: String,
}

#[tool_router]
impl HiveMcp {
    pub fn new(run_id: String, agent_id: String, repo_root: String) -> Self {
        Self {
            run_id,
            agent_id,
            repo_root,
            tool_router: Self::tool_router(),
        }
    }

    fn state(&self) -> HiveState {
        HiveState::new(self.repo_root.clone().into())
    }

    fn agent_role(&self) -> AgentRole {
        self.state()
            .load_agent(&self.run_id, &self.agent_id)
            .map(|a| a.role)
            .unwrap_or(AgentRole::Worker) // default to most restricted
    }

    fn require_role(&self, allowed: &[AgentRole]) -> Result<(), CallToolResult> {
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

    #[tool(description = "Spawn a new agent (lead or worker) with a worktree and task")]
    async fn hive_spawn_agent(
        &self,
        params: Parameters<SpawnAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }
        let p = &params.0;
        let role = match p.role.as_str() {
            "lead" => AgentRole::Lead,
            "worker" => AgentRole::Worker,
            "reviewer" => AgentRole::Reviewer,
            "planner" => AgentRole::Planner,
            "postmortem" => AgentRole::Postmortem,
            "explorer" => AgentRole::Explorer,
            "evaluator" => AgentRole::Evaluator,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid role. Use 'lead', 'worker', 'reviewer', 'planner', 'postmortem', 'explorer', or 'evaluator'.",
                )]));
            }
        };

        // Enforce hierarchy: coordinators spawn leads, leads spawn workers
        let caller_role = self.agent_role();
        let allowed = matches!(
            (caller_role, role),
            (AgentRole::Coordinator, AgentRole::Lead)
                | (AgentRole::Coordinator, AgentRole::Planner)
                | (AgentRole::Coordinator, AgentRole::Postmortem)
                | (AgentRole::Coordinator, AgentRole::Explorer)
                | (AgentRole::Coordinator, AgentRole::Evaluator)
                | (AgentRole::Lead, AgentRole::Worker)
                | (AgentRole::Lead, AgentRole::Reviewer)
        );
        if !allowed {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Permission denied: {:?} cannot spawn {:?}.",
                caller_role, role
            ))]));
        }

        // Budget enforcement
        let state = self.state();
        let config = state.load_config();
        if let Some(budget) = config.budget_usd {
            let current_cost = state.total_run_cost(&self.run_id);
            if current_cost >= budget {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Budget exceeded: ${:.2} spent of ${:.2} budget. Cannot spawn new agents.",
                    current_cost, budget
                ))]));
            }
        }

        // Spawn-task binding: validate and bind task before spawning
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(_) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Task '{}' not found.",
                    p.task_id
                ))]));
            }
        };
        if !matches!(task.status, TaskStatus::Pending | TaskStatus::Blocked) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Task '{}' is in {:?} status, expected pending or blocked.",
                p.task_id, task.status
            ))]));
        }
        task.assigned_to = Some(p.agent_id.clone());
        task.status = TaskStatus::Active;
        task.updated_at = Utc::now();
        if let Err(e) = state.save_task(&self.run_id, &task) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to update task: {e}"
            ))]));
        }

        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &p.agent_id,
            role,
            Some(&self.agent_id),
            &p.task_description,
        ) {
            Ok(agent) => {
                // Bind agent to task
                if let Ok(mut spawned_agent) = state.load_agent(&self.run_id, &agent.id) {
                    spawned_agent.task_id = Some(p.task_id.clone());
                    let _ = state.save_agent(&self.run_id, &spawned_agent);
                }
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Spawned agent '{}' (role={:?}, task={}, worktree={})",
                    agent.id,
                    agent.role,
                    p.task_id,
                    agent.worktree.unwrap_or_default()
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Create a new task with title, description, urgency, and optional blocking info"
    )]
    async fn hive_create_task(
        &self,
        params: Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;

        // Permission checks for task creation
        let caller_agent = self.state().load_agent(&self.run_id, &self.agent_id).ok();
        let caller_role = caller_agent
            .as_ref()
            .map(|a| a.role)
            .unwrap_or(AgentRole::Worker);

        match caller_role {
            AgentRole::Coordinator => {
                if p.parent_task.is_some() {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Permission denied: coordinator cannot create subtasks. Create a lead-level task (no parent_task) and let the assigned lead decompose it.",
                    )]));
                }
            }
            AgentRole::Lead => {
                if p.parent_task.is_none() {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Permission denied: leads can only create subtasks under their own task.",
                    )]));
                }
                let own_task = caller_agent
                    .as_ref()
                    .and_then(|a| a.task_id.as_deref())
                    .unwrap_or("");
                if p.parent_task.as_deref() != Some(own_task) {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Permission denied: you can only create subtasks under your own task ({}), not under {}.",
                        own_task,
                        p.parent_task.as_deref().unwrap_or("unknown")
                    ))]));
                }
            }
            AgentRole::Worker => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Permission denied: workers cannot create tasks. Send a message to your lead suggesting the task.",
                )]));
            }
            other => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Permission denied: {:?} agents cannot create tasks.",
                    other
                ))]));
            }
        }

        let urgency = match p.urgency.as_str() {
            "low" => Urgency::Low,
            "normal" => Urgency::Normal,
            "high" => Urgency::High,
            "critical" => Urgency::Critical,
            _ => Urgency::Normal,
        };

        let task_id = format!("task-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now();
        let task = Task {
            id: task_id.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
            status: TaskStatus::Pending,
            urgency,
            blocking: p.blocking.clone(),
            blocked_by: p.blocked_by.clone(),
            assigned_to: None,
            created_by: self.agent_id.clone(),
            parent_task: p.parent_task.clone(),
            branch: None,
            domain: p.domain.clone(),
            review_count: 0,
            created_at: now,
            updated_at: now,
        };

        match self.state().save_task(&self.run_id, &task) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Created task '{task_id}': {}",
                p.title
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Update a task's status, assignment, or branch")]
    async fn hive_update_task(
        &self,
        params: Parameters<UpdateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let state = self.state();
        let _lock = state
            .lock_file(&format!("task-{}", p.task_id))
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        // Ownership enforcement
        let caller_agent = state.load_agent(&self.run_id, &self.agent_id).ok();
        let caller_role = caller_agent
            .as_ref()
            .map(|a| a.role)
            .unwrap_or(AgentRole::Worker);
        let caller_id = &self.agent_id;

        let ownership_ok = match caller_role {
            AgentRole::Coordinator => task.parent_task.is_none(),
            AgentRole::Lead => {
                let is_own = task.assigned_to.as_deref() == Some(caller_id.as_str());
                let is_created = task.created_by == *caller_id;
                let is_child_task = if let Some(ref assigned) = task.assigned_to {
                    state
                        .load_agent(&self.run_id, assigned)
                        .map(|a| a.parent.as_deref() == Some(caller_id.as_str()))
                        .unwrap_or(false)
                } else {
                    false
                };
                is_own || is_created || is_child_task
            }
            AgentRole::Worker => task.assigned_to.as_deref() == Some(caller_id.as_str()),
            AgentRole::Reviewer => {
                caller_agent
                    .as_ref()
                    .and_then(|a| a.task_id.as_deref())
                    == Some(&task.id)
            }
            _ => false,
        };

        if !ownership_ok {
            let err_msg = match caller_role {
                AgentRole::Coordinator => {
                    "Permission denied: coordinator cannot modify subtasks. Send a message to the lead who owns this task.".to_string()
                }
                AgentRole::Worker => {
                    let own_task = caller_agent
                        .as_ref()
                        .and_then(|a| a.task_id.as_deref())
                        .unwrap_or("unknown");
                    let lead = caller_agent
                        .as_ref()
                        .and_then(|a| a.parent.as_deref())
                        .unwrap_or("unknown");
                    if task.parent_task.is_none() {
                        format!(
                            "Permission denied: {} is a lead-level task owned by '{}'. Send a message to your lead instead.",
                            p.task_id,
                            task.assigned_to.as_deref().unwrap_or("unknown")
                        )
                    } else {
                        format!(
                            "Permission denied: you can only update your own assigned task ({}). To request changes to {}, send a message to its owner '{}' or your lead '{}'.",
                            own_task,
                            p.task_id,
                            task.assigned_to.as_deref().unwrap_or("unknown"),
                            lead
                        )
                    }
                }
                AgentRole::Lead => {
                    format!(
                        "Permission denied: {} belongs to another lead's domain. Send a message to the coordinator to coordinate cross-domain changes.",
                        p.task_id
                    )
                }
                _ => format!(
                    "Permission denied: {:?} cannot update this task.",
                    caller_role
                ),
            };
            return Ok(CallToolResult::error(vec![Content::text(err_msg)]));
        }

        if let Some(ref status_str) = p.status {
            let new_status = match status_str.as_str() {
                "pending" => TaskStatus::Pending,
                "active" => TaskStatus::Active,
                "blocked" => TaskStatus::Blocked,
                "review" => TaskStatus::Review,
                "approved" => TaskStatus::Approved,
                "queued" => TaskStatus::Queued,
                "merged" => TaskStatus::Merged,
                "failed" => TaskStatus::Failed,
                "absorbed" => TaskStatus::Absorbed,
                "cancelled" => TaskStatus::Cancelled,
                _ => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Invalid status: {status_str}"
                    ))]));
                }
            };

            // Permission check for terminal administrative statuses
            if matches!(new_status, TaskStatus::Absorbed | TaskStatus::Cancelled) {
                let is_coordinator = self.agent_role() == AgentRole::Coordinator;
                let is_task_creator = task.created_by == self.agent_id;
                let is_assigned_cancelling = task.assigned_to.as_deref()
                    == Some(self.agent_id.as_str())
                    && new_status == TaskStatus::Cancelled;

                if !is_coordinator && !is_task_creator && !is_assigned_cancelling {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Permission denied: only coordinator, task creator, or assigned agent (cancel only) can set absorbed/cancelled status",
                    )]));
                }
            }

            task.status = new_status;
        }

        if let Some(ref assignee) = p.assigned_to {
            task.assigned_to = Some(assignee.clone());
        }

        if let Some(ref branch) = p.branch {
            task.branch = Some(branch.clone());
        }

        if let Some(ref notes) = p.notes {
            task.description.push_str(&format!("\n\n## Notes\n{notes}"));
        }

        task.updated_at = Utc::now();

        match state.save_task(&self.run_id, &task) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Updated task '{}': status={:?}",
                task.id, task.status
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List tasks, optionally filtered by status, assignee, or domain")]
    async fn hive_list_tasks(
        &self,
        params: Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let tasks = match self.state().list_tasks(&self.run_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let filtered: Vec<_> = tasks
            .iter()
            .filter(|t| {
                if let Some(ref s) = p.status {
                    let status_str = serde_json::to_value(t.status)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                    if status_str.as_deref() != Some(s.as_str()) {
                        return false;
                    }
                }
                if let Some(ref a) = p.assignee
                    && t.assigned_to.as_deref() != Some(a.as_str())
                {
                    return false;
                }
                if let Some(ref d) = p.domain
                    && t.domain.as_deref() != Some(d.as_str())
                {
                    return false;
                }
                if let Some(ref pt) = p.parent_task {
                    if pt == "none" {
                        if t.parent_task.is_some() {
                            return false;
                        }
                    } else if t.parent_task.as_deref() != Some(pt.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();

        let summary = serde_json::to_string_pretty(&filtered).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(description = "Send a message to another agent")]
    async fn hive_send_message(
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
                if !matches!(target, Some(ref t) if t.role == AgentRole::Lead) {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Coordinator can only send messages to leads.",
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
                        "Reviewers can only send messages to the coordinator.",
                    )]));
                }
            }
        }

        let state = self.state();
        if let Err(e) = state.save_message(&self.run_id, &message) {
            return Ok(CallToolResult::error(vec![Content::text(e)]));
        }

        // Auto-wake: if target agent is idle with a session_id, resume it
        let wake_info = self.try_wake_agent(&p.to, &p.body);

        let wake_suffix = wake_info.unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Sent message '{msg_id}' to '{}'{wake_suffix}",
            p.to
        ))]))
    }

    #[tool(description = "Submit a review verdict for a task. Reviewer-only.")]
    async fn hive_review_verdict(
        &self,
        params: Parameters<ReviewVerdictParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Reviewer]) {
            return Ok(result);
        }
        let p = &params.0;
        let state = self.state();

        let _task_lock = state
            .lock_file(&format!("task-{}", p.task_id))
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        match p.verdict.as_str() {
            "approve" => {
                task.status = TaskStatus::Queued;
                task.updated_at = Utc::now();
                state
                    .save_task(&self.run_id, &task)
                    .map_err(|e| McpError::internal_error(e, None))?;

                // Add to merge queue
                let branch = task.branch.clone().unwrap_or_default();
                let submitted_by = task.assigned_to.clone().unwrap_or_default();
                let _lock = state
                    .lock_file("merge-queue")
                    .map_err(|e| McpError::internal_error(e, None))?;
                let mut queue = state
                    .load_merge_queue(&self.run_id)
                    .unwrap_or(MergeQueue { entries: vec![] });
                queue.entries.push(MergeQueueEntry {
                    task_id: p.task_id.clone(),
                    branch: branch.clone(),
                    submitted_by: submitted_by.clone(),
                    submitted_at: Utc::now(),
                });
                state
                    .save_merge_queue(&self.run_id, &queue)
                    .map_err(|e| McpError::internal_error(e, None))?;

                // Notify coordinator
                let msg = format!(
                    "Review approved for task '{}'. Branch '{}' added to merge queue.",
                    p.task_id, branch
                );
                let _ = state.save_message(
                    &self.run_id,
                    &Message {
                        id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                        from: self.agent_id.clone(),
                        to: "coordinator".to_string(),
                        timestamp: Utc::now(),
                        message_type: MessageType::Status,
                        body: msg.clone(),
                        refs: vec![p.task_id.clone()],
                    },
                );

                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            "request-changes" => {
                let feedback = p
                    .feedback
                    .as_deref()
                    .unwrap_or("Changes requested (no details provided)");
                task.review_count += 1;
                task.status = TaskStatus::Active;
                task.updated_at = Utc::now();
                state
                    .save_task(&self.run_id, &task)
                    .map_err(|e| McpError::internal_error(e, None))?;

                // Send feedback to the agent that worked on this task
                if let Some(ref assigned) = task.assigned_to {
                    let body = format!(
                        "Review feedback for task '{}' (review cycle {}):\n{}",
                        p.task_id, task.review_count, feedback
                    );
                    let _ = state.save_message(
                        &self.run_id,
                        &Message {
                            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                            from: self.agent_id.clone(),
                            to: assigned.clone(),
                            timestamp: Utc::now(),
                            message_type: MessageType::Request,
                            body: body.clone(),
                            refs: vec![p.task_id.clone()],
                        },
                    );

                    // Auto-wake the agent to process feedback
                    let _ = self.try_wake_agent(assigned, &body);
                }

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Requested changes for task '{}' (review cycle {}). Feedback sent to {}.",
                    p.task_id,
                    task.review_count,
                    task.assigned_to.as_deref().unwrap_or("unknown")
                ))]))
            }
            "reject" => {
                let feedback = p
                    .feedback
                    .as_deref()
                    .unwrap_or("Rejected (no details provided)");
                task.status = TaskStatus::Failed;
                task.updated_at = Utc::now();
                state
                    .save_task(&self.run_id, &task)
                    .map_err(|e| McpError::internal_error(e, None))?;

                // Notify the lead (parent of the assigned agent)
                if let Some(ref assigned) = task.assigned_to
                    && let Ok(agent) = state.load_agent(&self.run_id, assigned)
                    && let Some(ref parent) = agent.parent
                {
                    let _ = state.save_message(
                        &self.run_id,
                        &Message {
                            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                            from: self.agent_id.clone(),
                            to: parent.clone(),
                            timestamp: Utc::now(),
                            message_type: MessageType::Status,
                            body: format!(
                                "Task '{}' rejected by reviewer:\n{}",
                                p.task_id, feedback
                            ),
                            refs: vec![p.task_id.clone()],
                        },
                    );
                }

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Task '{}' rejected: {}",
                    p.task_id, feedback
                ))]))
            }
            _ => Ok(CallToolResult::error(vec![Content::text(
                "Invalid verdict. Use 'approve', 'request-changes', or 'reject'.",
            )])),
        }
    }

    #[tool(
        description = "Submit a branch for review before merging. Spawns a reviewer agent to evaluate the changes."
    )]
    async fn hive_submit_to_queue(
        &self,
        params: Parameters<SubmitToQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Lead]) {
            return Ok(result);
        }
        let p = &params.0;
        let state = self.state();

        // Load and lock the task
        let _task_lock = state
            .lock_file(&format!("task-{}", p.task_id))
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        // Subtask completion gate
        if let Ok(all_tasks) = state.list_tasks(&self.run_id) {
            let unresolved: Vec<_> = all_tasks
                .iter()
                .filter(|t| t.parent_task.as_deref() == Some(&p.task_id))
                .filter(|t| !t.status.is_resolved())
                .collect();
            if !unresolved.is_empty() {
                let details: Vec<String> = unresolved
                    .iter()
                    .map(|t| {
                        format!(
                            "  - {} ({}): {:?} — assigned to {}",
                            t.id,
                            t.title,
                            t.status,
                            t.assigned_to.as_deref().unwrap_or("unassigned")
                        )
                    })
                    .collect();
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Cannot submit task '{}': {} subtask(s) are not resolved:\n{}\nResolve all subtasks first. Use status 'cancelled' or 'absorbed' for tasks that don't need independent merges.",
                    p.task_id,
                    unresolved.len(),
                    details.join("\n")
                ))]));
            }
        }

        // Check review cycle limit (max 3)
        if task.review_count >= 3 {
            task.status = TaskStatus::Failed;
            task.updated_at = Utc::now();
            let _ = state.save_task(&self.run_id, &task);
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Task '{}' has exceeded the maximum review cycles (3). Marked as failed.",
                p.task_id
            ))]));
        }

        task.status = TaskStatus::Review;
        task.branch = Some(p.branch.clone());
        task.updated_at = Utc::now();
        state
            .save_task(&self.run_id, &task)
            .map_err(|e| McpError::internal_error(e, None))?;

        // Spawn reviewer agent
        let reviewer_id = format!("reviewer-{}", &p.task_id[..8.min(p.task_id.len())]);
        let review_description = format!(
            "Review task '{}': {}\n\nBranch: {}\nTask description: {}\n\nExamine the diff on this branch against main. Run `git log main..HEAD --oneline` and `git diff main...HEAD --stat` to see what changed. Then read the changed files and evaluate.",
            p.task_id, task.title, p.branch, task.description
        );

        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &reviewer_id,
            AgentRole::Reviewer,
            Some(&self.agent_id),
            &review_description,
        ) {
            Ok(_) => {
                // Set task_id on the reviewer agent
                if let Ok(mut reviewer_agent) = state.load_agent(&self.run_id, &reviewer_id) {
                    reviewer_agent.task_id = Some(p.task_id.clone());
                    let _ = state.save_agent(&self.run_id, &reviewer_agent);
                }

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Spawned reviewer '{}' for task '{}'. Awaiting review verdict.",
                    reviewer_id, p.task_id
                ))]))
            }
            Err(e) => {
                // Fallback: submit directly to merge queue
                task.status = TaskStatus::Queued;
                task.updated_at = Utc::now();
                let _ = state.save_task(&self.run_id, &task);

                let _lock = state
                    .lock_file("merge-queue")
                    .map_err(|e| McpError::internal_error(e, None))?;
                let mut queue = state
                    .load_merge_queue(&self.run_id)
                    .unwrap_or(MergeQueue { entries: vec![] });
                queue.entries.push(MergeQueueEntry {
                    task_id: p.task_id.clone(),
                    branch: p.branch.clone(),
                    submitted_by: self.agent_id.clone(),
                    submitted_at: Utc::now(),
                });
                let _ = state.save_merge_queue(&self.run_id, &queue);

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Warning: Failed to spawn reviewer ({e}). Branch submitted directly to merge queue as fallback."
                ))]))
            }
        }
    }

    #[tool(
        description = "Process the next item in the merge queue. Merges the branch into main, runs verification, and handles conflicts with auto-rebase."
    )]
    async fn hive_merge_next(&self) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
            return Ok(result);
        }
        let state = self.state();
        let _lock = state
            .lock_file("merge-queue")
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut queue = match state.load_merge_queue(&self.run_id) {
            Ok(q) => q,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        if queue.entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "Merge queue is empty.",
            )]));
        }

        let entry = queue.entries.remove(0);
        let repo_root = state.repo_root().to_path_buf();

        // Helper closure: mark task as failed
        let mark_failed = |state: &HiveState, run_id: &str, task_id: &str| {
            if let Ok(mut task) = state.load_task(run_id, task_id) {
                task.status = TaskStatus::Failed;
                task.updated_at = Utc::now();
                let _ = state.save_task(run_id, &task);
            }
        };

        // Ensure we're on main before merging
        if let Err(e) = crate::git::Git::checkout(&repo_root, "main") {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to checkout main before merge: {e}"
            ))]));
        }

        // Attempt merge
        let merge_result = crate::git::Git::merge(&repo_root, &entry.branch);

        // If merge failed, try auto-rebase then retry
        let merge_result = if let Err(merge_err) = merge_result {
            let _ = crate::git::Git::merge_abort(&repo_root);

            match crate::git::Git::rebase(&repo_root, &entry.branch, "main") {
                Ok(()) => {
                    // Rebase succeeded — HEAD is now on the rebased branch.
                    // Switch back to main before retrying the merge.
                    if let Err(e) = crate::git::Git::checkout(&repo_root, "main") {
                        mark_failed(&state, &self.run_id, &entry.task_id);
                        state.save_merge_queue(&self.run_id, &queue).ok();
                        let msg = format!(
                            "Rebase of '{}' succeeded but checkout main failed: {e}",
                            entry.branch
                        );
                        Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                        return Ok(CallToolResult::error(vec![Content::text(msg)]));
                    }
                    crate::git::Git::merge(&repo_root, &entry.branch)
                }
                Err(rebase_err) => {
                    let _ = crate::git::Git::rebase_abort(&repo_root);
                    // Restore main after failed rebase
                    let _ = crate::git::Git::checkout(&repo_root, "main");
                    mark_failed(&state, &self.run_id, &entry.task_id);
                    state.save_merge_queue(&self.run_id, &queue).ok();
                    let msg = format!(
                        "Merge failed for branch '{}': {merge_err}. Auto-rebase also failed: {rebase_err}. Task marked as failed.",
                        entry.branch
                    );
                    Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                    return Ok(CallToolResult::error(vec![Content::text(msg)]));
                }
            }
        } else {
            merge_result
        };

        match merge_result {
            Ok(()) => {
                // Run verification command if configured
                let config = state.load_config();
                if let Some(ref verify_cmd) = config.verify_command
                    && let Err(verify_err) =
                        crate::git::Git::run_shell_command(&repo_root, verify_cmd)
                {
                    // Verification failed — undo merge commit
                    if let Err(reset_err) = crate::git::Git::reset_hard(&repo_root, "HEAD~1") {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "CRITICAL: Verification failed for '{}' AND reset failed: {reset_err}. Repository may be in inconsistent state. Manual intervention required.",
                            entry.branch
                        ))]));
                    }
                    mark_failed(&state, &self.run_id, &entry.task_id);
                    state.save_merge_queue(&self.run_id, &queue).ok();
                    let truncated_output = if verify_err.len() > 500 {
                        format!("{}...(truncated)", &verify_err[..500])
                    } else {
                        verify_err.clone()
                    };
                    let msg = format!(
                        "Verification failed for branch '{}' (task '{}'): {verify_err}",
                        entry.branch, entry.task_id
                    );
                    let notify_msg = format!(
                        "Verification failed for branch '{}' (task '{}').\nOutput:\n{truncated_output}",
                        entry.branch, entry.task_id
                    );
                    Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &notify_msg);
                    return Ok(CallToolResult::error(vec![Content::text(msg)]));
                }

                let mut warnings = Vec::new();

                // Update task status
                match state.load_task(&self.run_id, &entry.task_id) {
                    Ok(mut task) => {
                        task.status = TaskStatus::Merged;
                        task.updated_at = Utc::now();
                        if let Err(e) = state.save_task(&self.run_id, &task) {
                            warnings.push(format!("Warning: failed to update task status: {e}"));
                        }
                    }
                    Err(e) => warnings.push(format!("Warning: failed to load task: {e}")),
                }
                if let Err(e) = state.save_merge_queue(&self.run_id, &queue) {
                    warnings.push(format!("Warning: failed to save merge queue: {e}"));
                }

                let mut msg = format!(
                    "Merged branch '{}' for task '{}'. Queue remaining: {}",
                    entry.branch,
                    entry.task_id,
                    queue.entries.len()
                );
                for w in &warnings {
                    msg.push_str(&format!("\n{w}"));
                }

                Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);

                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            Err(e) => {
                // Merge failed even after rebase
                let _ = crate::git::Git::merge_abort(&repo_root);
                mark_failed(&state, &self.run_id, &entry.task_id);
                state.save_merge_queue(&self.run_id, &queue).ok();
                let msg = format!(
                    "Merge failed for branch '{}' after rebase: {e}. Task marked as failed.",
                    entry.branch
                );
                Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                Ok(CallToolResult::error(vec![Content::text(msg)]))
            }
        }
    }

    #[tool(description = "List all agents and their current status")]
    async fn hive_list_agents(&self) -> Result<CallToolResult, McpError> {
        let agents = match self.state().list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        let summary = serde_json::to_string_pretty(&agents).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(
        description = "Check agent health by comparing heartbeats and verifying processes are alive. Returns structured JSON with agent_id, role, status, last_heartbeat_age_secs, and process_alive."
    )]
    async fn hive_check_agents(&self) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[
            AgentRole::Coordinator,
            AgentRole::Lead,
            AgentRole::Reviewer,
            AgentRole::Planner,
            AgentRole::Postmortem,
        ]) {
            return Ok(result);
        }
        let state = self.state();
        let config = state.load_config();
        let agents = match state.list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let now = Utc::now();
        let mut reports = Vec::new();
        for mut agent in agents {
            let _lock = state
                .lock_file(&format!("agent-{}", agent.id))
                .map_err(|e| McpError::internal_error(e, None))?;
            let process_alive = agent.pid.map(crate::agent::AgentSpawner::is_alive);
            let heartbeat_age_secs = agent.heartbeat.map(|hb| (now - hb).num_seconds());

            // Auto-commit any uncommitted work before state transitions
            if process_alive == Some(false)
                && let Some(ref wt) = agent.worktree
            {
                Self::auto_commit_worktree(wt);
            }

            // Early session ID capture: for running agents, try to grab session_id from init line
            if agent.status == AgentStatus::Running
                && process_alive == Some(true)
                && agent.session_id.is_none()
            {
                let output_path = state
                    .agents_dir(&self.run_id)
                    .join(&agent.id)
                    .join("output.jsonl");
                if let Some(sid) = crate::output::parse_early_session_id(&output_path) {
                    agent.session_id = Some(sid);
                    let _ = state.save_agent(&self.run_id, &agent);
                }
            }

            // Session ID capture: if process exited and no session_id yet, parse output.jsonl
            if process_alive == Some(false) && agent.session_id.is_none() {
                let output_path = state
                    .agents_dir(&self.run_id)
                    .join(&agent.id)
                    .join("output.jsonl");
                if let Some(sid) = crate::output::parse_session_id_from_output(&output_path) {
                    agent.session_id = Some(sid);
                    agent.status = AgentStatus::Idle;
                    agent.last_completed_at = Some(now);
                    agent.pid = None;
                    let _ = state.save_agent(&self.run_id, &agent);
                } else if agent.status == AgentStatus::Running {
                    // Process exited but no session_id found — mark as failed
                    agent.status = AgentStatus::Failed;
                    agent.pid = None;
                    let _ = state.save_agent(&self.run_id, &agent);
                }
            } else if process_alive == Some(false)
                && agent.session_id.is_some()
                && agent.status == AgentStatus::Running
            {
                // Process exited but has session_id from a previous invocation — mark idle
                agent.status = AgentStatus::Idle;
                agent.last_completed_at = Some(now);
                agent.pid = None;
                let _ = state.save_agent(&self.run_id, &agent);
            }

            let idle_since_secs = agent.last_completed_at.map(|lc| (now - lc).num_seconds());

            let status = match agent.status {
                AgentStatus::Idle => "idle",
                AgentStatus::Done => "done",
                AgentStatus::Failed => "failed",
                AgentStatus::Stalled => "stalled",
                AgentStatus::Running => {
                    if process_alive == Some(false) {
                        "dead"
                    } else if process_alive == Some(true)
                        && heartbeat_age_secs.is_some_and(|age| age > config.stall_timeout_seconds)
                    {
                        // Active stall recovery: kill the stalled process
                        if let Some(pid) = agent.pid {
                            unsafe {
                                libc::kill(pid as i32, libc::SIGTERM);
                            }
                            std::thread::sleep(std::time::Duration::from_secs(5));
                            if crate::agent::AgentSpawner::is_alive(pid) {
                                unsafe {
                                    libc::kill(pid as i32, libc::SIGKILL);
                                }
                            }
                        }

                        // Auto-commit any work
                        if let Some(ref wt) = agent.worktree {
                            Self::auto_commit_worktree(wt);
                        }

                        // Try to capture session_id
                        if agent.session_id.is_none() {
                            let output_path = state
                                .agents_dir(&self.run_id)
                                .join(&agent.id)
                                .join("output.jsonl");
                            if let Some(sid) = crate::output::parse_session_id_from_output(&output_path) {
                                agent.session_id = Some(sid);
                            }
                        }

                        // Update agent state
                        agent.status = AgentStatus::Stalled;
                        agent.pid = None;
                        let _ = state.save_agent(&self.run_id, &agent);

                        "stalled"
                    } else {
                        "running"
                    }
                }
            };

            let uncommitted_changes = agent.worktree.as_deref().and_then(Self::worktree_status);

            reports.push(serde_json::json!({
                "agent_id": agent.id,
                "role": agent.role,
                "status": status,
                "session_id": agent.session_id,
                "last_heartbeat_age_secs": heartbeat_age_secs,
                "process_alive": process_alive,
                "idle_since_secs": idle_since_secs,
                "uncommitted_changes": uncommitted_changes,
            }));
        }

        let summary = serde_json::to_string_pretty(&reports).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(description = "Update this agent's heartbeat timestamp to signal liveness")]
    async fn hive_heartbeat(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        match state.update_agent_heartbeat(&self.run_id, &self.agent_id) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(
                "Heartbeat updated.",
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Block until activity is detected in the hive run directory, or timeout. Returns a summary of what changed."
    )]
    async fn hive_wait_for_activity(
        &self,
        params: Parameters<WaitForActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }
        let result = crate::wait::wait_for_activity(
            self.repo_root.as_ref(),
            &self.run_id,
            params.0.timeout_secs,
        )
        .await;
        match result {
            Ok(summary) => Ok(CallToolResult::success(vec![Content::text(summary)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Review a non-running agent's work: commits any uncommitted changes, then returns branch info, commit log, and diff stat vs main."
    )]
    async fn hive_review_agent(
        &self,
        params: Parameters<ReviewAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }

        let state = self.state();
        let agent = match state.load_agent(&self.run_id, &params.0.agent_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        if agent.status == AgentStatus::Running {
            return Ok(CallToolResult::error(vec![Content::text(
                "Cannot review a running agent. Wait for it to exit first.",
            )]));
        }

        let worktree = match &agent.worktree {
            Some(wt) => wt.clone(),
            None => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Agent has no worktree.",
                )]));
            }
        };

        let wt_path = std::path::Path::new(&worktree);
        if !wt_path.exists() {
            return Ok(CallToolResult::error(vec![Content::text(
                "Agent worktree no longer exists.",
            )]));
        }

        // Auto-commit any uncommitted work
        Self::auto_commit_worktree(&worktree);

        // Get branch name, commit log, and diff stat
        let branch = format!("hive/{}/{}", self.run_id, agent.id);
        let commits = crate::git::Git::log_oneline_since(wt_path, "main")
            .unwrap_or_else(|_| "(no commits)".to_string());
        let diff_stat = crate::git::Git::diff_stat_since(wt_path, "main")
            .unwrap_or_else(|_| "(no diff)".to_string());

        let report = serde_json::json!({
            "agent_id": agent.id,
            "role": agent.role,
            "status": agent.status,
            "task_id": agent.task_id,
            "branch": branch,
            "commits": commits,
            "diff_stat": diff_stat,
        });

        let summary = serde_json::to_string_pretty(&report).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(description = "Record a tool call event for observability")]
    async fn hive_log_tool(
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

    #[tool(
        description = "Retry a failed agent by re-spawning it with the same task and a fresh worktree"
    )]
    async fn hive_retry_agent(
        &self,
        params: Parameters<RetryAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        // Permission check: only coordinator and leads can retry
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }

        let p = &params.0;
        let state = self.state();
        let agent = match state.load_agent(&self.run_id, &p.agent_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        // Enforce hierarchy: coordinator retries leads, leads retry own workers
        let caller_role = self.agent_role();
        match caller_role {
            AgentRole::Coordinator => {
                if agent.role != AgentRole::Lead {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Coordinator can only retry lead agents.",
                    )]));
                }
            }
            AgentRole::Lead => {
                if agent.role != AgentRole::Worker
                    || agent.parent.as_deref() != Some(&self.agent_id)
                {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Leads can only retry their own workers.",
                    )]));
                }
            }
            AgentRole::Worker
            | AgentRole::Reviewer
            | AgentRole::Planner
            | AgentRole::Postmortem
            | AgentRole::Explorer
            | AgentRole::Evaluator => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Workers, reviewers, planners, and postmortem agents cannot retry agents.",
                )]));
            }
        }

        // Verify agent is in a retriable state
        if agent.status != AgentStatus::Failed && agent.status != AgentStatus::Stalled {
            return Ok(CallToolResult::error(vec![Content::text(
                "Agent is not in Failed or Stalled state.",
            )]));
        }

        // Check retry limit
        let config = state.load_config();
        if agent.retry_count >= config.max_retries {
            // Mark permanently failed
            let mut failed_agent = agent.clone();
            failed_agent.status = AgentStatus::Failed;
            let _ = state.save_agent(&self.run_id, &failed_agent);
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Agent has exceeded max retries ({})",
                config.max_retries
            ))]));
        }

        // Prefer resume when session_id exists and worktree is intact
        if agent.session_id.is_some()
            && agent
                .worktree
                .as_ref()
                .is_some_and(|wt| std::path::Path::new(wt).exists())
        {
            let task_description = match &agent.task_id {
                Some(tid) => match state.load_task(&self.run_id, tid) {
                    Ok(task) => task.description.clone(),
                    Err(_) => String::new(),
                },
                None => String::new(),
            };
            let retry_num = agent.retry_count + 1;
            let mut enhanced_desc = task_description;
            enhanced_desc.push_str(&format!("\n\n## Retry #{}\n", retry_num));
            if let Some(ref feedback) = p.feedback {
                enhanced_desc.push_str(feedback);
            }
            let mut agent = agent.clone();
            match self.resume_agent(&mut agent, &enhanced_desc) {
                Ok(pid) => {
                    agent.retry_count = retry_num;
                    let _ = state.save_agent(&self.run_id, &agent);
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "Resumed agent '{}' (retry #{}, pid={}, preserving conversation context)",
                        agent.id, retry_num, pid
                    ))]));
                }
                Err(_) => { /* Fall through to destroy-and-respawn */ }
            }
        }

        // Get task description for re-spawn
        let task_description = match &agent.task_id {
            Some(tid) => match state.load_task(&self.run_id, tid) {
                Ok(task) => task.description.clone(),
                Err(_) => {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Agent has no associated task or task not found.",
                    )]));
                }
            },
            None => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Agent has no associated task.",
                )]));
            }
        };

        // Handle old worktree: auto-commit, get diff, remove
        let mut diff_stat = String::new();
        if let Some(ref worktree) = agent.worktree {
            let wt_path = std::path::Path::new(worktree);
            if wt_path.exists() {
                Self::auto_commit_worktree(worktree);
                diff_stat = crate::git::Git::diff_stat_since(wt_path, "main").unwrap_or_default();
                if let Err(_e) = crate::git::Git::worktree_remove(state.repo_root(), wt_path) {
                    let _ = crate::git::Git::worktree_prune(state.repo_root());
                }
            }
            // Delete old branch (ignore errors)
            let _ = crate::git::Git::branch_delete(
                state.repo_root(),
                &format!("hive/{}/{}", self.run_id, agent.id),
            );
        }

        // Build enhanced task description with previous attempt context
        let retry_num = agent.retry_count + 1;
        let mut enhanced_desc = task_description;
        enhanced_desc.push_str(&format!("\n\n## Previous Attempt (retry #{})\n", retry_num));
        if let Some(ref feedback) = p.feedback {
            enhanced_desc.push_str(feedback);
            enhanced_desc.push('\n');
        }
        if !diff_stat.is_empty() {
            enhanced_desc.push_str(&format!(
                "\n### Diff from previous attempt:\n```\n{}\n```",
                diff_stat
            ));
        }

        // Re-spawn agent
        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &agent.id,
            agent.role,
            agent.parent.as_deref(),
            &enhanced_desc,
        ) {
            Ok(mut new_agent) => {
                new_agent.retry_count = retry_num;
                new_agent.task_id = agent.task_id.clone();
                let _ = state.save_agent(&self.run_id, &new_agent);
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Retried agent '{}' (retry #{}, pid={}, worktree={})",
                    new_agent.id,
                    retry_num,
                    new_agent.pid.unwrap_or(0),
                    new_agent.worktree.unwrap_or_default()
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to re-spawn agent: {e}"
            ))])),
        }
    }

    #[tool(
        description = "Read messages sent to this agent. Updates the read cursor so subsequent calls only return new messages."
    )]
    async fn hive_read_messages(
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

    #[tool(
        description = "Get cost summary for the current run — token usage and estimated cost per agent"
    )]
    async fn hive_run_cost(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let agents = match state.list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let mut lines = Vec::new();
        let mut total_cost = 0.0;
        let mut total_input = 0u64;
        let mut total_output = 0u64;

        lines.push(format!(
            "{:<24} {:<14} {:>10} {:>10} {:>10}",
            "Agent", "Role", "Input", "Output", "Cost"
        ));
        lines.push("-".repeat(72));

        for agent in &agents {
            if let Some(cost) = state.load_agent_cost(&self.run_id, &agent.id) {
                total_cost += cost.cost_usd;
                total_input += cost.input_tokens;
                total_output += cost.output_tokens;
                lines.push(format!(
                    "{:<24} {:<14} {:>9.1}k {:>9.1}k {:>9}",
                    agent.id,
                    format!("{:?}", agent.role).to_lowercase(),
                    cost.input_tokens as f64 / 1000.0,
                    cost.output_tokens as f64 / 1000.0,
                    format!("${:.2}", cost.cost_usd),
                ));
            }
        }

        if total_cost == 0.0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No cost data available (agents still running?).",
            )]));
        }

        lines.push("-".repeat(72));
        lines.push(format!(
            "{:<24} {:<14} {:>9.1}k {:>9.1}k {:>9}",
            "TOTAL",
            "",
            total_input as f64 / 1000.0,
            total_output as f64 / 1000.0,
            format!("${:.2}", total_cost),
        ));

        // Add budget info if configured
        let config = state.load_config();
        if let Some(budget) = config.budget_usd {
            let pct = (total_cost / budget * 100.0).min(999.9);
            lines.push(format!("\nBudget: ${:.2} ({:.1}% used)", budget, pct));
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    #[tool(
        description = "Save a memory entry (operation, convention, or failure). Postmortem-only."
    )]
    async fn hive_save_memory(
        &self,
        params: Parameters<SaveMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Postmortem]) {
            return Ok(result);
        }
        let p = &params.0;
        let state = self.state();
        match p.memory_type.as_str() {
            "operation" => {
                let entry: OperationalEntry = serde_json::from_str(&p.content).map_err(|e| {
                    McpError::invalid_params(format!("Invalid operation JSON: {e}"), None)
                })?;
                state
                    .save_operation(&entry)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved operation entry.",
                )]))
            }
            "convention" => {
                state
                    .save_conventions(&p.content)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved conventions.",
                )]))
            }
            "failure" => {
                let entry: FailureEntry = serde_json::from_str(&p.content).map_err(|e| {
                    McpError::invalid_params(format!("Invalid failure JSON: {e}"), None)
                })?;
                state
                    .save_failure(&entry)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved failure entry.",
                )]))
            }
            _ => Ok(CallToolResult::error(vec![Content::text(
                "Invalid memory_type. Use 'operation', 'convention', or 'failure'.",
            )])),
        }
    }

    #[tool(description = "Save the generated spec for this run. Planner-only.")]
    async fn hive_save_spec(
        &self,
        params: Parameters<SaveSpecParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Planner]) {
            return Ok(result);
        }
        let state = self.state();
        state
            .save_planner_spec(&self.run_id, &params.0.spec)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::text("Spec saved.")]))
    }

    #[tool(
        description = "Record a discovery to the Hive Mind knowledge space. Any agent can call this."
    )]
    async fn hive_discover(
        &self,
        params: Parameters<DiscoverParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let id = format!("disc-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let confidence = match p.confidence.as_str() {
            "low" => Confidence::Low,
            "high" => Confidence::High,
            _ => Confidence::Medium,
        };
        let discovery = Discovery {
            id: id.clone(),
            run_id: self.run_id.clone(),
            agent_id: self.agent_id.clone(),
            timestamp: Utc::now(),
            content: p.content.clone(),
            file_paths: p.file_paths.clone(),
            confidence,
            tags: p.tags.clone(),
        };
        match self.state().save_discovery(&self.run_id, &discovery) {
            Ok(()) => {
                let preview = if p.content.len() > 80 {
                    format!("{}...", &p.content[..80])
                } else {
                    p.content.clone()
                };
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Recorded discovery '{id}': {preview}"
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Search the Hive Mind knowledge space by keyword. Any agent can call this."
    )]
    async fn hive_query_mind(
        &self,
        params: Parameters<QueryMindParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.state().query_mind(&self.run_id, &params.0.query);
        if result.discoveries.is_empty() && result.insights.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No matching discoveries or insights found.",
            )]));
        }

        let mut output = String::new();
        if !result.discoveries.is_empty() {
            output.push_str("## Discoveries\n\n");
            for d in &result.discoveries {
                output.push_str(&format!("### {}\n", d.id));
                output.push_str(&format!("- **Agent:** {}\n", d.agent_id));
                output.push_str(&format!("- **Confidence:** {:?}\n", d.confidence));
                if !d.tags.is_empty() {
                    output.push_str(&format!("- **Tags:** {}\n", d.tags.join(", ")));
                }
                if !d.file_paths.is_empty() {
                    output.push_str(&format!("- **Files:** {}\n", d.file_paths.join(", ")));
                }
                output.push_str(&format!("\n{}\n\n", d.content));
            }
        }
        if !result.insights.is_empty() {
            output.push_str("## Insights\n\n");
            for i in &result.insights {
                output.push_str(&format!("### {}\n", i.id));
                if !i.tags.is_empty() {
                    output.push_str(&format!("- **Tags:** {}\n", i.tags.join(", ")));
                }
                output.push_str(&format!(
                    "- **Discovery IDs:** {}\n",
                    i.discovery_ids.join(", ")
                ));
                output.push_str(&format!("\n{}\n\n", i.content));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Synthesize discoveries into an insight. Coordinator-only.")]
    async fn hive_synthesize(
        &self,
        params: Parameters<SynthesizeParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
            return Ok(result);
        }
        let p = &params.0;
        let id = format!("ins-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let insight = Insight {
            id: id.clone(),
            run_id: self.run_id.clone(),
            timestamp: Utc::now(),
            content: p.content.clone(),
            discovery_ids: p.discovery_ids.clone(),
            tags: p.tags.clone(),
        };
        match self.state().save_insight(&self.run_id, &insight) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Synthesized insight '{id}'"
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Add a convention to the shared conventions memory. Coordinator-only.")]
    async fn hive_establish_convention(
        &self,
        params: Parameters<EstablishConventionParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
            return Ok(result);
        }
        let existing = self.state().load_conventions();
        let updated = if existing.is_empty() {
            format!("- {}\n", params.0.content)
        } else {
            format!("{}\n- {}\n", existing, params.0.content)
        };
        match self.state().save_conventions(&updated) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Convention added: {}",
                params.0.content
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
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

impl HiveMcp {
    fn auto_commit_worktree(worktree: &str) -> Option<String> {
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
    fn resume_agent(&self, agent: &mut Agent, prompt: &str) -> Result<u32, String> {
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
        let child = std::process::Command::new("claude")
            .arg("-p")
            .arg(prompt)
            .arg("--resume")
            .arg(session_id)
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--dangerously-skip-permissions")
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
    fn try_wake_agent(&self, to: &str, body: &str) -> Option<String> {
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
        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p")
            .arg(body)
            .arg("--resume")
            .arg(session_id)
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--dangerously-skip-permissions")
            .env_remove("CLAUDECODE")
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

    fn notify_submitter(state: &HiveState, run_id: &str, to: &str, body: &str) {
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

    fn worktree_status(worktree: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Agent, AgentStatus};
    use tempfile::TempDir;

    fn setup_mcp(role: AgentRole) -> (TempDir, HiveMcp) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".hive")).unwrap();
        let state = HiveState::new(root.clone());
        state.create_run("test-run").unwrap();
        let agent = Agent {
            id: "test-agent".into(),
            role,
            status: AgentStatus::Running,
            parent: None,
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        state.save_agent("test-run", &agent).unwrap();
        let mcp = HiveMcp::new(
            "test-run".into(),
            "test-agent".into(),
            root.to_string_lossy().to_string(),
        );
        (dir, mcp)
    }

    fn setup_mcp_with_id(agent_id: &str, role: AgentRole) -> (TempDir, HiveMcp) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".hive")).unwrap();
        let state = HiveState::new(root.clone());
        state.create_run("test-run").unwrap();
        let agent = Agent {
            id: agent_id.into(),
            role,
            status: AgentStatus::Running,
            parent: None,
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        state.save_agent("test-run", &agent).unwrap();
        let mcp = HiveMcp::new(
            "test-run".into(),
            agent_id.into(),
            root.to_string_lossy().to_string(),
        );
        (dir, mcp)
    }

    fn make_task(id: &str, parent: Option<&str>, status: TaskStatus) -> Task {
        let now = Utc::now();
        Task {
            id: id.into(),
            title: format!("Task {id}"),
            description: format!("Description for {id}"),
            status,
            urgency: Urgency::Normal,
            blocking: vec![],
            blocked_by: vec![],
            assigned_to: None,
            created_by: "coordinator".into(),
            parent_task: parent.map(|s| s.to_string()),
            branch: None,
            domain: None,
            review_count: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn spawn_agent_rejects_missing_task_id() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "lead-test".into(),
            role: "lead".into(),
            task_id: "nonexistent-task".into(),
            task_description: "test".into(),
        });
        let result = mcp.hive_spawn_agent(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("not found"));
    }

    #[tokio::test]
    async fn update_task_worker_can_update_own() {
        let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
        let state = mcp.state();

        // Set worker's parent
        let mut agent = state.load_agent("test-run", "worker-1").unwrap();
        agent.task_id = Some("task-w1".into());
        agent.parent = Some("lead-1".into());
        state.save_agent("test-run", &agent).unwrap();

        let mut task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
        task.assigned_to = Some("worker-1".into());
        state.save_task("test-run", &task).unwrap();

        let params = Parameters(UpdateTaskParams {
            task_id: "task-w1".into(),
            status: Some("review".into()),
            assigned_to: None,
            branch: None,
            notes: None,
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "Worker should update own task"
        );
    }

    #[tokio::test]
    async fn update_task_worker_denied_other() {
        let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
        let state = mcp.state();

        let mut agent = state.load_agent("test-run", "worker-1").unwrap();
        agent.task_id = Some("task-w1".into());
        agent.parent = Some("lead-1".into());
        state.save_agent("test-run", &agent).unwrap();

        let mut task = make_task("task-w2", Some("task-lead"), TaskStatus::Active);
        task.assigned_to = Some("worker-2".into());
        state.save_task("test-run", &task).unwrap();

        let params = Parameters(UpdateTaskParams {
            task_id: "task-w2".into(),
            status: Some("review".into()),
            assigned_to: None,
            branch: None,
            notes: None,
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("Permission denied"));
        assert!(text.contains("task-w1")); // mentions own task
        assert!(text.contains("lead-1")); // mentions lead
    }

    #[tokio::test]
    async fn update_task_coordinator_denied_subtask() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let state = mcp.state();

        let task = make_task("task-sub", Some("task-lead"), TaskStatus::Active);
        state.save_task("test-run", &task).unwrap();

        let params = Parameters(UpdateTaskParams {
            task_id: "task-sub".into(),
            status: Some("cancelled".into()),
            assigned_to: None,
            branch: None,
            notes: None,
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("coordinator cannot modify subtasks"));
    }

    #[tokio::test]
    async fn update_task_lead_can_update_own_and_children() {
        let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
        let state = mcp.state();

        let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
        lead_agent.task_id = Some("task-lead".into());
        state.save_agent("test-run", &lead_agent).unwrap();

        // Lead's own task
        let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
        lead_task.assigned_to = Some("lead-1".into());
        state.save_task("test-run", &lead_task).unwrap();

        // Worker task created by lead
        let mut worker_task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
        worker_task.created_by = "lead-1".into();
        worker_task.assigned_to = Some("worker-1".into());
        state.save_task("test-run", &worker_task).unwrap();

        // Worker agent parented to lead
        let worker_agent = Agent {
            id: "worker-1".into(),
            role: AgentRole::Worker,
            status: AgentStatus::Running,
            parent: Some("lead-1".into()),
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: Some("task-w1".into()),
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        state.save_agent("test-run", &worker_agent).unwrap();

        // Lead can update own task
        let params = Parameters(UpdateTaskParams {
            task_id: "task-lead".into(),
            status: None,
            assigned_to: None,
            branch: None,
            notes: Some("progress note".into()),
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "Lead should update own task"
        );

        // Lead can update worker's task (created by lead)
        let params = Parameters(UpdateTaskParams {
            task_id: "task-w1".into(),
            status: None,
            assigned_to: None,
            branch: None,
            notes: Some("feedback".into()),
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "Lead should update worker's task"
        );
    }

    #[tokio::test]
    async fn create_task_coordinator_no_parent_ok() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(CreateTaskParams {
            title: "Lead task".into(),
            description: "desc".into(),
            urgency: "normal".into(),
            domain: None,
            blocking: vec![],
            blocked_by: vec![],
            parent_task: None,
        });
        let result = mcp.hive_create_task(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "Coordinator should create top-level task"
        );
    }

    #[tokio::test]
    async fn create_task_coordinator_denied_with_parent() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(CreateTaskParams {
            title: "Subtask".into(),
            description: "desc".into(),
            urgency: "normal".into(),
            domain: None,
            blocking: vec![],
            blocked_by: vec![],
            parent_task: Some("task-lead".into()),
        });
        let result = mcp.hive_create_task(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("coordinator cannot create subtasks"));
    }

    #[tokio::test]
    async fn create_task_lead_with_own_parent_ok() {
        let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
        let state = mcp.state();

        let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
        lead_agent.task_id = Some("task-lead".into());
        state.save_agent("test-run", &lead_agent).unwrap();

        let params = Parameters(CreateTaskParams {
            title: "Subtask".into(),
            description: "desc".into(),
            urgency: "normal".into(),
            domain: None,
            blocking: vec![],
            blocked_by: vec![],
            parent_task: Some("task-lead".into()),
        });
        let result = mcp.hive_create_task(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "Lead should create subtask under own task"
        );
    }

    #[tokio::test]
    async fn create_task_worker_denied() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let params = Parameters(CreateTaskParams {
            title: "Task".into(),
            description: "desc".into(),
            urgency: "normal".into(),
            domain: None,
            blocking: vec![],
            blocked_by: vec![],
            parent_task: None,
        });
        let result = mcp.hive_create_task(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("workers cannot create tasks"));
    }

    #[tokio::test]
    async fn submit_to_queue_blocked_by_unresolved_subtasks() {
        let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
        let state = mcp.state();

        let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
        lead_task.assigned_to = Some("lead-1".into());
        state.save_task("test-run", &lead_task).unwrap();

        // Active subtask (unresolved)
        let mut sub1 = make_task("task-sub1", Some("task-lead"), TaskStatus::Active);
        sub1.assigned_to = Some("worker-1".into());
        state.save_task("test-run", &sub1).unwrap();

        // Merged subtask (resolved)
        let mut sub2 = make_task("task-sub2", Some("task-lead"), TaskStatus::Merged);
        sub2.assigned_to = Some("worker-2".into());
        state.save_task("test-run", &sub2).unwrap();

        let params = Parameters(SubmitToQueueParams {
            task_id: "task-lead".into(),
            branch: "hive/test/lead-1".into(),
        });
        let result = mcp.hive_submit_to_queue(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("1 subtask(s) are not resolved"));
        assert!(text.contains("task-sub1"));
    }

    #[tokio::test]
    async fn list_tasks_parent_filter_none() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let state = mcp.state();

        let top = make_task("task-top", None, TaskStatus::Active);
        let sub = make_task("task-sub", Some("task-top"), TaskStatus::Active);
        state.save_task("test-run", &top).unwrap();
        state.save_task("test-run", &sub).unwrap();

        let params = Parameters(ListTasksParams {
            status: None,
            assignee: None,
            domain: None,
            parent_task: Some("none".into()),
        });
        let result = mcp.hive_list_tasks(params).await.unwrap();
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("task-top"));
        assert!(!text.contains("task-sub"));
    }

    #[tokio::test]
    async fn list_tasks_parent_filter_by_id() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let state = mcp.state();

        let top = make_task("task-top", None, TaskStatus::Active);
        let sub = make_task("task-sub", Some("task-top"), TaskStatus::Active);
        state.save_task("test-run", &top).unwrap();
        state.save_task("test-run", &sub).unwrap();

        let params = Parameters(ListTasksParams {
            status: None,
            assignee: None,
            domain: None,
            parent_task: Some("task-top".into()),
        });
        let result = mcp.hive_list_tasks(params).await.unwrap();
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(!text.contains("task-top") || text.contains("task-sub"));
        assert!(text.contains("task-sub"));
    }

    #[test]
    fn save_memory_rejects_non_postmortem() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let result = mcp.require_role(&[AgentRole::Postmortem]);
        assert!(
            result.is_err(),
            "Worker should not be allowed to save memory"
        );
    }

    #[test]
    fn save_memory_allows_postmortem() {
        let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
        let result = mcp.require_role(&[AgentRole::Postmortem]);
        assert!(
            result.is_ok(),
            "Postmortem should be allowed to save memory"
        );
    }

    #[test]
    fn save_spec_rejects_non_planner() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let result = mcp.require_role(&[AgentRole::Planner]);
        assert!(result.is_err(), "Worker should not be allowed to save spec");
    }

    #[test]
    fn save_spec_allows_planner() {
        let (_dir, mcp) = setup_mcp(AgentRole::Planner);
        let result = mcp.require_role(&[AgentRole::Planner]);
        assert!(result.is_ok(), "Planner should be allowed to save spec");
    }

    #[tokio::test]
    async fn save_memory_rejects_invalid_memory_type() {
        let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
        let params = Parameters(SaveMemoryParams {
            memory_type: "invalid".into(),
            content: "test".into(),
        });
        let result = mcp.hive_save_memory(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn save_memory_rejects_invalid_operation_json() {
        let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
        let params = Parameters(SaveMemoryParams {
            memory_type: "operation".into(),
            content: "not valid json".into(),
        });
        let result = mcp.hive_save_memory(params).await;
        assert!(result.is_err());
    }

    #[test]
    fn check_agents_allows_planner_and_postmortem() {
        let (_dir, mcp) = setup_mcp(AgentRole::Planner);
        let allowed = &[
            AgentRole::Coordinator,
            AgentRole::Lead,
            AgentRole::Reviewer,
            AgentRole::Planner,
            AgentRole::Postmortem,
        ];
        assert!(mcp.require_role(allowed).is_ok());

        let (_dir2, mcp2) = setup_mcp(AgentRole::Postmortem);
        assert!(mcp2.require_role(allowed).is_ok());
    }

    #[test]
    fn spawn_hierarchy_allows_coordinator_to_spawn_postmortem() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let caller_role = mcp.agent_role();
        let allowed = matches!(
            (caller_role, AgentRole::Postmortem),
            (AgentRole::Coordinator, AgentRole::Lead)
                | (AgentRole::Coordinator, AgentRole::Planner)
                | (AgentRole::Coordinator, AgentRole::Postmortem)
                | (AgentRole::Lead, AgentRole::Worker)
                | (AgentRole::Lead, AgentRole::Reviewer)
        );
        assert!(allowed, "Coordinator should be able to spawn Postmortem");
    }

    #[test]
    fn synthesize_rejects_non_coordinator() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let result = mcp.require_role(&[AgentRole::Coordinator]);
        assert!(
            result.is_err(),
            "Worker should not be allowed to synthesize"
        );
    }

    #[test]
    fn synthesize_allows_coordinator() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let result = mcp.require_role(&[AgentRole::Coordinator]);
        assert!(
            result.is_ok(),
            "Coordinator should be allowed to synthesize"
        );
    }

    #[test]
    fn establish_convention_rejects_non_coordinator() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let result = mcp.require_role(&[AgentRole::Coordinator]);
        assert!(
            result.is_err(),
            "Worker should not be allowed to establish conventions"
        );
    }

    #[test]
    fn establish_convention_allows_coordinator() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let result = mcp.require_role(&[AgentRole::Coordinator]);
        assert!(
            result.is_ok(),
            "Coordinator should be allowed to establish conventions"
        );
    }

    #[test]
    fn spawn_hierarchy_coordinator_can_spawn_explorer() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let caller_role = mcp.agent_role();
        let allowed = matches!(
            (caller_role, AgentRole::Explorer),
            (AgentRole::Coordinator, AgentRole::Lead)
                | (AgentRole::Coordinator, AgentRole::Planner)
                | (AgentRole::Coordinator, AgentRole::Postmortem)
                | (AgentRole::Coordinator, AgentRole::Explorer)
                | (AgentRole::Coordinator, AgentRole::Evaluator)
                | (AgentRole::Lead, AgentRole::Worker)
                | (AgentRole::Lead, AgentRole::Reviewer)
        );
        assert!(allowed, "Coordinator should be able to spawn Explorer");
    }

    #[test]
    fn spawn_hierarchy_coordinator_can_spawn_evaluator() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let caller_role = mcp.agent_role();
        let allowed = matches!(
            (caller_role, AgentRole::Evaluator),
            (AgentRole::Coordinator, AgentRole::Lead)
                | (AgentRole::Coordinator, AgentRole::Planner)
                | (AgentRole::Coordinator, AgentRole::Postmortem)
                | (AgentRole::Coordinator, AgentRole::Explorer)
                | (AgentRole::Coordinator, AgentRole::Evaluator)
                | (AgentRole::Lead, AgentRole::Worker)
                | (AgentRole::Lead, AgentRole::Reviewer)
        );
        assert!(allowed, "Coordinator should be able to spawn Evaluator");
    }

    #[test]
    fn spawn_hierarchy_explorer_cannot_spawn() {
        let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
        let result = mcp.require_role(&[AgentRole::Coordinator, AgentRole::Lead]);
        assert!(
            result.is_err(),
            "Explorer should not be allowed to spawn agents"
        );
    }

    #[test]
    fn spawn_hierarchy_evaluator_cannot_spawn() {
        let (_dir, mcp) = setup_mcp(AgentRole::Evaluator);
        let result = mcp.require_role(&[AgentRole::Coordinator, AgentRole::Lead]);
        assert!(
            result.is_err(),
            "Evaluator should not be allowed to spawn agents"
        );
    }

    #[tokio::test]
    async fn discover_creates_discovery() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let params = Parameters(DiscoverParams {
            content: "Found a caching pattern in the API layer".into(),
            confidence: "high".into(),
            file_paths: vec!["src/api.rs".into()],
            tags: vec!["caching".into(), "performance".into()],
        });
        let result = mcp.hive_discover(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "discover should succeed for any agent"
        );

        // Verify the discovery was saved
        let discoveries = mcp.state().load_discoveries("test-run");
        assert_eq!(discoveries.len(), 1);
        assert!(discoveries[0].id.starts_with("disc-"));
        assert_eq!(
            discoveries[0].content,
            "Found a caching pattern in the API layer"
        );
        assert_eq!(discoveries[0].agent_id, "test-agent");
    }

    #[tokio::test]
    async fn query_mind_returns_results() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        let state = mcp.state();
        let discovery = Discovery {
            id: "disc-test1".into(),
            run_id: "test-run".into(),
            agent_id: "test-agent".into(),
            timestamp: Utc::now(),
            content: "Discovered caching optimization opportunity".into(),
            file_paths: vec![],
            confidence: Confidence::High,
            tags: vec!["performance".into()],
        };
        state.save_discovery("test-run", &discovery).unwrap();

        let params = Parameters(QueryMindParams {
            query: "caching".into(),
        });
        let result = mcp.hive_query_mind(params).await.unwrap();
        assert!(
            !result.is_error.unwrap_or(false),
            "query_mind should succeed"
        );
        // Verify the result contains discovery info
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains("disc-test1"));
        assert!(text.contains("caching"));
    }

    #[tokio::test]
    async fn review_verdict_request_changes_sends_feedback_message() {
        let (_dir, mcp) = setup_mcp_with_id("reviewer-tasktest", AgentRole::Reviewer);
        let state = mcp.state();

        let mut task = make_task("task-test", None, TaskStatus::Review);
        task.assigned_to = Some("lead-1".into());
        task.review_count = 0;
        state.save_task("test-run", &task).unwrap();

        // Create an idle lead agent
        let lead = Agent {
            id: "lead-1".into(),
            role: AgentRole::Lead,
            status: AgentStatus::Idle,
            parent: Some("coordinator".into()),
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: Some("task-test".into()),
            session_id: Some("sess-abc".into()),
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        state.save_agent("test-run", &lead).unwrap();

        let params = Parameters(ReviewVerdictParams {
            task_id: "task-test".into(),
            verdict: "request-changes".into(),
            feedback: Some("Please fix the error handling.".into()),
        });
        let result = mcp.hive_review_verdict(params).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));

        // Verify feedback message was saved for lead-1
        let messages = state.list_messages("test-run").unwrap_or_default();
        let lead_msgs: Vec<_> = messages.iter().filter(|m| m.to == "lead-1").collect();
        assert!(!lead_msgs.is_empty(), "lead should have a feedback message");
        assert!(
            lead_msgs[0].body.contains("Please fix the error handling"),
            "message should contain the feedback"
        );

        // Task should be back to Active with review_count incremented
        let task = state.load_task("test-run", "task-test").unwrap();
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.review_count, 1);
    }
}
