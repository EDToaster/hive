# Closed-Loop Autonomous Development — Full Implementation

**Goal:** Complete the remaining two capabilities from the closed-loop design: Run Memory and Self-Planning. Together with the already-implemented Active Review, these make hive a fully autonomous development system.

**Design doc:** `docs/plans/2026-03-08-closed-loop-design.md` has the full design. Read it first.

---

## Capability 1: Run Memory

A persistent knowledge store at `.hive/memory/` that accumulates across runs and feeds into agent prompts.

### What to build

**State layer (`src/state.rs`):**
- CRUD methods for three memory types:
  - `operations.jsonl` — one JSON entry per run (tasks, agents, failures, duration, cost, learnings)
  - `conventions.md` — codebase conventions discovered during runs
  - `failures.jsonl` — anti-patterns extracted from failed tasks
- Load methods that return formatted strings suitable for prompt injection
- Pruning: operations keeps last 10, conventions caps at 50 entries, failures caps at 30

**Post-mortem agent (`src/agent.rs`, `src/mcp.rs`, `src/main.rs`):**
- When `hive stop` is called, spawn a post-mortem agent that:
  - Reads all tasks, agents, messages, and costs from the completed run
  - Analyzes: what failed and why, token efficiency, spec quality, team sizing
  - Writes entries to operations.jsonl, updates conventions.md, adds failure patterns
- Add `hive_save_memory` MCP tool (post-mortem agent only) to write memory entries
- Post-mortem agent gets read-only hooks (same as reviewer) plus write access to `.hive/memory/` only

**Prompt injection (`src/agent.rs`):**
- Modify all prompt generators to include relevant memory:
  - Coordinator: operational patterns
  - Leads: conventions + failure patterns
  - Workers: conventions + failure patterns
  - Reviewers: conventions + failure patterns
- Load memory from `.hive/memory/` and append as `## Project Memory` section
- If memory exceeds ~2000 tokens, summarize before injection

**CLI (`src/cli.rs`, `src/main.rs`):**
- `hive memory` command — list all memory entries
- `hive memory show` — display full contents
- `hive memory prune` — remove stale entries

**TUI (`src/tui.rs`):**
- Memory panel showing active knowledge count (N operations, N conventions, N failures)

---

## Capability 2: Self-Planning

Replace human-written specs with a planner agent that writes its own spec from a goal string.

### What to build

**Planner agent (`src/agent.rs`):**
- New `Planner` variant in `AgentRole` enum (`src/types.rs`)
- Planner prompt: reads codebase, reads run memory, writes a spec in the standard format (goal, implementation details, lead decomposition, file boundaries, merge ordering)
- Read-only hooks (same as reviewer) — planner analyzes but doesn't implement
- The planner saves its spec to `.hive/runs/<id>/spec.md`

**Modified start flow (`src/main.rs`):**
- `hive start` detects whether the argument is a file path or a goal string
  - File path (contains `/` or ends in `.md`): existing flow, use file as spec
  - Goal string (everything else): spawn planner agent first
- Planner phase: spawn planner, wait for it to finish (with 5-minute timeout), then read the generated spec and launch the coordinator with it
- If planner fails or times out, report error — don't start the coordinator

**CLI (`src/cli.rs`):**
- Update `hive start` to accept either `spec: String` (file path or goal string)
- Add `--goal` flag as explicit alternative: `hive start --goal "Add WebSocket support"`

**MCP tools (`src/mcp.rs`):**
- Add `hive_save_spec` tool (planner-only) to write the spec file
- Planner gets read access to all existing hive tools for codebase analysis

**TUI (`src/tui.rs`):**
- Planning phase view: show "Planning..." with planner status before coordinator starts
- Spec viewer: show the generated spec content

**Role permissions update (`src/mcp.rs`):**
- Planner: can read everything, can save spec, cannot spawn agents or modify tasks
- Update the permissions table in all require_role checks

---

## File Boundaries for Leads

| Domain | Files | Notes |
|--------|-------|-------|
| types + state | `src/types.rs`, `src/state.rs` | New role variant, memory CRUD, memory types |
| agent prompts | `src/agent.rs` | Planner prompt, memory injection into all prompts, planner/postmortem hooks |
| MCP tools | `src/mcp.rs` | hive_save_memory, hive_save_spec, permissions, Planner role handling |
| CLI + main | `src/cli.rs`, `src/main.rs` | Modified start flow, hive memory command, postmortem spawning in stop |
| TUI | `src/tui.rs` | Memory panel, planning phase view, spec viewer |

These have clean boundaries. The types/state lead should merge first since others depend on the new types. After that, the rest can merge in any order with rebasing.
