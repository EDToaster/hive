# Hybrid Worktree Strategy: Findings

## Problem

`git worktree add` on large monorepos (535K files, 12GB working tree) takes **86 seconds** and uses **11 GB disk** per agent. With 10+ agents this becomes a blocker.

Key benchmark data:
| Approach | Time | Disk/agent |
|----------|------|------------|
| Full worktree | **86s** | **11 GB** |
| `--no-checkout` + sparse (1 domain) | **8s** | **463 MB** |

## Solution: Role-Based WorktreeStrategy

Implemented a `WorktreeStrategy` enum (in `src/types.rs`) with three variants:

```rust
pub enum WorktreeStrategy {
    Full,                              // All files (leads, reviewers)
    Sparse { paths: Vec<String> },     // Only specified dirs (workers, explorers)
    NoCheckout,                        // No files at all (postmortem)
}
```

### Default Per-Role Strategy

| Role | Default | Rationale |
|------|---------|-----------|
| Lead | `Full` | Needs broad context for conflict resolution & cross-domain review |
| Reviewer | `Full` | Needs full diff context |
| Worker | `Sparse { paths: ["src"] }` | Scoped to task domain, use task.domain if set |
| Explorer | `Sparse { paths: ["src"] }` | Implementation-focused, usually in src/ |
| Evaluator | `Sparse { paths: ["src"] }` | Comparing code across branches |
| Postmortem | `NoCheckout` | Reads .hive/ data only, no source needed |
| Coordinator | N/A | Never gets a worktree |

## Priority Resolution (Highest → Lowest)

1. **Per-spawn `sparse_paths`** — explicit `hive_spawn_agent(sparse_paths=[...])` override
2. **Task domain** — if worker has `task.domain = "payments"`, use `Sparse { paths: ["payments"] }`
3. **Config `worktree_<role>`** — `config.yaml` per-role override (e.g., `worktree_worker: full`)
4. **Role default** — hardcoded default for each role

## Git Operations

The correct sequence for sparse checkout after `--no-checkout`:

```
git worktree add --no-checkout <path> -b <branch>
git -C <path> sparse-checkout init --cone
git -C <path> sparse-checkout set <dir1> <dir2>
git -C <path> checkout     # ← REQUIRED: populates files
```

**Key finding**: `git sparse-checkout set` does NOT populate files when starting from `--no-checkout`. An explicit `git checkout` (no args) is required. This respects the sparse cone — only specified directories (plus root files) appear.

## Config Surface

### config.yaml (per-repo/project)
```yaml
# Override per-role defaults
worktree_lead: full
worktree_worker: sparse   # uses "src" as default path
worktree_postmortem: no_checkout
```

### Per-spawn override (MCP)
```
hive_spawn_agent(
  agent_id="worker-payments",
  role="worker",
  task_id="task-abc",
  sparse_paths=["payments", "billing"]  # explicit override
)
```

### Task domain (automatic)
Tasks with `domain="payments"` automatically scope workers to that domain:
- Worker gets `Sparse { paths: ["payments"] }` without any explicit config

## Answers to Design Questions

**1. Per-role vs per-task?** Both. Role sets the default; task.domain provides automatic per-task scoping for workers; per-spawn override handles exceptional cases.

**2. Sparse opt-in or opt-out?** Opt-out. Workers default to sparse (safe for new large repos), leads default to full (safe for conflict resolution). This is the right default for the slow-repo problem — the common case gets optimization automatically.

**3. Worker needs files outside sparse cone?** Three options:
   a. Coordinator spawns with broader `sparse_paths`
   b. Agent runs `git sparse-checkout add <new-dir>` to expand its cone
   c. Worker requests lead to handle cross-domain coordination

**4. Leads with sparse checkout?** Possible via config (`worktree_lead: sparse`), but full is the safe default since leads need cross-domain review context.

**5. Config surface?** Three layers: `HiveConfig.worktrees` (WorktreeConfig), task.domain (automatic), per-spawn sparse_paths parameter.

**6. Agent.worktree field?** Unchanged. Still stores the path as `Option<String>`. Strategy is embedded in the spawn process, not persisted per-agent (could be added if needed).

## Code Changes

- `src/types.rs`: Added `WorktreeStrategy` enum + `default_for_role()`
- `src/git.rs`: Added `worktree_add_no_checkout()`, `sparse_checkout_init()`, `sparse_checkout_set()`, `sparse_checkout_list()`, `checkout_populate()`
- `src/agent.rs`: Added `spawn_with_options()` with `strategy_override: Option<WorktreeStrategy>`; existing `spawn_with_model()` delegates to it
- `src/state/mod.rs`: Added `WorktreeConfig` struct + `worktree_<role>:` config parsing in `load_config()`
- `src/mcp/params.rs`: Added `sparse_paths: Option<Vec<String>>` to `SpawnAgentParams`
- `src/mcp/agent_tools.rs`: Strategy resolution logic in `hive_spawn_agent`

## Test Coverage

- 12 new tests: 4 for WorktreeStrategy (types.rs), 4 for git sparse checkout (git.rs), 2 for config parsing (state/tests.rs), 2 for WorktreeConfig resolution
- All 684 tests pass, zero clippy warnings

## Risks and Open Questions

1. **Root files in cone mode**: `git sparse-checkout init --cone` always checks out root-level files (`.gitignore`, `Cargo.toml`, etc.). This is usually desirable but adds small overhead.

2. **Escaping the cone mid-task**: An agent can call `git sparse-checkout add <dir>` to expand its cone. This should be documented in the worker prompt.

3. **Coordinator spawning without worktree**: Coordinators still get no worktree. Their spawn path (in `cli/run_cmds.rs`) bypasses `AgentSpawner` entirely and should remain unchanged.

4. **Postmortem with NoCheckout**: Postmortem agents read `.hive/` data, which is NOT in the worktree (it's at the repo root). The worktree path is set correctly in the agent config, and `.hive/` is discoverable by `HiveState::discover()`. No change needed.

5. **Reviewer with Full checkout**: Reviewers do a full checkout to read diff context. This may be optimizable in future (they could use `--no-checkout` + `git show` to read without creating files).
