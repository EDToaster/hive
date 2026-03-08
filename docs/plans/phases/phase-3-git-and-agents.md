# Phase 3: Git and Agent Operations

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build git operations module (worktree, merge, branch) and the agent spawn sequence (worktree creation, config writing, Claude Code launch).

**Prerequisite:** Phase 2 complete (state layer working).

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

### Task 3.1: Git operations module

**Files:**
- Create: `src/git.rs`
- Modify: `src/main.rs` (add module)

**Step 1: Write the git module**

This module shells out to `git` for all operations. Each function runs a command and returns structured output.

```rust
// src/git.rs
use std::path::Path;
use std::process::Command;

pub struct Git;

impl Git {
    fn run(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(format!("git {} failed: {stderr}", args.join(" ")))
        }
    }

    /// Create a new worktree with a new branch
    pub fn worktree_add(repo_root: &Path, worktree_path: &Path, branch: &str) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                "-b",
                branch,
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Remove a worktree
    pub fn worktree_remove(repo_root: &Path, worktree_path: &Path) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "remove",
                &worktree_path.to_string_lossy(),
                "--force",
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Prune stale worktree metadata
    pub fn worktree_prune(repo_root: &Path) -> Result<(), String> {
        Self::run(&["worktree", "prune"], repo_root)?;
        Ok(())
    }

    /// Merge a branch into the current branch
    pub fn merge(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["merge", branch, "--no-ff"], repo_root)?;
        Ok(())
    }

    /// Abort a merge in progress
    pub fn merge_abort(repo_root: &Path) -> Result<(), String> {
        Self::run(&["merge", "--abort"], repo_root)?;
        Ok(())
    }

    /// Check if there are merge conflicts
    pub fn has_conflicts(repo_root: &Path) -> Result<bool, String> {
        let output = Self::run(&["diff", "--name-only", "--diff-filter=U"], repo_root)?;
        Ok(!output.is_empty())
    }

    /// Get the current branch name
    pub fn current_branch(repo_root: &Path) -> Result<String, String> {
        Self::run(&["branch", "--show-current"], repo_root)
    }

    /// Checkout a branch
    pub fn checkout(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["checkout", branch], repo_root)?;
        Ok(())
    }

    /// Delete a branch
    pub fn branch_delete(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["branch", "-D", branch], repo_root)?;
        Ok(())
    }
}
```

**Step 2: Add module to main.rs**

Add `mod git;` to `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add src/git.rs src/main.rs
git commit -m "feat: add git operations module (worktree, merge, branch)"
```

---

### Task 3.2: Agent spawning module

**Files:**
- Create: `src/agent.rs`
- Modify: `src/main.rs` (add module)

This module handles the full agent spawn sequence: create worktree, write config files, launch claude code, register agent.

**Step 1: Write the agent module**

```rust
// src/agent.rs
use crate::git::Git;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use std::fs;
use std::process::Command;

pub struct AgentSpawner;

impl AgentSpawner {
    /// Full spawn sequence for a lead or worker agent.
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
                            "hive log-tool --run {run_id} --agent {agent_id} --tool $TOOL_NAME --status $TOOL_STATUS"
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

        // Step 3: Write .claude/settings.local.json (MCP config)
        let settings_json = serde_json::json!({
            "mcpServers": {
                "hive": {
                    "command": "hive",
                    "args": ["mcp", "--run", run_id, "--agent", agent_id]
                }
            }
        });
        fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 4: Write CLAUDE.local.md
        let prompt = Self::generate_prompt(agent_id, role, parent, task_description);
        fs::write(worktree_path.join("CLAUDE.local.md"), &prompt)
            .map_err(|e| e.to_string())?;

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

    fn generate_prompt(
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
```

Note: add `libc` dependency for process checking.

**Step 2: Add libc dependency**

Add to `Cargo.toml` under `[dependencies]`:
```toml
libc = "0.2"
```

**Step 3: Add module to main.rs**

Add `mod agent;` to `src/main.rs`.

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add src/agent.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: add agent spawn module (worktree, config, claude launch)"
```

---

## Next Phase

Proceed to Phase 4: `docs/plans/phases/phase-4-cli.md`
