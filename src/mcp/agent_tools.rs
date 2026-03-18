use super::HiveMcp;
use super::params::{RetryAgentParams, ReviewAgentParams, SpawnAgentParams, WaitForActivityParams};
use crate::types::{AgentRole, AgentStatus, TaskStatus, WorktreeStrategy};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = agent_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(description = "Spawn a new agent (lead or worker) with a worktree and task")]
    pub(crate) async fn hive_spawn_agent(
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
            "postmortem" => AgentRole::Postmortem,
            "explorer" => AgentRole::Explorer,
            "evaluator" => AgentRole::Evaluator,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid role. Use 'lead', 'worker', 'reviewer', 'postmortem', 'explorer', or 'evaluator'.",
                )]));
            }
        };

        // Enforce hierarchy: coordinators spawn leads, leads spawn workers
        let caller_role = self.agent_role();
        let allowed = matches!(
            (caller_role, role),
            (AgentRole::Coordinator, AgentRole::Lead)
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
        let task_description = task.description.clone();
        task.assigned_to = Some(p.agent_id.clone());
        task.status = TaskStatus::Active;
        task.updated_at = Utc::now();
        if let Err(e) = state.save_task(&self.run_id, &task) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to update task: {e}"
            ))]));
        }

        // Resolve worktree strategy (priority: global config > per-spawn > task domain > role config > role default)
        // Global override: if worktree_strategy is set in config.yaml, it wins unconditionally.
        let strategy_override = if config.global_worktree.is_some() {
            config.global_worktree.clone()
        } else {
            p.sparse_paths
                .as_ref()
                .map(|paths| {
                    if paths.is_empty() {
                        WorktreeStrategy::Full
                    } else {
                        WorktreeStrategy::Sparse {
                            paths: paths.clone(),
                        }
                    }
                })
                .or_else(|| {
                    // For workers with a task domain, use that domain as sparse path
                    if role == AgentRole::Worker {
                        task.domain.as_ref().map(|d| WorktreeStrategy::Sparse {
                            paths: vec![d.clone()],
                        })
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    // Check config-level per-role override (may differ from hardcoded role default)
                    let config_strategy = config.worktrees.resolve(role);
                    let default_strategy = WorktreeStrategy::default_for_role(role);
                    if config_strategy != default_strategy {
                        Some(config_strategy)
                    } else {
                        None
                    }
                })
        }; // end global_worktree else branch

        // Validate sparse paths exist in git
        if let Some(WorktreeStrategy::Sparse { ref paths }) = strategy_override {
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            match crate::git::Git::validate_sparse_paths(state.repo_root(), &path_refs) {
                Ok(invalid) if !invalid.is_empty() => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Sparse paths not found in repo: {}",
                        invalid.join(", ")
                    ))]));
                }
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Failed to validate sparse paths: {e}"
                    ))]));
                }
                _ => {}
            }
        }

        // Resolve source_paths for NoCheckout agents (reviewer, evaluator).
        // Priority: explicit param > auto-populate from parent worktree (for reviewers)
        let resolved_strategy = strategy_override
            .as_ref()
            .cloned()
            .unwrap_or_else(|| WorktreeStrategy::default_for_role(role));
        let source_paths = if let Some(paths) = &p.source_paths {
            paths.clone()
        } else if matches!(resolved_strategy, WorktreeStrategy::NoCheckout)
            && role == AgentRole::Reviewer
        {
            // Auto-populate: reviewer reads from the spawning lead's worktree
            state
                .load_agent(&self.run_id, &self.agent_id)
                .ok()
                .and_then(|a| a.worktree)
                .map(|w| vec![w])
                .unwrap_or_default()
        } else {
            vec![]
        };

        match crate::agent::AgentSpawner::spawn_with_options(
            &state,
            &self.run_id,
            &p.agent_id,
            role,
            Some(&self.agent_id),
            &task_description,
            p.model.as_deref(),
            strategy_override,
            source_paths,
        ) {
            Ok(agent) => {
                // Bind agent to task
                if let Ok(mut spawned_agent) = state.load_agent(&self.run_id, &agent.id) {
                    spawned_agent.task_id = Some(p.task_id.clone());
                    let _ = state.save_agent(&self.run_id, &spawned_agent);
                }
                self.append_event(
                    "task_changed",
                    &p.task_id,
                    &format!("activated, assigned to {}", p.agent_id),
                );
                self.append_event(
                    "agent_spawned",
                    &agent.id,
                    &format!("spawned {:?} for task {}", agent.role, p.task_id),
                );
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

    #[tool(description = "List all agents and their current status")]
    pub(crate) async fn hive_list_agents(&self) -> Result<CallToolResult, McpError> {
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
    pub(crate) async fn hive_check_agents(&self) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[
            AgentRole::Coordinator,
            AgentRole::Lead,
            AgentRole::Reviewer,
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
                    self.append_event(
                        "agent_changed",
                        &agent.id,
                        &format!(
                            "status changed to idle (task: {})",
                            agent.task_id.as_deref().unwrap_or("none")
                        ),
                    );
                    self.notify_parent_of_transition(&state, &agent);
                } else if agent.status == AgentStatus::Running {
                    // Process exited but no session_id found — mark as failed
                    agent.status = AgentStatus::Failed;
                    agent.pid = None;
                    let _ = state.save_agent(&self.run_id, &agent);
                    self.append_event(
                        "agent_changed",
                        &agent.id,
                        &format!(
                            "status changed to failed (task: {})",
                            agent.task_id.as_deref().unwrap_or("none")
                        ),
                    );
                    self.notify_parent_of_transition(&state, &agent);
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
                self.append_event(
                    "agent_changed",
                    &agent.id,
                    &format!(
                        "status changed to idle (task: {})",
                        agent.task_id.as_deref().unwrap_or("none")
                    ),
                );
                self.notify_parent_of_transition(&state, &agent);
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
                            if let Some(sid) =
                                crate::output::parse_session_id_from_output(&output_path)
                            {
                                agent.session_id = Some(sid);
                            }
                        }

                        // Update agent state
                        agent.status = AgentStatus::Stalled;
                        agent.pid = None;
                        let _ = state.save_agent(&self.run_id, &agent);
                        self.append_event(
                            "agent_changed",
                            &agent.id,
                            &format!(
                                "status changed to stalled (task: {})",
                                agent.task_id.as_deref().unwrap_or("none")
                            ),
                        );
                        self.notify_parent_of_transition(&state, &agent);

                        "stalled"
                    } else {
                        "running"
                    }
                }
            };

            let uncommitted_changes = agent.worktree.as_deref().and_then(Self::worktree_status);

            let (recent_commits, commit_count) = agent
                .worktree
                .as_deref()
                .map(|wt| {
                    let wt_path = std::path::Path::new(wt);
                    let base = agent.branched_from.as_deref().unwrap_or("main");
                    match crate::git::Git::log_oneline_since(wt_path, base) {
                        Ok(log) if !log.trim().is_empty() => {
                            let lines: Vec<&str> = log.trim().lines().collect();
                            let count = lines.len();
                            let mut commits: Vec<String> =
                                lines.iter().take(10).map(|s| s.to_string()).collect();
                            if count > 10 {
                                commits.push(format!("... and {} more", count - 10));
                            }
                            (Some(commits), count)
                        }
                        _ => (None, 0),
                    }
                })
                .unwrap_or((None, 0));

            reports.push(serde_json::json!({
                "agent_id": agent.id,
                "role": agent.role,
                "status": status,
                "session_id": agent.session_id,
                "last_heartbeat_age_secs": heartbeat_age_secs,
                "process_alive": process_alive,
                "idle_since_secs": idle_since_secs,
                "uncommitted_changes": uncommitted_changes,
                "recent_commits": recent_commits,
                "commit_count": commit_count,
            }));
        }

        let summary = serde_json::to_string_pretty(&reports).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(
        description = "Block until activity is detected in the hive run, or timeout. Returns a summary of what changed. The event cursor is automatically managed per-agent — consecutive calls will not miss events that occurred between calls."
    )]
    pub(crate) async fn hive_wait_for_activity(
        &self,
        params: Parameters<WaitForActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator, AgentRole::Lead]) {
            return Ok(result);
        }

        // Load the agent's stored cursor (default 0 for first call)
        let state = self.state();
        let cursor = state
            .load_agent(&self.run_id, &self.agent_id)
            .ok()
            .and_then(|a| a.wait_cursor)
            .unwrap_or(0);

        let result = crate::wait::wait_for_activity(
            self.repo_root.as_ref(),
            &self.run_id,
            params.0.timeout_secs,
            Some(&self.agent_id),
            cursor,
        )
        .await;

        match result {
            Ok((summary, new_cursor)) => {
                // Persist the new cursor so the next call picks up where this one left off
                if new_cursor != cursor
                    && let Ok(mut agent) = state.load_agent(&self.run_id, &self.agent_id)
                {
                    agent.wait_cursor = Some(new_cursor);
                    let _ = state.save_agent(&self.run_id, &agent);
                }
                Ok(CallToolResult::success(vec![Content::text(summary)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Review a non-running agent's work: commits any uncommitted changes, then returns branch info, commit log, and diff stat vs main."
    )]
    pub(crate) async fn hive_review_agent(
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
        let base = agent.branched_from.as_deref().unwrap_or("main");
        let commits = crate::git::Git::log_oneline_since(wt_path, base)
            .unwrap_or_else(|_| "(no commits)".to_string());
        let diff_stat = crate::git::Git::diff_stat_since(wt_path, base)
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

    #[tool(
        description = "Retry a failed agent by re-spawning it with the same task and a fresh worktree"
    )]
    pub(crate) async fn hive_retry_agent(
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
            | AgentRole::Postmortem
            | AgentRole::Explorer
            | AgentRole::Evaluator => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Workers, reviewers, and postmortem agents cannot retry agents.",
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
                let base = agent.branched_from.as_deref().unwrap_or("main");
                diff_stat = crate::git::Git::diff_stat_since(wt_path, base).unwrap_or_default();
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

        // Re-spawn agent (preserve original model)
        match crate::agent::AgentSpawner::spawn_with_model(
            &state,
            &self.run_id,
            &agent.id,
            agent.role,
            agent.parent.as_deref(),
            &enhanced_desc,
            agent.model.as_deref(),
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
}
