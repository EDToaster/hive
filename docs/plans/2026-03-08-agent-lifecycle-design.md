# Agent Lifecycle Redesign: `-p` + `--resume`

## Problem

Agents are launched with `claude --print`, which runs a single prompt and exits. This means:

- Messages are never delivered — no running process to inject into
- Code review feedback can't be sent back — the worker is already dead
- Leads can't redirect workers — they finish before the lead checks
- The rebase-and-resubmit flow requires spawning an entirely new agent

The spec envisions long-lived agents that receive messages as new conversation turns. The current model cannot support this.

## Solution

Replace `--print` with `claude -p` + `--resume`. Agents become a **sequence of `-p` invocations sharing a session ID**. The session is the continuity, not the PID.

## Agent Lifecycle Model

### States

- **Running** — A `-p` invocation is active (has PID). Heartbeats via PostToolUse hooks.
- **Idle** — Last invocation finished. No process. Waiting for coordinator to `--resume` with next message/task.
- **Failed** — Process exited with error. Coordinator decides recovery.
- **Done** — All work complete. No more invocations needed.

### Agent Struct Changes

Add to the `Agent` struct:

- `session_id: Option<String>` — set after first invocation completes, used for `--resume`
- `last_completed_at: Option<DateTime<Utc>>` — set when process exits, used to track idle duration

Existing fields:

- `pid: Option<u32>` — transient. Set during active invocation, cleared when process exits.
- `status` — gains a new `Idle` variant

### Uniform Model

Leads and workers both use the same `-p` + `--resume` pattern. The coordinator remains human-launched (unchanged).

## Spawn Sequence

1. **Create worktree** — same as today
2. **Write configs** — `.claude/hooks.json`, `.mcp.json`, `CLAUDE.local.md` — same as today
3. **Launch**: `claude -p "<prompt>" --output-format json --dangerously-skip-permissions` with stdout redirected to `.hive/runs/<run>/agents/<agent-id>/output.json`
4. **Register agent** — status `Running`, PID set, `session_id: None`
5. **MCP tool returns immediately** — coordinator does not block

### Session ID Capture

Session ID is captured lazily when the coordinator checks agents:

1. Coordinator calls `hive_check_agents`
2. Hive sees agent PID has exited + no `session_id` yet
3. Parses `output.json` for `session_id` from the JSON output
4. Stores `session_id` in agent metadata
5. Sets status to `Idle`, records `last_completed_at`

## Message Delivery

When `hive_send_message` is called targeting an agent:

1. Message is saved to disk (already works)
2. Check agent status:
   - **Idle + has session_id** — Spawn `claude -p "<message>" --resume <session_id> --output-format json --dangerously-skip-permissions` with stdout redirected to output file. Set agent status to `Running`, store new PID.
   - **Running** — Just save the message. Coordinator can re-deliver after agent goes idle.
   - **No session_id yet** — Agent's first invocation hasn't finished. Save message, deliver later.

`hive_send_message` to an idle agent automatically wakes it up by spawning a `--resume` invocation.

## `hive_wait_for_activity`

A blocking MCP tool that watches `.hive/` for changes, preventing the coordinator from polling in a tight loop.

### Behavior

- Blocks until a relevant change is detected or timeout expires (default 60s, configurable)
- Watches: agent status files, task files, message files, merge queue
- Returns a summary of what changed:
  - `"agent lead-backend exited (pid 12345 no longer alive)"`
  - `"task task-001 status changed to review"`
  - `"new message msg-0042 from lead-backend"`
- On timeout with no changes: `"no activity, N agents running, M idle"`

### Implementation

The MCP tool is async. Uses `notify` crate (already a dependency) with a tokio channel — watches `.hive/runs/<run>/` recursively, sends events through a channel, returns on first relevant event or timeout.

### Coordinator Loop

```
loop:
  1. hive_check_agents → parse session IDs, detect failures
  2. hive_list_tasks → take actions (merge, reassign, etc.)
  3. hive_wait_for_activity → sleep until something happens
  4. goto 1
```

## Heartbeat Changes

### During Active Invocation (Running)

- PostToolUse hook fires `hive heartbeat` on every tool call — same as today
- If heartbeat goes stale while PID is alive → agent is stuck
- `hive_check_agents` reports this as `stalled`

### Between Invocations (Idle)

- No process, no heartbeats — this is normal, not a failure
- `last_completed_at` tracks how long the agent has been idle
- `hive_check_agents` reports idle agents with their idle duration, not as stalled

### Health Report Format

Active agent:
```json
{
  "agent_id": "lead-backend",
  "role": "lead",
  "status": "running",
  "session_id": "def456",
  "process_alive": true,
  "last_heartbeat_age_secs": 3,
  "idle_since_secs": null
}
```

Idle agent:
```json
{
  "agent_id": "worker-001",
  "role": "worker",
  "status": "idle",
  "session_id": "abc123",
  "process_alive": false,
  "last_heartbeat_age_secs": null,
  "idle_since_secs": 45
}
```

## Error Handling

When a `-p` invocation exits with a non-zero exit code:

- Set agent status to `Failed`
- Coordinator sees it on next `hive_check_agents`
- Coordinator decides: respawn, reassign task, or abort

No auto-retry. Keep it simple.

## Agent Prompts

### Coordinator

```
You are the coordinator agent in a hive swarm.
Run ID: <run-id>
Agent ID: coordinator
Role: coordinator

## Spec
<spec content>

## Workflow
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- After spawning, call hive_wait_for_activity to wait for changes.
- When activity occurs, check agent status and tasks.
- When an agent goes idle, send it messages to deliver feedback or new work.
- Process the merge queue via hive_merge_next when leads submit work.
- Repeat until all tasks are merged.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Use hive_wait_for_activity instead of polling in tight loops.
- Let leads handle code review and task decomposition within their domain.
```

### Lead

```
You are a lead agent in a hive swarm.
Agent ID: <agent-id>
Role: lead
Parent: <parent>

## Your Assignment
<task description>

## Workflow
- Read the codebase to understand your domain.
- Decompose your assignment into specific worker tasks.
- Spawn workers via hive_spawn_agent for each task.
- When you have no more actions to take, finish your response.
  You will be resumed when workers complete or the coordinator sends a message.
- Review worker output (diffs) when resumed with review requests.
- Send workers back with feedback if changes are needed.
- Submit approved branches to the merge queue via hive_submit_to_queue.
- Report progress to the coordinator via hive_send_message.

## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
- When you have nothing to do, stop and wait to be resumed. Do not loop.
```

### Worker

```
You are a worker agent in a hive swarm.
Agent ID: <agent-id>
Role: worker
Parent: <parent>

## Your Task
<task description>

## Workflow
- Implement the task in your worktree.
- Run relevant tests and linters to verify your work.
- When done, call hive_update_task to set status to "review".
- If you discover an unrelated bug or issue, call hive_create_task
  with urgency and a description. It will be routed to your lead.
- When finished, stop. Your lead will resume you if changes are needed.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than your lead.
- Stay focused on your assigned task.
- When done, stop and wait. Do not loop.
```
