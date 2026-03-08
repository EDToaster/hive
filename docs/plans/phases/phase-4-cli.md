# Phase 4: CLI

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the full CLI with all commands: init, start, status, agents, tasks, messages, log-tool, logs, tui, mcp.

**Prerequisite:** Phase 3 complete (git and agent modules working).

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

### Task 4.1: CLI skeleton with `hive init` and `hive start`

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs` (replace with CLI entry point)

**Step 1: Write the CLI module**

```rust
// src/cli.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hive", about = "Agentic swarm coordinator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize .hive/ in the current repo
    Init,

    /// Start a new run with a spec file
    Start {
        /// Path to the spec file
        spec: String,
    },

    /// Show current run status
    Status,

    /// List agents and their health
    Agents,

    /// List tasks and statuses
    Tasks {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,
    },

    /// View message history
    Messages {
        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,
    },

    /// Record a tool call event (called by agent hooks)
    LogTool {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        duration: Option<i64>,
    },

    /// Query the event log
    Logs {
        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,
    },

    /// Launch the monitoring dashboard
    Tui,

    /// Run as MCP server (stdio transport)
    Mcp {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
    },
}
```

**Step 2: Rewrite main.rs as the CLI entry point**

```rust
// src/main.rs
mod agent;
mod cli;
mod git;
mod logging;
mod state;
mod types;

use clap::Parser;
use cli::{Cli, Commands};
use logging::LogDb;
use state::HiveState;
use std::fs;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cmd_init(),
        Commands::Start { spec } => cmd_start(&spec),
        Commands::Status => cmd_status(),
        Commands::Agents => cmd_agents(),
        Commands::Tasks { status, assignee } => cmd_tasks(status, assignee),
        Commands::Messages { agent } => cmd_messages(agent),
        Commands::LogTool {
            run,
            agent,
            tool,
            status,
            duration,
        } => cmd_log_tool(&run, &agent, &tool, &status, duration),
        Commands::Logs { agent } => cmd_logs(agent),
        Commands::Tui => cmd_tui(),
        Commands::Mcp { run, agent } => cmd_mcp(&run, &agent),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn cmd_init() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;

    // Check we're in a git repo
    if !cwd.join(".git").exists() {
        return Err("Not a git repository. Run 'git init' first.".into());
    }

    let hive_dir = cwd.join(".hive");
    if hive_dir.exists() {
        return Err(".hive/ already exists. Already initialized.".into());
    }

    fs::create_dir_all(hive_dir.join("runs")).map_err(|e| e.to_string())?;

    // Write default config
    let config = "# Hive configuration\nstall_timeout_seconds: 300\n";
    fs::write(hive_dir.join("config.yaml"), config).map_err(|e| e.to_string())?;

    println!("Initialized .hive/ in {}", cwd.display());
    Ok(())
}

fn cmd_start(spec_path: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    let spec_content = fs::read_to_string(spec_path)
        .map_err(|e| format!("Cannot read spec file '{spec_path}': {e}"))?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;
    state.save_spec(&run_id, &spec_content)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    println!("Created run: {run_id}");
    println!("Spec saved to .hive/runs/{run_id}/spec.md");

    // TODO: Spawn coordinator agent
    println!("TODO: Spawn coordinator agent");

    Ok(())
}

fn cmd_status() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;
    let tasks = state.list_tasks(&run_id)?;
    let queue = state.load_merge_queue(&run_id)?;

    println!("Run: {run_id}");
    println!("Agents: {} total", agents.len());
    for agent in &agents {
        println!(
            "  {} ({:?}) - {:?}",
            agent.id, agent.role, agent.status
        );
    }
    println!("Tasks: {} total", tasks.len());
    let by_status = |s: types::TaskStatus| tasks.iter().filter(|t| t.status == s).count();
    println!(
        "  pending={} active={} review={} merged={} failed={}",
        by_status(types::TaskStatus::Pending),
        by_status(types::TaskStatus::Active),
        by_status(types::TaskStatus::Review),
        by_status(types::TaskStatus::Merged),
        by_status(types::TaskStatus::Failed),
    );
    println!("Merge queue: {} entries", queue.entries.len());

    Ok(())
}

fn cmd_agents() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;

    if agents.is_empty() {
        println!("No agents.");
        return Ok(());
    }

    for agent in &agents {
        let alive = agent
            .pid
            .map(|pid| {
                if agent::AgentSpawner::is_alive(pid) {
                    "alive"
                } else {
                    "dead"
                }
            })
            .unwrap_or("no pid");
        println!(
            "{:<20} role={:<12?} status={:<10?} parent={:<20} process={}",
            agent.id,
            agent.role,
            agent.status,
            agent.parent.as_deref().unwrap_or("-"),
            alive,
        );
    }
    Ok(())
}

fn cmd_tasks(status_filter: Option<String>, assignee_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let tasks = state.list_tasks(&run_id)?;

    let filtered: Vec<_> = tasks
        .iter()
        .filter(|t| {
            if let Some(ref s) = status_filter {
                let status_str = serde_json::to_value(&t.status)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                if status_str.as_deref() != Some(s.as_str()) {
                    return false;
                }
            }
            if let Some(ref a) = assignee_filter {
                if t.assigned_to.as_deref() != Some(a.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No tasks match filters.");
        return Ok(());
    }

    for task in &filtered {
        println!(
            "{:<12} {:?} [{:?}] assigned={} - {}",
            task.id,
            task.status,
            task.urgency,
            task.assigned_to.as_deref().unwrap_or("-"),
            task.title,
        );
    }
    Ok(())
}

fn cmd_messages(agent_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let messages = state.list_messages(&run_id)?;

    let filtered: Vec<_> = messages
        .iter()
        .filter(|m| {
            if let Some(ref a) = agent_filter {
                return m.from == *a || m.to == *a;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No messages.");
        return Ok(());
    }

    for msg in &filtered {
        println!(
            "[{}] {} -> {} ({:?}): {}",
            msg.timestamp.format("%H:%M:%S"),
            msg.from,
            msg.to,
            msg.message_type,
            msg.body,
        );
    }
    Ok(())
}

fn cmd_log_tool(
    run_id: &str,
    agent_id: &str,
    tool: &str,
    status: &str,
    duration: Option<i64>,
) -> Result<(), String> {
    let state = HiveState::discover()?;
    let log_path = state.run_dir(run_id).join("log.db");
    let db = LogDb::open(&log_path)?;

    // Look up agent role
    let agent = state.load_agent(run_id, agent_id).ok();
    let role = agent
        .map(|a| format!("{:?}", a.role).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());

    db.log_tool_call(
        run_id,
        agent_id,
        &role,
        "claude-code",
        tool,
        None,
        status,
        duration,
    )?;
    Ok(())
}

fn cmd_logs(agent_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let log_path = state.run_dir(&run_id).join("log.db");
    let db = LogDb::open(&log_path)?;

    let summary = db.agent_tool_summary(&run_id)?;

    let filtered: Vec<_> = summary
        .iter()
        .filter(|(aid, _, _, _)| {
            if let Some(ref a) = agent_filter {
                return aid == a;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No log entries.");
        return Ok(());
    }

    println!("{:<20} {:<25} {:>8} {:>10}", "Agent", "Tool", "Count", "Avg ms");
    println!("{}", "-".repeat(65));
    for (agent_id, tool_name, count, avg_ms) in &filtered {
        println!("{:<20} {:<25} {:>8} {:>10}", agent_id, tool_name, count, avg_ms);
    }
    Ok(())
}

fn cmd_tui() -> Result<(), String> {
    // TODO: Phase 6
    println!("TUI not yet implemented.");
    Ok(())
}

fn cmd_mcp(_run_id: &str, _agent_id: &str) -> Result<(), String> {
    // TODO: Phase 5
    println!("MCP server not yet implemented.");
    Ok(())
}
```

**Step 3: Verify it compiles and runs**

Run: `cargo build && cargo run -- --help`
Expected: prints help text with all subcommands listed

Run: `cargo run -- init` (from the hive repo root)
Expected: "Initialized .hive/ in /Users/howard/src/hive" (or error if already exists)

**Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add CLI skeleton with init, start, status, agents, tasks, messages, logs"
```

---

## Next Phase

Proceed to Phase 5: `docs/plans/phases/phase-5-mcp-server.md`
