# Phase 3: Error Recovery & Agent Resilience

**Goal:** Make hive runs self-healing — automatically recover from agent crashes, context exhaustion, and stalled agents without human intervention.

---

## Feature 1: Automatic Agent Retry

When `hive_check_agents` detects a failed agent (process dead, no session_id captured), it should offer the coordinator/lead an option to retry.

### New MCP Tool: `hive_retry_agent`

**File: `src/mcp.rs`** — Add new tool:

```rust
#[tool(description = "Retry a failed agent by re-spawning it with the same task and a fresh worktree")]
async fn hive_retry_agent(&self, params: Parameters<RetryAgentParams>) -> Result<CallToolResult, McpError>
```

Parameters:
- `agent_id: String` — the failed agent to retry
- `feedback: Option<String>` — optional context about what went wrong, appended to the agent's prompt

Behavior:
1. Load the failed agent. Verify status is Failed or Stalled.
2. If the agent has a worktree, auto-commit any uncommitted work, then get the diff stat vs main.
3. Clean up the old worktree (`git worktree remove`).
4. Re-spawn the agent with the same role, parent, and task — but append the feedback and diff context to the task description so the new instance knows what was already attempted.
5. Update the agent record with new PID, worktree, Running status.

Permission: Coordinator can retry leads. Leads can retry their workers.

### Implementation

**File: `src/mcp.rs`**:
- Add `RetryAgentParams` struct
- Add `hive_retry_agent` tool method
- Enforce permissions (coordinator retries leads, leads retry workers)

**File: `src/agent.rs`**:
- Add `AgentSpawner::retry()` method that wraps the spawn logic but includes previous attempt context

**File: `src/git.rs`**:
- No changes needed — `worktree_remove` and `worktree_add` already exist

### Tests
- Retry a failed agent creates new worktree and process
- Retry a running agent returns error
- Permission enforcement (worker can't retry, lead can only retry own workers)

---

## Feature 2: Stall Detection & Auto-Recovery

Currently, stalled agents are detected in `hive_check_agents` but nothing happens automatically. Add proactive recovery.

### Enhanced `hive_check_agents`

**File: `src/mcp.rs`** — When an agent is detected as stalled:

1. Check if the process is alive but not responding (heartbeat age > stall_timeout):
   - Send SIGTERM to the process
   - Wait 5 seconds
   - If still alive, send SIGKILL
2. Auto-commit any uncommitted work in the worktree
3. Try to capture session_id from output.json
4. Mark agent as Stalled (not Failed — distinguishes crash from stall)
5. Return the stalled agents in the check results so coordinator/lead can decide to retry

### Config Addition

**File: `src/state.rs`** — Add to HiveConfig:

```rust
pub max_retries: u32,  // default: 2
```

Parse from config.yaml:
```yaml
max_retries: 2
```

### Agent Retry Counter

**File: `src/types.rs`** — Add to Agent struct:

```rust
pub retry_count: u32,
```

Default to 0, increment on each retry. When retry_count >= max_retries, mark as Failed permanently and notify parent.

---

## Feature 3: Graceful Context Exhaustion Handling

When a Claude Code agent hits context limits, it exits. The agent should checkpoint its progress first.

### Implementation

This is largely handled by the existing Stop hook + auto-commit-on-exit flow. What's missing:

**File: `src/agent.rs`** — Enhance the worker prompt to include:

```
## Context Management
- If you notice your context is getting large, summarize your progress so far in a commit message.
- Before making large file reads, check if smaller targeted reads would suffice.
- If you're running low on context, commit your work, update the task status to "review" with a note about remaining work, and stop.
```

**File: `src/mcp.rs`** — Add `notes` field to `UpdateTaskParams`:

```rust
pub notes: Option<String>,
```

When set, append to the task description so the next agent (or retried agent) knows what was already done.

---

## Lead Decomposition

**Lead 1: Types & State** (domain: `types-state`, files: `src/types.rs`, `src/state.rs`)
- Add `retry_count` to Agent struct
- Add `max_retries` to HiveConfig with parsing
- Add tests

**Lead 2: Agent & MCP** (domain: `agent-mcp`, files: `src/agent.rs`, `src/mcp.rs`)
- Add `hive_retry_agent` tool
- Enhance `hive_check_agents` with stall recovery
- Add `notes` field to UpdateTaskParams
- Enhance agent prompts for context management
- Add tests
