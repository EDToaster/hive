# Hive-Pi: Full TypeScript Rewrite Using pi-mono

**Date:** 2026-03-19
**Status:** Design approved
**Approach:** Monolithic package (Approach 1)

## Goals

Rewrite Hive as a TypeScript application using pi-mono's `Agent` class and `pi-ai` provider abstraction. This replaces Claude Code subprocess spawning with in-process agent instances, gaining:

1. **Multi-provider support** — use any LLM (Anthropic, OpenAI, Google, local) for different agents
2. **Direct agent lifecycle control** — prompt, steer, abort, compact via pi-mono's Agent API
3. **Simplified communication** — in-memory message passing via `agent.steer()`, no MCP/RPC overhead
4. **Crash recovery** — persisted state allows resuming interrupted runs

## Architecture

Single long-running Node.js process. All agents run in-process as pi-mono `Agent` instances.

```
hive start <spec.md>
  → parse spec → create tasks → spawn coordinator Agent
  → coordinator spawns leads → leads spawn workers
  → agents work via tool calls (native AgentTool objects)
  → completed branches submitted to merge queue
  → merge queue processes sequentially
  → hive stop (cleanup worktrees, save postmortem)
```

### Key Design Decisions

- **In-process agents**: All agents as `Agent` objects in one Node process. Fast, simple. No crash isolation between agents (acceptable for MVP).
- **Native AgentTools**: Hive operations defined as pi-mono `AgentTool` objects. Direct function calls, no MCP serialization.
- **Push-based messaging**: `agent.steer()` injects messages mid-conversation. Replaces the fragile Stop-hook → resume-spawn dance.
- **Simplified roles**: Coordinator, Lead, Worker only. No Reviewer, Postmortem, Explorer, Evaluator for MVP.
- **In-memory + disk persistence**: Primary state in-memory, persisted to `.hive/` for crash recovery.

## Role Hierarchy

Three roles: Coordinator → Lead → Worker.

| Capability | Coordinator | Lead | Worker |
|---|:-:|:-:|:-:|
| spawn_agent | leads | workers | no |
| create_task | yes | subtasks only | no |
| update_task | yes | own tasks | own task |
| send_message | to leads | to workers + coordinator | to own lead |
| submit_to_queue | process queue | yes | no |
| query_tasks | all | all | own lead's tasks |

## State & Crash Recovery

### Persisted to `.hive/` on every state change:

- **`state.json`** — tasks (status, assignment, dependencies), agent registry (id, role, model, status), merge queue entries. Written atomically (write-temp + rename).
- **`agents/{id}/messages.jsonl`** — append-only log of each agent's conversation history (every AgentMessage).
- **`agents/{id}/meta.json`** — agent config: system prompt, model, tools granted, worktree path, branch name.

### Recovery flow (`hive resume`):

1. Read `state.json` — restore task graph and agent registry
2. For each agent that was `active`: recreate `Agent` instance from `meta.json`
3. Replay `messages.jsonl` into agent's message history via `Agent.replaceMessages()`
4. Resume with `agent.continue()` — picks up where it left off
5. Worktrees survive process death (git worktrees are on disk)

### Not persisted:

- Streaming state (reconstructed on resume)
- Pending tool calls (LLM re-requests if needed)

## Agent Lifecycle & Tools

### Creation flow:

1. `new Agent({ initialState: { model, systemPrompt, tools } })`
2. Tools are role-scoped `AgentTool[]` — direct function references
3. `agent.steer()` queue for mid-conversation message injection
4. `agent.subscribe(event => ...)` for logging/monitoring

### Tool implementation pattern:

```typescript
const createTaskTool: AgentTool = {
  name: "create_task",
  label: "Create Task",
  description: "Create a new task with title and description",
  parameters: TaskCreateSchema,  // TypeBox schema
  execute: async (toolCallId, params) => {
    const task = hive.taskManager.create(params);
    return { content: [{ type: "text", text: JSON.stringify(task) }], details: task };
  }
};
```

### Message delivery:

When agent A sends a message to agent B:
1. `send_message` tool handler calls `agentB.steer(userMessage)`
2. pi-mono injects it after B's current turn completes
3. Direct push — no polling, no file watching

### Agent failure:

On `agent_end` with error: mark task as failed, notify parent agent. Parent can retry by spawning fresh agent with same task + error context.

## Git Isolation & Merge Queue

### Worktrees with sparse checkout:

- **Coordinator**: no worktree (strategic role)
- **Leads**: `.hive/worktrees/{agent-id}/` with branch `hive/{run-id}/{agent-id}`
- **Workers**: same pattern, branched from their lead's branch
- **Sparse checkout**: scoped to task's `domain` paths (e.g., `src/state/`)
- Domain paths validated against git tree at task creation time

### Merge queue (sequential):

1. Leads submit completed branches via `submit_to_queue` tool
2. Queue processes one at a time: checkout main → merge → verify → advance
3. On conflict: auto-rebase, re-verify, retry
4. On rebase failure: mark task failed, notify lead

### Cleanup:

`hive stop` removes all worktrees and branches. `--keep-worktrees` flag to preserve.

## CLI Commands (MVP)

```
hive start <spec.md>     # Parse spec, create run, spawn coordinator
hive resume [run-id]     # Resume from persisted state
hive status              # Show tasks, agents, statuses
hive stop                # Graceful shutdown
hive agents              # List agents with status, model, task
hive tasks               # List tasks with status, assignee, deps
hive cost                # Token usage and cost per agent
hive logs <agent-id>     # Tail agent's conversation
```

No TUI for MVP. Structured events emitted to stdout for monitoring:
```json
{"type":"agent_spawned","agentId":"lead-1","role":"lead","model":"claude-sonnet-4-6"}
{"type":"task_status_changed","taskId":"t1","from":"pending","to":"active"}
{"type":"merge_completed","branch":"hive/run-1/lead-1","conflicts":false}
```

## Project Structure

```
hive-pi/
├── package.json
├── tsconfig.json
├── src/
│   ├── main.ts           # CLI entry point
│   ├── hive.ts           # Core Hive class
│   ├── state.ts          # In-memory state + .hive/ persistence
│   ├── task-manager.ts   # Task CRUD, dependency graph, status machine
│   ├── agent-manager.ts  # Agent lifecycle: create, steer, abort, cleanup
│   ├── git.ts            # Worktree ops, sparse checkout, merge queue
│   ├── tools/
│   │   ├── task-tools.ts
│   │   ├── message-tools.ts
│   │   ├── spawn-tools.ts
│   │   ├── merge-tools.ts
│   │   └── index.ts      # Role-based tool set builder
│   ├── prompts/
│   │   ├── coordinator.ts
│   │   ├── lead.ts
│   │   └── worker.ts
│   └── types.ts
└── test/
    └── ...
```

## Dependencies

- `@mariozechner/pi-ai` — LLM provider abstraction, streaming
- `@mariozechner/pi-agent-core` — Agent class, tool system, agent loop
- `commander` — CLI parsing
- `@sinclair/typebox` — JSON schema for tool parameters

No dependency on `@mariozechner/pi-coding-agent`. We use the lower-level `pi-agent-core` directly.

## Estimated Size

~1.5-2K lines TypeScript for MVP (vs ~25K lines Rust currently).

## What's Deferred

- TUI monitoring dashboard
- Explorer/Evaluator modes
- Reviewer agents (auto-spawn on queue submit)
- Postmortem agent
- Hive Mind (knowledge sharing between runs)
- Cost budgets and limits
- Web UI
- SDK/library extraction
