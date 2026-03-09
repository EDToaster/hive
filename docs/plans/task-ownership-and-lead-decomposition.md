# Task Ownership and Lead Decomposition

## Summary

Establish rigorous task ownership where every task has a single accountable agent, leads decompose their own tasks into subtasks for workers, and the system enforces ownership rules with helpful error messages.

## Background

Currently task ownership is informal:
- `assigned_to` is set but not enforced — any agent can update any task
- `parent_task` is decorative — used only for TUI rendering
- `hive_spawn_agent` doesn't bind agents to tasks (`agent.task_id` is always None)
- Leads can submit to the merge queue even if subtasks are still active
- The coordinator creates all tasks (including worker-level), limiting lead autonomy

Phase 1 (already shipped) added `Absorbed` and `Cancelled` terminal statuses with `is_resolved()` and `is_success()` helpers. This phase builds on that foundation.

## Design

### 1. Spawn-Task Binding

`hive_spawn_agent` gains a required `task_id` parameter.

On spawn:
1. Validate the task exists and is in Pending or Blocked status
2. Set `task.assigned_to = agent_id`
3. Set `task.status = Active`
4. Set `agent.task_id = task_id`

This creates a bidirectional binding: `Agent.task_id` points to the task, `Task.assigned_to` points to the agent.

`SpawnAgentParams` in `mcp.rs` changes:
```rust
pub struct SpawnAgentParams {
    pub agent_id: String,
    pub role: String,
    pub task_id: String,          // NEW: required
    pub task_description: String,
}
```

### 2. Ownership Enforcement in hive_update_task

Every call to `hive_update_task` checks whether the caller is authorized to modify the target task. Derive task level from `parent_task`: no parent = lead-level, has parent = worker-level.

**Permission rules:**

| Caller Role | Can Modify |
|-------------|-----------|
| Coordinator | Tasks with no `parent_task` (lead-level tasks) |
| Lead | Own assigned task, tasks they created (`created_by`), tasks assigned to their workers |
| Worker | Only their own assigned task (`assigned_to == caller`) |
| Reviewer | Only the task they were spawned to review |

**On denial, return actionable guidance.** Examples:

- Worker tries to update a sibling's task:
  > "Permission denied: you can only update your own assigned task (task-abc123). To request changes to task-def456, send a message to its owner 'worker-2' or your lead 'lead-backend'."

- Worker tries to update parent lead task:
  > "Permission denied: task-abc123 is a lead-level task owned by 'lead-backend'. Send a message to your lead instead."

- Lead tries to update another lead's subtask:
  > "Permission denied: task-abc123 belongs to lead 'lead-frontend'. Send a message to the coordinator to coordinate cross-domain changes."

### 3. Ownership Enforcement in hive_create_task

**Permission rules:**

| Caller Role | Can Create |
|-------------|-----------|
| Coordinator | Tasks with no `parent_task` (lead-level) |
| Lead | Tasks with `parent_task` set to their own assigned task |
| Worker | Cannot create tasks (use `hive_send_message` to suggest tasks to lead) |

**On denial:**

- Coordinator tries to create a subtask:
  > "Permission denied: coordinator cannot create subtasks. Create a lead-level task (no parent_task) and let the assigned lead decompose it."

- Worker tries to create a task:
  > "Permission denied: workers cannot create tasks. Send a message to your lead suggesting the task."

- Lead tries to create a subtask under another lead's task:
  > "Permission denied: you can only create subtasks under your own task (task-abc123), not under task-def456."

### 4. Subtask Completion Gate in hive_submit_to_queue

Before allowing a lead to submit to the merge queue, check that all subtasks of the submitted task are resolved.

```
let subtasks = all_tasks.filter(|t| t.parent_task == Some(task_id));
let unresolved: Vec<_> = subtasks.filter(|t| !t.status.is_resolved()).collect();

if !unresolved.is_empty() {
    return error:
      "Cannot submit task-abc123: {N} subtask(s) are not resolved:
       - task-x (title): active — assigned to worker-1
       - task-y (title): review — assigned to worker-2
       Resolve all subtasks first. Use status 'cancelled' or 'absorbed' for tasks that don't need independent merges."
}
```

### 5. Coordinator Prompt Changes

The coordinator's role shifts from micromanager to strategist:

**Remove:**
- "Create ALL tasks FIRST with proper blocked_by relationships before spawning any leads"

**Replace with:**
```
## Task Creation Protocol
- Create one task per domain/lead. These are high-level tasks describing WHAT needs to happen, not HOW.
- Set blocked_by relationships between lead-level tasks for cross-domain dependencies.
- Use the domain field to tag each task for file-conflict prevention.
- Set urgency: critical for blocking tasks, high for core features, normal for polish.
- Each task title should describe the domain and goal, not implementation steps.
- Do NOT create worker-level subtasks — leads will decompose their own tasks.
- After creating all lead-level tasks, spawn one lead per task.
```

### 6. Lead Prompt Changes

Leads become responsible for decomposition:

**Replace current "Delegation Protocol" section with:**
```
## Task Decomposition Protocol
- Read the relevant source files to understand the codebase and your task's scope.
- Break your task into subtasks using hive_create_task with parent_task set to your task ID.
- Each subtask should be a focused unit of work for one worker (usually one file or feature).
- Spawn one worker per subtask using hive_spawn_agent with the subtask's task_id.
- You own the lifecycle of every subtask you create.

## Subtask Lifecycle
- Monitor workers via hive_wait_for_activity and hive_check_agents.
- When workers finish, review their work with hive_review_agent.
- If a subtask is no longer needed, set it to "cancelled".
- If a subtask's work was incorporated into another branch, set it to "absorbed".
- You CANNOT submit to the merge queue until ALL subtasks are resolved (merged, failed, cancelled, or absorbed).
- Only submit to merge queue after all subtasks are resolved and your branch is ready.
```

### 7. Worker Prompt Changes

Minor — workers already have the right structure. Add:
```
## If No Code Changes Needed
- If after analysis you determine no code changes are required, set your task status to "cancelled"
  with a note explaining why. Do not submit to review — there's nothing to review.
```

### 8. hive_list_tasks Enhancement

Add optional `parent_task` filter parameter so the coordinator can list only lead-level tasks:

```rust
pub struct ListTasksParams {
    pub status: Option<String>,
    pub assigned_to: Option<String>,
    pub domain: Option<String>,
    pub parent_task: Option<String>,  // NEW: filter by parent, use "none" for top-level
}
```

When `parent_task` is `"none"`, return only tasks where `parent_task.is_none()` (lead-level tasks). When set to a task ID, return only that task's subtasks. This gives the coordinator clean visibility without subtask noise.

## Migration

- `SpawnAgentParams` adds required `task_id` — breaking change for agent prompts, but prompts are generated fresh each run so no backward compatibility concern.
- No new fields on `Task` — ownership derived from existing `assigned_to`, `created_by`, `parent_task`.
- No new fields on `Agent` — `task_id` already exists but is currently never populated.
- No data migration — purely behavioral changes to MCP tool handlers and prompt templates.

## Implementation Domains

1. **types + state**: No changes needed (Phase 1 already added `is_resolved()`).

2. **mcp**:
   - `hive_spawn_agent`: add `task_id` param, bind agent↔task on spawn
   - `hive_update_task`: ownership check with role-aware error messages
   - `hive_create_task`: permission check on who can create what level
   - `hive_submit_to_queue`: subtask completion gate using `is_resolved()`
   - `hive_list_tasks`: add `parent_task` filter

3. **agent**: Prompt template updates for coordinator, lead, worker roles (both regular and explore variants).

4. **tests**: Ownership denial cases, subtask gate cases, spawn-task binding, helpful error message content.
