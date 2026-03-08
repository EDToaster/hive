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

    /// Stop the current run and clean up worktrees
    Stop,
}
