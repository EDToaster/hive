# Remove Hardcoded "main" Branch References

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace all hardcoded `"main"` branch references so hive works on any branch (e.g., feature branches).

**Architecture:** Add a `branched_from: Option<String>` field to the `Agent` struct, populated at spawn time. For merge operations, read the current branch of the repo root at runtime via `Git::current_branch`. For diff/log operations, use the target agent's `branched_from` field. CLI commands load agent metadata the same way.

**Tech Stack:** Rust, serde (with `#[serde(default)]` for backwards compat)

---

### Task 1: Add `branched_from` field to Agent struct

**Files:**
- Modify: `src/types.rs:62-90` (Agent struct)
- Modify: `src/mcp/tests.rs:18-32` (setup_mcp Agent literal)
- Modify: `src/mcp/tests.rs:48-62` (setup_mcp_with_id Agent literal)

**Step 1: Add the field to Agent**

In `src/types.rs`, find the `Agent` struct and add `branched_from` after `model`:

```rust
    pub model: Option<ModelTier>,
    #[serde(default)]
    pub branched_from: Option<String>,
```

`#[serde(default)]` ensures existing `.hive/` agent JSON without this field deserializes as `None`.

**Step 2: Fix all Agent struct literals in tests**

Every `Agent { ... }` literal in `src/mcp/tests.rs` needs `branched_from: None`. There are two helper functions that construct Agent literals — `setup_mcp` (line 18) and `setup_mcp_with_id` (line 48). Add `branched_from: None` to both.

Grep for any other `Agent {` struct literals across the codebase and fix them too.

**Step 3: Run tests to verify compilation**

Run: `cargo test --all-targets --no-run`
Expected: compiles successfully

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add branched_from field to Agent struct"
```

---

### Task 2: Populate `branched_from` at spawn time

**Files:**
- Modify: `src/agent.rs:35-60` (spawn_with_model)

**Step 1: Write the failing test**

In `src/agent.rs` (or a test module if one exists), add a test that spawns an agent and asserts `branched_from` is populated. However, since `AgentSpawner::spawn` requires a real git repo and runs `claude` as a subprocess, this is hard to unit test directly. Instead, we'll verify via an integration-style check after the implementation.

**Step 2: Store the resolved start point in the Agent**

In `src/agent.rs` `spawn_with_model`, after the worktree is created (line 60), resolve what the agent branched from and store it. The logic:

- If `start_point` is `Some(branch)` (worker branching from lead), use that branch name.
- If `start_point` is `None` (lead branching from HEAD), call `Git::current_branch(state.repo_root())` and use the result. Fall back to `"HEAD"` if that fails.

After the worktree creation block (after line 60), add:

```rust
        let branched_from = match &start_point {
            Some(sp) => sp.clone(),
            None => Git::current_branch(state.repo_root()).unwrap_or_else(|_| "HEAD".into()),
        };
```

Then when constructing the `Agent` struct later in the function, set:

```rust
            branched_from: Some(branched_from),
```

Find where the `Agent` struct is constructed in this function and add the field.

**Step 3: Run tests to verify compilation**

Run: `cargo test --all-targets --no-run`
Expected: compiles successfully

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: populate branched_from at agent spawn time"
```

---

### Task 3: Add `base_branch` helper to HiveMcp

**Files:**
- Modify: `src/mcp/mod.rs:49-58` (impl HiveMcp)

**Step 1: Write the helper**

Add a method to `HiveMcp` that reads the current branch of the repo root. This is used by `hive_merge_next` (coordinator merges into whatever branch the main worktree is on).

```rust
    /// Returns the current branch of the repo root (the merge target).
    pub(crate) fn repo_current_branch(&self) -> String {
        crate::git::Git::current_branch(std::path::Path::new(&self.repo_root))
            .unwrap_or_else(|_| "main".into())
    }
```

Also add a helper that gets the `branched_from` for a given agent (used by diff/log tools):

```rust
    /// Returns the branch an agent was created from, falling back to repo current branch.
    pub(crate) fn agent_base_branch(&self, agent_id: &str) -> String {
        self.state()
            .load_agent(&self.run_id, agent_id)
            .ok()
            .and_then(|a| a.branched_from)
            .unwrap_or_else(|| self.repo_current_branch())
    }
```

**Step 2: Run tests**

Run: `cargo test --all-targets --no-run`
Expected: compiles

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: add base branch helper methods to HiveMcp"
```

---

### Task 4: Update `hive_merge_next` to use dynamic branch

**Files:**
- Modify: `src/mcp/queue_tools.rs:320-353`

**Step 1: Write failing test**

Add to `src/mcp/tests.rs`:

```rust
#[tokio::test]
async fn merge_next_uses_repo_current_branch_not_hardcoded_main() {
    // This test verifies the code path doesn't reference "main" literally.
    // We test by reading the source and confirming no hardcoded "main" in merge logic.
    // Functional testing requires a real git repo, covered by existing integration tests.
    let source = std::fs::read_to_string("src/mcp/queue_tools.rs").unwrap();
    let merge_fn_start = source.find("fn hive_merge_next").unwrap();
    let merge_fn_body = &source[merge_fn_start..];
    // Find the end of the function (next `pub(crate) async fn` or end of impl)
    let fn_end = merge_fn_body[1..].find("pub(crate) async fn")
        .map(|i| i + 1)
        .unwrap_or(merge_fn_body.len());
    let merge_fn_body = &merge_fn_body[..fn_end];
    assert!(!merge_fn_body.contains("\"main\""),
        "hive_merge_next should not contain hardcoded \"main\" — use repo_current_branch() instead");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test merge_next_uses_repo_current_branch -v`
Expected: FAIL (the current code still has `"main"`)

**Step 3: Replace hardcoded "main" in hive_merge_next**

In `src/mcp/queue_tools.rs`, in the `hive_merge_next` function:

At the top of the function (after acquiring the lock and loading the queue, around line 309), resolve the target branch:

```rust
        let target_branch = self.repo_current_branch();
```

Then replace all 4 instances of `"main"` with `&target_branch`:

- Line 321: `crate::git::Git::checkout(&repo_root, "main")` → `crate::git::Git::checkout(&repo_root, &target_branch)`
- Line 334: `crate::git::Git::rebase(&repo_root, &entry.branch, "main")` → `crate::git::Git::rebase(&repo_root, &entry.branch, &target_branch)`
- Line 338: `crate::git::Git::checkout(&repo_root, "main")` → `crate::git::Git::checkout(&repo_root, &target_branch)`
- Line 353: `crate::git::Git::checkout(&repo_root, "main")` → `crate::git::Git::checkout(&repo_root, &target_branch)`

Also update the error/comment strings that say "main" to say the actual branch name, e.g.:
- `"Failed to checkout main before merge"` → `format!("Failed to checkout '{}' before merge: {e}", target_branch)`

**Step 4: Run test to verify it passes**

Run: `cargo test merge_next_uses_repo_current_branch -v`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test --all-targets`
Expected: all pass

**Step 6: Commit**

```bash
git add -A
git commit -m "feat: hive_merge_next uses dynamic target branch instead of hardcoded main"
```

---

### Task 5: Update `hive_check_agents` to use `agent.branched_from`

**Files:**
- Modify: `src/mcp/agent_tools.rs:270-290`

**Step 1: Replace hardcoded "main"**

In `hive_check_agents`, at line 276, replace:

```rust
match crate::git::Git::log_oneline_since(wt_path, "main") {
```

with:

```rust
let base = agent.branched_from.as_deref()
    .unwrap_or_else(|| "main");  // backwards compat for old agents
match crate::git::Git::log_oneline_since(wt_path, base) {
```

Note: we use the agent's own `branched_from`, not the caller's. The caller (coordinator/lead) is checking the health of *that* agent, so the diff should be relative to where *that* agent branched from.

**Step 2: Run tests**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: hive_check_agents uses agent.branched_from for diff base"
```

---

### Task 6: Update `hive_review_agent` to use `agent.branched_from`

**Files:**
- Modify: `src/mcp/agent_tools.rs:386-391`

**Step 1: Replace hardcoded "main"**

In `hive_review_agent`, around lines 388-390, replace:

```rust
let commits = crate::git::Git::log_oneline_since(wt_path, "main")
    .unwrap_or_else(|_| "(no commits)".to_string());
let diff_stat = crate::git::Git::diff_stat_since(wt_path, "main")
    .unwrap_or_else(|_| "(no diff)".to_string());
```

with:

```rust
let base = agent.branched_from.as_deref().unwrap_or("main");
let commits = crate::git::Git::log_oneline_since(wt_path, base)
    .unwrap_or_else(|_| "(no commits)".to_string());
let diff_stat = crate::git::Git::diff_stat_since(wt_path, base)
    .unwrap_or_else(|_| "(no diff)".to_string());
```

**Step 2: Run tests**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: hive_review_agent uses agent.branched_from for diff base"
```

---

### Task 7: Update `hive_retry_agent` to use `agent.branched_from`

**Files:**
- Modify: `src/mcp/agent_tools.rs:534`

**Step 1: Replace hardcoded "main"**

At line 534, replace:

```rust
diff_stat = crate::git::Git::diff_stat_since(wt_path, "main").unwrap_or_default();
```

with:

```rust
let base = agent.branched_from.as_deref().unwrap_or("main");
diff_stat = crate::git::Git::diff_stat_since(wt_path, base).unwrap_or_default();
```

**Step 2: Run tests**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: hive_retry_agent uses agent.branched_from for diff base"
```

---

### Task 8: Update CLI `cmd_review_agent` to use `agent.branched_from`

**Files:**
- Modify: `src/cli/agent_cmds.rs:174-178`

**Step 1: Replace hardcoded "main"**

At lines 175-178, replace:

```rust
let commits = crate::git::Git::log_oneline_since(wt_path, "main")
    .unwrap_or_else(|_| "(no commits)".to_string());
let diff_stat = crate::git::Git::diff_stat_since(wt_path, "main")
    .unwrap_or_else(|_| "(no diff)".to_string());
```

with:

```rust
let base = agent.branched_from.as_deref().unwrap_or("main");
let commits = crate::git::Git::log_oneline_since(wt_path, base)
    .unwrap_or_else(|_| "(no commits)".to_string());
let diff_stat = crate::git::Git::diff_stat_since(wt_path, base)
    .unwrap_or_else(|_| "(no diff)".to_string());
```

**Step 2: Run tests**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: CLI review-agent uses agent.branched_from for diff base"
```

---

### Task 9: Update CLI `cmd_status` to use repo current branch

**Files:**
- Modify: `src/cli/run_cmds.rs:356-366`

**Step 1: Replace hardcoded "main"**

The `hive status` command shows merged commits. It runs `git log --oneline --since=... main` on the repo root. This should use the current branch of the repo root (same logic as merge_next — it's showing what was merged into the working branch).

Replace lines 358-366:

```rust
    let current_branch = crate::git::Git::current_branch(state.repo_root())
        .unwrap_or_else(|_| "main".into());
    if let Ok(output) = std::process::Command::new("git")
        .args([
            "log",
            "--oneline",
            &format!("--since={}", since_date),
            &current_branch,
        ])
        .current_dir(state.repo_root())
        .output()
```

**Step 2: Run tests**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Commit**

```bash
git add -A
git commit -m "feat: CLI status uses current branch for merged commits log"
```

---

### Task 10: Run clippy and final verification

**Step 1: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings

**Step 2: Run full test suite**

Run: `cargo test --all-targets`
Expected: all pass

**Step 3: Check formatting**

Run: `cargo fmt --all -- --check`
Expected: no formatting issues

**Step 4: Grep for remaining hardcoded "main" references**

Run: `grep -rn '"main"' src/ --include='*.rs' | grep -v test | grep -v 'web.rs'`

Verify the only remaining `"main"` references are:
- `src/git.rs:161` — `git init -b main` (test helper, fine)
- Fallback strings like `unwrap_or("main")` or `unwrap_or_else(|_| "main".into())` (backwards compat, fine)
- `src/mcp/mod.rs` — the `repo_current_branch` fallback (fine)

No remaining `"main"` should appear as a direct branch target in business logic.

**Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: final cleanup for dynamic branch support"
```
