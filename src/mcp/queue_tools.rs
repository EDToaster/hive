use super::HiveMcp;
use super::params::{ReviewVerdictParams, SubmitToQueueParams};
use crate::types::{AgentRole, MergeQueue, MergeQueueEntry, Message, MessageType, TaskStatus};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = queue_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(description = "Submit a review verdict for a task. Reviewer-only.")]
    pub(crate) async fn hive_review_verdict(
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
        description = "Submit a branch for review before merging. Spawns a reviewer agent to evaluate the changes. Leads submit their own branches; coordinators can submit explorer branches on behalf of the run."
    )]
    pub(crate) async fn hive_submit_to_queue(
        &self,
        params: Parameters<SubmitToQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Lead, AgentRole::Coordinator]) {
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
    pub(crate) async fn hive_merge_next(&self) -> Result<CallToolResult, McpError> {
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
        let target_branch = self.repo_current_branch();

        // Helper closure: mark task as failed
        let mark_failed = |state: &crate::state::HiveState, run_id: &str, task_id: &str| {
            if let Ok(mut task) = state.load_task(run_id, task_id) {
                task.status = TaskStatus::Failed;
                task.updated_at = Utc::now();
                let _ = state.save_task(run_id, &task);
            }
        };

        // Ensure we're on the target branch before merging
        if let Err(e) = crate::git::Git::checkout(&repo_root, &target_branch) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to checkout {target_branch} before merge: {e}"
            ))]));
        }

        // Attempt merge
        let merge_result = crate::git::Git::merge(&repo_root, &entry.branch);

        // If merge failed, try auto-rebase then retry
        let merge_result = if let Err(merge_err) = merge_result {
            let _ = crate::git::Git::merge_abort(&repo_root);

            match crate::git::Git::rebase(&repo_root, &entry.branch, &target_branch) {
                Ok(()) => {
                    // Rebase succeeded — HEAD is now on the rebased branch.
                    // Switch back to the target branch before retrying the merge.
                    if let Err(e) = crate::git::Git::checkout(&repo_root, &target_branch) {
                        mark_failed(&state, &self.run_id, &entry.task_id);
                        state.save_merge_queue(&self.run_id, &queue).ok();
                        let msg = format!(
                            "Rebase of '{}' succeeded but checkout {target_branch} failed: {e}",
                            entry.branch
                        );
                        Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                        return Ok(CallToolResult::error(vec![Content::text(msg)]));
                    }
                    crate::git::Git::merge(&repo_root, &entry.branch)
                }
                Err(rebase_err) => {
                    let _ = crate::git::Git::rebase_abort(&repo_root);
                    // Restore target branch after failed rebase
                    let _ = crate::git::Git::checkout(&repo_root, &target_branch);
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
}
