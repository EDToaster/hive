# Phase 4: Atomic State Operations

**Goal:** Prevent state corruption from concurrent agent writes by adding file locking to all state operations.

**Why:** Multiple agents write to `.hive/` concurrently (task updates, agent heartbeats, merge queue). Without locking, two concurrent writes to the same JSON file can corrupt it. This is especially dangerous for `merge-queue.json` which multiple leads may write to simultaneously.

---

## Approach: File Locking with `fs2`

Use the `fs2` crate for cross-platform advisory file locks. Each state write operation acquires an exclusive lock on a lockfile before reading/modifying/writing the target file.

### Implementation

**File: `Cargo.toml`** — Add dependency:
```toml
fs2 = "0.4"
```

**File: `src/state.rs`** — Add locking helpers:

```rust
use fs2::FileExt;

impl HiveState {
    /// Acquire an exclusive lock for a state file operation.
    /// Returns a guard that releases the lock on drop.
    fn lock_file(&self, name: &str) -> Result<std::fs::File, String> {
        let lock_path = self.hive_dir().join(format!("{name}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| format!("Failed to open lock file: {e}"))?;
        file.lock_exclusive()
            .map_err(|e| format!("Failed to acquire lock: {e}"))?;
        Ok(file)
    }
}
```

### Files to Lock

The critical concurrent-access files are:

1. **`merge-queue.json`** — Multiple leads submit simultaneously
   - Lock name: `merge-queue`
   - Wrap `load_merge_queue` + `save_merge_queue` pairs in callers

2. **`agents/<id>/agent.json`** — Heartbeat updates, status changes
   - Lock name: `agent-{agent_id}`
   - Wrap `update_agent_heartbeat`, and any read-modify-write patterns in mcp.rs

3. **`tasks/<id>.json`** — Status updates from multiple agents
   - Lock name: `task-{task_id}`
   - Wrap the load_task + save_task pattern in `hive_update_task`

### MCP Changes

**File: `src/mcp.rs`** — Add locking around critical read-modify-write operations:

For `hive_submit_to_queue`:
```rust
let _lock = state.lock_file("merge-queue")?;
let mut queue = state.load_merge_queue(&self.run_id)?;
queue.entries.push(entry);
state.save_merge_queue(&self.run_id, &queue)?;
// lock released on drop
```

For `hive_merge_next`:
```rust
let _lock = state.lock_file("merge-queue")?;
// ... entire merge operation ...
```

For `hive_update_task`:
```rust
let _lock = state.lock_file(&format!("task-{}", p.task_id))?;
let mut task = state.load_task(&self.run_id, &p.task_id)?;
// ... modify ...
state.save_task(&self.run_id, &task)?;
```

For `hive_check_agents` (agent status updates):
```rust
let _lock = state.lock_file(&format!("agent-{}", agent.id))?;
// ... check and update agent status ...
state.save_agent(&self.run_id, &agent)?;
```

### Atomic Write Helper

Also add atomic writes — write to a temp file then rename, preventing partial writes:

**File: `src/state.rs`**:
```rust
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
    Ok(())
}
```

Replace all `fs::write` calls in save methods with `atomic_write`.

### Tests

- `lock_file_creates_lockfile` — verify lock file is created
- `lock_file_prevents_concurrent_access` — spawn two threads, verify sequential access
- `atomic_write_produces_valid_file` — write and verify contents
- `atomic_write_doesnt_corrupt_on_partial_failure` — verify original file intact if tmp write fails
- Existing tests continue to pass (locking is transparent)

---

## Lead Decomposition

**Lead 1: state-locking** (domain: `state`, files: `src/state.rs`, `Cargo.toml`)
- Add `fs2` dependency
- Implement `lock_file` method
- Implement `atomic_write` helper
- Replace `fs::write` with `atomic_write` in all save methods
- Add tests

**Lead 2: mcp-locking** (domain: `mcp`, files: `src/mcp.rs`)
- Add lock acquisition to `hive_submit_to_queue`
- Add lock acquisition to `hive_merge_next`
- Add lock acquisition to `hive_update_task`
- Add lock acquisition to agent status updates in `hive_check_agents`
- Ensure locks are released promptly (guard scope)
- Test that operations still work correctly with locking
