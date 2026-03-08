# Agent Work Preservation

## Problem

Agents exit without committing their work. In the first hive run, every worker produced useful code but none committed it. The coordinator had to manually inspect worktrees, copy diffs, and apply patches. The merge queue and review flow were unusable because there was nothing on the branches to merge or review.

## Solution

Three changes that work together:

1. **Auto-commit on exit** — `hive_check_agents` commits uncommitted work when it detects a process has exited
2. **Uncommitted changes reporting** — `hive_check_agents` includes worktree diff status in health reports for all agents
3. **`hive_review_agent` tool** — lets the coordinator inspect what a dead/idle agent produced without leaving MCP
4. **Prompt updates** — instruct agents to commit with descriptive messages as they work

## 1. Auto-Commit on Agent Exit

### Location

In `hive_check_agents` (mcp.rs), in the existing code path that detects a process has exited and transitions agents to Idle or Failed.

### Behavior

After detecting process exit, before updating agent state:

1. Check if the agent has a worktree path
2. Run `git status --porcelain` in the worktree
3. If output is non-empty (there are changes):
   - `git add -A`
   - `git commit -m "wip: auto-commit on agent exit"`
4. Proceed with existing Idle/Failed transition

### Edge Cases

- No worktree (coordinator): skip
- Worktree path doesn't exist (already cleaned up): skip
- `git commit` fails (e.g. empty after add): ignore error, continue

## 2. Uncommitted Changes in Health Reports

### Location

In `hive_check_agents` (mcp.rs), when building the JSON report for each agent.

### Behavior

For every agent that has a worktree:

1. Run `git status --porcelain` in the worktree
2. If non-empty, include the raw output as `"uncommitted_changes": "<output>"`
3. If clean or no worktree, include `"uncommitted_changes": null`

This field is populated for **all** agents, not just exited ones. For running/stalled agents it tells the coordinator whether the agent has produced any work. For exited agents after auto-commit, it will be `null` (confirming the commit worked).

### Updated Health Report Format

```json
{
  "agent_id": "worker-001",
  "role": "worker",
  "status": "idle",
  "session_id": "abc123",
  "process_alive": false,
  "last_heartbeat_age_secs": null,
  "idle_since_secs": 45,
  "uncommitted_changes": null
}
```

## 3. hive_review_agent Tool

### MCP Tool Definition

```
hive_review_agent(agent_id: String) -> CallToolResult
```

**Permissions:** Coordinator and Lead only.

**Target agent must be:** Idle, Failed, or Dead (not Running).

### Behavior

1. Load the agent by ID
2. Validate agent status is not Running
3. Check worktree for uncommitted changes — if any, `git add -A && git commit -m "wip: salvaged by coordinator"`
4. Compute and return:
   - `agent_id`, `role`, `status`, `task_id`
   - `branch`: the agent's git branch name
   - `commits`: output of `git log --oneline main..<branch>`
   - `diff_stat`: output of `git diff --stat main..<branch>`

### Return Format

```json
{
  "agent_id": "worker-001",
  "role": "worker",
  "status": "idle",
  "task_id": "task-abc123",
  "branch": "hive/run-id/worker-001",
  "commits": "b5a7582 feat: add session_id to Agent\n009dc8a feat: add wait module",
  "diff_stat": " src/types.rs | 8 +++++\n src/mcp.rs   | 12 ++++++--\n 2 files changed, 18 insertions(+), 2 deletions(-)"
}
```

### CLI Exposure

`hive review-agent <agent-id>` — optional, wraps the same logic for human use.

## 4. Prompt Updates

### Worker Prompt Addition

Add to the Responsibilities section:

```
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
```

### Lead Prompt Addition

Add to the Responsibilities section:

```
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
```

## Files Modified

- `src/mcp.rs` — auto-commit in `hive_check_agents`, uncommitted_changes field, new `hive_review_agent` tool
- `src/agent.rs` — prompt updates for leads and workers
- `src/cli.rs` — optional `ReviewAgent` CLI command
- `src/main.rs` — wire CLI command
- `src/git.rs` — may need helper for `git status --porcelain` and `git log --oneline`
