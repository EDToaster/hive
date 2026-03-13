use crate::agent;
use crate::git;
use crate::logging::LogDb;
use crate::output;
use crate::state::HiveState;
use crate::types;

pub fn cmd_agents() -> Result<(), String> {
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

pub fn cmd_heartbeat(run_id: &str, agent_id: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    state.update_agent_heartbeat(run_id, agent_id)
}

pub fn cmd_agent_exit(run_id: &str, agent_id: &str) -> Result<(), String> {
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

pub fn cmd_log_tool(
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

pub fn cmd_logs(agent_filter: Option<String>) -> Result<(), String> {
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

pub fn cmd_review_agent(agent_id: &str, run: Option<String>) -> Result<(), String> {
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
    let base = agent.branched_from.as_deref().unwrap_or("main");
    let commits = crate::git::Git::log_oneline_since(wt_path, base)
        .unwrap_or_else(|_| "(no commits)".to_string());
    let diff_stat =
        crate::git::Git::diff_stat_since(wt_path, base).unwrap_or_else(|_| "(no diff)".to_string());

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
