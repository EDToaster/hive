use super::HiveMcp;
use super::params::{CreateTaskParams, ListTasksParams, UpdateTaskParams};
use crate::types::{AgentRole, Task, TaskStatus, Urgency};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = task_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(
        description = "Create a new task with title, description, urgency, and optional blocking info"
    )]
    pub(crate) async fn hive_create_task(
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

        // Validate domain path exists in git
        if let Some(ref domain) = p.domain {
            let state = self.state();
            match crate::git::Git::validate_sparse_paths(state.repo_root(), &[domain.as_str()]) {
                Ok(invalid) if !invalid.is_empty() => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Domain path not found in repo: {domain}"
                    ))]));
                }
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Failed to validate domain path: {e}"
                    ))]));
                }
                _ => {}
            }
        }

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
            commit_message: None,
            submitted_by: None,
            created_at: now,
            updated_at: now,
        };

        match self.state().save_task(&self.run_id, &task) {
            Ok(()) => {
                self.append_event(
                    "task_created",
                    &task_id,
                    &format!("task created: {}", p.title),
                );
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Created task '{task_id}': {}",
                    p.title
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Update a task's status, assignment, or branch")]
    pub(crate) async fn hive_update_task(
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
            AgentRole::Worker | AgentRole::Explorer | AgentRole::Evaluator => {
                task.assigned_to.as_deref() == Some(caller_id.as_str())
            }
            AgentRole::Reviewer => {
                caller_agent.as_ref().and_then(|a| a.task_id.as_deref()) == Some(&task.id)
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
            Ok(()) => {
                self.append_event(
                    "task_changed",
                    &task.id,
                    &format!("status changed to {:?}", task.status),
                );
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Updated task '{}': status={:?}",
                    task.id, task.status
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List tasks, optionally filtered by status, assignee, or domain")]
    pub(crate) async fn hive_list_tasks(
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
}
