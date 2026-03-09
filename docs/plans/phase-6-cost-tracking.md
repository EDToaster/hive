# Phase 6: Cost & Token Tracking

**Goal:** Give operators and agents visibility into run costs, and enforce budget limits to prevent runaway spending.

**Context:** The building blocks exist — `AgentCost` struct in `types.rs`, `load_agent_cost()` in `state.rs` (parses `output.json`), and cost display in `cmd_summary`. What's missing: a standalone CLI command, an MCP tool for agents, and budget enforcement.

---

## Domain 1: Config & State (Lead: cost-config)

**Files:** `src/state.rs`

### Add `budget_usd` to HiveConfig

Add `pub budget_usd: Option<f64>` field to `HiveConfig` struct. Default to `None`.

Parse it in `load_config()` using the same line-by-line pattern as existing fields:
```rust
if let Some(value) = line.strip_prefix("budget_usd:")
    && let Ok(v) = value.trim().parse::<f64>()
{
    config.budget_usd = Some(v);
}
```

### Add `total_run_cost()` helper

Add a method to `HiveState` that computes the total cost for a run by iterating all agents and summing `load_agent_cost()`:
```rust
pub fn total_run_cost(&self, run_id: &str) -> f64 {
    // load all agents, sum load_agent_cost for each
}
```

### Tests

- `load_config_reads_budget_usd` — parse `budget_usd: 25.0` correctly
- `load_config_budget_usd_none_when_missing` — defaults to None
- `load_config_budget_usd_ignores_invalid` — non-numeric value → None

---

## Domain 2: CLI Command (Lead: cost-cli)

**Files:** `src/cli.rs`, `src/main.rs`

### Add `hive cost` command

Add `Commands::Cost` variant to cli.rs with an optional `--run` arg.

Add `cmd_cost()` in main.rs that:
1. Loads the run (active or specified)
2. Loads all agents
3. Calls `load_agent_cost()` for each
4. Prints a table:
```
Run Cost: $X.XX
Budget:   $Y.YY (Z% used)  ← only if budget_usd is set

Agent                Role         Input     Output    Cost
coordinator          coordinator  123.4k    45.6k     $1.23
lead-backend         lead         98.7k     32.1k     $0.87
...
```
5. If no agents have cost data yet, print "No cost data available (agents still running?)."

---

## Domain 3: MCP Tool + Budget Enforcement (Lead: cost-mcp)

**Files:** `src/mcp.rs`

### Add `hive_run_cost` MCP tool

New tool that any agent can call to check run spending:
```rust
#[tool(description = "Get cost summary for the current run — token usage and estimated cost per agent")]
async fn hive_run_cost(&self) -> Result<CallToolResult, McpError>
```

Returns formatted text with per-agent costs and total, same data as the CLI command.

### Add budget enforcement to `hive_spawn_agent`

Before spawning, check if budget would be exceeded:
```rust
let config = state.load_config();
if let Some(budget) = config.budget_usd {
    let current_cost = state.total_run_cost(&self.run_id);
    if current_cost >= budget {
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "Budget exceeded: ${:.2} spent of ${:.2} budget. Cannot spawn new agents.",
            current_cost, budget
        ))]));
    }
}
```

This is a soft check (can't predict future cost), but prevents spawning when already over budget.

---

## Lead Decomposition

| Lead | Domain | Primary Files | Can Merge Independently |
|------|--------|---------------|------------------------|
| lead-config | Config & state | `src/state.rs` | Yes |
| lead-cli | CLI command | `src/cli.rs`, `src/main.rs` | After config (uses `total_run_cost`) |
| lead-mcp | MCP + budget | `src/mcp.rs` | After config (uses `total_run_cost`, `budget_usd`) |

**Merge order:**
1. lead-config (no dependencies)
2. lead-cli (depends on `total_run_cost`)
3. lead-mcp (depends on `total_run_cost` and `budget_usd`)
