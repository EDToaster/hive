# Closed-Loop Autonomous Development

**Goal:** Transform hive from a mechanical orchestrator into an autonomous development system that can take a high-level goal, plan work, execute it, review its own output, and learn from every run.

---

## Three Capabilities

### 1. Active Review

Every piece of work goes through a dedicated **reviewer agent** before merge. The reviewer is read-only — it evaluates code but cannot modify it.

#### Flow

When a lead calls `hive_submit_to_queue`, instead of the branch entering the merge queue directly, hive spawns a reviewer agent that:

1. Checks out the submitted branch (read-only access to the worktree)
2. Reads the original task description — the "what was requested"
3. Reviews the diff against that intent: correctness, completeness, code quality, scope discipline
4. Produces a structured verdict via `hive_review_verdict`:
   - **approve** — enters merge queue, coordinator merges as today
   - **request-changes** — specific feedback sent to the original agent, which auto-resumes to address it. On completion, the agent re-submits and a new review cycle begins.
   - **reject** — task marked failed with explanation. Lead is notified.

#### Read-Only Enforcement

Reviewers are sandboxed via **hooks**, not just prompt instructions:

- `hooks.json` blocks all write tools: Edit, Write, Bash commands that modify files (detected via pre-command hook patterns), NotebookEdit
- Only allows: Read, Glob, Grep, and hive MCP tools (`hive_review_verdict`, `hive_read_messages`, `hive_list_tasks`)
- The reviewer prompt also states the constraint, but hooks are the hard enforcement layer

#### Review Cycles

A task may go through multiple review cycles. Each cycle:
- Spawns a fresh reviewer agent (no state from previous reviews — clean perspective)
- Tracks cycle count on the task (`review_count` field)
- After 3 failed reviews, task is auto-rejected to prevent infinite loops

### 2. Self-Planning

Replace human-written specs with a **planner agent** that analyzes the codebase and writes its own spec.

#### Flow

```
hive start "Add WebSocket support for real-time status updates"
```

When `hive start` receives a goal string (not a file path), it:

1. Creates the run as today
2. Spawns a **planner agent** — a one-shot agent that:
   a. Reads the codebase: module structure, public APIs, test patterns, Cargo.toml dependencies
   b. Reads run memory (Section 3): past specs, conventions, failure patterns
   c. Writes a full spec in the same format as `docs/plans/` files: goal, implementation details, lead decomposition, file boundaries, merge ordering
   d. Saves the spec to `.hive/runs/<id>/spec.md`
   e. Exits
3. The coordinator starts with this spec baked into its CLAUDE.local.md, same as today

**Fallback:** `hive start <spec-file>` still works. The planner is only invoked when given a quoted goal string. Power users can still write their own specs when they want precise control.

#### Planner Constraints

- The planner runs in a read-only worktree (same hooks as reviewer)
- It has a time budget — if it hasn't produced a spec within 5 minutes, hive kills it and reports failure
- The planner's spec is logged and viewable via TUI and CLI, so the operator can see what was planned before execution begins

### 3. Run Memory

A persistent knowledge store at `.hive/memory/` that accumulates across runs and feeds into agent prompts automatically.

#### Three Knowledge Types

**Operational patterns** (`.hive/memory/operations.jsonl`):

After each run, a **post-mortem agent** analyzes results and extracts:
- Which tasks failed and why (merge conflict, test failure, wrong approach, review rejection)
- Token efficiency per agent — exploring vs. implementing ratio
- Spec quality — did the decomposition produce clean merges or conflicts?
- Team sizing — was it too granular or too coarse?

One JSON entry per run, machine-readable. Keeps last 10 runs, oldest pruned automatically.

**Codebase conventions** (`.hive/memory/conventions.md`):

Discovered during planning and review:
- Error handling patterns ("uses `thiserror`, not string errors")
- Test patterns ("tests in same file under `#[cfg(test)] mod tests`")
- Architecture rules ("all state mutations go through `HiveState` methods")

Living document, capped at 50 entries. Updated by the post-mortem agent when it notices patterns. Contradicted entries are removed.

**Failure patterns** (`.hive/memory/failures.jsonl`):

Anti-patterns extracted from failed tasks:
- "Never run `cargo fmt` on files outside your domain — causes merge conflicts"
- "Workers touching both `types.rs` and `mcp.rs` always conflict with other leads"
- "Leads that implement directly instead of spawning workers produce lower quality"

Capped at 30 entries. Pruned when contradicted or stale (>20 runs old).

#### Prompt Injection

Each agent role gets relevant memory appended to its prompt:

| Role | Gets |
|------|------|
| Planner | operational patterns + conventions |
| Coordinator | operational patterns |
| Leads | conventions + failure patterns |
| Workers | conventions + failure patterns |
| Reviewers | conventions + failure patterns |

Memory is injected as a `## Project Memory` section in the generated CLAUDE.local.md. Size-controlled: if memory exceeds 2000 tokens, it's summarized before injection.

---

## Autonomous Run Lifecycle

```
Human: hive start "Add WebSocket support for real-time status updates"
                    |
                    v
            +---------------+
            |    Planner    |  Reads codebase + memory -> writes spec
            +-------+-------+
                    v
            +---------------+
            |  Coordinator  |  Creates tasks, spawns leads per spec
            +-------+-------+
                    v
            +---------------+
            | Leads + Workers|  Implement in isolated worktrees
            +-------+-------+
                    v
            +---------------+
            |   Reviewer    |  Read-only review against task intent
            |               |  approve / request-changes / reject
            +-------+-------+
                    |
           +--------+--------+
           v        v        v
        approve  changes   reject
           |        |        |
           v        v        v
        merge    feedback  fail task
        queue    -> agent   + explain
                 resumes
                    |
                    v
            +---------------+
            |  Post-mortem  |  Analyzes run -> updates memory
            +---------------+
                    |
                    v
            Human reviews merged code
```

---

## Codebase Changes

| Component | Changes |
|-----------|---------|
| `src/types.rs` | Add `Planner` and `Reviewer` to `AgentRole`. Add `review_count` to Task. Add memory types (OperationalEntry, FailureEntry). |
| `src/agent.rs` | Prompt generators for planner + reviewer roles. Reviewer `hooks.json` with write-tool blocking. Memory injection into all role prompts. |
| `src/mcp.rs` | Modify `hive_submit_to_queue` to spawn reviewer instead of direct queue entry. Add `hive_review_verdict` tool (reviewer-only). Update role permissions table. |
| `src/state.rs` | Memory CRUD: load/save/prune for operations, conventions, failures. Planner spec save/load. |
| `src/main.rs` | `cmd_start` detects goal string vs file path, spawns planner if goal. `cmd_stop` spawns post-mortem agent. Add `cmd_memory` for inspecting the knowledge store. |
| `src/cli.rs` | `hive start` accepts file path or quoted goal. Add `hive memory` subcommand (list, show, prune). |
| `src/tui.rs` | "In Review" task status rendering. Planning phase view. Memory panel showing active knowledge. Spec viewer for planner output. Review cycle count display. |

---

## Implementation Order

Each phase is a hive run, dogfooding the system:

### Phase 1: Active Review
- Add `Reviewer` role to types
- Reviewer prompt generation with read-only hooks
- `hive_review_verdict` MCP tool
- Modify `hive_submit_to_queue` to spawn reviewer
- Review cycle tracking (review_count, max 3 cycles)
- TUI: "In Review" status, reviewer agents in agent list

### Phase 2: Run Memory
- Memory types and state CRUD
- Post-mortem agent (spawned by `hive stop`)
- Memory injection into all agent prompts
- `hive memory` CLI command
- TUI: memory panel
- Size control and pruning

### Phase 3: Self-Planning
- Planner role and prompt generation (read-only hooks)
- `hive start` goal-string detection
- Planner spawning and spec generation flow
- Planner time budget enforcement
- TUI: planning phase view, spec viewer
- Integration with run memory for informed planning

---

## Role Permissions (Updated)

| Capability | Coordinator | Lead | Worker | Reviewer | Planner |
|------------|-------------|------|--------|----------|---------|
| Spawn agents | leads only | workers only | no | no | no |
| Submit to merge queue | process only | yes | no | no | no |
| Review verdict | no | no | no | yes | no |
| Send messages | to leads | to workers + coord | to own lead | to original agent | no |
| Write to worktree | n/a (no worktree) | yes | yes | **no (hooks enforce)** | **no (hooks enforce)** |
| Read codebase | yes | yes | yes | yes | yes |

---

## Design Decisions

**Why a separate reviewer agent instead of enhancing the lead's review?**
Leads have context bias — they wrote the spec and managed the worker. A fresh agent with no prior context provides a more honest review. It also means review quality doesn't depend on how good the lead is at reviewing.

**Why hooks for read-only enforcement instead of just prompts?**
Prompts are suggestions. Hooks are enforcement. An agent that "helpfully" fixes a typo it found during review would cause merge conflicts and break the isolation model. Hooks make this impossible.

**Why a one-shot planner instead of making the coordinator plan?**
Separation of concerns. The planner's job is analysis and spec-writing. The coordinator's job is execution management. Combining them would make the coordinator prompt enormous and reduce quality at both tasks. Also, the planner can fail without affecting the coordinator.

**Why cap review cycles at 3?**
Infinite review loops burn tokens without progress. If an agent can't address review feedback in 3 tries, the approach is likely wrong and needs human intervention or a different task decomposition.

**Why JSONL for operational and failure patterns?**
Append-only, easy to prune (delete lines), easy to parse, no corruption risk from concurrent writes. Conventions use markdown because they're human-readable documentation that operators may want to edit directly.
