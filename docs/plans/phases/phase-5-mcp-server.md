# Phase 5: MCP Server

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the MCP server with all hive tools (spawn, task CRUD, messages, merge queue, agents, logging) over stdio transport.

**Prerequisite:** Phase 4 complete (CLI skeleton working).

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

### Task 5.1: MCP server with all hive tools

**Files:**
- Create: `src/mcp.rs`
- Modify: `src/main.rs` (wire up `cmd_mcp`)

**Step 1: Write the MCP server module**

Reference the `rmcp` crate API: use `#[tool_router]` macro to define tools, `Parameters<T>` for structured input, `CallToolResult` for output. Serve over stdio.

```rust
// src/mcp.rs
use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::*;
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

    #[tool(description = "Spawn a new agent (lead or worker) with a worktree and task")]
    async fn hive_spawn_agent(
        &self,
        params: Parameters<SpawnAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let role = match p.role.as_str() {
            "lead" => AgentRole::Lead,
            "worker" => AgentRole::Worker,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid role. Use 'lead' or 'worker'.",
                )]))
            }
        };

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

    #[tool(description = "Create a new task with title, description, urgency, and optional blocking info")]
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
                    ))]))
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
                    let status_str = serde_json::to_value(&t.status)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                    if status_str.as_deref() != Some(s.as_str()) {
                        return false;
                    }
                }
                if let Some(ref a) = p.assignee {
                    if t.assigned_to.as_deref() != Some(a.as_str()) {
                        return false;
                    }
                }
                if let Some(ref d) = p.domain {
                    if t.domain.as_deref() != Some(d.as_str()) {
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

        match self.state().save_message(&self.run_id, &message) {
            Ok(()) => {
                // TODO: inject message into target agent's Claude Code conversation
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Sent message '{msg_id}' to '{}'",
                    p.to
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Submit an approved branch to the merge queue")]
    async fn hive_submit_to_queue(
        &self,
        params: Parameters<SubmitToQueueParams>,
    ) -> Result<CallToolResult, McpError> {
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

    #[tool(description = "Process the next item in the merge queue. Merges the branch into main and runs tests.")]
    async fn hive_merge_next(&self) -> Result<CallToolResult, McpError> {
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

        // Attempt merge
        let repo_root = state.repo_root();
        match crate::git::Git::merge(repo_root, &entry.branch) {
            Ok(()) => {
                // Update task status
                if let Ok(mut task) = state.load_task(&self.run_id, &entry.task_id) {
                    task.status = TaskStatus::Merged;
                    task.updated_at = Utc::now();
                    let _ = state.save_task(&self.run_id, &task);
                }
                state.save_merge_queue(&self.run_id, &queue).ok();
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Merged branch '{}' for task '{}'. Queue remaining: {}",
                    entry.branch,
                    entry.task_id,
                    queue.entries.len()
                ))]))
            }
            Err(e) => {
                // Abort the failed merge
                let _ = crate::git::Git::merge_abort(repo_root);
                // Put entry back at front
                queue.entries.insert(0, entry.clone());
                state.save_merge_queue(&self.run_id, &queue).ok();
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Merge failed for branch '{}': {e}. Entry remains in queue.",
                    entry.branch
                ))]))
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

    #[tool(description = "Check agent health by comparing heartbeats and verifying processes are alive")]
    async fn hive_check_agents(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let agents = match state.list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let mut report = Vec::new();
        for agent in &agents {
            let alive = agent
                .pid
                .map(|pid| crate::agent::AgentSpawner::is_alive(pid))
                .unwrap_or(false);
            let stalled = agent.heartbeat.is_some_and(|hb| {
                (Utc::now() - hb).num_seconds() > 300 // 5 min threshold
            });

            let status_note = if !alive && agent.status == AgentStatus::Running {
                "DEAD (process not found)"
            } else if stalled {
                "STALLED (no heartbeat > 5min)"
            } else {
                "OK"
            };

            report.push(format!(
                "{}: {:?} - {status_note}",
                agent.id, agent.status
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(
            report.join("\n"),
        )]))
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
}

/// Start the MCP server on stdio
pub async fn run_mcp_server(run_id: &str, agent_id: &str) -> Result<(), String> {
    let repo_root = HiveState::discover()?.repo_root().to_string_lossy().to_string();
    let server = HiveMcp::new(run_id.to_string(), agent_id.to_string(), repo_root);

    let (stdin, stdout) = rmcp::transport::io::stdio();
    server
        .serve((stdin, stdout))
        .await
        .map_err(|e| format!("MCP server error: {e}"))?;

    Ok(())
}
```

**Step 2: Wire up cmd_mcp in main.rs**

Replace the `cmd_mcp` function:

```rust
fn cmd_mcp(run_id: &str, agent_id: &str) -> Result<(), String> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(crate::mcp::run_mcp_server(run_id, agent_id))
}
```

Add `mod mcp;` to the top of `main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles. There may be some API mismatches with rmcp that need adjusting — the exact `#[tool_router]` and `serve` API should be checked against rmcp 1.1.0 docs. Adjust as needed.

**Step 4: Commit**

```bash
git add src/mcp.rs src/main.rs
git commit -m "feat: add MCP server with all hive tools (spawn, task, message, merge, observe)"
```

---

## Next Phase

Proceed to Phase 6: `docs/plans/phases/phase-6-tui.md`
