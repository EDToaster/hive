mod agent;
mod cli;
#[allow(dead_code)]
mod git;
mod logging;
mod mcp;
mod output;
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
        Commands::Start { spec, goal } => cmd_start(spec, goal),
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
            args_summary,
        } => cmd_log_tool(&run, &agent, &tool, &status, duration, args_summary.as_deref()),
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
        Commands::Summary { run } => cmd_summary(run),
        Commands::Cost { run } => cmd_cost(run),
        Commands::History => cmd_history(),
        Commands::Memory { command } => cmd_memory(command),
        Commands::Explore { intent } => cmd_explore(&intent),
        Commands::Mind { command } => cmd_mind(command),
        Commands::AgentExit { run, agent } => cmd_agent_exit(&run, &agent),
        Commands::Stop => cmd_stop(),
        Commands::Watch { interval } => cmd_watch(interval),
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
    let config = "# Hive configuration\n\
        # How long (seconds) before an agent with no heartbeat is considered stalled\n\
        stall_timeout_seconds: 300\n\
        \n\
        # Maximum retry attempts for failed agents\n\
        max_retries: 2\n\
        \n\
        # Command to run for verification after merges (uncomment to enable)\n\
        # verify_command: \"cargo test --all-targets && cargo clippy --all-targets -- -D warnings\"\n\
        \n\
        # Maximum budget in USD for a single run (uncomment to enable)\n\
        # budget_usd: 50.0\n";
    fs::write(hive_dir.join("config.yaml"), config).map_err(|e| e.to_string())?;

    println!("Initialized .hive/ in {}", cwd.display());
    Ok(())
}

fn cmd_start(spec: Option<String>, goal: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;

    // Determine whether this is a file-path or a goal-string start
    enum StartMode {
        SpecFile(String),
        Goal(String),
    }

    let mode = if let Some(goal_str) = goal {
        StartMode::Goal(goal_str)
    } else if let Some(spec_str) = spec {
        if spec_str.contains('/') || spec_str.ends_with(".md") {
            StartMode::SpecFile(spec_str)
        } else {
            StartMode::Goal(spec_str)
        }
    } else {
        return Err("Provide a spec file path or a goal string".into());
    };

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    let spec_content = match mode {
        StartMode::SpecFile(spec_path) => {
            let content = fs::read_to_string(&spec_path)
                .map_err(|e| format!("Cannot read spec file '{spec_path}': {e}"))?;
            state.save_spec(&run_id, &content)?;
            println!("Created run: {run_id}");
            content
        }
        StartMode::Goal(goal_str) => {
            println!("Created run: {run_id}");
            println!("Planning phase: analyzing codebase and generating spec...");

            let memory = state.load_memory_for_prompt(&crate::types::AgentRole::Planner);
            let planner_agent = crate::agent::AgentSpawner::spawn(
                &state,
                &run_id,
                "planner",
                crate::types::AgentRole::Planner,
                None,
                &goal_str,
            )?;

            // Wait up to 5 minutes for planner to finish
            let timeout = std::time::Duration::from_secs(300);
            let start = std::time::Instant::now();
            let _ = memory; // memory is injected by spawn via generate_prompt
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Some(pid) = planner_agent.pid {
                    if !crate::agent::AgentSpawner::is_alive(pid) {
                        break;
                    }
                } else {
                    break;
                }
                if start.elapsed() > timeout {
                    return Err("Planner agent timed out after 5 minutes".into());
                }
            }

            // Check if planner produced a spec
            match state.load_planner_spec(&run_id) {
                Some(content) => {
                    println!("Spec generated successfully.");
                    content
                }
                None => return Err("Planner agent did not produce a spec".into()),
            }
        }
    };

    // Write coordinator CLAUDE.local.md to the base repo
    let codebase_summary = crate::agent::AgentSpawner::generate_codebase_summary(state.repo_root());
    let memory = state.load_memory_for_prompt(&crate::types::AgentRole::Coordinator);
    let coordinator_prompt = crate::agent::AgentSpawner::coordinator_prompt(
        &run_id,
        &spec_content,
        &codebase_summary,
        &memory,
    );
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
                        "INPUT=$(cat); TOOL=$(printf '%s' \"$INPUT\" | jq -r '.tool_name'); ARGS=$(printf '%s' \"$INPUT\" | jq -r '(.tool_input // {{}} | to_entries | map(.key + \"=\" + (.value | tostring | .[0:80])) | join(\", \"))'); hive log-tool --run {run_id} --agent coordinator --tool \"$TOOL\" --status success --args-summary \"$ARGS\""
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

    // Write .mcp.json for coordinator MCP (merge with existing)
    crate::agent::AgentSpawner::write_mcp_config(
        &repo_root.join(".mcp.json"),
        &run_id,
        "coordinator",
    )?;

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
    let meta = state.load_run_metadata(&run_id)?;

    // ANSI color codes
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const RED: &str = "\x1b[31m";
    const CYAN: &str = "\x1b[36m";
    const BOLD: &str = "\x1b[1m";
    const RESET: &str = "\x1b[0m";

    // Duration
    let elapsed = chrono::Utc::now() - meta.created_at;
    let total_seconds = elapsed.num_seconds().max(0);
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;
    let status_str = format!("{:?}", meta.status).to_lowercase();

    println!("{BOLD}Run:{RESET} {CYAN}{run_id}{RESET} ({status_str}, {minutes}m {secs}s)");

    // Agent counts by status
    let agent_count = |s: types::AgentStatus| agents.iter().filter(|a| a.status == s).count();
    let mut agent_parts: Vec<String> = Vec::new();
    for (status, label, color) in [
        (types::AgentStatus::Running, "running", GREEN),
        (types::AgentStatus::Idle, "idle", YELLOW),
        (types::AgentStatus::Done, "done", RESET),
        (types::AgentStatus::Failed, "failed", RED),
        (types::AgentStatus::Stalled, "stalled", RED),
    ] {
        let count = agent_count(status);
        if count > 0 {
            agent_parts.push(format!("{color}{count} {label}{RESET}"));
        }
    }
    println!(
        "{BOLD}Agents:{RESET} {}",
        if agent_parts.is_empty() {
            "0".to_string()
        } else {
            agent_parts.join(" · ")
        }
    );

    // Task counts by status
    let task_count = |s: types::TaskStatus| tasks.iter().filter(|t| t.status == s).count();
    let mut task_parts: Vec<String> = Vec::new();
    for (status, label, color) in [
        (types::TaskStatus::Active, "active", GREEN),
        (types::TaskStatus::Pending, "pending", YELLOW),
        (types::TaskStatus::Blocked, "blocked", YELLOW),
        (types::TaskStatus::Review, "review", YELLOW),
        (types::TaskStatus::Approved, "approved", GREEN),
        (types::TaskStatus::Queued, "queued", YELLOW),
        (types::TaskStatus::Merged, "merged", RESET),
        (types::TaskStatus::Absorbed, "absorbed", RESET),
        (types::TaskStatus::Failed, "failed", RED),
        (types::TaskStatus::Cancelled, "cancelled", RESET),
    ] {
        let count = task_count(status);
        if count > 0 {
            task_parts.push(format!("{color}{count} {label}{RESET}"));
        }
    }
    println!(
        "{BOLD}Tasks:{RESET}  {}",
        if task_parts.is_empty() {
            "0".to_string()
        } else {
            task_parts.join(" · ")
        }
    );

    // Merge queue
    let queue_len = queue.entries.len();
    println!(
        "{BOLD}Queue:{RESET}  {queue_len} {}",
        if queue_len == 1 { "entry" } else { "entries" }
    );

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
    args_summary: Option<&str>,
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
        args_summary,
        status,
        duration,
    )?;
    Ok(())
}

fn cmd_heartbeat(run_id: &str, agent_id: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    state.update_agent_heartbeat(run_id, agent_id)
}

fn cmd_agent_exit(run_id: &str, agent_id: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    let mut agent = state.load_agent(run_id, agent_id)?;

    // Only transition if currently Running — otherwise silently succeed (idempotent)
    if agent.status != types::AgentStatus::Running {
        return Ok(());
    }

    // Auto-commit any uncommitted work
    if let Some(ref wt) = agent.worktree {
        let wt_path = std::path::Path::new(wt);
        if wt_path.exists()
            && let Ok(status) = git::Git::status_porcelain(wt_path)
            && !status.is_empty()
        {
            let _ = git::Git::add_all(wt_path);
            let _ = git::Git::commit(wt_path, "wip: auto-commit on agent exit");
        }
    }

    // Parse session_id from output
    let output_path = state.agents_dir(run_id).join(agent_id).join("output.jsonl");
    agent.session_id = output::parse_session_id_from_output(&output_path);

    // Transition to Idle
    agent.status = types::AgentStatus::Idle;
    agent.last_completed_at = Some(chrono::Utc::now());
    agent.pid = None;
    state.save_agent(run_id, &agent)?;

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
        None,
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

fn cmd_summary(run: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };

    let metadata = state.load_run_metadata(&run_id)?;
    let agents = state.list_agents(&run_id)?;
    let tasks = state.list_tasks(&run_id)?;

    // Duration
    let now = chrono::Utc::now();
    let duration = now - metadata.created_at;
    let mins = duration.num_minutes();
    let secs = duration.num_seconds() % 60;

    println!("Run: {} ({:?})", metadata.id, metadata.status);
    println!("Duration: {}m {}s", mins, secs);

    // Agent costs
    let mut total_cost = 0.0;
    let mut agent_costs: Vec<(String, types::AgentCost)> = Vec::new();
    for agent in &agents {
        if let Some(cost) = state.load_agent_cost(&run_id, &agent.id) {
            total_cost += cost.cost_usd;
            agent_costs.push((agent.id.clone(), cost));
        }
    }

    if total_cost > 0.0 {
        println!("Total Cost: ${:.2}", total_cost);
    }

    // Agent counts
    let running = agents
        .iter()
        .filter(|a| a.status == types::AgentStatus::Running)
        .count();
    let done = agents
        .iter()
        .filter(|a| a.status == types::AgentStatus::Done)
        .count();
    let failed = agents
        .iter()
        .filter(|a| a.status == types::AgentStatus::Failed)
        .count();
    println!(
        "\nAgents: {} spawned, {} completed, {} failed",
        agents.len(),
        done,
        failed
    );
    if running > 0 {
        println!("        {} still running", running);
    }

    // Task counts
    let task_count = |s: types::TaskStatus| tasks.iter().filter(|t| t.status == s).count();
    let merged = task_count(types::TaskStatus::Merged);
    let task_failed = task_count(types::TaskStatus::Failed);
    println!(
        "Tasks:  {} created, {} merged, {} failed",
        tasks.len(),
        merged,
        task_failed,
    );

    // Per-agent cost
    if !agent_costs.is_empty() {
        println!("\nPer-Agent Cost:");
        for (id, cost) in &agent_costs {
            println!(
                "  {:<20} ${:.2}  ({}k input, {}k output)",
                id,
                cost.cost_usd,
                cost.input_tokens / 1000,
                cost.output_tokens / 1000,
            );
        }
    }

    // Merged commits
    let since_date = metadata.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
    if let Ok(output) = std::process::Command::new("git")
        .args([
            "log",
            "--oneline",
            &format!("--since={}", since_date),
            "main",
        ])
        .current_dir(state.repo_root())
        .output()
    {
        let commits = String::from_utf8_lossy(&output.stdout);
        let commits = commits.trim();
        if !commits.is_empty() {
            println!("\nMerged Changes:");
            for line in commits.lines() {
                println!("  - {}", line);
            }
        }
    }

    Ok(())
}

fn cmd_cost(run: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };

    let agents = state.list_agents(&run_id)?;

    let mut total_cost = 0.0;
    let mut agent_costs: Vec<(&types::Agent, types::AgentCost)> = Vec::new();
    for agent in &agents {
        if let Some(cost) = state.load_agent_cost(&run_id, &agent.id) {
            total_cost += cost.cost_usd;
            agent_costs.push((agent, cost));
        }
    }

    if agent_costs.is_empty() {
        println!("No cost data available (agents still running?).");
        return Ok(());
    }

    println!("Run Cost: ${:.2}", total_cost);

    // TODO: uncomment after lead-config merges
    // let config = state.load_config();
    // if let Some(budget) = config.budget_usd {
    //     let pct = (total_cost / budget * 100.0) as u32;
    //     println!("Budget:   ${:.2} ({}% used)", budget, pct);
    // }

    println!();
    println!(
        "{:<20} {:<12} {:>10} {:>10} {:>10}",
        "Agent", "Role", "Input", "Output", "Cost"
    );
    println!("{}", "-".repeat(65));

    for (agent, cost) in &agent_costs {
        println!(
            "{:<20} {:<12} {:>9}k {:>9}k {:>9}",
            agent.id,
            format!("{:?}", agent.role).to_lowercase(),
            cost.input_tokens / 1000,
            cost.output_tokens / 1000,
            format!("${:.2}", cost.cost_usd),
        );
    }

    Ok(())
}

fn cmd_history() -> Result<(), String> {
    let state = HiveState::discover()?;
    let runs = state.list_runs()?;

    if runs.is_empty() {
        println!("No runs found.");
        return Ok(());
    }

    println!(
        "{:<12} {:<12} {:<22} {:>7} {:>7} {:>7}",
        "ID", "Status", "Created", "Agents", "Tasks", "Merged"
    );
    println!("{}", "-".repeat(70));

    for run in &runs {
        let agents_count = state.list_agents(&run.id).map(|a| a.len()).unwrap_or(0);
        let tasks = state.list_tasks(&run.id).unwrap_or_default();
        let merged = tasks
            .iter()
            .filter(|t| t.status == types::TaskStatus::Merged)
            .count();
        println!(
            "{:<12} {:<12?} {:<22} {:>7} {:>7} {:>7}",
            &run.id,
            run.status,
            run.created_at.format("%Y-%m-%d %H:%M"),
            agents_count,
            tasks.len(),
            merged,
        );
    }

    Ok(())
}

fn cmd_watch(interval: u64) -> Result<(), String> {
    loop {
        // Clear screen
        print!("\x1b[2J\x1b[H");

        let now = chrono::Utc::now();
        println!(
            "=== Hive Watch (every {}s) === {}",
            interval,
            now.format("%H:%M:%S")
        );
        println!();

        let state = HiveState::discover()?;
        let run_id = state.active_run_id()?;
        let meta = state.load_run_metadata(&run_id)?;
        let agents = state.list_agents(&run_id)?;
        let tasks = state.list_tasks(&run_id)?;
        let queue = state.load_merge_queue(&run_id)?;

        // Duration
        let elapsed = chrono::Utc::now() - meta.created_at;
        let total_secs = elapsed.num_seconds().max(0);
        let mins = total_secs / 60;
        let secs = total_secs % 60;

        println!("Run: {} ({:?}, {}m {}s)", run_id, meta.status, mins, secs);

        // Agent counts by status
        let count_agents = |s: types::AgentStatus| agents.iter().filter(|a| a.status == s).count();
        let mut agent_parts = vec![];
        for (status, label) in [
            (types::AgentStatus::Running, "running"),
            (types::AgentStatus::Idle, "idle"),
            (types::AgentStatus::Done, "done"),
            (types::AgentStatus::Failed, "failed"),
            (types::AgentStatus::Stalled, "stalled"),
        ] {
            let c = count_agents(status);
            if c > 0 {
                agent_parts.push(format!("{c} {label}"));
            }
        }
        println!(
            "Agents: {}",
            if agent_parts.is_empty() {
                "none".to_string()
            } else {
                agent_parts.join(" · ")
            }
        );

        // Task counts by status
        let count_tasks = |s: types::TaskStatus| tasks.iter().filter(|t| t.status == s).count();
        let mut task_parts = vec![];
        for (status, label) in [
            (types::TaskStatus::Active, "active"),
            (types::TaskStatus::Pending, "pending"),
            (types::TaskStatus::Review, "review"),
            (types::TaskStatus::Merged, "merged"),
            (types::TaskStatus::Absorbed, "absorbed"),
            (types::TaskStatus::Failed, "failed"),
            (types::TaskStatus::Cancelled, "cancelled"),
        ] {
            let c = count_tasks(status);
            if c > 0 {
                task_parts.push(format!("{c} {label}"));
            }
        }
        println!(
            "Tasks:  {}",
            if task_parts.is_empty() {
                "none".to_string()
            } else {
                task_parts.join(" · ")
            }
        );

        println!("Queue:  {} entries", queue.entries.len());

        // Recent messages
        println!();
        println!("--- Recent Activity ---");
        let messages = state.list_messages(&run_id)?;
        let recent: Vec<_> = messages.iter().rev().take(5).collect();
        if recent.is_empty() {
            println!("  (no messages)");
        } else {
            for msg in recent.iter().rev() {
                let body = if msg.body.len() > 80 {
                    &msg.body[..80]
                } else {
                    &msg.body
                };
                println!(
                    "  [{}] {} -> {}: {}",
                    msg.timestamp.format("%H:%M:%S"),
                    msg.from,
                    msg.to,
                    body
                );
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

fn cmd_memory(command: Option<cli::MemoryCommands>) -> Result<(), String> {
    let state = HiveState::discover()?;
    match command {
        None => {
            let ops = state.load_operations();
            let conventions = state.load_conventions();
            let failures = state.load_failures();
            let conv_lines = conventions.lines().filter(|l| !l.trim().is_empty()).count();
            println!("Memory:");
            println!("  Operations: {} entries", ops.len());
            println!("  Conventions: {} lines", conv_lines);
            println!("  Failures: {} entries", failures.len());
            Ok(())
        }
        Some(cli::MemoryCommands::Show) => {
            let ops = state.load_operations();
            let conventions = state.load_conventions();
            let failures = state.load_failures();

            println!("=== Operations ({}) ===", ops.len());
            for op in &ops {
                println!("{}", serde_json::to_string_pretty(op).unwrap_or_default());
            }

            println!("\n=== Conventions ===");
            if conventions.is_empty() {
                println!("(none)");
            } else {
                println!("{}", conventions);
            }

            println!("\n=== Failures ({}) ===", failures.len());
            for f in &failures {
                println!("{}", serde_json::to_string_pretty(f).unwrap_or_default());
            }
            Ok(())
        }
        Some(cli::MemoryCommands::Prune) => {
            state.prune_memory()?;
            println!("Memory pruned.");
            Ok(())
        }
    }
}

fn cmd_explore(intent: &str) -> Result<(), String> {
    let state = HiveState::discover()?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    // Save intent as spec
    state.save_spec(&run_id, intent)?;

    // Generate coordinator prompt for explore mode
    let codebase_summary = crate::agent::AgentSpawner::generate_codebase_summary(state.repo_root());
    let memory = state.load_memory_for_prompt(&crate::types::AgentRole::Coordinator);
    let coordinator_prompt = crate::agent::AgentSpawner::explore_coordinator_prompt(
        &run_id,
        intent,
        &codebase_summary,
        &memory,
    );

    // Write coordinator CLAUDE.local.md to the base repo
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
                        "INPUT=$(cat); TOOL=$(printf '%s' \"$INPUT\" | jq -r '.tool_name'); ARGS=$(printf '%s' \"$INPUT\" | jq -r '(.tool_input // {{}} | to_entries | map(.key + \"=\" + (.value | tostring | .[0:80])) | join(\", \"))'); hive log-tool --run {run_id} --agent coordinator --tool \"$TOOL\" --status success --args-summary \"$ARGS\""
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

    // Write .mcp.json for coordinator MCP (merge with existing)
    crate::agent::AgentSpawner::write_mcp_config(
        &repo_root.join(".mcp.json"),
        &run_id,
        "coordinator",
    )?;

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

    println!("Created explore run: {run_id}");
    println!("Coordinator configured for EXPLORE mode.");
    println!("Launch Claude Code in this directory to begin.");
    println!("Run 'hive tui' in another terminal to monitor progress.");

    Ok(())
}

fn cmd_mind(command: Option<cli::MindCommands>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    match command {
        None => {
            let discoveries = state.load_discoveries(&run_id);
            let insights = state.load_insights(&run_id);
            println!("Hive Mind for run {run_id}:");
            println!("  Discoveries: {}", discoveries.len());
            println!("  Insights: {}", insights.len());

            if !discoveries.is_empty() {
                println!("\n--- Recent Discoveries ---");
                for disc in discoveries.iter().rev().take(5).rev() {
                    let content_preview = if disc.content.len() > 80 {
                        &disc.content[..80]
                    } else {
                        &disc.content
                    };
                    println!(
                        "  {} [{}] by {} ({:?}): {}",
                        disc.id,
                        disc.tags.join(", "),
                        disc.agent_id,
                        disc.confidence,
                        content_preview
                    );
                }
            }

            if !insights.is_empty() {
                println!("\n--- Insights ---");
                for ins in &insights {
                    println!("  {} [{}]: {}", ins.id, ins.tags.join(", "), ins.content);
                    println!("    Based on: {}", ins.discovery_ids.join(", "));
                }
            }

            Ok(())
        }
        Some(cli::MindCommands::Query { query }) => {
            let result = state.query_mind(&run_id, &query);
            println!("Hive Mind query: \"{}\"", query);
            println!(
                "Found {} discoveries, {} insights",
                result.discoveries.len(),
                result.insights.len()
            );

            if !result.discoveries.is_empty() {
                println!("\n--- Matching Discoveries ---");
                for disc in &result.discoveries {
                    println!(
                        "  {} [{}] by {} ({:?})",
                        disc.id,
                        disc.tags.join(", "),
                        disc.agent_id,
                        disc.confidence
                    );
                    println!("    {}", disc.content);
                    if !disc.file_paths.is_empty() {
                        println!("    Files: {}", disc.file_paths.join(", "));
                    }
                }
            }

            if !result.insights.is_empty() {
                println!("\n--- Matching Insights ---");
                for ins in &result.insights {
                    println!("  {} [{}]: {}", ins.id, ins.tags.join(", "), ins.content);
                    println!("    Based on: {}", ins.discovery_ids.join(", "));
                }
            }

            Ok(())
        }
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

    // Spawn post-mortem analysis agent
    println!("Spawning post-mortem analysis agent...");
    match crate::agent::AgentSpawner::spawn(
        &state,
        &run_id,
        "postmortem",
        crate::types::AgentRole::Postmortem,
        None,
        "Analyze the completed run and extract learnings",
    ) {
        Ok(pm_agent) => {
            let timeout = std::time::Duration::from_secs(180);
            let start = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Some(pid) = pm_agent.pid {
                    if !crate::agent::AgentSpawner::is_alive(pid) {
                        println!("Post-mortem analysis complete.");
                        break;
                    }
                } else {
                    break;
                }
                if start.elapsed() > timeout {
                    eprintln!("Warning: post-mortem agent timed out after 3 minutes.");
                    break;
                }
            }
            // Clean up postmortem worktree
            if let Some(ref wt) = pm_agent.worktree {
                let wt_path = std::path::Path::new(wt.as_str());
                if wt_path.exists() {
                    let _ = crate::git::Git::worktree_remove(state.repo_root(), wt_path);
                }
            }
            crate::git::Git::worktree_prune(state.repo_root()).ok();
        }
        Err(e) => {
            eprintln!("Warning: failed to spawn post-mortem agent: {e}");
        }
    }

    // Clean up coordinator files
    let repo_root = state.repo_root();
    let _ = std::fs::remove_file(repo_root.join("CLAUDE.local.md"));
    let _ = crate::agent::AgentSpawner::remove_hive_mcp_entry(&repo_root.join(".mcp.json"));
    let _ = std::fs::remove_file(repo_root.join(".claude/settings.local.json"));

    // Mark run as completed
    if let Ok(mut metadata) = state.load_run_metadata(&run_id) {
        metadata.status = types::RunStatus::Completed;
        let _ = state.save_run_metadata(&run_id, &metadata);
    }

    println!("Run {run_id} stopped.\n");

    // Print summary
    cmd_summary(Some(run_id))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use crate::cli::{Cli, Commands, MemoryCommands, MindCommands};

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
}
