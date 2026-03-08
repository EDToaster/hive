mod agent;
mod cli;
#[allow(dead_code)]
mod git;
mod logging;
mod mcp;
#[allow(dead_code)]
mod state;
mod tui;
mod types;
mod wait;

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
        Commands::Heartbeat { run, agent } => cmd_heartbeat(&run, &agent),
        Commands::Logs { agent } => cmd_logs(agent),
        Commands::Tui => cmd_tui(),
        Commands::Mcp { run, agent } => cmd_mcp(&run, &agent),
        Commands::Wait { run, timeout } => cmd_wait(run, timeout),
        Commands::ReviewAgent { agent_id, run } => cmd_review_agent(&agent_id, run),
        Commands::ReadMessages {
            agent,
            run,
            unread,
            stop_hook,
        } => cmd_read_messages(&agent, run, unread, stop_hook),
        Commands::Stop => cmd_stop(),
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

    // Write coordinator CLAUDE.local.md to the base repo
    let codebase_summary = crate::agent::AgentSpawner::generate_codebase_summary(state.repo_root());
    let coordinator_prompt = crate::agent::AgentSpawner::coordinator_prompt(&run_id, &spec_content, &codebase_summary);
    let repo_root = state.repo_root();
    fs::write(repo_root.join("CLAUDE.local.md"), &coordinator_prompt).map_err(|e| e.to_string())?;

    // Write .claude/settings.local.json for coordinator hooks
    let claude_dir = repo_root.join(".claude");
    fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
    let settings_json = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent coordinator --tool {{}} --status success"
                    )
                }]
            }]
        }
    });
    fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&settings_json).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Write .mcp.json for coordinator MCP
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "hive": {
                "command": "hive",
                "args": ["mcp", "--run", &run_id, "--agent", "coordinator"]
            }
        }
    });
    fs::write(
        repo_root.join(".mcp.json"),
        serde_json::to_string_pretty(&mcp_config).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Register coordinator agent (no PID — user launches claude manually)
    let coordinator = crate::types::Agent {
        id: "coordinator".to_string(),
        role: crate::types::AgentRole::Coordinator,
        status: crate::types::AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: Some(chrono::Utc::now()),
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent(&run_id, &coordinator)?;

    println!("Coordinator configured. Launch Claude Code in this directory to begin.");
    println!("Run 'hive tui' in another terminal to monitor progress.");

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
        println!("  {} ({:?}) - {:?}", agent.id, agent.role, agent.status);
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
                let status_str = serde_json::to_value(t.status)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                if status_str.as_deref() != Some(s.as_str()) {
                    return false;
                }
            }
            if let Some(ref a) = assignee_filter
                && t.assigned_to.as_deref() != Some(a.as_str())
            {
                return false;
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

fn cmd_heartbeat(run_id: &str, agent_id: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    state.update_agent_heartbeat(run_id, agent_id)
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

    println!(
        "{:<20} {:<25} {:>8} {:>10}",
        "Agent", "Tool", "Count", "Avg ms"
    );
    println!("{}", "-".repeat(65));
    for (agent_id, tool_name, count, avg_ms) in &filtered {
        println!(
            "{:<20} {:<25} {:>8} {:>10}",
            agent_id, tool_name, count, avg_ms
        );
    }
    Ok(())
}

fn cmd_tui() -> Result<(), String> {
    crate::tui::run_tui()
}

fn cmd_mcp(run_id: &str, agent_id: &str) -> Result<(), String> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(crate::mcp::run_mcp_server(run_id, agent_id))
}

fn cmd_wait(run: Option<String>, timeout: u64) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = rt.block_on(crate::wait::wait_for_activity(
        state.repo_root(),
        &run_id,
        timeout,
    ))?;
    println!("{result}");
    Ok(())
}

fn cmd_review_agent(agent_id: &str, run: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };
    let agent = state.load_agent(&run_id, agent_id)?;

    if agent.status == types::AgentStatus::Running {
        return Err("Cannot review a running agent.".into());
    }

    let worktree = agent.worktree.as_deref().ok_or("Agent has no worktree.")?;
    let wt_path = std::path::Path::new(worktree);
    if !wt_path.exists() {
        return Err("Agent worktree no longer exists.".into());
    }

    // Auto-commit any uncommitted work
    let status = crate::git::Git::status_porcelain(wt_path).unwrap_or_default();
    if !status.is_empty() {
        let _ = crate::git::Git::add_all(wt_path);
        let _ = crate::git::Git::commit(wt_path, "wip: salvaged by coordinator");
        println!("Auto-committed uncommitted changes.");
    }

    let branch = format!("hive/{run_id}/{agent_id}");
    let commits = crate::git::Git::log_oneline_since(wt_path, "main")
        .unwrap_or_else(|_| "(no commits)".to_string());
    let diff_stat = crate::git::Git::diff_stat_since(wt_path, "main")
        .unwrap_or_else(|_| "(no diff)".to_string());

    println!("Agent:  {}", agent.id);
    println!("Role:   {:?}", agent.role);
    println!("Status: {:?}", agent.status);
    println!("Branch: {branch}");
    if let Some(tid) = &agent.task_id {
        println!("Task:   {tid}");
    }
    println!("\n--- Commits ---\n{commits}");
    println!("\n--- Diff Stat ---\n{diff_stat}");
    Ok(())
}

fn cmd_read_messages(
    agent_id: &str,
    run: Option<String>,
    unread: bool,
    stop_hook: bool,
) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };

    let agent = state.load_agent(&run_id, agent_id)?;

    let since = if unread {
        // Use max(messages_read_at, last_completed_at) as the "unread" cursor
        match (agent.messages_read_at, agent.last_completed_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    } else {
        None
    };

    let messages = state.load_messages_for_agent(&run_id, agent_id, since)?;

    if stop_hook {
        if messages.is_empty() {
            Ok(())
        } else {
            let json = serde_json::to_string_pretty(&messages).map_err(|e| e.to_string())?;
            eprintln!("Unread messages for agent {}:\n{}", agent_id, json);
            std::process::exit(2);
        }
    } else {
        let json = serde_json::to_string_pretty(&messages).map_err(|e| e.to_string())?;
        println!("{}", json);
        Ok(())
    }
}

fn cmd_stop() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;

    // Kill agent processes and update status
    for agent in &agents {
        let mut updated = agent.clone();
        if let Some(pid) = agent.pid
            && crate::agent::AgentSpawner::is_alive(pid)
        {
            // SAFETY: We are sending SIGTERM to a process we own (a spawned agent).
            // The pid was obtained from our own agent records and verified alive above.
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            println!("Stopped agent {} (pid {})", agent.id, pid);
        }
        updated.status = types::AgentStatus::Done;
        let _ = state.save_agent(&run_id, &updated);
    }

    // Remove worktrees
    for agent in &agents {
        if let Some(ref wt) = agent.worktree {
            let wt_path = std::path::Path::new(wt);
            if wt_path.exists() {
                match crate::git::Git::worktree_remove(state.repo_root(), wt_path) {
                    Ok(()) => println!("Removed worktree for {}", agent.id),
                    Err(e) => {
                        eprintln!("Warning: failed to remove worktree for {}: {e}", agent.id)
                    }
                }
            }
        }
    }

    crate::git::Git::worktree_prune(state.repo_root()).ok();

    // Clean up coordinator files
    let repo_root = state.repo_root();
    let _ = std::fs::remove_file(repo_root.join("CLAUDE.local.md"));
    let _ = std::fs::remove_file(repo_root.join(".mcp.json"));
    let _ = std::fs::remove_file(repo_root.join(".claude/settings.local.json"));

    println!("Run {run_id} stopped.");
    Ok(())
}
