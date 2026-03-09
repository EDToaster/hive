# Phase 2: Quality Gates & Merge Hardening

**Goal:** Make hive runs reliable by verifying code before merging and handling merge conflicts automatically.

**Why first:** Without quality gates, hive merges untested code. Without conflict resolution, a single conflict stalls the entire queue. These two failures are the most common run-killers.

---

## Feature 1: Verification Command

### Config

Add `verify_command` to `.hive/config.yaml`:

```yaml
stall_timeout_seconds: 300
verify_command: "cargo test --all-targets && cargo clippy --all-targets -- -D warnings"
```

### Implementation

**File: `src/state.rs`** — Extend `HiveConfig`:

```rust
pub struct HiveConfig {
    pub stall_timeout_seconds: i64,
    pub verify_command: Option<String>,
}
```

Parse it from config.yaml the same way `stall_timeout_seconds` is parsed — simple line-by-line `strip_prefix("verify_command:")`.

**File: `src/git.rs`** — Add a verification runner:

```rust
/// Run a shell command in the given directory. Returns Ok(output) or Err(stderr).
pub fn run_shell_command(cwd: &Path, command: &str) -> Result<String, String>
```

This runs `sh -c <command>` in the given cwd and returns stdout on success, stderr on failure.

**File: `src/mcp.rs`** — Modify `hive_merge_next`:

After a successful `Git::merge()`, before marking the task as merged:
1. Load config via `state.load_config()`
2. If `verify_command` is Some, run it via `Git::run_shell_command(repo_root, &cmd)`
3. If verification fails:
   - Abort/reset the merge: `git reset --hard HEAD~1` (undo the merge commit)
   - Mark the task as `failed` with updated_at
   - Remove the entry from the queue (don't put it back)
   - Return an error with the verification output
4. If verification passes, proceed as normal (mark merged)

### Lead Decomposition

**Lead 1: Backend** (domain: `backend`, files: `src/state.rs`, `src/git.rs`)
- Add `verify_command` to `HiveConfig` struct and parsing
- Add `run_shell_command` to `Git`
- Add tests for both

**Lead 2: MCP** (domain: `mcp`, files: `src/mcp.rs`)
- Modify `hive_merge_next` to run verification after merge
- Handle verification failure (reset, mark failed, notify)
- Add integration-style test

---

## Feature 2: Auto-Rebase on Merge Conflict

When `hive_merge_next` encounters a merge conflict, instead of just aborting and putting the entry back:

1. Abort the merge
2. Attempt to rebase the branch onto main: `git rebase main <branch>` (from repo root)
3. If rebase succeeds (no conflicts):
   - Retry the merge
4. If rebase fails:
   - Abort the rebase
   - Mark the task as `failed`
   - Remove from queue
   - Return error with conflict details

### Implementation

**File: `src/git.rs`** — Add rebase operations:

```rust
/// Rebase a branch onto a target branch
pub fn rebase(repo_root: &Path, branch: &str, onto: &str) -> Result<(), String>

/// Abort a rebase in progress
pub fn rebase_abort(repo_root: &Path) -> Result<(), String>
```

**File: `src/mcp.rs`** — Modify the error path of `hive_merge_next`:

Replace the current "abort and put back" logic with:
1. Abort merge
2. Try rebase
3. If rebase succeeds, retry merge
4. If still fails, mark failed + remove from queue

### Lead Assignment

This work is small enough to add to Lead 1's scope (it's all in `src/git.rs` for the rebase operations). The MCP changes go to Lead 2.

---

## Feature 3: Post-Merge Notification

After processing a merge queue entry (success or failure), the coordinator should be able to inform the submitting lead of the result. Currently `hive_merge_next` returns the result to the coordinator, but the lead doesn't know.

### Implementation

**File: `src/mcp.rs`** — After merge (success or failure):
- Auto-send a message from coordinator to `entry.submitted_by` with the merge result
- This uses the existing `save_message` infrastructure
- The lead's Stop hook will pick it up and resume the lead

This goes to Lead 2's scope.

---

## Testing

All features must pass `cargo test --all-targets` and `cargo clippy --all-targets -- -D warnings` before merge.

### New tests needed:

**state.rs:**
- `load_config_reads_verify_command` — parses verify_command from config
- `load_config_verify_command_none_when_missing` — returns None when not in config

**git.rs:**
- `run_shell_command_success` — runs a simple command, returns stdout
- `run_shell_command_failure` — runs a failing command, returns stderr
- `rebase_onto_main_succeeds` — rebases a clean branch
- `rebase_conflicting_branch_fails` — rebase with conflicts returns error

**mcp.rs (integration-style):**
- Verification of the merge flow is harder to unit test since it requires a full MCP context. The leads should verify manually that `cargo test` passes end-to-end.
