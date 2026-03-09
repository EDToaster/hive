# Speculative Swarm Implementation Plan

**Goal:** Add explore mode, Hive Mind knowledge space, explorer/evaluator agent roles, and the `hive explore` CLI command to enable speculative, divergent development workflows.

**Architecture:** Extends the existing agent hierarchy with two new roles (Explorer, Evaluator), adds a real-time shared knowledge store (`.hive/runs/<run>/mind/`) with discovery/insight/query MCP tools accessible to all agents, and introduces `hive explore` as a lightweight alternative to `hive start` that spawns explorers instead of the full lead/worker pipeline.

**Tech Stack:** Rust, clap (CLI), rmcp (MCP), serde/serde_json (serialization), chrono (timestamps), uuid (IDs), existing HiveState pattern for filesystem operations.

---

## Domain 1: Types and State (foundation — everything else depends on this)

**Files:** `src/types.rs`, `src/state.rs`

Add `Explorer` and `Evaluator` variants to the `AgentRole` enum (with `#[serde(rename_all = "lowercase")]` like existing roles). Fix any non-exhaustive match warnings in `agent.rs` and `state.rs` by adding placeholder arms for the new roles.

Add three new types to `src/types.rs`:
- `Confidence` enum: `Low`, `Medium`, `High` (serde lowercase)
- `Discovery` struct: `id`, `run_id`, `agent_id`, `timestamp` (DateTime<Utc>), `content`, `file_paths` (Vec, serde default), `confidence`, `tags` (Vec, serde default)
- `Insight` struct: `id`, `run_id`, `timestamp`, `content`, `discovery_ids` (Vec), `tags` (Vec, serde default)

Add Hive Mind state operations to `src/state.rs`:
- `mind_dir(run_id) -> PathBuf` — returns `.hive/runs/<run>/mind/`
- `save_discovery` / `load_discoveries` — JSONL append/read at `mind/discoveries.jsonl` (same pattern as `save_operation` / `load_operations`)
- `save_insight` / `load_insights` — JSONL append/read at `mind/insights.jsonl`
- `query_mind(run_id, query) -> MindQueryResult` — case-insensitive keyword search across discovery content/tags/file_paths and insight content/tags. Returns `MindQueryResult { discoveries: Vec<Discovery>, insights: Vec<Insight> }`

Update `load_memory_for_prompt` to include conventions and failures for Explorer and Evaluator roles (same access as Worker).

**Tests:** Roundtrip serialization for all new types. Save/load/query for discoveries and insights. Query matching on content, tags, and file paths. Memory prompt inclusion for new roles.

---

## Domain 2: MCP Tools (depends on Domain 1)

**Files:** `src/mcp.rs`

Add four new MCP tools with parameter structs:

1. **`hive_discover`** — any agent can call. Records a discovery to the Hive Mind. Params: `content` (String), `confidence` (String, default "medium"), `file_paths` (Vec, default empty), `tags` (Vec, default empty). Generates a `disc-<uuid8>` ID, constructs a `Discovery`, calls `state.save_discovery`.

2. **`hive_query_mind`** — any agent can call. Searches the Hive Mind by keyword. Params: `query` (String). Calls `state.query_mind`, formats results as readable markdown with discovery IDs, tags, agent, confidence, content, and file paths.

3. **`hive_synthesize`** — coordinator-only (use existing `require_role` pattern). Promotes discoveries into an insight. Params: `content` (String), `discovery_ids` (Vec), `tags` (Vec, default empty). Generates an `ins-<uuid8>` ID, calls `state.save_insight`.

4. **`hive_establish_convention`** — coordinator-only. Appends a convention to `conventions.md`. Params: `content` (String). Calls `state.load_conventions`, appends, calls `state.save_conventions`.

Update `hive_spawn_agent`:
- Add `"explorer"` and `"evaluator"` to role parsing (alongside existing `"lead"` / `"worker"`)
- Update hierarchy enforcement: coordinator can spawn `Lead | Explorer | Evaluator`, lead can spawn `Worker` (explorers and evaluators cannot spawn anything)

**Tests:** Permission checks for synthesize/establish_convention (coordinator-only). Spawn permission tests for new roles.

---

## Domain 3: Agent Prompts (depends on Domain 1)

**Files:** `src/agent.rs`

Add Explorer prompt to `generate_prompt` match arm. Key elements:
- Role identification (agent ID, role: explorer, parent)
- Mandate from task description
- Discovery protocol: use `hive_discover` for findings, `hive_query_mind` before starting
- Produce: working prototype, structured analysis, or proof-of-concept
- Commit work, call `hive_update_task` to set status "review" when done
- Constraints: no spawning, no merge queue, no messages except to coordinator, focus on learning not polish

Add Evaluator prompt to `generate_prompt` match arm. Key elements:
- Role identification
- Read and analyze code on each explorer branch
- Run tests on each branch
- Compare on: lines changed, test coverage, complexity, correctness, maintainability
- Query Hive Mind for explorer discoveries
- Write structured comparison (evaluation.md) in own worktree
- Constraints: READ-ONLY for source code, only Read/Glob/Grep/Bash for tests

Update `spawn()` settings_json: Evaluator gets read-only hooks (same as Reviewer). Explorer gets default read-write hooks (same as Worker).

Add `explore_coordinator_prompt(run_id, intent, codebase_summary, memory) -> String` as a new method on `AgentSpawner`. This prompt puts the coordinator in EXPLORE mode with three phases:
- **Phase 1 (Think Mode):** Analyze codebase, query Hive Mind, present analysis, discuss with human, refine direction
- **Phase 2 (Explore Mode):** Create tasks, spawn explorers (always include one adversarial explorer), wait for completion, spawn evaluator, present comparison
- **Phase 3 (Decision):** Present three options — merge directly, refine, or escalate to full execution
- Constraints: don't skip Phase 1, don't read/write code directly, only spawn explorers/evaluators

**Tests:** Explorer prompt contains mandate, hive_discover, hive_query_mind, does NOT contain hive_spawn_agent or hive_submit_to_queue. Evaluator prompt contains READ-ONLY and comparison instructions. Explore coordinator prompt contains EXPLORE mode, Think Mode, adversarial explorer, and Hive Mind tool references.

---

## Domain 4: CLI Commands (depends on Domains 1, 3)

**Files:** `src/cli.rs`, `src/main.rs`

Add `Explore` command to `src/cli.rs`:
```
Explore { intent: String }
```

Implement `cmd_explore` in `src/main.rs`. It follows the same pattern as `cmd_start` but:
- Creates a run, initializes log.db, saves intent as spec
- Uses `explore_coordinator_prompt` instead of `coordinator_prompt`
- Writes the same coordinator config files (CLAUDE.local.md, settings.local.json, .mcp.json)
- Registers coordinator agent
- Prints instructions to launch Claude Code

Add `Mind` command with optional subcommand:
```
Mind { command: Option<MindCommands> }
MindCommands::Query { query: String }
```

Implement `cmd_mind`:
- No subcommand: show summary (discovery count, insight count, recent 5 discoveries, all insights)
- `query` subcommand: call `state.query_mind`, print matching discoveries and insights with metadata

**Tests:** CLI help shows `explore` and `mind` commands. Build succeeds.

---

## Ordering / Dependencies

```
Domain 1 (Types + State)  ←  everything depends on this
    ├── Domain 2 (MCP Tools)
    ├── Domain 3 (Agent Prompts)
    └── Domain 4 (CLI Commands) ← also depends on Domain 3
```

Domain 1 must merge first. Domains 2 and 3 can run in parallel after Domain 1. Domain 4 depends on Domain 3 (for `explore_coordinator_prompt`).
