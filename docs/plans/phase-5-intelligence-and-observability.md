# Phase 5: Agent Intelligence & Observability

**Goal:** Make agents smarter, track costs, and provide run summaries. This is a large multi-domain change that should use 4+ leads working in parallel.

---

## Domain 1: Smarter Agent Prompts (Lead: prompt-engineering)

**Files:** `src/agent.rs`

The current agent prompts are generic templates. They need to be significantly enhanced to produce better agent behavior based on observed issues from previous runs.

### Coordinator Prompt Enhancements

The coordinator currently wastes ~2 min reading the codebase (47 Bash calls in run 1) before creating tasks. Fix this by:

1. **Pre-compute codebase summary in `hive start`**: Before writing CLAUDE.local.md, scan the repo and generate a brief summary of the project structure (languages, key directories, file counts, test framework). Embed this in the coordinator prompt so it doesn't need to explore.

2. **Add explicit task creation guidance**: The coordinator prompt should include:
   ```
   ## Task Creation Protocol
   - Create ALL tasks FIRST with proper blocked_by relationships before spawning any leads.
   - Use the domain field to tag each task for file-conflict prevention.
   - Set urgency: critical for blocking tasks, high for core features, normal for polish.
   - Each task title should be specific and actionable, not vague.
   ```

3. **Add merge queue management guidance**:
   ```
   ## Merge Queue Protocol
   - After hive_wait_for_activity reports a queue entry, immediately call hive_merge_next.
   - If merge fails, notify the lead and consider using hive_retry_agent.
   - After each merge, rebuild if needed and check for regressions.
   ```

### Lead Prompt Enhancements

Leads currently sometimes do implementation work themselves instead of spawning workers. Fix:

1. **Enforce delegation pattern**:
   ```
   ## Delegation Protocol
   - ALWAYS spawn workers for implementation. You are a manager, not an implementer.
   - Read the relevant source files to understand the codebase, then write a detailed implementation plan.
   - Spawn one worker per logical unit of work (usually one file or one feature).
   - After spawning workers, use hive_wait_for_activity and hive_check_agents to monitor.
   - When workers finish, review their work with hive_review_agent before submitting.
   ```

2. **Add review checklist**:
   ```
   ## Code Review Protocol
   - Use hive_review_agent to see commits and diff stat.
   - Verify: tests pass (check worker's output), no unrelated changes, matches the task description.
   - If changes needed, send a message to the worker explaining what to fix. They will be auto-resumed.
   - Only submit to merge queue after review passes.
   ```

3. **Add proactive health checking**:
   ```
   ## Health Monitoring
   - After spawning workers, call hive_check_agents every 60 seconds.
   - If a worker is idle or failed, review their work immediately.
   - Don't wait indefinitely — if hive_wait_for_activity times out, check agents.
   ```

### Worker Prompt Enhancements

Workers need better guidance on quality and scope:

1. **Add test-first guidance**:
   ```
   ## Implementation Protocol
   - Read the existing code in your target file(s) FIRST to understand patterns and conventions.
   - Write tests BEFORE implementation when possible.
   - Run tests after every significant change: `cargo test --all-targets`
   - Run clippy before finishing: `cargo clippy --all-targets -- -D warnings`
   - Fix any issues before marking the task as review.
   ```

2. **Add scope discipline**:
   ```
   ## Scope Discipline
   - Only modify files in your assigned domain. Do not touch files outside your scope.
   - If you discover a bug in another file, create a task for it — don't fix it yourself.
   - Do not run `cargo fmt` on the entire project — only format files you modified.
   - Keep commits focused: one logical change per commit.
   ```

3. **Add completion protocol**:
   ```
   ## Completion Protocol
   - Before finishing: git add your changed files, commit with a descriptive message.
   - Run the full test suite one final time.
   - Call hive_update_task to set status to "review".
   - Send a message to your lead summarizing what you implemented and any concerns.
   - Then stop. Do not loop or do additional work.
   ```

### Implementation

The `generate_prompt` function in `agent.rs` should be significantly expanded. Also modify `coordinator_prompt` in the same way.

Add a new function `generate_codebase_summary(repo_root: &Path) -> String` that runs:
- `find . -name "*.rs" | wc -l` (or equivalent)
- `ls src/` to list modules
- `head -5 Cargo.toml` for project name/version
- Returns a 5-10 line summary

Call this from `cmd_start` in `main.rs` and embed in the coordinator prompt.

---

## Domain 2: Cost & Token Tracking (Lead: cost-tracking)

**Files:** `src/logging.rs`, `src/types.rs`, `src/mcp.rs`, `src/cli.rs`, `src/main.rs`

### Token Tracking

Agents already produce `output.json` with token counts. Add infrastructure to aggregate this.

**File: `src/types.rs`** — Add:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub session_duration_secs: u64,
}
```

**File: `src/state.rs`** — Add:
```rust
pub fn load_agent_cost(&self, run_id: &str, agent_id: &str) -> Option<AgentCost>
```

This reads `agents/<agent_id>/output.json`, parses the `num_input_tokens`, `num_output_tokens`, `session_duration_seconds` fields, and estimates cost using Claude pricing ($15/M input, $75/M output for Opus).

**File: `src/mcp.rs`** — Add new tool:
```rust
#[tool(description = "Get cost summary for the current run — token usage and estimated cost per agent")]
async fn hive_run_cost(&self) -> Result<CallToolResult, McpError>
```

This iterates all agents, loads their costs, and returns a formatted summary.

### CLI Cost Command

**File: `src/cli.rs`** — Add:
```
Commands::Cost => cmd_cost()
```

**File: `src/main.rs`** — Add `cmd_cost()` that prints per-agent and total cost for the active run.

### Budget Limits (Config)

**File: `src/state.rs`** — Add to HiveConfig:
```rust
pub budget_usd: Option<f64>,
```

Parse from config.yaml. When set, `hive_spawn_agent` checks total run cost against budget before spawning.

---

## Domain 3: Run Lifecycle & Summaries (Lead: run-lifecycle)

**Files:** `src/cli.rs`, `src/main.rs`, `src/state.rs`, `src/types.rs`

### Run Summary

**File: `src/main.rs`** — Add `cmd_summary()`:

When a run completes or is stopped, generate a summary:
```
Run: <id>
Duration: Xm Ys
Total Cost: $X.XX
Agents: X spawned, X completed, X failed
Tasks: X created, X merged, X failed

Merged Changes:
  - <commit msg 1>
  - <commit msg 2>
  ...

Per-Agent Cost:
  coordinator  $X.XX  (Xk input, Xk output)
  lead-foo     $X.XX  (Xk input, Xk output)
  ...
```

**File: `src/cli.rs`** — Add `Commands::Summary { run: Option<String> }`.

### Run History

**File: `src/main.rs`** — Add `cmd_history()`:

List all past runs with their status, duration, task counts, and cost.

**File: `src/cli.rs`** — Add `Commands::History`.

### Enhanced `hive stop`

Modify `cmd_stop` to automatically print the run summary after stopping.

---

## Domain 4: Enhanced CLI UX (Lead: cli-ux)

**Files:** `src/cli.rs`, `src/main.rs`

### Better `hive status` Output

Rewrite `cmd_status` to produce a more informative, color-coded output:
```
Run: abc123 (active, 5m 32s)
Agents: 3 running · 1 idle · 1 done
Tasks:  2 active · 1 merged · 1 pending
Queue:  1 entry
```

Use terminal colors (via the `colored` crate or similar, or just ANSI codes directly) for status indicators.

### `hive watch` Command

Add a non-TUI watch mode that prints status updates to stdout every N seconds:

**File: `src/cli.rs`** — Add:
```
Commands::Watch { interval: Option<u64> }
```

**File: `src/main.rs`** — Add `cmd_watch()`:
- Loops every `interval` seconds (default 10)
- Clears screen and prints current status
- Shows recent activity (last 5 messages + tool calls)
- Exits on Ctrl-C

This is lighter than the full ratatui TUI and works in any terminal.

### `hive init` Improvements

After `hive init`, write a sensible default config.yaml with comments:
```yaml
# Hive configuration
stall_timeout_seconds: 300
max_retries: 2
# verify_command: "cargo test --all-targets && cargo clippy --all-targets -- -D warnings"
# budget_usd: 50.0
```

---

## Lead Decomposition

All four domains have clean file boundaries and can run fully in parallel.

| Lead | Domain | Primary Files | Can Merge Independently |
|------|--------|---------------|------------------------|
| lead-prompts | Smarter prompts | `src/agent.rs`, `src/main.rs` (codebase summary) | Yes |
| lead-cost | Cost tracking | `src/logging.rs`, `src/types.rs`, `src/state.rs`, `src/mcp.rs` | Yes |
| lead-lifecycle | Run summaries | `src/cli.rs`, `src/main.rs` | Yes |
| lead-cli-ux | CLI improvements | `src/cli.rs`, `src/main.rs` | Depends on lifecycle |

**Note:** lead-lifecycle and lead-cli-ux both touch `src/cli.rs` and `src/main.rs`. To avoid conflicts:
- lead-lifecycle handles: `Commands::Summary`, `Commands::History`, enhanced `cmd_stop`
- lead-cli-ux handles: `Commands::Watch`, enhanced `cmd_status`, enhanced `cmd_init`
- Merge lifecycle FIRST, then cli-ux rebases onto it.

**Merge order:**
1. lead-prompts (no dependencies)
2. lead-cost (no dependencies, can merge in parallel with prompts)
3. lead-lifecycle (no dependencies on 1 or 2)
4. lead-cli-ux (after lifecycle due to shared files)
