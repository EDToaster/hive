# Hive Dogfooding Process

This document captures the process and learnings from using hive to iteratively improve itself — a full dogfooding loop where the product was used to build its own features.

---

## Overview

Starting from a working but minimal orchestrator (~2500 lines of Rust), hive was used to coordinate Claude Code agents to implement 8 phases of improvements across 8 runs. The process was managed by a human-in-the-loop operator (also a Claude Code instance) who wrote specs, launched runs, monitored progress, and iterated.

**Starting state:** 100 tests, basic agent lifecycle, no quality gates, no error recovery, no locking, generic prompts.

**Ending state:** 155 tests, 7000+ lines of Rust. Autonomous development system with self-planning, active code review, run memory, cost tracking, budget enforcement, verification before merge, auto-rebase, agent retry, stall detection, file locking, atomic writes, enhanced prompts, run summaries, history, watch command, color-coded CLI, and a monitoring TUI.

---

## The Iteration Loop

Each iteration follows this cycle:

### 1. Analyze

Before writing any spec, analyze the current state:
- What's broken or unreliable?
- What's the highest-impact improvement?
- What did the last run reveal about hive's own weaknesses?

Use subagents to review merged code quality and run efficiency after each run. This produces concrete findings (bugs, inefficiencies, missing error handling) that feed into the next spec.

### 2. Write Spec

Write a spec as a markdown file in `docs/plans/`. The spec should include:
- **Goal**: One sentence on why this matters.
- **What to build**: High-level description per domain — what the feature does, which files are involved.
- **File boundaries**: Which leads own which files, to avoid merge conflicts.
- **Dependency ordering**: Which tasks must merge first.

**Key lesson (revised):** High-level domain-based specs outperform detailed step-by-step plans. Earlier runs used exact function signatures and line numbers; later runs used domain descriptions and let leads decompose the work themselves. The agents are capable of figuring out implementation details — the spec's job is to define *what* to build and *where*, not *how*.

Alternatively, with self-planning enabled, you can skip the spec entirely:
```bash
hive start "Add WebSocket support for real-time agent status updates"
```
This spawns a planner agent that reads the codebase and writes its own spec.

### 3. Start Run

```bash
cargo install --path .              # ensure latest binary
hive start docs/plans/<spec>.md     # create run from spec file
# OR
hive start "Add feature X to Y"    # create run from goal string (spawns planner)
```

This generates:
- `.hive/runs/<id>/` directory structure
- `CLAUDE.local.md` with coordinator prompt + spec
- `.mcp.json` for hive MCP server
- `.claude/settings.local.json` for hooks

### 4. Launch Coordinator

Open a new terminal pane and start Claude Code:
```bash
claude --dangerously-skip-permissions
```

Then instruct it to begin:
```
Begin. Read CLAUDE.local.md for your assignment. Create all tasks with
blocked_by relationships first, then spawn leads. Manage merge order per spec.
```

**Key lesson:** The coordinator needs explicit instructions to create tasks first, then spawn. Without this, it starts exploring the codebase and wastes 2+ minutes before acting.

### 5. Monitor

Open the TUI in a separate pane:
```bash
hive tui
```

Periodically check status:
```bash
hive status && hive agents && hive tasks
```

Watch for:
- Workers showing `process=dead` but `status=Running` (lead hasn't detected completion yet)
- Tasks stuck in Review (reviewer is evaluating, or reviewer requested changes and worker is fixing)
- Review cycles — tasks may go through up to 3 review rounds before auto-rejection
- Reviewer agents spawning (appears as `reviewer-task-XXX` with `role=Reviewer`)
- Merge queue entries (coordinator needs to process them)
- Failed tasks (may need retry or investigation)
- Stalled agents (no heartbeat for 5+ minutes — auto-killed and can be retried)

### 6. Rebuild After Merges

**Critical:** After each merge to main that changes `src/state.rs`, `src/types.rs`, or other core files, rebuild the hive binary:

```bash
cargo install --path .
```

Running agents use the `hive` binary via MCP. If the binary is stale and the on-disk state format changed (e.g., new fields on Agent struct), agents will fail to deserialize state.

### 7. Wait for Completion

**Never stop a run prematurely.** Let the coordinator finish processing all merges and marking tasks. The run is done when:
- All tasks are Merged or Failed
- The merge queue is empty
- The coordinator is idle or has stopped

### 8. Verify

After all tasks merge:
```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

If tests fail, the next spec should include fixes.

### 9. Post-Run Analysis

`hive stop` now automatically spawns a **post-mortem agent** that:
- Analyzes all tasks, agents, messages, and costs from the completed run
- Writes operational learnings to `.hive/memory/operations.jsonl`
- Updates codebase conventions in `.hive/memory/conventions.md`
- Records failure patterns in `.hive/memory/failures.jsonl`

This memory persists across runs and is automatically injected into future agent prompts, making each run smarter than the last.

You can also manually review with:
```bash
hive memory          # list all memory entries
hive cost            # per-agent cost breakdown
hive summary         # run summary with merged commits
```

### 10. Clean Up

```bash
hive stop                              # stop run, remove worktrees, run post-mortem
# close TUI and coordinator panes
```

---

## Run History

### Run 1: Quality Gates (140c4a5c)
- **Duration:** 13 min | **Agents:** 5 | **Cost:** ~$4.73
- **What:** Added `verify_command` config, `run_shell_command`, `Git::rebase`, auto-rebase on merge conflict, post-merge notifications
- **Decomposition:** 2 leads (backend: state.rs + git.rs, MCP: mcp.rs), 2 workers under backend lead
- **Result:** 2/2 tasks merged, tests 100→100
- **Findings:**
  - Coordinator made 47 Bash calls exploring codebase before creating tasks (2 min wasted)
  - lead-mcp did implementation directly instead of spawning workers
  - `git reset --hard` used via `run_shell_command` instead of proper Git method
  - Queue save failures silently ignored with `.ok()`

### Run 2: Merge Safety Fixes (06478001)
- **Duration:** ~8 min | **Agents:** 3
- **What:** Fixed unsafe `git reset --hard` via shell, added proper `Git::reset_hard` method, added error handling for reset failures, added config parsing tests
- **Decomposition:** 1 lead, 1 worker (small focused fix)
- **Result:** Work merged, tests 100→104
- **Lesson:** Small targeted fixes are fast but don't leverage hive's parallelism well

### Run 3: Error Recovery (19a6f244)
- **Duration:** ~12 min | **Agents:** 5
- **What:** Added `retry_count` to Agent, `max_retries` config, `hive_retry_agent` MCP tool, enhanced stall detection with SIGTERM/SIGKILL escalation, `notes` field on UpdateTaskParams, context management prompts
- **Decomposition:** 2 leads (types+state, agent+MCP), proper sequential ordering
- **Result:** 2/2 tasks merged, tests 104→110
- **Findings:**
  - Workers show `process=dead` but leads don't notice until they call `hive_check_agents`
  - Need proactive health checking in lead prompts

### Run 4: Atomic State (635da4c8)
- **Duration:** ~10 min | **Agents:** 5
- **What:** Added `fs2` file locking, `lock_file` method, `atomic_write` helper (write-to-tmp-then-rename), lock acquisition around all MCP read-modify-write operations
- **Decomposition:** 2 leads (state.rs locking, mcp.rs lock integration)
- **Result:** 2/2 tasks merged, tests 110→114
- **Findings:** Same dead-worker-detection delay pattern observed

### Run 5: Intelligence & Observability (f524d61a)
- **Duration:** 12 min | **Agents:** 11 | **Tasks:** 8
- **What:** Enhanced all agent prompts (codebase summary, delegation enforcement, review checklists, health monitoring), run summary command, history command, `hive watch`, color-coded `hive status`, enhanced `hive init`
- **Decomposition:** 4 parallel leads (prompts, cost-tracking, lifecycle, cli-ux), 7 workers
- **Result:** 4/8 tasks merged, 1 failed (cost tracking), 3 approved but not merged before premature stop
- **Lesson:** Never stop a run early. The coordinator was still processing approved tasks when killed.

### Run 6: Cost Tracking (e15598fd)
- **Duration:** ~8 min | **Agents:** 7 | **Tasks:** 6
- **What:** Added `budget_usd` config + `total_run_cost()` helper, `hive cost` CLI command, `hive_run_cost` MCP tool, budget enforcement in `hive_spawn_agent`
- **Decomposition:** 3 leads (config, CLI, MCP), workers under each
- **Result:** 6/6 tasks merged (plus run 5's unmerged CLI work), tests 119→123
- **Note:** Also landed run 5's previously unmerged branches (hive watch, enhanced status/init)

### Run 7: Active Review (2fa2cf65)
- **Duration:** 27 min | **Agents:** 9 | **Tasks:** 6
- **What:** Added `Reviewer` role, reviewer prompt with read-only hook enforcement (PreToolUse blocks Edit/Write/Bash writes), `hive_review_verdict` MCP tool (approve/request-changes/reject), modified `hive_submit_to_queue` to spawn reviewer instead of direct queue entry, review cycle tracking (max 3 cycles), TUI reviewer display
- **Decomposition:** 4 leads (types, agent, mcp, tui) — types merged first, then agent + tui in parallel, then mcp last
- **Result:** 6/6 tasks merged, tests 123→125
- **Findings:**
  - Lead-mcp caught a scope violation: the MCP worker had modified `agent.rs` (lead-agent's domain). Lead sent it back with specific fix instructions, worker corrected by resetting the file.
  - Worktree branch locking prevented merge of `lead-mcp` branch. Lead worked around by creating a copy branch `lead-mcp-submit`. This is a known limitation when a branch is checked out in a worktree.

### Run 8: Run Memory + Self-Planning (fda20d42)
- **Duration:** 44 min | **Agents:** 23 | **Tasks:** 7
- **What:** Full closed-loop implementation — run memory (`.hive/memory/` with operations, conventions, failures), post-mortem agent (auto-spawns on `hive stop`), memory injection into all agent prompts, `hive memory` CLI, planner agent role, `hive start "goal string"` self-planning flow, `hive_save_memory` and `hive_save_spec` MCP tools, TUI memory panel + planning phase view
- **Decomposition:** 5 leads (types/state, agent prompts, MCP tools, CLI/main, TUI). Types/state merged first as critical dependency, then 4 leads ran in parallel.
- **Result:** 7/7 tasks merged, tests 125→155
- **First run with active review working:** 5 reviewer agents spawned automatically across the run. Reviewers caught issues and sent workers back for changes before approving.
- **Findings:**
  - Active review pipeline worked end-to-end: submit → reviewer spawns → evaluates → verdict → approve/request-changes
  - High-level specs work better than micromanaged plans — leads decomposed their own work effectively
  - 23 agents is the largest swarm so far — coordinator managed the complexity well
  - 3 workers stalled (heartbeat timeout), coordinator retried them successfully
  - Post-mortem agent spawned automatically on `hive stop` and wrote memory entries

---

## Key Learnings

### About Hive's Design

1. **Stateless architecture works well.** Every `hive` invocation reads from disk. No daemon means no crashes, no stale state, easy debugging. The tradeoff is performance (file I/O on every call) but for the scale of agent orchestration, it's fine.

2. **Git worktrees are the right isolation model.** Each agent gets its own branch and directory. Merge conflicts are caught at merge time, not during development. The auto-rebase feature resolves most conflicts automatically.

3. **The coordinator-lead-worker hierarchy is effective.** Coordinators stay strategic, leads manage scope, workers implement. When leads try to implement directly, quality drops and the parallelism advantage is lost.

4. **Message injection via resume works but has latency.** When a message is sent to an idle agent, hive resumes its Claude Code session with the message. This takes 5-10 seconds for the agent to spin up and process. Not instant, but reliable.

### About Agent Behavior

5. **High-level specs outperform micromanaged ones.** Early runs used exact function signatures and line numbers. Later runs used domain-level descriptions and let leads decompose. The agents are capable of figuring out implementation details — the spec's job is to define *what* and *where*, not *how*. Run 8 (23 agents, 7 tasks) used a fully high-level spec and produced the best results.

6. **Leads must be told to delegate.** Without explicit "ALWAYS spawn workers" instructions, leads often implement code themselves. This defeats the purpose of the hierarchy.

7. **Workers need scope discipline.** Without explicit "only modify files in your domain" instructions, workers run `cargo fmt` on the whole project or make drive-by fixes to unrelated files, causing merge conflicts.

8. **Health checking is not automatic.** Leads don't proactively check if workers are alive. They wait for `hive_wait_for_activity` to time out, which can take minutes. The prompts now instruct leads to call `hive_check_agents` every 60 seconds.

9. **The coordinator should create all tasks before spawning any agents.** This produces a clear dependency graph and prevents leads from starting work before prerequisites are defined.

### About the Operator Workflow

10. **Rebuild the binary after every merge.** Agents use the `hive` CLI. If the binary is stale after a merge changes the state format, subsequent agent operations fail.

11. **Zellij pane management requires care.** Always verify focus before sending keystrokes. Use `write 13` (CR) not `write 10` (LF) to submit input in Claude Code.

12. **Post-run analysis is essential.** Every run reveals issues that become the next spec. The code quality subagent catches bugs the agents missed. The efficiency subagent identifies wasted work patterns.

13. **Bigger specs produce better parallelism.** Small specs (1-2 leads) don't leverage hive's strength. Run 8 with 5 leads and 23 agents was the most productive.

14. **Never stop a run prematurely.** Let the coordinator finish all merges and task updates. Killing a run mid-merge can leave approved work unmerged and task statuses inconsistent.

### About Active Review (Runs 7-8)

15. **Automated review catches real issues.** In run 7, a reviewer's scope check led the lead to catch a worker modifying files outside its domain. The worker was sent back with specific fix instructions and corrected the issue.

16. **Review cycles add latency but improve quality.** Each review cycle spawns a fresh reviewer agent (5-10 second startup) and takes 1-2 minutes to evaluate. Tasks may go through 2-3 cycles. The max of 3 cycles prevents infinite loops.

17. **Read-only hooks are essential.** Reviewer agents are sandboxed via PreToolUse hooks that block Edit, Write, and dangerous Bash commands. Prompt instructions alone are insufficient — agents will "helpfully" fix typos they find during review, causing merge conflicts.

### About Self-Planning and Memory (Run 8)

18. **Post-mortem analysis automates the iteration loop.** Previously, post-run analysis required manually launching subagents. Now `hive stop` spawns a post-mortem agent that writes structured learnings to `.hive/memory/`. This feeds into future runs automatically.

19. **Memory injection must be size-controlled.** All memory is injected into agent prompts as a `## Project Memory` section. If memory grows too large, it needs summarization before injection to avoid consuming too much context.

---

## Current Feature Set (After Run 8)

| Feature | Status | Run |
|---------|--------|-----|
| Quality gates (verify_command) | Done | 1 |
| Auto-rebase on merge conflict | Done | 1 |
| Merge safety (proper Git methods) | Done | 2 |
| Error recovery (retry, stall detection) | Done | 3 |
| Atomic state (file locking) | Done | 4 |
| Enhanced prompts (delegation, review, health) | Done | 5 |
| CLI improvements (summary, history, watch, status) | Done | 5-6 |
| Cost/token tracking + budget enforcement | Done | 6 |
| Active review (reviewer agents, read-only hooks) | Done | 7 |
| Run memory (operations, conventions, failures) | Done | 8 |
| Self-planning (planner agent from goal string) | Done | 8 |
| Post-mortem analysis (auto on stop) | Done | 8 |

### Agent Roles

| Role | Purpose | Worktree | Write Access |
|------|---------|----------|-------------|
| Coordinator | Strategic orchestration, merge queue | No | N/A |
| Lead | Domain management, worker oversight | Yes | Yes |
| Worker | Implementation | Yes | Yes |
| Reviewer | Code review against task intent | Yes | **No (hook-enforced)** |
| Planner | Codebase analysis, spec writing | Yes | **No (hook-enforced)** |
| Postmortem | Run analysis, memory updates | No | Memory only |

---

## Remaining Work

### Not Yet Implemented
- **Human checkpoints** — No approval gates or interactive intervention during runs.
- **Codebase-aware task assignment** — No pre-flight analysis of which files each task will touch. Conflict avoidance is manual (via spec file boundaries).
- **CI integration** — Verification runs locally in the merge step. No webhook or external CI pipeline support.
- **Multi-project support** — Memory is per-project (`.hive/memory/`). No cross-project knowledge sharing.
- **Streaming progress** — Agents report only on completion. No real-time streaming of partial work.

### Known Issues
- Workers that exit are not immediately detected by leads. The `hive_check_agents` call is needed to trigger status transitions, and leads don't always call it promptly.
- The YAML config parser is line-by-line string matching, not a proper YAML parser. Works for simple values but fragile for complex configs.
- Worktree branch locking prevents merge when a branch is still checked out. Workaround: create a copy branch. Could be improved by merging from a detached HEAD or using `git merge --no-checkout`.
- `git rebase` changes HEAD of the worktree where it runs. Fixed by adding explicit `checkout main` before/after rebase in `hive_merge_next`, but other git operations may have similar issues.
