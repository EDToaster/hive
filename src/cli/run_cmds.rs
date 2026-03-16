use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types;
use std::fs;

pub fn cmd_init() -> Result<(), String> {
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

pub fn cmd_start(spec_path: &str) -> Result<(), String> {
    let state = HiveState::discover()?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    let spec_content = {
        let content = fs::read_to_string(spec_path)
            .map_err(|e| format!("Cannot read spec file '{spec_path}': {e}"))?;
        state.save_spec(&run_id, &content)?;
        println!("Created run: {run_id}");
        content
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
        model: None,
        branched_from: None,
        wait_cursor: None,
    };
    state.save_agent(&run_id, &coordinator)?;

    println!(
        "Coordinator configured. Run `claude --dangerously-skip-permissions` to start the coordinator."
    );
    println!("Run 'hive tui' in another terminal to monitor progress.");

    Ok(())
}

pub fn cmd_status() -> Result<(), String> {
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

pub fn cmd_summary(run: Option<String>) -> Result<(), String> {
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
    let current_branch =
        crate::git::Git::current_branch(state.repo_root()).unwrap_or_else(|_| "main".into());
    let since_date = metadata.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
    if let Ok(output) = std::process::Command::new("git")
        .args([
            "log",
            "--oneline",
            &format!("--since={}", since_date),
            &current_branch,
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

pub fn cmd_cost(run: Option<String>) -> Result<(), String> {
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

pub fn cmd_config() -> Result<(), String> {
    let state = HiveState::discover()?;
    let config = state.load_config();

    println!("Hive Configuration (.hive/config.yaml)");
    println!("{}", "-".repeat(45));
    println!("stall_timeout_seconds: {}", config.stall_timeout_seconds);
    println!("max_retries:           {}", config.max_retries);
    println!(
        "budget_usd:            {}",
        config
            .budget_usd
            .map(|b| format!("${:.2}", b))
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!(
        "verify_command:        {}",
        config.verify_command.as_deref().unwrap_or("(none)")
    );
    println!(
        "fallback_model:        {}",
        config.fallback_model.as_deref().unwrap_or("(none)")
    );

    println!();
    println!("Model Mapping (role → model)");
    println!("{}", "-".repeat(45));
    let roles = [
        crate::types::AgentRole::Coordinator,
        crate::types::AgentRole::Lead,
        crate::types::AgentRole::Worker,
        crate::types::AgentRole::Reviewer,
        crate::types::AgentRole::Postmortem,
        crate::types::AgentRole::Explorer,
        crate::types::AgentRole::Evaluator,
    ];
    for role in roles {
        let model = config.models.model_for_role(role);
        let is_default = model == role.default_model();
        let suffix = if is_default {
            " (default)"
        } else {
            " (override)"
        };
        println!(
            "  {:<15} → {}{}",
            format!("{:?}", role).to_lowercase(),
            model,
            suffix
        );
    }
    println!();
    println!("Note: The coordinator model is set by the human when launching claude.");
    println!(
        "Per-spawn overrides via hive_spawn_agent(model=...) take priority over role defaults."
    );

    Ok(())
}

pub fn cmd_history() -> Result<(), String> {
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

pub fn cmd_tui() -> Result<(), String> {
    crate::tui::run_tui()
}

pub fn cmd_web(port: u16) -> Result<(), String> {
    crate::web::run_web(port)
}

pub fn cmd_mcp(run_id: &str, agent_id: &str) -> Result<(), String> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(crate::mcp::run_mcp_server(run_id, agent_id))
}

pub fn cmd_wait(run: Option<String>, timeout: u64) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    // Start from the current max event ID so we only see new events
    let cursor = {
        let log_path = state.run_dir(&run_id).join("log.db");
        crate::logging::LogDb::open(&log_path)
            .ok()
            .and_then(|db| db.max_event_id(&run_id).ok())
            .unwrap_or(0)
    };
    let (result, _cursor) = rt.block_on(crate::wait::wait_for_activity(
        state.repo_root(),
        &run_id,
        timeout,
        None,
        cursor,
    ))?;
    println!("{result}");
    Ok(())
}

pub fn cmd_watch(interval: u64) -> Result<(), String> {
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

pub fn cmd_explore(intent: &str) -> Result<(), String> {
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
        model: None,
        branched_from: None,
        wait_cursor: None,
    };
    state.save_agent(&run_id, &coordinator)?;

    println!("Created explore run: {run_id}");
    println!("Coordinator configured for EXPLORE mode.");
    println!("Run `claude --dangerously-skip-permissions` to start the coordinator.");
    println!("Run 'hive tui' in another terminal to monitor progress.");

    Ok(())
}

pub fn cmd_stop() -> Result<(), String> {
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
