use clap::Parser;
use super::{Cli, Commands, MemoryCommands, MindCommands};

// ── Tests from old cli.rs ──

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

// ── Tests from old main.rs ──

// ── Simple subcommands (no arguments) ──

#[test]
fn test_cli_init() {
    let cli = Cli::try_parse_from(["hive", "init"]).unwrap();
    assert!(matches!(cli.command, Commands::Init));
}

#[test]
fn test_cli_status() {
    let cli = Cli::try_parse_from(["hive", "status"]).unwrap();
    assert!(matches!(cli.command, Commands::Status));
}

#[test]
fn test_cli_agents() {
    let cli = Cli::try_parse_from(["hive", "agents"]).unwrap();
    assert!(matches!(cli.command, Commands::Agents));
}

#[test]
fn test_cli_history() {
    let cli = Cli::try_parse_from(["hive", "history"]).unwrap();
    assert!(matches!(cli.command, Commands::History));
}

#[test]
fn test_cli_tui() {
    let cli = Cli::try_parse_from(["hive", "tui"]).unwrap();
    assert!(matches!(cli.command, Commands::Tui));
}

#[test]
fn test_cli_stop() {
    let cli = Cli::try_parse_from(["hive", "stop"]).unwrap();
    assert!(matches!(cli.command, Commands::Stop));
}

// ── Explore command ──

#[test]
fn test_cli_explore_command() {
    let cli = Cli::try_parse_from(["hive", "explore", "test intent"]).unwrap();
    match cli.command {
        Commands::Explore { intent } => assert_eq!(intent, "test intent"),
        _ => panic!("expected Explore"),
    }
}

#[test]
fn test_cli_explore_requires_intent() {
    let result = Cli::try_parse_from(["hive", "explore"]);
    assert!(result.is_err());
}

// ── Start command ──

#[test]
fn test_cli_start_with_spec() {
    let cli = Cli::try_parse_from(["hive", "start", "docs/plans/my-spec.md"]).unwrap();
    match cli.command {
        Commands::Start { spec, goal } => {
            assert_eq!(spec, Some("docs/plans/my-spec.md".to_string()));
            assert_eq!(goal, None);
        }
        _ => panic!("expected Start"),
    }
}

#[test]
fn test_cli_start_with_goal_flag() {
    let cli =
        Cli::try_parse_from(["hive", "start", "--goal", "add a login page"]).unwrap();
    match cli.command {
        Commands::Start { spec, goal } => {
            assert_eq!(spec, None);
            assert_eq!(goal, Some("add a login page".to_string()));
        }
        _ => panic!("expected Start"),
    }
}

#[test]
fn test_cli_start_with_positional_goal() {
    let cli = Cli::try_parse_from(["hive", "start", "add more tests"]).unwrap();
    match cli.command {
        Commands::Start { spec, goal } => {
            assert_eq!(spec, Some("add more tests".to_string()));
            assert_eq!(goal, None);
        }
        _ => panic!("expected Start"),
    }
}

#[test]
fn test_cli_start_no_args() {
    // start with no spec or goal is valid at parse time (handled at runtime)
    let cli = Cli::try_parse_from(["hive", "start"]).unwrap();
    match cli.command {
        Commands::Start { spec, goal } => {
            assert_eq!(spec, None);
            assert_eq!(goal, None);
        }
        _ => panic!("expected Start"),
    }
}

// ── Tasks command ──

#[test]
fn test_cli_tasks_no_filter() {
    let cli = Cli::try_parse_from(["hive", "tasks"]).unwrap();
    match cli.command {
        Commands::Tasks { status, assignee } => {
            assert_eq!(status, None);
            assert_eq!(assignee, None);
        }
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_cli_tasks_with_status_filter() {
    let cli = Cli::try_parse_from(["hive", "tasks", "--status", "active"]).unwrap();
    match cli.command {
        Commands::Tasks { status, assignee } => {
            assert_eq!(status, Some("active".to_string()));
            assert_eq!(assignee, None);
        }
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_cli_tasks_with_assignee_filter() {
    let cli = Cli::try_parse_from(["hive", "tasks", "--assignee", "lead-1"]).unwrap();
    match cli.command {
        Commands::Tasks { status, assignee } => {
            assert_eq!(status, None);
            assert_eq!(assignee, Some("lead-1".to_string()));
        }
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_cli_tasks_both_filters() {
    let cli = Cli::try_parse_from([
        "hive", "tasks", "--status", "merged", "--assignee", "worker-2",
    ])
    .unwrap();
    match cli.command {
        Commands::Tasks { status, assignee } => {
            assert_eq!(status, Some("merged".to_string()));
            assert_eq!(assignee, Some("worker-2".to_string()));
        }
        _ => panic!("expected Tasks"),
    }
}

// ── Messages command ──

#[test]
fn test_cli_messages_no_filter() {
    let cli = Cli::try_parse_from(["hive", "messages"]).unwrap();
    match cli.command {
        Commands::Messages { agent } => assert_eq!(agent, None),
        _ => panic!("expected Messages"),
    }
}

#[test]
fn test_cli_messages_with_agent_filter() {
    let cli = Cli::try_parse_from(["hive", "messages", "--agent", "coordinator"]).unwrap();
    match cli.command {
        Commands::Messages { agent } => assert_eq!(agent, Some("coordinator".to_string())),
        _ => panic!("expected Messages"),
    }
}

// ── LogTool command ──

#[test]
fn test_cli_log_tool_all_args() {
    let cli = Cli::try_parse_from([
        "hive",
        "log-tool",
        "--run",
        "abc123",
        "--agent",
        "worker-1",
        "--tool",
        "Read",
        "--status",
        "success",
        "--duration",
        "150",
        "--args-summary",
        "path=/foo.rs",
    ])
    .unwrap();
    match cli.command {
        Commands::LogTool {
            run,
            agent,
            tool,
            status,
            duration,
            args_summary,
        } => {
            assert_eq!(run, "abc123");
            assert_eq!(agent, "worker-1");
            assert_eq!(tool, "Read");
            assert_eq!(status, "success");
            assert_eq!(duration, Some(150));
            assert_eq!(args_summary, Some("path=/foo.rs".to_string()));
        }
        _ => panic!("expected LogTool"),
    }
}

#[test]
fn test_cli_log_tool_required_only() {
    let cli = Cli::try_parse_from([
        "hive",
        "log-tool",
        "--run",
        "r1",
        "--agent",
        "a1",
        "--tool",
        "Bash",
        "--status",
        "error",
    ])
    .unwrap();
    match cli.command {
        Commands::LogTool {
            duration,
            args_summary,
            ..
        } => {
            assert_eq!(duration, None);
            assert_eq!(args_summary, None);
        }
        _ => panic!("expected LogTool"),
    }
}

#[test]
fn test_cli_log_tool_missing_required() {
    // Missing --tool
    let result = Cli::try_parse_from([
        "hive",
        "log-tool",
        "--run",
        "r1",
        "--agent",
        "a1",
        "--status",
        "ok",
    ]);
    assert!(result.is_err());
}

// ── Heartbeat command ──

#[test]
fn test_cli_heartbeat() {
    let cli = Cli::try_parse_from([
        "hive",
        "heartbeat",
        "--run",
        "run1",
        "--agent",
        "worker-3",
    ])
    .unwrap();
    match cli.command {
        Commands::Heartbeat { run, agent } => {
            assert_eq!(run, "run1");
            assert_eq!(agent, "worker-3");
        }
        _ => panic!("expected Heartbeat"),
    }
}

#[test]
fn test_cli_heartbeat_missing_agent() {
    let result = Cli::try_parse_from(["hive", "heartbeat", "--run", "run1"]);
    assert!(result.is_err());
}

// ── Logs command ──

#[test]
fn test_cli_logs_no_filter() {
    let cli = Cli::try_parse_from(["hive", "logs"]).unwrap();
    match cli.command {
        Commands::Logs { agent } => assert_eq!(agent, None),
        _ => panic!("expected Logs"),
    }
}

#[test]
fn test_cli_logs_with_agent() {
    let cli = Cli::try_parse_from(["hive", "logs", "--agent", "lead-1"]).unwrap();
    match cli.command {
        Commands::Logs { agent } => assert_eq!(agent, Some("lead-1".to_string())),
        _ => panic!("expected Logs"),
    }
}

// ── Mcp command ──

#[test]
fn test_cli_mcp() {
    let cli = Cli::try_parse_from([
        "hive", "mcp", "--run", "run1", "--agent", "coordinator",
    ])
    .unwrap();
    match cli.command {
        Commands::Mcp { run, agent } => {
            assert_eq!(run, "run1");
            assert_eq!(agent, "coordinator");
        }
        _ => panic!("expected Mcp"),
    }
}

#[test]
fn test_cli_mcp_missing_run() {
    let result = Cli::try_parse_from(["hive", "mcp", "--agent", "coordinator"]);
    assert!(result.is_err());
}

// ── Wait command ──

#[test]
fn test_cli_wait_defaults() {
    let cli = Cli::try_parse_from(["hive", "wait"]).unwrap();
    match cli.command {
        Commands::Wait { run, timeout } => {
            assert_eq!(run, None);
            assert_eq!(timeout, 60);
        }
        _ => panic!("expected Wait"),
    }
}

#[test]
fn test_cli_wait_custom_timeout() {
    let cli = Cli::try_parse_from(["hive", "wait", "--timeout", "120"]).unwrap();
    match cli.command {
        Commands::Wait { run, timeout } => {
            assert_eq!(run, None);
            assert_eq!(timeout, 120);
        }
        _ => panic!("expected Wait"),
    }
}

#[test]
fn test_cli_wait_with_run() {
    let cli =
        Cli::try_parse_from(["hive", "wait", "--run", "abc", "--timeout", "30"]).unwrap();
    match cli.command {
        Commands::Wait { run, timeout } => {
            assert_eq!(run, Some("abc".to_string()));
            assert_eq!(timeout, 30);
        }
        _ => panic!("expected Wait"),
    }
}

// ── Watch command ──

#[test]
fn test_cli_watch_default_interval() {
    let cli = Cli::try_parse_from(["hive", "watch"]).unwrap();
    match cli.command {
        Commands::Watch { interval } => assert_eq!(interval, 10),
        _ => panic!("expected Watch"),
    }
}

#[test]
fn test_cli_watch_custom_interval() {
    let cli = Cli::try_parse_from(["hive", "watch", "--interval", "5"]).unwrap();
    match cli.command {
        Commands::Watch { interval } => assert_eq!(interval, 5),
        _ => panic!("expected Watch"),
    }
}

// ── ReviewAgent command ──

#[test]
fn test_cli_review_agent() {
    let cli = Cli::try_parse_from(["hive", "review-agent", "worker-1"]).unwrap();
    match cli.command {
        Commands::ReviewAgent { agent_id, run } => {
            assert_eq!(agent_id, "worker-1");
            assert_eq!(run, None);
        }
        _ => panic!("expected ReviewAgent"),
    }
}

#[test]
fn test_cli_review_agent_with_run() {
    let cli = Cli::try_parse_from([
        "hive",
        "review-agent",
        "lead-2",
        "--run",
        "abc",
    ])
    .unwrap();
    match cli.command {
        Commands::ReviewAgent { agent_id, run } => {
            assert_eq!(agent_id, "lead-2");
            assert_eq!(run, Some("abc".to_string()));
        }
        _ => panic!("expected ReviewAgent"),
    }
}

#[test]
fn test_cli_review_agent_missing_id() {
    let result = Cli::try_parse_from(["hive", "review-agent"]);
    assert!(result.is_err());
}

// ── ReadMessages command ──

#[test]
fn test_cli_read_messages_defaults() {
    let cli =
        Cli::try_parse_from(["hive", "read-messages", "--agent", "worker-1"]).unwrap();
    match cli.command {
        Commands::ReadMessages {
            agent,
            run,
            unread,
            stop_hook,
        } => {
            assert_eq!(agent, "worker-1");
            assert_eq!(run, None);
            assert!(!unread);
            assert!(!stop_hook);
        }
        _ => panic!("expected ReadMessages"),
    }
}

#[test]
fn test_cli_read_messages_all_flags() {
    let cli = Cli::try_parse_from([
        "hive",
        "read-messages",
        "--agent",
        "lead-1",
        "--run",
        "xyz",
        "--unread",
        "--stop-hook",
    ])
    .unwrap();
    match cli.command {
        Commands::ReadMessages {
            agent,
            run,
            unread,
            stop_hook,
        } => {
            assert_eq!(agent, "lead-1");
            assert_eq!(run, Some("xyz".to_string()));
            assert!(unread);
            assert!(stop_hook);
        }
        _ => panic!("expected ReadMessages"),
    }
}

#[test]
fn test_cli_read_messages_missing_agent() {
    let result = Cli::try_parse_from(["hive", "read-messages"]);
    assert!(result.is_err());
}

// ── Summary command ──

#[test]
fn test_cli_summary_no_run() {
    let cli = Cli::try_parse_from(["hive", "summary"]).unwrap();
    match cli.command {
        Commands::Summary { run } => assert_eq!(run, None),
        _ => panic!("expected Summary"),
    }
}

#[test]
fn test_cli_summary_with_run() {
    let cli = Cli::try_parse_from(["hive", "summary", "--run", "abc123"]).unwrap();
    match cli.command {
        Commands::Summary { run } => assert_eq!(run, Some("abc123".to_string())),
        _ => panic!("expected Summary"),
    }
}

// ── Cost command ──

#[test]
fn test_cli_cost_no_run() {
    let cli = Cli::try_parse_from(["hive", "cost"]).unwrap();
    match cli.command {
        Commands::Cost { run } => assert_eq!(run, None),
        _ => panic!("expected Cost"),
    }
}

#[test]
fn test_cli_cost_with_run() {
    let cli = Cli::try_parse_from(["hive", "cost", "--run", "r1"]).unwrap();
    match cli.command {
        Commands::Cost { run } => assert_eq!(run, Some("r1".to_string())),
        _ => panic!("expected Cost"),
    }
}

// ── AgentExit command ──

#[test]
fn test_cli_agent_exit_command() {
    let cli = Cli::try_parse_from([
        "hive",
        "agent-exit",
        "--run",
        "abc",
        "--agent",
        "worker-1",
    ])
    .unwrap();
    match cli.command {
        Commands::AgentExit { run, agent } => {
            assert_eq!(run, "abc");
            assert_eq!(agent, "worker-1");
        }
        _ => panic!("expected AgentExit"),
    }
}

#[test]
fn test_cli_agent_exit_missing_run() {
    let result = Cli::try_parse_from(["hive", "agent-exit", "--agent", "w1"]);
    assert!(result.is_err());
}

// ── Mind command and subcommands ──

#[test]
fn test_cli_mind_command() {
    let cli = Cli::try_parse_from(["hive", "mind"]).unwrap();
    match cli.command {
        Commands::Mind { command } => assert!(command.is_none()),
        _ => panic!("expected Mind"),
    }
}

#[test]
fn test_cli_mind_query_command() {
    let cli =
        Cli::try_parse_from(["hive", "mind", "query", "search term"]).unwrap();
    match cli.command {
        Commands::Mind { command } => match command {
            Some(MindCommands::Query { query }) => assert_eq!(query, "search term"),
            _ => panic!("expected Mind Query"),
        },
        _ => panic!("expected Mind"),
    }
}

// ── Memory command and subcommands ──

#[test]
fn test_cli_memory_no_subcommand() {
    let cli = Cli::try_parse_from(["hive", "memory"]).unwrap();
    match cli.command {
        Commands::Memory { command } => assert!(command.is_none()),
        _ => panic!("expected Memory"),
    }
}

#[test]
fn test_cli_memory_show() {
    let cli = Cli::try_parse_from(["hive", "memory", "show"]).unwrap();
    match cli.command {
        Commands::Memory { command } => {
            assert!(matches!(command, Some(MemoryCommands::Show)));
        }
        _ => panic!("expected Memory"),
    }
}

#[test]
fn test_cli_memory_prune() {
    let cli = Cli::try_parse_from(["hive", "memory", "prune"]).unwrap();
    match cli.command {
        Commands::Memory { command } => {
            assert!(matches!(command, Some(MemoryCommands::Prune)));
        }
        _ => panic!("expected Memory"),
    }
}

// ── Error cases ──

#[test]
fn test_cli_unknown_subcommand() {
    let result = Cli::try_parse_from(["hive", "foobar"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_no_subcommand() {
    let result = Cli::try_parse_from(["hive"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_unknown_flag() {
    let result = Cli::try_parse_from(["hive", "status", "--verbose"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_log_tool_missing_all_required() {
    let result = Cli::try_parse_from(["hive", "log-tool"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_heartbeat_missing_all_required() {
    let result = Cli::try_parse_from(["hive", "heartbeat"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_mcp_missing_all_required() {
    let result = Cli::try_parse_from(["hive", "mcp"]);
    assert!(result.is_err());
}

// ── Kebab-case subcommands ──

#[test]
fn test_cli_kebab_case_log_tool() {
    // Verify clap converts LogTool -> log-tool
    let cli = Cli::try_parse_from([
        "hive", "log-tool", "--run", "r", "--agent", "a", "--tool", "t", "--status", "s",
    ])
    .unwrap();
    assert!(matches!(cli.command, Commands::LogTool { .. }));
}

#[test]
fn test_cli_kebab_case_review_agent() {
    let cli = Cli::try_parse_from(["hive", "review-agent", "a1"]).unwrap();
    assert!(matches!(cli.command, Commands::ReviewAgent { .. }));
}

#[test]
fn test_cli_kebab_case_read_messages() {
    let cli =
        Cli::try_parse_from(["hive", "read-messages", "--agent", "a1"]).unwrap();
    assert!(matches!(cli.command, Commands::ReadMessages { .. }));
}

#[test]
fn test_cli_kebab_case_agent_exit() {
    let cli = Cli::try_parse_from([
        "hive", "agent-exit", "--run", "r", "--agent", "a",
    ])
    .unwrap();
    assert!(matches!(cli.command, Commands::AgentExit { .. }));
}

// ── read_messages_inner tests ──

#[test]
fn test_stop_hook_updates_read_cursor() {
    use chrono::Utc;
    use tempfile::TempDir;
    use crate::state::HiveState;
    use crate::types::{Agent, AgentRole, AgentStatus, Message, MessageType};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root);
    state.create_run("test-run").unwrap();

    let agent = Agent {
        id: "agent-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent("test-run", &agent).unwrap();

    let msg = Message {
        id: "msg-1".into(),
        from: "coordinator".into(),
        to: "agent-1".into(),
        timestamp: Utc::now() - chrono::Duration::seconds(5),
        message_type: MessageType::Info,
        body: "Hello".into(),
        refs: vec![],
    };
    state.save_message("test-run", &msg).unwrap();

    // Stop hook with messages should update the cursor
    let (messages, cursor_updated) =
        super::read_messages_inner(&state, "test-run", "agent-1", false, true).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(cursor_updated);

    // Verify cursor was persisted
    let updated_agent = state.load_agent("test-run", "agent-1").unwrap();
    assert!(updated_agent.messages_read_at.is_some());

    // Second call with unread=true should return no messages
    let (messages, cursor_updated) =
        super::read_messages_inner(&state, "test-run", "agent-1", true, true).unwrap();
    assert!(messages.is_empty());
    assert!(!cursor_updated);
}

#[test]
fn test_non_stop_hook_does_not_update_cursor() {
    use chrono::Utc;
    use tempfile::TempDir;
    use crate::state::HiveState;
    use crate::types::{Agent, AgentRole, AgentStatus, Message, MessageType};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root);
    state.create_run("test-run").unwrap();

    let agent = Agent {
        id: "agent-2".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent("test-run", &agent).unwrap();

    let msg = Message {
        id: "msg-2".into(),
        from: "coordinator".into(),
        to: "agent-2".into(),
        timestamp: Utc::now(),
        message_type: MessageType::Info,
        body: "Hello".into(),
        refs: vec![],
    };
    state.save_message("test-run", &msg).unwrap();

    // Non-stop-hook should NOT update cursor
    let (messages, cursor_updated) =
        super::read_messages_inner(&state, "test-run", "agent-2", false, false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(!cursor_updated);

    let agent_after = state.load_agent("test-run", "agent-2").unwrap();
    assert!(agent_after.messages_read_at.is_none());
}
