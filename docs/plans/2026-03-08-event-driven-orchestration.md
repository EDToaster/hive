# Event-Driven Orchestration

**Goal:** Replace poll-based coordination with automatic notifications, enforce task-agent binding at spawn time, and eliminate heartbeat overhead. Agents react to events instead of polling for changes.

---

## Problem

Three coordination inefficiencies slow down hive runs and require operator babysitting:

1. **Loose task-agent binding.** `hive_spawn_agent` takes free-text `task_description` with no link to a task. Agents are supposed to call `hive_update_task(assigned_to=...)` separately but often don't. Result: the TUI shows unassigned tasks that agents are actually working on.

2. **Poll-based status detection.** Leads must call `hive_check_agents` on a timer to notice workers finished. Coordinators must poll `hive_wait_for_activity` to notice queue entries. This adds minutes of latency between "work done" and "next step begins."

3. **Heartbeat overhead.** Agents waste MCP tool calls on `hive_heartbeat` just to signal liveness.

---

## Change 1: `hive_spawn_agent` requires `task_id`

### Current signature

```
hive_spawn_agent(agent_id: String, role: String, task_description: String)
```

### New signature

```
hive_spawn_agent(agent_id: String, role: String, task_id: String)
```

### Behavior

1. Load the task by `task_id`. Fail if it doesn't exist or is already assigned to another agent.
2. Set `task.assigned_to = agent_id` and `task.status = Active`.
3. Pull `task.title` + `task.description` into the agent's CLAUDE.local.md prompt.
4. Store `task_id` in the agent's MCP server context (alongside `agent_id`, `run_id`, `role`).
5. The agent's prompt includes: "Your task is `<task_id>`: `<title>`. When done, call `hive_update_task(status=review)`."

### Role restrictions

Restrict `hive_spawn_agent` to `lead` and `worker` roles only. Remove `reviewer`, `planner`, and `postmortem` from its allowed roles. These are system-spawned:
- Reviewers: spawned internally by `hive_submit_to_queue`
- Planners: spawned internally by `hive start`
- Postmortem: spawned internally by `hive stop`

### What this eliminates

- No more orphaned tasks with no assignee
- No more agents working on things the system can't track
- No more redundant `hive_update_task(assigned_to=...)` calls that agents forget

### File changes

- `src/mcp.rs`: Change `SpawnAgentParams` — replace `task_description: String` with `task_id: String`. In `hive_spawn_agent`, load task, validate, set assigned_to/status, pass task to spawner. Remove reviewer/planner/postmortem from allowed roles.
- `src/types.rs`: If `SpawnAgentParams` is defined here, update accordingly.
- `src/agent.rs`: `AgentSpawner::spawn` takes a `Task` reference instead of `&str` description. Prompt generation uses task title/description. Bake `task_id` into MCP server args.
- `src/mcp.rs` (MCP server init): Read `task_id` from args and store in `HiveMcpServer` struct.

---

## Change 2: Auto-notification on state changes

Reuse the existing `claude --resume <session> --message` injection mechanism from `hive_send_message` to automatically notify agents when state changes affect them.

### Notification table

| Trigger | Who gets notified | Message content |
|---------|-------------------|-----------------|
| `hive_update_task(status=review)` | Parent lead of calling agent | "Worker {agent} completed task {task_id} '{title}'. Review the diff." |
| `hive_submit_to_queue(task_id)` | Coordinator | "Lead {agent} submitted task {task_id} '{title}' for merge." |
| `hive_review_verdict(approve)` | Coordinator | "Task {task_id} '{title}' approved by reviewer. Queued for merge." |
| `hive_review_verdict(request-changes)` | Parent lead of reviewing agent | "Reviewer requested changes on task {task_id}: {feedback}" |
| Task becomes unblocked | Assigned agent | "Task {task_id} '{title}' is now unblocked. Proceed." |
| `hive_merge_next` succeeds | Lead who submitted the task | "Task {task_id} merged to main successfully." |

### Unblock detection

When a task transitions to `merged` (either via `hive_update_task` or `hive_merge_next`), scan all tasks in the run. For each task with `blocked_by` containing the merged task's ID:
1. Remove the merged task ID from `blocked_by`.
2. If `blocked_by` is now empty and task has an `assigned_to` agent, inject an unblock notification.

### Implementation

Extract the auto-wake logic from `hive_send_message` (lines 498-530 of mcp.rs) into a shared helper:

```rust
fn notify_agent(state: &State, run_id: &str, target_agent_id: &str, message: &str) -> Result<(), String>
```

Call this helper from the relevant MCP handlers as a side effect after the primary state change succeeds.

### What this eliminates

- Leads no longer need to poll `hive_check_agents` on a timer
- Coordinators react to queue entries immediately
- Blocked agents wake up as soon as blockers merge
- Leads learn about merge results without checking

### What stays

- `hive_check_agents` remains as a manual fallback
- `hive_wait_for_activity` remains for general monitoring
- Message routing permissions unchanged

### File changes

- `src/mcp.rs`: Extract `notify_agent` helper. Add notification calls to `hive_update_task`, `hive_submit_to_queue`, `hive_review_verdict`, `hive_merge_next`. Add unblock scan to merge success path.
- `src/state.rs`: Add `find_newly_unblocked_tasks(run_id, merged_task_id) -> Vec<Task>` helper.

---

## Change 3: Passive heartbeat

### Current mechanism

Agents call `hive_heartbeat` explicitly. `last_heartbeat` is checked against a stall threshold.

### New mechanism

Remove `hive_heartbeat` as a tool. Instead, update `agent.last_heartbeat` as a side effect of **any** MCP tool call.

Add a wrapper or early-return check at the top of each MCP handler (or in a shared pre-handler if the rmcp framework supports middleware):

```rust
// At the start of every MCP tool handler:
self.touch_heartbeat();
```

Where `touch_heartbeat` loads the agent, updates `last_heartbeat = Utc::now()`, and saves.

### Stall detection

Unchanged. Still compares `now - last_heartbeat > stall_threshold`. An agent that stops calling any Hive tools for 5+ minutes is genuinely stalled.

### Edge case

Long-running commands (cargo build, cargo test) can take 2-3 minutes without any MCP calls. The existing 5-minute stall threshold already covers this.

### File changes

- `src/mcp.rs`: Remove `hive_heartbeat` handler. Add `touch_heartbeat()` method to `HiveMcpServer`. Call it at the top of every tool handler.
- `src/agent.rs`: Remove heartbeat instructions from agent prompts.

---

## Files changed (summary)

| File | Changes |
|------|---------|
| `src/mcp.rs` | Spawn requires task_id, role restriction, notification side effects, extract `notify_agent`, heartbeat middleware, remove `hive_heartbeat` |
| `src/types.rs` | `SpawnAgentParams` field change |
| `src/agent.rs` | Spawn takes Task instead of description string, prompt uses task data, bake task_id into MCP args, remove heartbeat prompt instructions |
| `src/state.rs` | `find_newly_unblocked_tasks` helper |
| `src/wait.rs` | Update if it references heartbeat directly |

---

## Domain boundaries for parallel leads

- **Lead 1 (types + state):** `src/types.rs`, `src/state.rs` — SpawnAgentParams change, unblock helper
- **Lead 2 (agent prompts):** `src/agent.rs` — spawn signature, prompt generation, remove heartbeat instructions
- **Lead 3 (MCP tools):** `src/mcp.rs` — spawn logic, notification side effects, notify_agent helper, heartbeat middleware, remove hive_heartbeat
- **Lead 4 (wait/tests):** `src/wait.rs`, test files — update wait logic, add integration tests

Lead 1 merges first (types are a dependency), then leads 2, 3, 4 in parallel.
