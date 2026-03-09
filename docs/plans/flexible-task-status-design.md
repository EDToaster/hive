# Flexible Task Status: Terminal Statuses for Subtask Lifecycle

## Problem Statement

Two scenarios create stuck tasks with no clean resolution path:

1. **"No changes needed" tasks** — A worker determines no code changes are required. They can't submit to the merge queue (nothing to merge), but there's no terminal status besides `Merged` or `Failed`. The task gets stuck in `Review`.

2. **Subtasks after parent merges** — When a parent task's branch merges, its subtasks may still be in `Review` or `Active`. Their work was included in the parent's merge, but they have no way to reflect that.

## Proposed New Statuses

### Option A: Two new statuses (Recommended)

| Status | Semantics | Use Case |
|--------|-----------|----------|
| `Absorbed` | Work was incorporated into a parent/sibling task's merge. No independent merge needed. | Subtask whose changes were part of a parent branch that merged. |
| `Cancelled` | Task was determined unnecessary. No work product. | "No changes needed" tasks, tasks made obsolete by other work. |

### Option B: Three new statuses (More granular)

Adds `Superseded` — task was replaced by a different approach from another task. This is a subset of `Cancelled` semantics and adds complexity without much benefit. **Not recommended** — `Cancelled` with a note covers this case.

### Why Option A

- `Absorbed` vs `Cancelled` is a meaningful semantic distinction: "work happened but merged elsewhere" vs "work was unnecessary"
- Both are unambiguously terminal — no merge queue interaction needed
- Two statuses is minimal; three adds taxonomy overhead for agents to distinguish `Cancelled` from `Superseded`

## Detailed Design

### 1. TaskStatus Enum Changes

```rust
// src/types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Active,
    Blocked,
    Review,
    Approved,
    Queued,
    Merged,
    Failed,
    Absorbed,   // NEW: work merged via parent/sibling
    Cancelled,  // NEW: task determined unnecessary
}
```

### 2. Permission Model

| Status | Who Can Set | How |
|--------|------------|-----|
| `Absorbed` | Lead (for own subtasks), Coordinator (any task) | Via `hive_update_task` with permission check |
| `Cancelled` | Lead (for own subtasks), Coordinator (any task), Worker (own task only) | Via `hive_update_task` with permission check |

**Key constraint**: Only the task's lead (creator or parent agent) or coordinator should be able to absorb/cancel tasks. Workers can cancel their own task if they determine it's unnecessary, but cannot absorb (that's a lead-level decision about task decomposition).

Implementation in `hive_update_task`:

```rust
// When setting absorbed or cancelled, enforce permissions
if matches!(new_status, TaskStatus::Absorbed | TaskStatus::Cancelled) {
    let agent = state.load_agent(&self.run_id, &self.agent_id)?;
    let is_coordinator = agent.role == AgentRole::Coordinator;
    let is_task_creator = task.created_by == self.agent_id;
    let is_assigned_worker = task.assigned_to.as_deref() == Some(&self.agent_id)
        && agent.role == AgentRole::Worker
        && new_status == TaskStatus::Cancelled;

    if !is_coordinator && !is_task_creator && !is_assigned_worker {
        return Ok(CallToolResult::error(vec![Content::text(
            "Permission denied: only coordinator, task creator, or assigned worker (cancel only) can set this status"
        )]));
    }
}
```

### 3. Interaction with Merge Queue and Reviewer Flow

**No interaction needed.** That's the whole point.

- `Absorbed` and `Cancelled` bypass the merge queue entirely
- No reviewer is spawned — these are administrative decisions, not code review decisions
- If a task is currently in `Review` with a reviewer running, the lead/coordinator can still set it to `Absorbed` or `Cancelled`. The reviewer agent will find the task status changed and should gracefully exit.

**Review verdict does NOT get new options.** Reviewers review code. "No changes needed" is a task-level decision made before submitting for review.

### 4. Aggregate Parent Completion Check

Define "resolved" as a helper concept:

```rust
impl TaskStatus {
    /// A task is "resolved" if it's in a terminal state — no further action needed.
    pub fn is_resolved(&self) -> bool {
        matches!(self,
            TaskStatus::Merged
            | TaskStatus::Failed
            | TaskStatus::Absorbed
            | TaskStatus::Cancelled
        )
    }

    /// A task is "successfully resolved" if it completed its purpose.
    pub fn is_success(&self) -> bool {
        matches!(self,
            TaskStatus::Merged
            | TaskStatus::Absorbed
        )
    }
}
```

A parent task can be considered "complete" when all its subtasks are resolved:

```rust
fn all_subtasks_resolved(state: &HiveState, run_id: &str, parent_id: &str) -> bool {
    let tasks = state.list_tasks(run_id).unwrap_or_default();
    tasks.iter()
        .filter(|t| t.parent_task.as_deref() == Some(parent_id))
        .all(|t| t.status.is_resolved())
}
```

This could be used in:
- TUI to show parent completion progress
- A future `hive_check_completion` tool
- Coordinator logic to decide when a lead's work is done

### 5. TUI Display Changes

```rust
// src/tui.rs - task_status_color
fn task_status_color(s: TaskStatus) -> Color {
    match s {
        TaskStatus::Active | TaskStatus::Approved => Color::Green,
        TaskStatus::Merged => Color::LightBlue,
        TaskStatus::Queued | TaskStatus::Review | TaskStatus::Blocked => Color::Yellow,
        TaskStatus::Pending => Color::Gray,
        TaskStatus::Failed => Color::Red,
        TaskStatus::Absorbed => Color::Cyan,      // distinct from Merged (LightBlue)
        TaskStatus::Cancelled => Color::DarkGray,  // dimmed, clearly inactive
    }
}

// src/tui.rs - task_status_bullet
fn task_status_bullet(s: TaskStatus) -> &'static str {
    match s {
        // ... existing ...
        TaskStatus::Absorbed => "\u{25C8} absorbed",  // diamond bullet — merged elsewhere
        TaskStatus::Cancelled => "\u{2205} cancelled", // empty set — nothing to do
    }
}
```

The task summary bar should include `Absorbed` and `Cancelled` in the counts. These should also cause task rows to render dimmed (like `Merged` and `Failed`).

### 6. Solving the Stuck Task Problem

**Scenario 1: "No changes needed"**
```
Worker analyzes task → determines no code changes needed
Worker calls hive_update_task(status="cancelled", notes="Analysis complete: no changes needed because X")
Task moves to Cancelled (terminal) ✓
```

**Scenario 2: Subtask absorbed by parent merge**
```
Lead has subtasks A, B, C
Lead merges A's branch which also covers B's scope
Lead calls hive_update_task(task_id=B, status="absorbed", notes="Changes included in task A's merge")
Task B moves to Absorbed (terminal) ✓
```

**Scenario 3: Task made obsolete**
```
Coordinator realizes task is no longer needed due to changed requirements
Coordinator calls hive_update_task(task_id=X, status="cancelled", notes="Superseded by new approach")
Task X moves to Cancelled (terminal) ✓
```

## Migration / Backwards Compatibility

- Existing JSON task files with only the old statuses will continue to deserialize fine (serde just gets new variants)
- No data migration needed — new statuses only appear when explicitly set
- The `is_resolved()` helper makes it easy to update any code that currently checks `Merged || Failed`

## Summary of Code Changes

| File | Change |
|------|--------|
| `src/types.rs` | Add `Absorbed`, `Cancelled` to `TaskStatus` enum. Add `is_resolved()` and `is_success()` methods. |
| `src/mcp.rs` (`hive_update_task`) | Accept "absorbed" and "cancelled" in status string match. Add permission check for these terminal statuses. |
| `src/tui.rs` | Add color, bullet, and summary bar entries for new statuses. Dim absorbed/cancelled rows. |
| Tests | Update `task_status_all_variants_roundtrip` test. Add permission check tests. |
