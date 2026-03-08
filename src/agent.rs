use crate::git::Git;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use std::fs;
use std::process::Command;

pub struct AgentSpawner;

impl AgentSpawner {
    /// Full spawn sequence for a lead or worker agent.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        state: &HiveState,
        run_id: &str,
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
    ) -> Result<Agent, String> {
        let worktree_path = state.worktree_path(run_id, agent_id);
        let branch = format!("hive/{run_id}/{agent_id}");

        // Step 1: Create worktree
        Git::worktree_add(state.repo_root(), &worktree_path, &branch)?;

        // Step 2: Write .claude/hooks.json
        let claude_dir = worktree_path.join(".claude");
        fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;

        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent {agent_id} --tool {{}} --status success"
                        )
                    }]
                }]
            }
        });
        fs::write(
            claude_dir.join("hooks.json"),
            serde_json::to_string_pretty(&hooks_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 3: Write .mcp.json at worktree root
        let mcp_json = serde_json::json!({
            "mcpServers": {
                "hive": {
                    "command": "hive",
                    "args": ["mcp", "--run", run_id, "--agent", agent_id]
                }
            }
        });
        fs::write(
            worktree_path.join(".mcp.json"),
            serde_json::to_string_pretty(&mcp_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 4: Write CLAUDE.local.md
        let prompt = Self::generate_prompt(agent_id, role, parent, task_description);
        fs::write(worktree_path.join("CLAUDE.local.md"), &prompt).map_err(|e| e.to_string())?;

        // Step 5: Launch claude code process
        let child = Command::new("claude")
            .arg("--print")
            .arg("--dangerously-skip-permissions")
            .arg(&prompt)
            .current_dir(&worktree_path)
            .spawn()
            .map_err(|e| format!("Failed to launch claude: {e}"))?;

        // Step 6: Register agent
        let agent = Agent {
            id: agent_id.to_string(),
            role,
            status: AgentStatus::Running,
            parent: parent.map(|s| s.to_string()),
            pid: Some(child.id()),
            worktree: Some(worktree_path.to_string_lossy().to_string()),
            heartbeat: Some(Utc::now()),
            task_id: None,
        };
        state.save_agent(run_id, &agent)?;

        Ok(agent)
    }

    pub fn coordinator_prompt(run_id: &str, spec_content: &str) -> String {
        format!(
            r#"You are the coordinator agent in a hive swarm.
Run ID: {run_id}
Agent ID: coordinator
Role: coordinator

## Spec
{spec_content}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.
"#
        )
    }

    pub(crate) fn generate_prompt(
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
    ) -> String {
        match role {
            AgentRole::Coordinator => format!(
                r#"You are the coordinator agent in a hive swarm.
Agent ID: {agent_id}
Role: coordinator

## Your Assignment
{task_description}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.
"#
            ),
            AgentRole::Lead => format!(
                r#"You are a lead agent in a hive swarm.
Agent ID: {agent_id}
Role: lead
Parent: {}

## Your Assignment
{task_description}

## Responsibilities
- Decompose your assignment into specific worker tasks.
- Spawn workers via hive_spawn_agent for each task.
- Review worker output when they submit for review.
- Send workers back with feedback if changes are needed.
- Submit approved branches to the merge queue via hive_submit_to_queue.
- Report progress to the coordinator via hive_send_message.

## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Worker => format!(
                r#"You are a worker agent in a hive swarm.
Agent ID: {agent_id}
Role: worker
Parent: {}

## Your Task
{task_description}

## Responsibilities
- Implement the task in your worktree.
- Run relevant tests and linters to verify your work.
- When done, call hive_update_task to set status to "review".
- If you discover an unrelated bug or issue, call hive_create_task
  with urgency and a description. It will be routed to your lead.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than your lead.
- Stay focused on your assigned task.
"#,
                parent.unwrap_or("unknown")
            ),
        }
    }

    /// Check if an agent process is still alive by PID
    pub fn is_alive(pid: u32) -> bool {
        // Send signal 0 to check if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_prompt_contains_role_and_id() {
        let prompt = AgentSpawner::generate_prompt(
            "coord-1",
            AgentRole::Coordinator,
            None,
            "Build a REST API",
        );
        assert!(prompt.contains("Agent ID: coord-1"));
        assert!(prompt.contains("Role: coordinator"));
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("Decompose the spec"));
        assert!(prompt.contains("Do NOT read or write implementation code"));
    }

    #[test]
    fn lead_prompt_contains_parent() {
        let prompt = AgentSpawner::generate_prompt(
            "lead-1",
            AgentRole::Lead,
            Some("coord-1"),
            "Handle backend domain",
        );
        assert!(prompt.contains("Agent ID: lead-1"));
        assert!(prompt.contains("Role: lead"));
        assert!(prompt.contains("Parent: coord-1"));
        assert!(prompt.contains("Handle backend domain"));
        assert!(prompt.contains("Spawn workers"));
        assert!(prompt.contains("Submit approved branches"));
    }

    #[test]
    fn lead_prompt_defaults_parent_to_coordinator() {
        let prompt = AgentSpawner::generate_prompt("lead-1", AgentRole::Lead, None, "task");
        assert!(prompt.contains("Parent: coordinator"));
    }

    #[test]
    fn worker_prompt_contains_parent_and_constraints() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement login endpoint",
        );
        assert!(prompt.contains("Agent ID: worker-1"));
        assert!(prompt.contains("Role: worker"));
        assert!(prompt.contains("Parent: lead-1"));
        assert!(prompt.contains("Implement login endpoint"));
        assert!(prompt.contains("Do not spawn other agents"));
        assert!(prompt.contains("Do not submit to the merge queue"));
    }

    #[test]
    fn worker_prompt_defaults_parent_to_unknown() {
        let prompt = AgentSpawner::generate_prompt("worker-1", AgentRole::Worker, None, "task");
        assert!(prompt.contains("Parent: unknown"));
    }

    #[test]
    fn is_alive_returns_true_for_current_process() {
        assert!(AgentSpawner::is_alive(std::process::id()));
    }

    #[test]
    fn is_alive_returns_false_for_bogus_pid() {
        assert!(!AgentSpawner::is_alive(99999999));
    }
}
