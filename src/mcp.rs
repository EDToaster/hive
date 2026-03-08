use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types::{
    AgentRole, AgentStatus, MergeQueueEntry, Message, MessageType, Task, TaskStatus, Urgency,
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

        match crate::agent::AgentSpawner::spawn(
            &self.state(),
            &self.run_id,
            &p.agent_id,
            role,
            Some(&self.agent_id),
            &p.task_description,
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
        }

        let state = self.state();
        if let Err(e) = state.save_message(&self.run_id, &message) {
            return Ok(CallToolResult::error(vec![Content::text(e)]));
        }

        // Auto-wake: if target agent is idle with a session_id, resume it
        let mut wake_info = None;
        if let Ok(mut target_agent) = state.load_agent(&self.run_id, &p.to)
            && target_agent.status == AgentStatus::Idle
            && let Some(ref session_id) = target_agent.session_id
        {
            // Spawn a --resume invocation
            let agent_output_dir = state.agents_dir(&self.run_id).join(&target_agent.id);
            let output_file = std::fs::File::create(agent_output_dir.join("output.json"))
                .map_err(|e| format!("Failed to create output file: {e}"));
            if let Ok(output_file) = output_file {
                let worktree = target_agent.worktree.clone().unwrap_or_default();
                let stderr_file = std::fs::File::create(agent_output_dir.join("stderr.log")).ok();
                let mut cmd = std::process::Command::new("claude");
                cmd.arg("-p")
                    .arg(&p.body)
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
                let result = cmd.spawn();
                match result {
                    Ok(child) => {
                        target_agent.status = AgentStatus::Running;
                        target_agent.pid = Some(child.id());
                        target_agent.heartbeat = Some(Utc::now());
                        let _ = state.save_agent(&self.run_id, &target_agent);
                        wake_info = Some(format!(" (woke agent '{}', pid {})", p.to, child.id()));
                    }
                    Err(e) => {
                        wake_info = Some(format!(" (failed to wake agent '{}': {e})", p.to));
                    }
                }
            }
        }

        let wake_suffix = wake_info.unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Sent message '{msg_id}' to '{}'{wake_suffix}",
            p.to
        ))]))
    }

    #[tool(description = "Submit an approved branch to the merge queue")]
    async fn hive_submit_to_queue(
        &self,
        params: Parameters<SubmitToQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Lead]) {
            return Ok(result);
        }
        let p = &params.0;
        let state = self.state();
        let mut queue = match state.load_merge_queue(&self.run_id) {
            Ok(q) => q,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        queue.entries.push(MergeQueueEntry {
            task_id: p.task_id.clone(),
            branch: p.branch.clone(),
            submitted_by: self.agent_id.clone(),
            submitted_at: Utc::now(),
        });

        match state.save_merge_queue(&self.run_id, &queue) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Submitted branch '{}' for task '{}' to merge queue (position: {})",
                p.branch,
                p.task_id,
                queue.entries.len()
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
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

        // Attempt merge
        let merge_result = crate::git::Git::merge(&repo_root, &entry.branch);

        // If merge failed, try auto-rebase then retry
        let merge_result = if let Err(merge_err) = merge_result {
            let _ = crate::git::Git::merge_abort(&repo_root);

            match crate::git::Git::rebase(&repo_root, &entry.branch, "main") {
                Ok(()) => {
                    // Rebase succeeded, retry merge
                    crate::git::Git::merge(&repo_root, &entry.branch)
                }
                Err(rebase_err) => {
                    let _ = crate::git::Git::rebase_abort(&repo_root);
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
                    Self::notify_submitter(
                        &state,
                        &self.run_id,
                        &entry.submitted_by,
                        &notify_msg,
                    );
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

                Self::notify_submitter(
                    &state,
                    &self.run_id,
                    &entry.submitted_by,
                    &msg,
                );

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
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
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
                    } else if heartbeat_age_secs
                        .is_some_and(|age| age > config.stall_timeout_seconds)
                    {
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
