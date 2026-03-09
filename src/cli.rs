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

    /// Start a new run with a spec file or goal string
    Start {
        /// Path to spec file, or goal string (use --goal for explicit goal mode)
        spec: Option<String>,
        /// Provide a goal string directly (alternative to positional arg)
        #[arg(long)]
        goal: Option<String>,
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

    /// Update an agent's heartbeat timestamp (called by agent hooks)
    Heartbeat {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
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

    /// Wait for activity in the current run (blocks until change or timeout)
    Wait {
        #[arg(long)]
        run: Option<String>,
        /// Timeout in seconds (default 60)
        #[arg(long, default_value = "60")]
        timeout: u64,
    },

    /// Review a non-running agent's work (commits, diff stat)
    ReviewAgent {
        /// Agent ID to review
        agent_id: String,
        /// Run ID (defaults to active run)
        #[arg(long)]
        run: Option<String>,
    },

    /// Read messages for an agent
    ReadMessages {
        /// Agent ID whose messages to read
        #[arg(long)]
        agent: String,
        /// Run ID (defaults to active run)
        #[arg(long)]
        run: Option<String>,
        /// Only show unread messages (since last read or last idle)
        #[arg(long)]
        unread: bool,
        /// Stop hook mode: exit 2 with stderr output if unread messages exist
        #[arg(long)]
        stop_hook: bool,
    },

    /// Show run summary (cost, agents, tasks, merged commits)
    Summary {
        /// Run ID (defaults to active run)
        #[arg(long)]
        run: Option<String>,
    },

    /// Show cost breakdown for the current run
    Cost {
        /// Run ID (defaults to active run)
        #[arg(long)]
        run: Option<String>,
    },

    /// List all past runs
    History,

    /// View and manage run memory
    Memory {
        #[command(subcommand)]
        command: Option<MemoryCommands>,
    },

    /// Stop the current run and clean up worktrees
    Stop,

    /// Watch run status with periodic refresh
    Watch {
        /// Refresh interval in seconds (default 10)
        #[arg(long, default_value = "10")]
        interval: u64,
    },
}

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Display full memory contents
    Show,
    /// Remove stale entries (prune operations to 10, failures to 30)
    Prune,
}
