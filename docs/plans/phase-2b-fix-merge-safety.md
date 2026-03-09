# Phase 2b: Fix Merge Safety Issues

**Goal:** Fix code quality issues found in the Phase 2 quality gates implementation. These are bugs that make the merge flow unreliable.

---

## Issue 1: Unsafe `git reset --hard` via shell command

**File:** `src/mcp.rs` (in `hive_merge_next`)
**File:** `src/git.rs`

Currently the verification failure path does:
```rust
let _ = crate::git::Git::run_shell_command(&repo_root, "git reset --hard HEAD~1");
```

Problems:
- Uses `run_shell_command` (sh -c) instead of a proper Git method
- Silently ignores failures with `let _`
- If reset fails, repo is left with a bad merge commit but task is marked failed

### Fix

**In `src/git.rs`**, add:
```rust
/// Reset the current branch hard to a given ref (e.g. "HEAD~1")
pub fn reset_hard(repo_root: &Path, to_ref: &str) -> Result<(), String> {
    Self::run(&["reset", "--hard", to_ref], repo_root)?;
    Ok(())
}
```

**In `src/mcp.rs`**, replace the `run_shell_command` call with:
```rust
if let Err(reset_err) = crate::git::Git::reset_hard(&repo_root, "HEAD~1") {
    // Reset failed — repo is in a bad state
    // Return error explaining the situation
    return Ok(CallToolResult::error(vec![Content::text(format!(
        "CRITICAL: Verification failed for '{}' AND reset failed: {reset_err}. Repository may be in inconsistent state. Manual intervention required.",
        entry.branch
    ))]));
}
```

### Tests

**In `src/git.rs`:**
- `reset_hard_undoes_commit` — make a commit, reset_hard HEAD~1, verify commit is gone
- `reset_hard_invalid_ref_fails` — reset to bogus ref returns error

---

## Issue 2: Post-merge notification should include verification output

Currently the notification sent to the lead after a failed merge just says it failed. It should include the verification command's stderr/stdout so the lead knows what went wrong.

**In `src/mcp.rs`**, the notification message for verification failure should include the verification error output (truncated to 500 chars to avoid message bloat).

---

## Issue 3: Verify command config should handle quoted multi-word values

The current parser handles `"` and `'` wrapping, but if someone writes:
```yaml
verify_command: "cargo test && cargo clippy"
```
It correctly strips quotes. But edge case: what about YAML-style multiline or no quotes with `&&`? The current parser handles the no-quotes case fine (`unwrap_or(value)` falls through to the raw value). This is actually OK for now — just add a test to verify.

**In `src/state.rs` tests**, add:
- `load_config_verify_command_with_ampersand` — verify `cargo test && cargo clippy` parses correctly without quotes
- `load_config_verify_command_with_quotes` — verify `"cargo test && cargo clippy"` strips quotes

---

## Lead Decomposition

This is a small, focused fix. One lead is sufficient.

**Lead 1: merge-fix** (domain: `merge-fix`, files: `src/git.rs`, `src/mcp.rs`, `src/state.rs`)
- Add `Git::reset_hard` method with tests
- Fix `hive_merge_next` to use `Git::reset_hard` and handle failure
- Improve notification messages to include verification output
- Add state.rs config parsing tests
- Run `cargo test --all-targets && cargo clippy --all-targets -- -D warnings` before finishing
