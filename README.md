# Hive

A stateless CLI and MCP server that orchestrates a swarm of [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agents for autonomous software development.

Hive manages agent lifecycles, task state, message passing, git worktree isolation, and a sequential merge queue. All state lives in `.hive/` as JSON files + SQLite. No daemon — every invocation reads and writes `.hive/` directly.

```
User
  │
  │  writes spec or goal, runs `hive start`
  ▼
Coordinator (Claude Code)
  │  uses hive MCP tools to orchestrate
  │
  ├── Lead: Backend (worktree, tactical)
  │     ├── Worker: implement endpoint
  │     └── Worker: write tests
  │
  ├── Lead: Frontend (worktree, tactical)
  │     └── Worker: build component
  │
  └── Lead: Infrastructure (worktree, spun up mid-run)
        └── Worker: add CI config
```

## Features

- **Agent hierarchy** — Coordinator decomposes specs into domain chunks for leads, who spawn workers for implementation. Permissions enforced per role.
- **Git worktree isolation** — Each lead and worker gets its own worktree and branch. No conflicts during development; merge conflicts caught at merge time.
- **Sequential merge queue** — Leads submit approved branches; the coordinator merges one at a time with auto-rebase on conflict and optional post-merge verification.
- **Active code review** — Submitting to the merge queue auto-spawns a read-only reviewer agent that evaluates changes against the task description before approval.
- **Self-planning** — Pass a goal string instead of a spec file. A planner agent analyzes the codebase and writes a detailed implementation spec.
- **Run memory** — Post-mortem agent auto-spawns on `hive stop`, writing operational learnings, codebase conventions, and failure patterns to `.hive/memory/`. Injected into future agent prompts.
- **Hive Mind** — Shared knowledge space where agents record discoveries and conventions during a run. Coordinator can synthesize insights. Searchable via CLI and MCP tools.
- **Explore mode** — `hive explore` launches divergent exploration runs for codebase analysis and research.
- **Cost tracking** — Per-agent token usage and USD cost, with optional budget enforcement.
- **Per-role model configuration** — Default model tiers per role (Opus for coordinator/planner, Sonnet for lead/worker/explorer/evaluator, Haiku for reviewer/postmortem) with per-role overrides in config.
- **Monitoring TUI** — Real-time dashboard showing agent tree, task board, merge queue, and activity stream.
- **Event-driven orchestration** — File watching with `hive wait` for efficient activity detection. Auto-notifications on state changes.
- **Stall detection and retry** — Heartbeat-based health checking with configurable timeouts. Failed agents can be retried with fresh worktrees.

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024)
- [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code) (`claude` must be on PATH)
- Git

## Installation

```bash
git clone <repo-url>
cd hive
cargo install --path .
```

## Quick Start

### 1. Initialize

```bash
cd your-project    # must be a git repository
hive init          # creates .hive/ directory with default config
```

### 2. Start a run

From a spec file:
```bash
hive start docs/plans/my-feature.md
```

Or from a goal string (spawns a planner agent to write the spec):
```bash
hive start "Add WebSocket support for real-time status updates"
```

### 3. Launch the coordinator

In a new terminal:
```bash
claude --dangerously-skip-permissions
```

Then instruct it:
```
Begin. Read CLAUDE.local.md for your assignment. Create all tasks with
blocked_by relationships first, then spawn leads.
```

### 4. Monitor

```bash
hive tui           # interactive dashboard
# or
hive status        # one-shot status
hive agents        # list agents
hive tasks         # list tasks
hive watch         # auto-refreshing status
```

### 5. Finish

```bash
hive stop          # stops agents, removes worktrees, runs post-mortem analysis
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `hive init` | Initialize `.hive/` in the current git repo |
| `hive start <spec\|goal>` | Start a new run from a spec file or goal string |
| `hive status` | Show current run status (agents, tasks, queue) |
| `hive agents` | List all agents with role, status, and process health |
| `hive tasks [--status X] [--assignee Y]` | List tasks with optional filters |
| `hive messages [--agent X]` | View message history |
| `hive logs [--agent X]` | Query tool call event log |
| `hive cost [--run X]` | Per-agent cost breakdown |
| `hive summary [--run X]` | Run summary with merged commits |
| `hive history` | List all past runs |
| `hive memory [show\|prune]` | View or manage cross-run memory |
| `hive mind [query <keyword>]` | Search the Hive Mind knowledge space |
| `hive explore <intent>` | Start a divergent exploration run |
| `hive config` | Show current configuration (model mappings, budget, etc.) |
| `hive tui` | Launch the interactive monitoring dashboard |
| `hive watch [--interval N]` | Auto-refreshing status display |
| `hive review-agent <id>` | Review a completed agent's commits and diff |
| `hive stop` | Stop current run, clean up worktrees, run post-mortem |
| `hive wait [--timeout N]` | Block until activity is detected or timeout |

Internal commands used by agent hooks:

| Command | Description |
|---------|-------------|
| `hive mcp --run <id> --agent <id>` | Run as stdio MCP server for an agent |
| `hive log-tool --run <id> --agent <id> --tool <name> --status <s>` | Record tool call event |
| `hive heartbeat --run <id> --agent <id>` | Update agent heartbeat |
| `hive read-messages --agent <id> [--unread] [--stop-hook]` | Read messages (used by Stop hook) |
| `hive agent-exit --run <id> --agent <id>` | Transition agent to Idle on exit (used by Stop hook) |

## MCP Tools

Agents interact with hive via these MCP tools:

| Tool | Description | Available to |
|------|-------------|-------------|
| `hive_spawn_agent` | Spawn a lead or worker with worktree and task | Coordinator (leads), Lead (workers) |
| `hive_create_task` | Create a task with title, urgency, dependencies | All roles |
| `hive_update_task` | Update task status, assignment, or branch | All roles |
| `hive_list_tasks` | Query tasks by status, assignee, or domain | All roles |
| `hive_send_message` | Send a message to another agent | All roles (scoped) |
| `hive_read_messages` | Read messages sent to this agent | All roles |
| `hive_submit_to_queue` | Submit branch for review + merge | Lead only |
| `hive_merge_next` | Process next merge queue entry | Coordinator only |
| `hive_list_agents` | List all agents and statuses | All roles |
| `hive_check_agents` | Health check via heartbeat + process liveness | Coordinator, Lead |
| `hive_review_agent` | View agent's commits and diff stat | Coordinator, Lead |
| `hive_retry_agent` | Re-spawn a failed agent with fresh worktree | Coordinator, Lead |
| `hive_review_verdict` | Submit approve/request-changes/reject | Reviewer only |
| `hive_wait_for_activity` | Block until state change or timeout | Coordinator, Lead |
| `hive_run_cost` | Get token usage and cost per agent | All roles |
| `hive_heartbeat` | Update heartbeat timestamp | All roles |
| `hive_log_tool` | Record a tool call event | All roles |
| `hive_save_memory` | Write memory entry (operational/convention/failure) | Postmortem only |
| `hive_save_spec` | Save generated spec for the run | Planner only |
| `hive_record_discovery` | Record a discovery to the Hive Mind | All roles |
| `hive_search_mind` | Search the Hive Mind by keyword | All roles |
| `hive_synthesize_insight` | Synthesize discoveries into an insight | Coordinator only |
| `hive_add_convention` | Add a convention to shared memory | Coordinator only |

## Agent Roles

| Role | Purpose | Worktree | Default Model | Spawns |
|------|---------|----------|---------------|--------|
| **Coordinator** | Strategic orchestration, merge queue processing | No | Opus | Leads |
| **Lead** | Domain management, worker oversight, code review | Yes | Sonnet | Workers |
| **Worker** | Implementation of specific tasks | Yes | Sonnet | None |
| **Reviewer** | Automated code review against task intent | Yes (read-only) | Haiku | None |
| **Planner** | Codebase analysis and spec generation | Yes (read-only) | Opus | None |
| **Postmortem** | Run analysis and memory extraction | Yes | Haiku | None |
| **Explorer** | Divergent codebase exploration and research | Yes | Sonnet | None |
| **Evaluator** | Evaluation and assessment tasks | Yes | Sonnet | None |

Model tiers can be overridden per-role in `.hive/config.yaml` under `models:`.

### Role permissions

| Capability | Coordinator | Lead | Worker |
|-----------|-------------|------|--------|
| Spawn agents | Leads only | Workers only | No |
| Submit to merge queue | Process only | Yes | No |
| Send messages | To leads | To workers + coordinator | To own lead only |

## Task Lifecycle

```
pending ──► active ──► review ──► approved ──► queued ──► merged
               │          │
               ▼          ▼
            blocked    active (sent back with feedback)
               │
               ▼
            active (unblocked)

Any state ──► failed
Any state ──► absorbed   (subtask rolled into parent)
Any state ──► cancelled  (no changes needed)
```

## Architecture

Single Rust binary, three entry points:

- **`hive <command>`** — CLI for humans
- **`hive mcp --run <id> --agent <id>`** — stdio MCP server for Claude Code agents
- **`hive tui`** — read-only monitoring dashboard

### Module layout

```
src/
  main.rs       — CLI dispatch
  cli/          — clap command definitions and handlers
    mod.rs      — CLI struct, subcommands
    run_cmds.rs — init, start, stop, status, cost, summary, etc.
    agent_cmds.rs — agents, logs, heartbeat, review-agent
    task_cmds.rs  — tasks
    message_cmds.rs — messages, read-messages
    memory_cmds.rs  — memory, mind
  types.rs      — Agent, Task, Message, MergeQueue, ModelConfig, enums
  state/        — all .hive/ filesystem reads/writes (nothing else touches disk)
    mod.rs      — core state operations
    agents.rs   — agent state
    tasks.rs    — task state
    messages.rs — message state
    queue.rs    — merge queue state
    memory.rs   — run memory and hive mind state
  git.rs        — shells out to git CLI for worktree, merge, branch ops
  agent.rs      — agent spawn: worktree creation, config generation, process launch
  mcp/          — MCP server via rmcp with #[tool_router] macro
    mod.rs      — server setup and tool routing
    agent_tools.rs — spawn, list, check, retry, heartbeat, wait, review
    task_tools.rs  — create, update, list tasks
    queue_tools.rs — submit, merge, review verdict
    message_tools.rs — send, read messages, log tool
    mind_tools.rs  — record discovery, search, synthesize, conventions
    misc_tools.rs  — cost, save memory, save spec
    params.rs      — shared parameter types
  tui/          — ratatui monitoring dashboard
    mod.rs      — app state and main loop
    render.rs   — pane rendering
    input.rs    — keyboard handling
    overlay.rs  — detail overlays
    tree.rs     — agent hierarchy tree
    helpers.rs  — formatting utilities
  output.rs     — output capture for agent processes
  logging.rs    — SQLite logging layer (log.db)
  wait.rs       — file watching for activity detection
```

### Key design decisions

- **Stateless** — Hive reads `.hive/` from disk on every call. No in-memory state between invocations. No daemon means no crashes, no stale state, easy debugging.
- **MCP as primary interface** — Agents interact with hive via structured MCP tool calls. The CLI is a thin wrapper for humans.
- **Agent config via CLAUDE.local.md** — Generated per-agent with role, task, and constraints. Additive to any existing project CLAUDE.md, not checked into git.
- **Message injection** — Messages are delivered directly into agent conversations via Claude Code's resume mechanism, not polled.
- **Atomic state** — File locking (`fs2`) and atomic writes (write-to-tmp-then-rename) prevent partial reads/writes.

### `.hive/` directory structure

```
.hive/
  config.yaml                    # stall timeout, verify command, budget, model overrides
  memory/
    operations.jsonl             # run summaries and learnings
    conventions.md               # discovered codebase conventions
    failures.jsonl               # failure patterns
  runs/
    <run-id>/
      run.json                   # run metadata (id, created_at, status)
      spec.md                    # original spec
      merge-queue.json           # ordered list of branches to merge
      log.db                     # SQLite: tool calls, timing, events
      agents/
        <agent-id>/
          agent.json             # role, status, parent, pid, worktree, heartbeat
          output.json            # claude output capture
          stderr.log             # stderr capture
          cost.json              # token usage and cost
      tasks/
        <task-id>.json           # title, description, status, urgency, dependencies
      messages/
        <message-id>.json        # from, to, type, body, refs
      worktrees/
        <agent-id>/              # git worktrees for leads and workers
```

## Configuration

`.hive/config.yaml`:

```yaml
# How long (seconds) before an agent with no heartbeat is considered stalled
stall_timeout_seconds: 300

# Maximum retry attempts for failed agents
max_retries: 2

# Command to run for verification after merges (uncomment to enable)
# verify_command: "cargo test --all-targets && cargo clippy --all-targets -- -D warnings"

# Maximum budget in USD for a single run (uncomment to enable)
# budget_usd: 50.0

# Per-role model overrides (uncomment to customize)
# models:
#   coordinator: opus
#   lead: sonnet
#   worker: sonnet
#   reviewer: haiku
#   planner: opus
#   postmortem: haiku
#   explorer: sonnet
#   evaluator: sonnet
```

## TUI Dashboard

The TUI provides four panes navigable with `Tab`:

- **Swarm** — Agent hierarchy tree with status, heartbeat age, and task assignment. Merge queue shown below.
- **Tasks** — Table of all tasks with status, assignee, and title. Review cycle count shown.
- **Activity** — Interleaved message and tool call stream from all agents. Filterable by selected agent.
- **Spec** — Shows the generated spec (when using self-planning).

Keyboard: `j/k` to navigate, `Enter` for detail overlay, `Esc` to dismiss, `G` for auto-scroll, `q` to quit.

## How It Works

### Agent spawn sequence

When `hive_spawn_agent` is called:

1. **Create worktree** — `git worktree add` with a new branch `hive/<run-id>/<agent-id>`
2. **Write hooks config** — `.claude/settings.local.json` with PostToolUse hooks for logging + heartbeat, and a Stop hook that checks for unread messages
3. **Write MCP config** — `.mcp.json` registering hive as an MCP server for this agent
4. **Write instructions** — `CLAUDE.local.md` generated per-role with task description, responsibilities, constraints, and any run memory
5. **Launch Claude Code** — `claude -p <prompt> --output-format json --dangerously-skip-permissions`
6. **Register agent** — Write agent metadata to `.hive/runs/<run-id>/agents/<agent-id>/`

### Review pipeline

1. Lead calls `hive_submit_to_queue` with a task and branch
2. Hive spawns a reviewer agent in the lead's worktree
3. Reviewer reads code changes, evaluates against task description
4. Reviewer calls `hive_review_verdict` with approve, request-changes, or reject
5. On approve: branch enters the merge queue
6. On request-changes: lead receives feedback, can fix and resubmit (max 3 cycles)
7. On reject: task is marked failed

### Merge process

1. Coordinator calls `hive_merge_next`
2. Hive checks out `main`, attempts merge with `--no-ff`
3. On conflict: auto-rebase attempted. If rebase fails, lead is notified to rebase and resubmit
4. If `verify_command` is configured, runs verification. On failure: merge is rolled back
5. On success: task marked as merged, branch cleaned up

## Writing Specs

Specs are markdown files describing what to build. High-level domain-based specs outperform detailed step-by-step plans — leads decompose their own work effectively.

A good spec includes:
- **Goal** — One paragraph on what to build and why
- **Implementation details** — Technical description of changes
- **Lead decomposition** — Domain chunks with file boundaries (no file in two domains)
- **Merge ordering** — Which leads must merge first due to type/API dependencies

Example structure:
```markdown
# Feature: WebSocket Support

## Goal
Add real-time status updates via WebSocket connections.

## Leads

### Lead 1: Types and State (merge first)
- Files: src/types.rs, src/state.rs
- Add WebSocket connection tracking types
- Persist connection state in .hive/

### Lead 2: Server (depends on Lead 1)
- Files: src/ws.rs (new), src/main.rs
- WebSocket upgrade endpoint
- Broadcast state changes to connected clients

### Lead 3: TUI Integration
- Files: src/tui.rs
- Replace polling with WebSocket subscription
```

## Development

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo test --all-targets       # run all tests (651 tests)
cargo clippy --all-targets -- -D warnings   # lint
cargo fmt --all -- --check     # check formatting
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `rmcp` + `schemars` | MCP server with JSON Schema tool definitions |
| `ratatui` + `crossterm` | Terminal UI dashboard |
| `rusqlite` | SQLite for tool call logging |
| `tokio` | Async runtime |
| `serde` + `serde_json` | JSON serialization |
| `chrono` | Timestamps |
| `uuid` | Unique IDs |
| `notify` | Filesystem watching |
| `fs2` | File locking |
| `libc` | Process management (signals, waitpid) |

## License

MIT
