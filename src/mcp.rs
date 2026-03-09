use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types::{
    AgentRole, AgentStatus, FailureEntry, MergeQueue, MergeQueueEntry, Message, MessageType,
    OperationalEntry, Task, TaskStatus, Urgency,
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
    #[allow(dead_code)]
    task_id: Option<String>,
    tool_router: ToolRouter<Self>,
}

// --- Parameter types ---

#[derive(Deserialize, JsonSchema)]
pub struct SpawnAgentParams {
    /// Agent ID (unique name like "lead-backend" or "worker-001")
    pub agent_id: String,
    /// Role: "lead" or "worker"
    pub role: String,
    /// Task ID to assign (mutually exclusive with `task`)
    pub task_id: Option<String>,
    /// Inline task description — auto-creates a task (mutually exclusive with `task_id`)
    pub task: Option<String>,
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

#[tool_router]
impl HiveMcp {
    pub fn new(
        run_id: String,
        agent_id: String,
        repo_root: String,
        task_id: Option<String>,
    ) -> Self {
        Self {
            run_id,
            agent_id,
            repo_root,
            task_id,
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
        self.touch_heartbeat();
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }
        let p = &params.0;
        let role = match p.role.as_str() {
            "lead" => AgentRole::Lead,
            "worker" => AgentRole::Worker,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid role. Use 'lead' or 'worker'.",
                )]));
            }
        };

        // Enforce hierarchy: coordinators spawn leads, leads spawn workers
        let caller_role = self.agent_role();
        let allowed = matches!(
            (caller_role, role),
            (AgentRole::Coordinator, AgentRole::Lead) | (AgentRole::Lead, AgentRole::Worker)
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

        // Resolve task: either load existing or auto-create from inline description
        let mut task = match (&p.task_id, &p.task) {
            (Some(_), Some(_)) => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Provide either 'task_id' or 'task', not both.",
                )]));
            }
            (None, None) => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Provide either 'task_id' (existing task) or 'task' (auto-create).",
                )]));
            }
            (Some(tid), None) => {
                let t = match state.load_task(&self.run_id, tid) {
                    Ok(t) => t,
                    Err(e) => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "Failed to load task '{tid}': {e}"
                        ))]));
                    }
                };
                if let Some(ref assigned) = t.assigned_to
                    && assigned != &p.agent_id
                {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Task '{}' is already assigned to '{assigned}'.",
                        tid
                    ))]));
                }
                t
            }
            (None, Some(desc)) => {
                let task_id = format!("task-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                let now = Utc::now();
                Task {
                    id: task_id,
                    title: desc.clone(),
                    description: desc.clone(),
                    status: TaskStatus::Pending,
                    urgency: Urgency::Normal,
                    blocking: vec![],
                    blocked_by: vec![],
                    assigned_to: None,
                    created_by: self.agent_id.clone(),
                    parent_task: None,
                    branch: None,
                    domain: None,
                    review_count: 0,
                    created_at: now,
                    updated_at: now,
                }
            }
        };
        task.assigned_to = Some(p.agent_id.clone());
        task.status = TaskStatus::Active;
        task.updated_at = Utc::now();
        if let Err(e) = state.save_task(&self.run_id, &task) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to save task: {e}"
            ))]));
        }

        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &p.agent_id,
            role,
            Some(&self.agent_id),
            &task,
        ) {
            Ok(agent) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Spawned agent '{}' (role={:?}, worktree={})",
                agent.id,
                agent.role,
                agent.worktree.unwrap_or_default()
            ))])),
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
        self.touch_heartbeat();
        let p = &params.0;
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
        self.touch_heartbeat();
        let p = &params.0;
        let state = self.state();
        let _lock = state
            .lock_file(&format!("task-{}", p.task_id))
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        if let Some(ref status_str) = p.status {
            task.status = match status_str.as_str() {
                "pending" => TaskStatus::Pending,
                "active" => TaskStatus::Active,
                "blocked" => TaskStatus::Blocked,
                "review" => TaskStatus::Review,
                "approved" => TaskStatus::Approved,
                "queued" => TaskStatus::Queued,
                "merged" => TaskStatus::Merged,
                "skipped" => TaskStatus::Skipped,
                "failed" => TaskStatus::Failed,
                _ => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Invalid status: {status_str}"
                    ))]));
                }
            };
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
            Ok(()) => {
                // Notify parent when status becomes review
                if task.status == TaskStatus::Review
                    && let Ok(agent) = state.load_agent(&self.run_id, &self.agent_id)
                    && let Some(ref parent) = agent.parent
                {
                    self.notify_agent(
                        parent,
                        &format!(
                            "Worker {} completed task {} '{}'. Review the diff.",
                            self.agent_id, task.id, task.title
                        ),
                    );
                }

                // Auto-close subtasks when parent is merged or skipped
                if matches!(task.status, TaskStatus::Merged | TaskStatus::Skipped)
                    && let Ok(closed) = state.close_subtasks(&self.run_id, &task.id, task.status)
                {
                    for sub in &closed {
                        eprintln!(
                            "Auto-closed subtask {} '{}' as {:?}",
                            sub.id, sub.title, task.status
                        );
                    }
                }

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Updated task '{}': status={:?}",
                    task.id, task.status
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List tasks, optionally filtered by status, assignee, or domain")]
    async fn hive_list_tasks(
        &self,
        params: Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
            AgentRole::Reviewer | AgentRole::Planner | AgentRole::Postmortem => {
                // These roles can only message the coordinator
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

        // Auto-wake: if target agent is idle with a session_id, resume it
        let wake_info = Self::wake_idle_agent(&state, &self.run_id, &p.to, &p.body);
        let wake_suffix = wake_info.map(|w| format!(" {w}")).unwrap_or_default();
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
        self.touch_heartbeat();
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

                // Notify coordinator (with wake)
                let msg = format!(
                    "Review approved for task '{}'. Branch '{}' added to merge queue.",
                    p.task_id, branch
                );
                self.notify_agent("coordinator", &msg);

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

                // Send feedback to the agent that worked on this task (with wake)
                if let Some(ref assigned) = task.assigned_to {
                    self.notify_agent(
                        assigned,
                        &format!(
                            "Review feedback for task '{}' (review cycle {}):\n{}",
                            p.task_id, task.review_count, feedback
                        ),
                    );

                    // Also notify the parent lead
                    if let Ok(agent) = state.load_agent(&self.run_id, assigned)
                        && let Some(ref parent) = agent.parent
                    {
                        self.notify_agent(
                            parent,
                            &format!(
                                "Reviewer requested changes on task {}: {}",
                                p.task_id, feedback
                            ),
                        );
                    }
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
        self.touch_heartbeat();
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
        let review_task = Task {
            id: p.task_id.clone(),
            title: format!("Review: {}", task.title),
            description: review_description,
            status: TaskStatus::Review,
            urgency: task.urgency,
            blocking: vec![],
            blocked_by: vec![],
            assigned_to: Some(reviewer_id.clone()),
            created_by: self.agent_id.clone(),
            parent_task: None,
            branch: task.branch.clone(),
            domain: task.domain.clone(),
            review_count: task.review_count,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &reviewer_id,
            AgentRole::Reviewer,
            Some(&self.agent_id),
            &review_task,
        ) {
            Ok(_) => {
                // Set task_id on the reviewer agent
                if let Ok(mut reviewer_agent) = state.load_agent(&self.run_id, &reviewer_id) {
                    reviewer_agent.task_id = Some(p.task_id.clone());
                    let _ = state.save_agent(&self.run_id, &reviewer_agent);
                }

                // Notify coordinator about submission
                self.notify_agent(
                    "coordinator",
                    &format!(
                        "Lead {} submitted task {} '{}' for review.",
                        self.agent_id, p.task_id, task.title
                    ),
                );

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

                // Notify coordinator about direct submission
                self.notify_agent(
                    "coordinator",
                    &format!(
                        "Lead {} submitted task {} '{}' for merge (direct, reviewer spawn failed).",
                        self.agent_id, p.task_id, task.title
                    ),
                );

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
        self.touch_heartbeat();
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

                // Update task status and auto-close subtasks
                match state.load_task(&self.run_id, &entry.task_id) {
                    Ok(mut task) => {
                        task.status = TaskStatus::Merged;
                        task.updated_at = Utc::now();
                        if let Err(e) = state.save_task(&self.run_id, &task) {
                            warnings.push(format!("Warning: failed to update task status: {e}"));
                        }
                        if let Ok(closed) =
                            state.close_subtasks(&self.run_id, &entry.task_id, TaskStatus::Merged)
                        {
                            for sub in &closed {
                                eprintln!(
                                    "Auto-closed subtask {} '{}' as Merged",
                                    sub.id, sub.title
                                );
                            }
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

                // Unblock detection: notify agents whose tasks are now unblocked
                if let Ok(unblocked) =
                    state.find_newly_unblocked_tasks(&self.run_id, &entry.task_id)
                {
                    for unblocked_task in unblocked {
                        if let Some(ref agent_id) = unblocked_task.assigned_to {
                            Self::notify_and_wake(
                                &state,
                                &self.run_id,
                                "coordinator",
                                agent_id,
                                &format!(
                                    "Task '{}' '{}' is now unblocked. Proceed.",
                                    unblocked_task.id, unblocked_task.title
                                ),
                            );
                        }
                    }
                }

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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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

            // Session ID capture: if process exited and no session_id yet, parse output.json
            if process_alive == Some(false) && agent.session_id.is_none() {
                let output_path = state
                    .agents_dir(&self.run_id)
                    .join(&agent.id)
                    .join("output.json");
                if let Some(sid) = Self::parse_session_id_from_output(&output_path) {
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
                                .join("output.json");
                            if let Some(sid) = Self::parse_session_id_from_output(&output_path) {
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

    #[tool(
        description = "Block until activity is detected in the hive run directory, or timeout. Returns a summary of what changed."
    )]
    async fn hive_wait_for_activity(
        &self,
        params: Parameters<WaitForActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
            | AgentRole::Postmortem => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Only coordinators and leads can retry agents.",
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

        // Get task for re-spawn
        let mut retry_task = match &agent.task_id {
            Some(tid) => match state.load_task(&self.run_id, tid) {
                Ok(task) => task,
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
        let mut enhanced_desc = retry_task.description.clone();
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
        retry_task.description = enhanced_desc;

        // Re-spawn agent
        match crate::agent::AgentSpawner::spawn(
            &state,
            &self.run_id,
            &agent.id,
            agent.role,
            agent.parent.as_deref(),
            &retry_task,
        ) {
            Ok(mut new_agent) => {
                new_agent.retry_count = retry_num;
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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
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
        self.touch_heartbeat();
        if let Err(result) = self.require_role(&[AgentRole::Planner]) {
            return Ok(result);
        }
        let state = self.state();
        state
            .save_planner_spec(&self.run_id, &params.0.spec)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::text("Spec saved.")]))
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
    fn touch_heartbeat(&self) {
        let _ = self
            .state()
            .update_agent_heartbeat(&self.run_id, &self.agent_id);
    }

    fn wake_idle_agent(
        state: &HiveState,
        run_id: &str,
        agent_id: &str,
        message: &str,
    ) -> Option<String> {
        if let Ok(mut target_agent) = state.load_agent(run_id, agent_id)
            && target_agent.status == AgentStatus::Idle
            && let Some(ref session_id) = target_agent.session_id
        {
            let agent_output_dir = state.agents_dir(run_id).join(&target_agent.id);
            let output_file = std::fs::File::create(agent_output_dir.join("output.json")).ok()?;
            let worktree = target_agent.worktree.clone().unwrap_or_default();
            let stderr_file = std::fs::File::create(agent_output_dir.join("stderr.log")).ok();
            let mut cmd = std::process::Command::new("claude");
            cmd.arg("-p")
                .arg(message)
                .arg("--resume")
                .arg(session_id)
                .arg("--output-format")
                .arg("json")
                .arg("--dangerously-skip-permissions")
                .env_remove("CLAUDECODE")
                .current_dir(&worktree)
                .stdin(std::process::Stdio::null())
                .stdout(output_file);
            if let Some(f) = stderr_file {
                cmd.stderr(std::process::Stdio::from(f));
            }
            match cmd.spawn() {
                Ok(child) => {
                    target_agent.status = AgentStatus::Running;
                    target_agent.pid = Some(child.id());
                    target_agent.heartbeat = Some(Utc::now());
                    let _ = state.save_agent(run_id, &target_agent);
                    Some(format!("(woke agent '{}', pid {})", agent_id, child.id()))
                }
                Err(e) => Some(format!("(failed to wake agent '{}': {e})", agent_id)),
            }
        } else {
            None
        }
    }

    fn notify_agent(&self, target_agent_id: &str, message: &str) {
        let state = self.state();
        let msg = Message {
            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
            from: self.agent_id.clone(),
            to: target_agent_id.to_string(),
            timestamp: Utc::now(),
            message_type: MessageType::Info,
            body: message.to_string(),
            refs: vec![],
        };
        let _ = state.save_message(&self.run_id, &msg);
        Self::wake_idle_agent(&state, &self.run_id, target_agent_id, message);
    }

    fn notify_and_wake(
        state: &HiveState,
        run_id: &str,
        from: &str,
        target_agent_id: &str,
        message: &str,
    ) {
        let msg = Message {
            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
            from: from.to_string(),
            to: target_agent_id.to_string(),
            timestamp: Utc::now(),
            message_type: MessageType::Info,
            body: message.to_string(),
            refs: vec![],
        };
        let _ = state.save_message(run_id, &msg);
        Self::wake_idle_agent(state, run_id, target_agent_id, message);
    }

    fn parse_session_id_from_output(output_path: &std::path::Path) -> Option<String> {
        let data = std::fs::read_to_string(output_path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&data).ok()?;
        json.get("session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

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

    fn notify_submitter(state: &HiveState, run_id: &str, to: &str, body: &str) {
        Self::notify_and_wake(state, run_id, "coordinator", to, body);
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
    let server = HiveMcp::new(run_id.to_string(), agent_id.to_string(), repo_root, None);

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
            None,
        );
        (dir, mcp)
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
    fn spawn_hierarchy_coordinator_spawns_lead() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let caller_role = mcp.agent_role();
        let allowed = matches!(
            (caller_role, AgentRole::Lead),
            (AgentRole::Coordinator, AgentRole::Lead) | (AgentRole::Lead, AgentRole::Worker)
        );
        assert!(allowed, "Coordinator should be able to spawn Lead");
    }

    #[tokio::test]
    async fn spawn_rejects_reviewer_role() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "reviewer-1".into(),
            role: "reviewer".into(),
            task_id: Some("task-1".into()),
            task: None,
        });
        let result = mcp.hive_spawn_agent(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn spawn_rejects_planner_role() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "planner-1".into(),
            role: "planner".into(),
            task_id: Some("task-1".into()),
            task: None,
        });
        let result = mcp.hive_spawn_agent(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn spawn_rejects_both_task_id_and_task() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "lead-test".into(),
            role: "lead".into(),
            task_id: Some("task-123".into()),
            task: Some("Some text".into()),
        });
        let result = mcp.hive_spawn_agent(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn spawn_rejects_neither_task_id_nor_task() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "lead-test".into(),
            role: "lead".into(),
            task_id: None,
            task: None,
        });
        let result = mcp.hive_spawn_agent(params).await.unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn spawn_with_inline_task_creates_task() {
        let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
        let params = Parameters(SpawnAgentParams {
            agent_id: "lead-inline".into(),
            role: "lead".into(),
            task_id: None,
            task: Some("Implement the widget feature".into()),
        });
        // Spawn will fail (no git repo in test) but task should be created
        let _ = mcp.hive_spawn_agent(params).await.unwrap();
        let tasks = mcp.state().list_tasks(&mcp.run_id).unwrap_or_default();
        assert!(
            tasks
                .iter()
                .any(|t| t.title == "Implement the widget feature"),
            "Inline task should have been created"
        );
    }

    #[test]
    fn touch_heartbeat_updates_timestamp() {
        let (_dir, mcp) = setup_mcp(AgentRole::Worker);
        // Initial heartbeat should be None
        let state = mcp.state();
        let agent_before = state.load_agent("test-run", "test-agent").unwrap();
        assert!(agent_before.heartbeat.is_none());

        mcp.touch_heartbeat();

        let agent_after = state.load_agent("test-run", "test-agent").unwrap();
        assert!(agent_after.heartbeat.is_some());
    }
}
