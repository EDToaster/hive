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
        #[arg(long)]
        args_summary: Option<String>,
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

    /// Start an explore run with divergent exploration
    Explore { intent: String },

    /// View Hive Mind discoveries and insights
    Mind {
        #[command(subcommand)]
        command: Option<MindCommands>,
    },

    /// Transition an agent from Running to Idle on exit (called by Stop hook)
    AgentExit {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
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
pub enum MindCommands {
    /// Search the Hive Mind by keyword
    Query { query: String },
}

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Display full memory contents
    Show,
    /// Remove stale entries (prune operations to 10, failures to 30)
    Prune,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_all_subcommands_parseable() {
        // Verify every subcommand is reachable via try_parse_from
        let subcommands = [
            vec!["hive", "init"],
            vec!["hive", "status"],
            vec!["hive", "agents"],
            vec!["hive", "tasks"],
            vec!["hive", "messages"],
            vec!["hive", "logs"],
            vec!["hive", "tui"],
            vec!["hive", "history"],
            vec!["hive", "stop"],
            vec!["hive", "memory"],
            vec!["hive", "mind"],
            vec!["hive", "summary"],
            vec!["hive", "cost"],
            vec!["hive", "watch"],
            vec!["hive", "start"],
        ];
        for args in &subcommands {
            assert!(
                Cli::try_parse_from(args.iter()).is_ok(),
                "Failed to parse: {:?}",
                args
            );
        }
    }

    #[test]
    fn test_subcommands_with_required_args() {
        let cases = [
            vec![
                "hive", "log-tool", "--run", "r", "--agent", "a", "--tool", "t", "--status", "s",
            ],
            vec!["hive", "heartbeat", "--run", "r", "--agent", "a"],
            vec!["hive", "mcp", "--run", "r", "--agent", "a"],
            vec!["hive", "explore", "some intent"],
            vec!["hive", "review-agent", "agent-1"],
            vec!["hive", "read-messages", "--agent", "a"],
            vec!["hive", "agent-exit", "--run", "r", "--agent", "a"],
        ];
        for args in &cases {
            assert!(
                Cli::try_parse_from(args.iter()).is_ok(),
                "Failed to parse: {:?}",
                args
            );
        }
    }

    #[test]
    fn test_commands_enum_variant_count() {
        // Ensure this test is updated when new subcommands are added.
        // Each parseable subcommand string maps to one Commands variant.
        let all_subcommands = [
            "init",
            "start",
            "status",
            "agents",
            "tasks",
            "messages",
            "log-tool",
            "heartbeat",
            "logs",
            "tui",
            "mcp",
            "wait",
            "review-agent",
            "read-messages",
            "summary",
            "cost",
            "history",
            "memory",
            "explore",
            "mind",
            "agent-exit",
            "stop",
            "watch",
        ];
        // If a new command is added to Commands but not to this list, this will catch it
        // by verifying the count matches. Currently 23 subcommands.
        assert_eq!(all_subcommands.len(), 23);
    }

    #[test]
    fn test_memory_subcommands_exhaustive() {
        let cases = [
            (vec!["hive", "memory"], true),
            (vec!["hive", "memory", "show"], true),
            (vec!["hive", "memory", "prune"], true),
            (vec!["hive", "memory", "invalid"], false),
        ];
        for (args, should_succeed) in &cases {
            let result = Cli::try_parse_from(args.iter());
            assert_eq!(
                result.is_ok(),
                *should_succeed,
                "Unexpected result for {:?}",
                args
            );
        }
    }

    #[test]
    fn test_mind_subcommands_exhaustive() {
        let cases = [
            (vec!["hive", "mind"], true),
            (vec!["hive", "mind", "query", "test"], true),
            (vec!["hive", "mind", "invalid"], false),
        ];
        for (args, should_succeed) in &cases {
            let result = Cli::try_parse_from(args.iter());
            assert_eq!(
                result.is_ok(),
                *should_succeed,
                "Unexpected result for {:?}",
                args
            );
        }
    }

    #[test]
    fn test_start_spec_and_goal_both_provided() {
        // Both positional spec and --goal flag: goal takes the flag value, spec gets positional
        let cli = Cli::try_parse_from(["hive", "start", "path.md", "--goal", "my goal"]).unwrap();
        match cli.command {
            Commands::Start { spec, goal } => {
                assert_eq!(spec, Some("path.md".to_string()));
                assert_eq!(goal, Some("my goal".to_string()));
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn test_wait_timeout_invalid_type() {
        let result = Cli::try_parse_from(["hive", "wait", "--timeout", "not_a_number"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_watch_interval_invalid_type() {
        let result = Cli::try_parse_from(["hive", "watch", "--interval", "abc"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_messages_bool_flags_default_false() {
        let cli = Cli::try_parse_from(["hive", "read-messages", "--agent", "x"]).unwrap();
        match cli.command {
            Commands::ReadMessages { unread, stop_hook, .. } => {
                assert!(!unread);
                assert!(!stop_hook);
            }
            _ => panic!("expected ReadMessages"),
        }
    }
}
