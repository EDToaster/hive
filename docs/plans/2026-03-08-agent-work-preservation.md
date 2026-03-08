# Agent Work Preservation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Prevent lost work when hive agents exit by auto-committing uncommitted changes, reporting worktree status in health checks, and providing a review tool for inspecting agent output.

**Architecture:** Four changes to the hive codebase: (1) git helper functions for status/add/commit/log, (2) auto-commit + uncommitted_changes field in hive_check_agents, (3) new hive_review_agent MCP tool + CLI, (4) prompt updates telling agents to commit their work.

**Tech Stack:** Rust, git CLI (shelled out via `std::process::Command`), MCP via `rmcp` crate.

---

### Task 1: Add git helper functions

**Files:**
- Modify: `src/git.rs:6-93` (add methods to `impl Git`)
- Test: `src/git.rs:96-237` (add tests in existing `mod tests`)

**Step 1: Write failing tests for new git helpers**

Add these tests at the bottom of the `mod tests` block in `src/git.rs` (before the closing `}`):

```rust
#[test]
fn status_porcelain_empty_on_clean_repo() {
    let dir = init_test_repo();
    let status = Git::status_porcelain(dir.path()).unwrap();
    assert!(status.is_empty());
}

#[test]
fn status_porcelain_shows_changes() {
    let dir = init_test_repo();
    fs::write(dir.path().join("new.txt"), "hello").unwrap();
    let status = Git::status_porcelain(dir.path()).unwrap();
    assert!(status.contains("new.txt"));
}

#[test]
fn add_all_stages_files() {
    let dir = init_test_repo();
    fs::write(dir.path().join("new.txt"), "hello").unwrap();
    Git::add_all(dir.path()).unwrap();
    let status = Git::run(&["status", "--porcelain"], dir.path()).unwrap();
    assert!(status.contains("A  new.txt") || status.contains("A new.txt"));
}

#[test]
fn commit_with_message_creates_commit() {
    let dir = init_test_repo();
    fs::write(dir.path().join("file.txt"), "content").unwrap();
    Git::add_all(dir.path()).unwrap();
    Git::commit(dir.path(), "test commit").unwrap();
    let log = Git::run(&["log", "--oneline", "-1"], dir.path()).unwrap();
    assert!(log.contains("test commit"));
}

#[test]
fn commit_empty_repo_fails() {
    let dir = init_test_repo();
    assert!(Git::commit(dir.path(), "empty").is_err());
}

#[test]
fn log_oneline_since_returns_commits() {
    let dir = init_test_repo();
    let main = Git::current_branch(dir.path()).unwrap();
    Git::run(&["checkout", "-b", "feature"], dir.path()).unwrap();
    fs::write(dir.path().join("f.txt"), "x").unwrap();
    Git::add_all(dir.path()).unwrap();
    Git::commit(dir.path(), "feature work").unwrap();
    let log = Git::log_oneline_since(dir.path(), &main).unwrap();
    assert!(log.contains("feature work"));
}

#[test]
fn diff_stat_since_returns_stats() {
    let dir = init_test_repo();
    let main = Git::current_branch(dir.path()).unwrap();
    Git::run(&["checkout", "-b", "feature2"], dir.path()).unwrap();
    fs::write(dir.path().join("g.txt"), "y").unwrap();
    Git::add_all(dir.path()).unwrap();
    Git::commit(dir.path(), "feature2 work").unwrap();
    let stat = Git::diff_stat_since(dir.path(), &main).unwrap();
    assert!(stat.contains("g.txt"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --all-targets 2>&1 | grep "error\[E"`
Expected: Compilation errors — `status_porcelain`, `add_all`, `commit`, `log_oneline_since`, `diff_stat_since` not found.

**Step 3: Implement the git helpers**

Add these methods inside `impl Git` in `src/git.rs` (after the `branch_delete` method, before the closing `}`):

```rust
/// Get porcelain status (machine-readable) for a worktree
pub fn status_porcelain(cwd: &Path) -> Result<String, String> {
    Self::run(&["status", "--porcelain"], cwd)
}

/// Stage all changes (tracked and untracked)
pub fn add_all(cwd: &Path) -> Result<(), String> {
    Self::run(&["add", "-A"], cwd)?;
    Ok(())
}

/// Create a commit with the given message
pub fn commit(cwd: &Path, message: &str) -> Result<(), String> {
    Self::run(&["commit", "-m", message], cwd)?;
    Ok(())
}

/// Get one-line log of commits since a base branch
pub fn log_oneline_since(cwd: &Path, base: &str) -> Result<String, String> {
    Self::run(&["log", "--oneline", &format!("{base}..HEAD")], cwd)
}

/// Get diff --stat since a base branch
pub fn diff_stat_since(cwd: &Path, base: &str) -> Result<String, String> {
    Self::run(&["diff", "--stat", &format!("{base}..HEAD")], cwd)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --all-targets 2>&1 | tail -5`
Expected: All tests pass (68 existing + 7 new = 75).

**Step 5: Commit**

```bash
git add src/git.rs
git commit -m "feat: add git helpers for status, add, commit, log, and diff-stat"
```

---

### Task 2: Auto-commit on agent exit + uncommitted_changes in health report

**Files:**
- Modify: `src/mcp.rs:594-655` (the `hive_check_agents` method)

**Step 1: Write a helper function for auto-commit**

Add this private method to the second `impl HiveMcp` block (near `parse_session_id_from_output` around line 700):

```rust
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
```

**Step 2: Add auto-commit to the exit detection code paths**

In `hive_check_agents`, find the two exit detection blocks. In the first block (line ~599, `process_alive == Some(false) && agent.session_id.is_none()`), add the auto-commit call before the state transition:

```rust
if process_alive == Some(false) && agent.session_id.is_none() {
    // Auto-commit any uncommitted work before transitioning
    if let Some(ref wt) = agent.worktree {
        Self::auto_commit_worktree(wt);
    }
    // ... existing session_id capture logic ...
```

In the second block (line ~616, `process_alive == Some(false) && agent.session_id.is_some()`), add the same:

```rust
} else if process_alive == Some(false)
    && agent.session_id.is_some()
    && agent.status == AgentStatus::Running
{
    // Auto-commit any uncommitted work before transitioning
    if let Some(ref wt) = agent.worktree {
        Self::auto_commit_worktree(wt);
    }
    // ... existing idle transition logic ...
```

**Step 3: Add uncommitted_changes to the health report**

In the JSON report builder (around line 647), add the new field. Replace the existing `reports.push(serde_json::json!({...}))` with:

```rust
let uncommitted_changes = agent
    .worktree
    .as_deref()
    .and_then(Self::worktree_status);

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
```

**Step 4: Run tests**

Run: `cargo test --all-targets 2>&1 | tail -5`
Expected: All tests pass.

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -3`
Expected: Clean.

**Step 5: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: auto-commit on agent exit + uncommitted_changes in health reports"
```

---

### Task 3: hive_review_agent MCP tool

**Files:**
- Modify: `src/mcp.rs` (add params struct + tool method)

**Step 1: Add the params struct**

Add after `WaitForActivityParams` (around line 119):

```rust
#[derive(Deserialize, JsonSchema)]
pub struct ReviewAgentParams {
    /// Agent ID to review
    pub agent_id: String,
}
```

**Step 2: Add the tool method**

Add the tool method inside the `#[tool_router]` impl block, after `hive_wait_for_activity` (around line 700):

```rust
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
```

**Step 3: Run tests and lint**

Run: `cargo test --all-targets 2>&1 | tail -5`
Expected: All tests pass.

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -3`
Expected: Clean.

**Step 4: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: add hive_review_agent MCP tool for inspecting agent output"
```

---

### Task 4: hive review-agent CLI command

**Files:**
- Modify: `src/cli.rs` (add command variant)
- Modify: `src/main.rs` (add dispatch + handler)

**Step 1: Add CLI command**

In `src/cli.rs`, add before the `Stop` variant:

```rust
/// Review a non-running agent's work (commits, diff stat)
ReviewAgent {
    /// Agent ID to review
    agent_id: String,
    /// Run ID (defaults to active run)
    #[arg(long)]
    run: Option<String>,
},
```

**Step 2: Add dispatch and handler in main.rs**

In the `match cli.command` block, add:

```rust
Commands::ReviewAgent { agent_id, run } => cmd_review_agent(&agent_id, run),
```

Add the handler function (before `cmd_stop`):

```rust
fn cmd_review_agent(agent_id: &str, run: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };
    let agent = state.load_agent(&run_id, agent_id)?;

    if agent.status == AgentStatus::Running {
        return Err("Cannot review a running agent.".into());
    }

    let worktree = agent
        .worktree
        .as_deref()
        .ok_or("Agent has no worktree.")?;
    let wt_path = std::path::Path::new(worktree);
    if !wt_path.exists() {
        return Err("Agent worktree no longer exists.".into());
    }

    // Auto-commit any uncommitted work
    let status = crate::git::Git::status_porcelain(wt_path).unwrap_or_default();
    if !status.is_empty() {
        let _ = crate::git::Git::add_all(wt_path);
        let _ = crate::git::Git::commit(wt_path, "wip: salvaged by coordinator");
        println!("Auto-committed uncommitted changes.");
    }

    let branch = format!("hive/{run_id}/{agent_id}");
    let commits = crate::git::Git::log_oneline_since(wt_path, "main")
        .unwrap_or_else(|_| "(no commits)".to_string());
    let diff_stat = crate::git::Git::diff_stat_since(wt_path, "main")
        .unwrap_or_else(|_| "(no diff)".to_string());

    println!("Agent:  {}", agent.id);
    println!("Role:   {:?}", agent.role);
    println!("Status: {:?}", agent.status);
    println!("Branch: {branch}");
    if let Some(tid) = &agent.task_id {
        println!("Task:   {tid}");
    }
    println!("\n--- Commits ---\n{commits}");
    println!("\n--- Diff Stat ---\n{diff_stat}");
    Ok(())
}
```

Add `use crate::types::AgentStatus;` at the top of main.rs if not already imported.

**Step 3: Run tests and lint**

Run: `cargo test --all-targets 2>&1 | tail -5`
Expected: All tests pass.

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -3`
Expected: Clean.

**Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add hive review-agent CLI command"
```

---

### Task 5: Prompt updates

**Files:**
- Modify: `src/agent.rs:167-220` (lead and worker prompt templates)

**Step 1: Update lead prompt**

In `src/agent.rs`, in the `AgentRole::Lead` match arm, add these two lines to the Responsibilities section (after "Report progress to the coordinator"):

```
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
```

**Step 2: Update worker prompt**

In the `AgentRole::Worker` match arm, add the same two lines to the Responsibilities section (after "If you discover an unrelated bug"):

```
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
```

**Step 3: Update tests**

Add assertions to the existing `lead_prompt_contains_parent` and `worker_prompt_contains_parent_and_constraints` tests:

```rust
assert!(prompt.contains("Commit your work"));
```

**Step 4: Run tests**

Run: `cargo test --all-targets 2>&1 | tail -5`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add src/agent.rs
git commit -m "feat: add commit instructions to lead and worker prompts"
```
