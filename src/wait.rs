use notify::{Event, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::state::HiveState;
use crate::types::{Agent, AgentStatus, MergeQueue, Message, Task};

/// Context for filtering events to only those relevant to a specific agent's team.
pub struct AgentContext {
    pub agent_id: String,
    /// Self + parent + direct children agent IDs
    pub team: HashSet<String>,
    /// Task IDs assigned to any team member
    pub relevant_tasks: HashSet<String>,
}

impl AgentContext {
    pub fn new(repo_root: &Path, run_id: &str, agent_id: &str) -> Self {
        let state = HiveState::new(repo_root.to_path_buf());
        let agents = state.list_agents(run_id).unwrap_or_default();
        let tasks = state.list_tasks(run_id).unwrap_or_default();

        let mut team = HashSet::new();
        team.insert(agent_id.to_string());

        // Find our agent to get parent
        if let Some(me) = agents.iter().find(|a| a.id == agent_id)
            && let Some(ref parent) = me.parent
        {
            team.insert(parent.clone());
        }

        // Find direct children (agents whose parent is us)
        for agent in &agents {
            if agent.parent.as_deref() == Some(agent_id) {
                team.insert(agent.id.clone());
            }
        }

        // Collect tasks assigned to any team member
        let relevant_tasks: HashSet<String> = tasks
            .iter()
            .filter(|t| t.assigned_to.as_ref().is_some_and(|a| team.contains(a)))
            .map(|t| t.id.clone())
            .collect();

        Self {
            agent_id: agent_id.to_string(),
            team,
            relevant_tasks,
        }
    }
}

/// Returns true if the only differences between two agents are timestamp-only
/// fields (heartbeat, messages_read_at, last_completed_at).
fn is_heartbeat_only_change(old: &Agent, new: &Agent) -> bool {
    old.id == new.id
        && old.role == new.role
        && old.status == new.status
        && old.parent == new.parent
        && old.pid == new.pid
        && old.worktree == new.worktree
        && old.task_id == new.task_id
        && old.session_id == new.session_id
}

fn describe_event(
    run_dir: &Path,
    path: &Path,
    agent_snapshots: &HashMap<String, Agent>,
    ctx: Option<&AgentContext>,
) -> Option<String> {
    let rel = path.strip_prefix(run_dir).ok()?;
    let components: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();

    match components.as_slice() {
        // agents/<id>/agent.json
        ["agents", agent_id, "agent.json"] => {
            // Filter: only team members
            if let Some(ctx) = ctx
                && !ctx.team.contains(*agent_id)
            {
                return None;
            }

            let data = std::fs::read_to_string(path).ok()?;
            let agent: Agent = serde_json::from_str(&data).ok()?;

            // Skip heartbeat-only changes
            if let Some(old) = agent_snapshots.get(*agent_id)
                && is_heartbeat_only_change(old, &agent)
            {
                return None;
            }

            let process_alive = agent.pid.map(crate::agent::AgentSpawner::is_alive);
            Some(format!(
                "agent {} status: {:?}, process_alive: {}",
                agent_id,
                agent.status,
                match process_alive {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "unknown",
                }
            ))
        }
        // tasks/<file>.json
        ["tasks", filename] if filename.ends_with(".json") => {
            let data = std::fs::read_to_string(path).ok()?;
            let task: Task = serde_json::from_str(&data).ok()?;

            // Filter: task assigned to team or in relevant_tasks
            if let Some(ctx) = ctx {
                let assigned_to_team = task
                    .assigned_to
                    .as_ref()
                    .is_some_and(|a| ctx.team.contains(a));
                if !assigned_to_team && !ctx.relevant_tasks.contains(&task.id) {
                    return None;
                }
            }

            Some(format!(
                "task {} status changed to {:?}",
                task.id, task.status
            ))
        }
        // messages/<file>.json
        ["messages", filename] if filename.ends_with(".json") => {
            let data = std::fs::read_to_string(path).ok()?;
            let msg: Message = serde_json::from_str(&data).ok()?;

            // Filter: only messages to us or from team
            if let Some(ctx) = ctx
                && msg.to != ctx.agent_id
                && !ctx.team.contains(&msg.from)
            {
                return None;
            }

            Some(format!(
                "new message {} from {} to {}",
                msg.id, msg.from, msg.to
            ))
        }
        // merge-queue.json — always pass (coordinators use None; leads see all queue changes)
        ["merge-queue.json"] => {
            let data = std::fs::read_to_string(path).ok()?;
            let queue: MergeQueue = serde_json::from_str(&data).ok()?;
            Some(format!(
                "merge queue updated ({} entries)",
                queue.entries.len()
            ))
        }
        _ => None,
    }
}

fn is_relevant_path(run_dir: &Path, path: &Path) -> bool {
    let rel = match path.strip_prefix(run_dir) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let first = match rel.iter().next().and_then(|c| c.to_str()) {
        Some(c) => c,
        None => return false,
    };
    matches!(first, "agents" | "tasks" | "messages") || rel.to_str() == Some("merge-queue.json")
}

fn timeout_summary(repo_root: &Path, run_id: &str, timeout_secs: u64) -> String {
    let state = HiveState::new(repo_root.to_path_buf());
    let agents = state.list_agents(run_id).unwrap_or_default();

    let mut running = 0u32;
    let mut idle = 0u32;
    let mut done = 0u32;
    let mut failed = 0u32;
    let mut stalled = 0u32;
    for agent in &agents {
        match agent.status {
            crate::types::AgentStatus::Running => running += 1,
            crate::types::AgentStatus::Idle => idle += 1,
            crate::types::AgentStatus::Done => done += 1,
            crate::types::AgentStatus::Failed => failed += 1,
            crate::types::AgentStatus::Stalled => stalled += 1,
        }
    }

    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if idle > 0 {
        parts.push(format!("{idle} idle"));
    }
    if done > 0 {
        parts.push(format!("{done} done"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if stalled > 0 {
        parts.push(format!("{stalled} stalled"));
    }

    if parts.is_empty() {
        format!("no activity detected within {timeout_secs}s. 0 agents")
    } else {
        format!(
            "no activity detected within {timeout_secs}s. {}",
            parts.join(", ")
        )
    }
}

pub fn agent_status_digest(repo_root: &Path, run_id: &str, agent_id: Option<&str>) -> String {
    let state = HiveState::new(repo_root.to_path_buf());
    let agents = state.list_agents(run_id).unwrap_or_default();

    if agents.is_empty() {
        return String::new();
    }

    // If agent_id provided, filter to team members only
    let team: Option<HashSet<String>> = agent_id.map(|id| {
        let mut set = HashSet::new();
        set.insert(id.to_string());
        if let Some(me) = agents.iter().find(|a| a.id == id)
            && let Some(ref parent) = me.parent
        {
            set.insert(parent.clone());
        }
        for agent in &agents {
            if agent.parent.as_deref() == Some(id) {
                set.insert(agent.id.clone());
            }
        }
        set
    });

    let now = chrono::Utc::now();
    let mut lines = Vec::new();

    for agent in &agents {
        if let Some(ref team) = team
            && !team.contains(&agent.id)
        {
            continue;
        }

        let timing = match agent.status {
            AgentStatus::Idle => agent
                .last_completed_at
                .map(|t| {
                    let secs = (now - t).num_seconds().max(0);
                    format!("idle {secs}s")
                })
                .unwrap_or_else(|| "idle".to_string()),
            AgentStatus::Running => agent
                .heartbeat
                .map(|t| {
                    let secs = (now - t).num_seconds().max(0);
                    format!("heartbeat {secs}s ago")
                })
                .unwrap_or_else(|| "running".to_string()),
            other => format!("{other:?}").to_lowercase(),
        };

        let task_info = agent.task_id.as_deref().unwrap_or("no task");

        lines.push(format!(
            "{}: {:?} ({}, {})",
            agent.id, agent.status, task_info, timing,
        ));
    }

    if lines.is_empty() {
        return String::new();
    }

    format!("\n\n--- Agent Status ---\n{}", lines.join("\n"))
}

pub async fn wait_for_activity(
    repo_root: &Path,
    run_id: &str,
    timeout_secs: u64,
    agent_id: Option<&str>,
) -> Result<String, String> {
    let run_dir = repo_root.join(".hive").join("runs").join(run_id);

    if !run_dir.exists() {
        return Err(format!(
            "Run directory does not exist: {}",
            run_dir.display()
        ));
    }

    // Snapshot current agent states before watching so we can detect heartbeat-only changes
    let state = HiveState::new(repo_root.to_path_buf());
    let agent_snapshots: HashMap<String, Agent> = state
        .list_agents(run_id)
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let repo_root_owned = repo_root.to_path_buf();
    let run_id_owned = run_id.to_string();
    let run_dir_owned = run_dir.clone();
    let timeout_dur = timeout_secs;
    let agent_id_owned = agent_id.map(|s| s.to_string());

    // Use spawn_blocking so the std::sync::mpsc recv_timeout doesn't block the tokio runtime
    tokio::task::spawn_blocking(move || {
        // Build AgentContext if agent_id is provided (for cross-team filtering)
        let ctx = agent_id_owned
            .as_deref()
            .map(|id| AgentContext::new(&repo_root_owned, &run_id_owned, id));

        let (tx, rx) = std::sync::mpsc::channel::<Vec<std::path::PathBuf>>();

        let run_dir_for_watcher = run_dir_owned.clone();
        let _watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let relevant: Vec<_> = event
                    .paths
                    .into_iter()
                    .filter(|p| is_relevant_path(&run_dir_for_watcher, p))
                    .collect();
                if !relevant.is_empty() {
                    let _ = tx.send(relevant);
                }
            }
        })
        .and_then(|mut w| {
            w.watch(&run_dir_owned, RecursiveMode::Recursive)?;
            Ok(w)
        })
        .map_err(|e| format!("Failed to create file watcher: {e}"))?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_dur);

        let digest =
            agent_status_digest(&repo_root_owned, &run_id_owned, agent_id_owned.as_deref());

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                let mut result = timeout_summary(&repo_root_owned, &run_id_owned, timeout_dur);
                result.push_str(&digest);
                return Ok(result);
            }

            match rx.recv_timeout(remaining) {
                Ok(paths) => {
                    for path in &paths {
                        if let Some(mut desc) =
                            describe_event(&run_dir_owned, path, &agent_snapshots, ctx.as_ref())
                        {
                            desc.push_str(&digest);
                            return Ok(desc);
                        }
                    }
                    // All events were filtered out or heartbeat-only, keep waiting
                    continue;
                }
                Err(_) => {
                    let mut result = timeout_summary(&repo_root_owned, &run_id_owned, timeout_dur);
                    result.push_str(&digest);
                    return Ok(result);
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn timeout_returns_no_activity_message() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let run_dir = dir.path().join(".hive").join("runs").join(run_id);
        std::fs::create_dir_all(run_dir.join("agents")).unwrap();
        std::fs::create_dir_all(run_dir.join("tasks")).unwrap();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();

        let result = wait_for_activity(dir.path(), run_id, 1, None)
            .await
            .unwrap();
        assert!(
            result.contains("no activity detected within 1s"),
            "unexpected result: {result}"
        );
    }

    #[test]
    fn describe_event_parses_task_file() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("tasks")).unwrap();

        let task = serde_json::json!({
            "id": "task-1",
            "title": "Test task",
            "description": "A test",
            "status": "active",
            "urgency": "normal",
            "blocking": [],
            "blocked_by": [],
            "assigned_to": null,
            "created_by": "test",
            "parent_task": null,
            "branch": null,
            "domain": null,
            "created_at": "2026-03-08T00:00:00Z",
            "updated_at": "2026-03-08T00:00:00Z"
        });
        let task_path = run_dir.join("tasks").join("task-1.json");
        std::fs::write(&task_path, serde_json::to_string_pretty(&task).unwrap()).unwrap();

        let desc = describe_event(run_dir, &task_path, &HashMap::new(), None).unwrap();
        assert!(desc.contains("task-1"), "unexpected: {desc}");
        assert!(desc.contains("Active"), "unexpected: {desc}");
    }

    #[test]
    fn describe_event_parses_message_file() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();

        let msg = serde_json::json!({
            "id": "msg-42",
            "from": "lead-1",
            "to": "coordinator",
            "timestamp": "2026-03-08T00:00:00Z",
            "message_type": "info",
            "body": "hello",
            "refs": []
        });
        let msg_path = run_dir.join("messages").join("msg-42.json");
        std::fs::write(&msg_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();

        let desc = describe_event(run_dir, &msg_path, &HashMap::new(), None).unwrap();
        assert!(desc.contains("msg-42"), "unexpected: {desc}");
        assert!(desc.contains("lead-1"), "unexpected: {desc}");
        assert!(desc.contains("coordinator"), "unexpected: {desc}");
    }

    #[test]
    fn describe_event_parses_merge_queue() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();

        let queue = serde_json::json!({
            "entries": [
                {
                    "task_id": "task-1",
                    "branch": "hive/run/lead-1",
                    "submitted_by": "lead-1",
                    "submitted_at": "2026-03-08T00:00:00Z"
                }
            ]
        });
        let queue_path = run_dir.join("merge-queue.json");
        std::fs::write(&queue_path, serde_json::to_string_pretty(&queue).unwrap()).unwrap();

        let desc = describe_event(run_dir, &queue_path, &HashMap::new(), None).unwrap();
        assert!(desc.contains("merge queue"), "unexpected: {desc}");
        assert!(desc.contains("1 entries"), "unexpected: {desc}");
    }

    #[test]
    fn describe_event_returns_none_for_irrelevant_file() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        let path = run_dir.join("run.json");
        std::fs::write(&path, "{}").unwrap();

        assert!(describe_event(run_dir, &path, &HashMap::new(), None).is_none());
    }

    #[test]
    fn is_relevant_path_filters_correctly() {
        let run_dir = Path::new("/tmp/repo/.hive/runs/run-1");

        assert!(is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/agents/lead-1/agent.json")
        ));
        assert!(is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/tasks/task-1.json")
        ));
        assert!(is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/messages/msg-1.json")
        ));
        assert!(is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/merge-queue.json")
        ));
        assert!(!is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/run.json")
        ));
        assert!(!is_relevant_path(
            run_dir,
            Path::new("/tmp/repo/.hive/runs/run-1/worktrees/foo/bar")
        ));
    }

    #[test]
    fn timeout_summary_with_no_agents() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let run_dir = dir.path().join(".hive").join("runs").join(run_id);
        std::fs::create_dir_all(run_dir.join("agents")).unwrap();

        let result = timeout_summary(dir.path(), run_id, 60);
        assert_eq!(result, "no activity detected within 60s. 0 agents");
    }

    #[test]
    fn timeout_summary_with_agents() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let run_dir = dir.path().join(".hive").join("runs").join(run_id);
        std::fs::create_dir_all(run_dir.join("agents")).unwrap();

        let state = crate::state::HiveState::new(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join(".hive")).unwrap();

        // Save two agents
        let running_agent = crate::types::Agent {
            id: "agent-1".into(),
            role: crate::types::AgentRole::Worker,
            status: crate::types::AgentStatus::Running,
            parent: None,
            pid: None,
            worktree: None,
            heartbeat: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            task_id: None,
            retry_count: 0,
            model: None,
        };
        state.save_agent(run_id, &running_agent).unwrap();

        let done_agent = crate::types::Agent {
            id: "agent-2".into(),
            role: crate::types::AgentRole::Worker,
            status: crate::types::AgentStatus::Done,
            parent: None,
            pid: None,
            worktree: None,
            heartbeat: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            task_id: None,
            retry_count: 0,
            model: None,
        };
        state.save_agent(run_id, &done_agent).unwrap();

        let result = timeout_summary(dir.path(), run_id, 30);
        assert!(result.contains("1 running"), "unexpected: {result}");
        assert!(result.contains("1 done"), "unexpected: {result}");
    }

    fn make_test_agent(id: &str) -> Agent {
        Agent {
            id: id.into(),
            role: crate::types::AgentRole::Worker,
            status: crate::types::AgentStatus::Running,
            parent: Some("lead-1".into()),
            pid: Some(12345),
            worktree: Some("/tmp/wt".into()),
            heartbeat: Some(chrono::Utc::now()),
            session_id: Some("sess-1".into()),
            last_completed_at: None,
            messages_read_at: None,
            task_id: Some("task-1".into()),
            retry_count: 0,
            model: None,
        }
    }

    #[test]
    fn is_heartbeat_only_change_returns_true_for_heartbeat_update() {
        let old = make_test_agent("w-1");
        let mut new = old.clone();
        new.heartbeat = Some(chrono::Utc::now() + chrono::Duration::seconds(30));
        assert!(is_heartbeat_only_change(&old, &new));
    }

    #[test]
    fn is_heartbeat_only_change_returns_true_for_messages_read_at_update() {
        let old = make_test_agent("w-1");
        let mut new = old.clone();
        new.messages_read_at = Some(chrono::Utc::now());
        assert!(is_heartbeat_only_change(&old, &new));
    }

    #[test]
    fn is_heartbeat_only_change_returns_false_for_status_change() {
        let old = make_test_agent("w-1");
        let mut new = old.clone();
        new.status = crate::types::AgentStatus::Done;
        assert!(!is_heartbeat_only_change(&old, &new));
    }

    #[test]
    fn is_heartbeat_only_change_returns_false_for_task_change() {
        let old = make_test_agent("w-1");
        let mut new = old.clone();
        new.task_id = Some("task-2".into());
        assert!(!is_heartbeat_only_change(&old, &new));
    }

    #[test]
    fn is_heartbeat_only_change_returns_false_for_pid_change() {
        let old = make_test_agent("w-1");
        let mut new = old.clone();
        new.pid = Some(99999);
        assert!(!is_heartbeat_only_change(&old, &new));
    }

    #[test]
    fn describe_event_skips_heartbeat_only_agent_change() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("agents").join("w-1")).unwrap();

        let agent = make_test_agent("w-1");
        let mut snapshots = HashMap::new();
        snapshots.insert("w-1".to_string(), agent.clone());

        // Write agent with only heartbeat changed
        let mut updated = agent;
        updated.heartbeat = Some(chrono::Utc::now() + chrono::Duration::seconds(10));
        let agent_path = run_dir.join("agents").join("w-1").join("agent.json");
        std::fs::write(&agent_path, serde_json::to_string(&updated).unwrap()).unwrap();

        assert!(describe_event(run_dir, &agent_path, &snapshots, None).is_none());
    }

    #[test]
    fn describe_event_reports_agent_status_change() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("agents").join("w-1")).unwrap();

        let agent = make_test_agent("w-1");
        let mut snapshots = HashMap::new();
        snapshots.insert("w-1".to_string(), agent.clone());

        // Write agent with status changed
        let mut updated = agent;
        updated.status = crate::types::AgentStatus::Done;
        updated.pid = None;
        let agent_path = run_dir.join("agents").join("w-1").join("agent.json");
        std::fs::write(&agent_path, serde_json::to_string(&updated).unwrap()).unwrap();

        let desc = describe_event(run_dir, &agent_path, &snapshots, None);
        assert!(desc.is_some(), "should report status change");
        assert!(desc.unwrap().contains("Done"));
    }

    #[test]
    fn describe_event_reports_new_agent_not_in_snapshot() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("agents").join("w-2")).unwrap();

        // Empty snapshots — agent is new
        let snapshots = HashMap::new();

        let agent = make_test_agent("w-2");
        let agent_path = run_dir.join("agents").join("w-2").join("agent.json");
        std::fs::write(&agent_path, serde_json::to_string(&agent).unwrap()).unwrap();

        let desc = describe_event(run_dir, &agent_path, &snapshots, None);
        assert!(desc.is_some(), "new agent should be reported");
        assert!(desc.unwrap().contains("w-2"));
    }

    // --- AgentContext tests ---

    fn setup_hive_dir(dir: &Path, run_id: &str) -> crate::state::HiveState {
        let state = crate::state::HiveState::new(dir.to_path_buf());
        let run_dir = dir.join(".hive").join("runs").join(run_id);
        std::fs::create_dir_all(run_dir.join("agents")).unwrap();
        std::fs::create_dir_all(run_dir.join("tasks")).unwrap();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();
        state
    }

    fn make_agent_with_parent(id: &str, parent: Option<&str>, task_id: Option<&str>) -> Agent {
        Agent {
            id: id.into(),
            role: crate::types::AgentRole::Worker,
            status: crate::types::AgentStatus::Running,
            parent: parent.map(|s| s.into()),
            pid: Some(12345),
            worktree: Some("/tmp/wt".into()),
            heartbeat: Some(chrono::Utc::now()),
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            task_id: task_id.map(|s| s.into()),
            retry_count: 0,
            model: None,
        }
    }

    fn make_task(id: &str, assigned_to: Option<&str>) -> Task {
        let now = chrono::Utc::now();
        Task {
            id: id.into(),
            title: format!("Task {id}"),
            description: "test".into(),
            status: crate::types::TaskStatus::Active,
            urgency: crate::types::Urgency::Normal,
            blocking: vec![],
            blocked_by: vec![],
            assigned_to: assigned_to.map(|s| s.into()),
            created_by: "test".into(),
            parent_task: None,
            branch: None,
            domain: None,
            review_count: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn agent_context_computes_team_correctly() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let state = setup_hive_dir(dir.path(), run_id);

        // lead-1 has parent coordinator, children worker-1, worker-2
        let lead = make_agent_with_parent("lead-1", Some("coordinator"), Some("task-lead"));
        let w1 = make_agent_with_parent("worker-1", Some("lead-1"), Some("task-1"));
        let w2 = make_agent_with_parent("worker-2", Some("lead-1"), Some("task-2"));
        let other = make_agent_with_parent("worker-other", Some("lead-2"), Some("task-other"));

        state.save_agent(run_id, &lead).unwrap();
        state.save_agent(run_id, &w1).unwrap();
        state.save_agent(run_id, &w2).unwrap();
        state.save_agent(run_id, &other).unwrap();

        let t1 = make_task("task-lead", Some("lead-1"));
        let t2 = make_task("task-1", Some("worker-1"));
        let t3 = make_task("task-2", Some("worker-2"));
        let t4 = make_task("task-other", Some("worker-other"));
        state.save_task(run_id, &t1).unwrap();
        state.save_task(run_id, &t2).unwrap();
        state.save_task(run_id, &t3).unwrap();
        state.save_task(run_id, &t4).unwrap();

        let ctx = AgentContext::new(dir.path(), run_id, "lead-1");

        // Team should be: lead-1, coordinator (parent), worker-1, worker-2 (children)
        assert!(ctx.team.contains("lead-1"));
        assert!(ctx.team.contains("coordinator"));
        assert!(ctx.team.contains("worker-1"));
        assert!(ctx.team.contains("worker-2"));
        assert!(!ctx.team.contains("worker-other"));
        assert!(!ctx.team.contains("lead-2"));

        // Relevant tasks: tasks assigned to team members
        assert!(ctx.relevant_tasks.contains("task-lead"));
        assert!(ctx.relevant_tasks.contains("task-1"));
        assert!(ctx.relevant_tasks.contains("task-2"));
        assert!(!ctx.relevant_tasks.contains("task-other"));
    }

    #[test]
    fn describe_event_filters_messages_not_addressed_to_team() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: HashSet::new(),
        };

        // Message from outside team to outside team — should be filtered
        let msg = serde_json::json!({
            "id": "msg-1",
            "from": "lead-2",
            "to": "worker-other",
            "timestamp": "2026-03-08T00:00:00Z",
            "message_type": "info",
            "body": "hello",
            "refs": []
        });
        let msg_path = run_dir.join("messages").join("msg-1.json");
        std::fs::write(&msg_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();

        let desc = describe_event(run_dir, &msg_path, &HashMap::new(), Some(&ctx));
        assert!(
            desc.is_none(),
            "should filter message not addressed to team"
        );
    }

    #[test]
    fn describe_event_passes_messages_addressed_to_agent() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: HashSet::new(),
        };

        // Message addressed to us
        let msg = serde_json::json!({
            "id": "msg-2",
            "from": "lead-2",
            "to": "lead-1",
            "timestamp": "2026-03-08T00:00:00Z",
            "message_type": "info",
            "body": "hello",
            "refs": []
        });
        let msg_path = run_dir.join("messages").join("msg-2.json");
        std::fs::write(&msg_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();

        let desc = describe_event(run_dir, &msg_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_some(), "should pass message addressed to agent");
    }

    #[test]
    fn describe_event_passes_messages_from_team_member() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("messages")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: HashSet::new(),
        };

        // Message from team member to someone else
        let msg = serde_json::json!({
            "id": "msg-3",
            "from": "worker-1",
            "to": "coordinator",
            "timestamp": "2026-03-08T00:00:00Z",
            "message_type": "info",
            "body": "hello",
            "refs": []
        });
        let msg_path = run_dir.join("messages").join("msg-3.json");
        std::fs::write(&msg_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();

        let desc = describe_event(run_dir, &msg_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_some(), "should pass message from team member");
    }

    #[test]
    fn describe_event_filters_agent_not_in_team() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("agents").join("worker-other")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: HashSet::new(),
        };

        let agent = make_test_agent("worker-other");
        let agent_path = run_dir
            .join("agents")
            .join("worker-other")
            .join("agent.json");
        std::fs::write(&agent_path, serde_json::to_string(&agent).unwrap()).unwrap();

        let desc = describe_event(run_dir, &agent_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_none(), "should filter agent not in team");
    }

    #[test]
    fn describe_event_passes_agent_in_team() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("agents").join("worker-1")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: HashSet::new(),
        };

        let agent = make_agent_with_parent("worker-1", Some("lead-1"), None);
        let agent_path = run_dir.join("agents").join("worker-1").join("agent.json");
        std::fs::write(&agent_path, serde_json::to_string(&agent).unwrap()).unwrap();

        let desc = describe_event(run_dir, &agent_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_some(), "should pass agent in team");
    }

    #[test]
    fn describe_event_filters_task_not_in_team() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("tasks")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: ["task-1".to_string()].into_iter().collect(),
        };

        let task = serde_json::json!({
            "id": "task-other",
            "title": "Other task",
            "description": "A test",
            "status": "active",
            "urgency": "normal",
            "blocking": [],
            "blocked_by": [],
            "assigned_to": "worker-other",
            "created_by": "test",
            "parent_task": null,
            "branch": null,
            "domain": null,
            "created_at": "2026-03-08T00:00:00Z",
            "updated_at": "2026-03-08T00:00:00Z"
        });
        let task_path = run_dir.join("tasks").join("task-other.json");
        std::fs::write(&task_path, serde_json::to_string_pretty(&task).unwrap()).unwrap();

        let desc = describe_event(run_dir, &task_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_none(), "should filter task not assigned to team");
    }

    #[test]
    fn describe_event_passes_task_assigned_to_team() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        std::fs::create_dir_all(run_dir.join("tasks")).unwrap();

        let ctx = AgentContext {
            agent_id: "lead-1".into(),
            team: ["lead-1", "coordinator", "worker-1"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            relevant_tasks: ["task-1".to_string()].into_iter().collect(),
        };

        let task = serde_json::json!({
            "id": "task-new",
            "title": "New task",
            "description": "A test",
            "status": "active",
            "urgency": "normal",
            "blocking": [],
            "blocked_by": [],
            "assigned_to": "worker-1",
            "created_by": "test",
            "parent_task": null,
            "branch": null,
            "domain": null,
            "created_at": "2026-03-08T00:00:00Z",
            "updated_at": "2026-03-08T00:00:00Z"
        });
        let task_path = run_dir.join("tasks").join("task-new.json");
        std::fs::write(&task_path, serde_json::to_string_pretty(&task).unwrap()).unwrap();

        let desc = describe_event(run_dir, &task_path, &HashMap::new(), Some(&ctx));
        assert!(desc.is_some(), "should pass task assigned to team member");
    }

    #[test]
    fn agent_status_digest_produces_correct_format() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let state = setup_hive_dir(dir.path(), run_id);

        let mut agent = make_agent_with_parent("worker-1", Some("lead-1"), Some("task-1"));
        agent.status = crate::types::AgentStatus::Idle;
        agent.last_completed_at = Some(chrono::Utc::now());
        state.save_agent(run_id, &agent).unwrap();

        let mut agent2 = make_agent_with_parent("worker-2", Some("lead-1"), Some("task-2"));
        agent2.heartbeat = Some(chrono::Utc::now());
        state.save_agent(run_id, &agent2).unwrap();

        let digest = agent_status_digest(dir.path(), run_id, None);
        assert!(
            digest.contains("--- Agent Status ---"),
            "unexpected: {digest}"
        );
        assert!(digest.contains("worker-1"), "unexpected: {digest}");
        assert!(digest.contains("worker-2"), "unexpected: {digest}");
        assert!(digest.contains("Idle"), "unexpected: {digest}");
        assert!(digest.contains("Running"), "unexpected: {digest}");
    }

    #[test]
    fn agent_status_digest_filters_to_team() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let state = setup_hive_dir(dir.path(), run_id);

        let lead = make_agent_with_parent("lead-1", Some("coordinator"), Some("task-lead"));
        let w1 = make_agent_with_parent("worker-1", Some("lead-1"), Some("task-1"));
        let other = make_agent_with_parent("worker-other", Some("lead-2"), Some("task-other"));

        state.save_agent(run_id, &lead).unwrap();
        state.save_agent(run_id, &w1).unwrap();
        state.save_agent(run_id, &other).unwrap();

        let digest = agent_status_digest(dir.path(), run_id, Some("lead-1"));
        assert!(digest.contains("lead-1"), "should include self: {digest}");
        assert!(
            digest.contains("worker-1"),
            "should include child: {digest}"
        );
        assert!(
            !digest.contains("worker-other"),
            "should not include other team: {digest}"
        );
    }

    #[test]
    fn agent_status_digest_empty_when_no_agents() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        setup_hive_dir(dir.path(), run_id);

        let digest = agent_status_digest(dir.path(), run_id, None);
        assert_eq!(digest, "");
    }

    #[tokio::test]
    async fn wait_for_activity_with_agent_id_filters() {
        let dir = TempDir::new().unwrap();
        let run_id = "test-run";
        let state = setup_hive_dir(dir.path(), run_id);

        let lead = make_agent_with_parent("lead-1", Some("coordinator"), None);
        state.save_agent(run_id, &lead).unwrap();

        // With agent_id, should still timeout but include digest
        let result = wait_for_activity(dir.path(), run_id, 1, Some("lead-1"))
            .await
            .unwrap();
        assert!(
            result.contains("no activity detected"),
            "unexpected: {result}"
        );
        assert!(
            result.contains("--- Agent Status ---"),
            "should include digest: {result}"
        );
        assert!(result.contains("lead-1"), "should include agent: {result}");
    }
}
