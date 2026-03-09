use notify::{Event, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;

use crate::state::HiveState;
use crate::types::{Agent, MergeQueue, Message, Task};

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
) -> Option<String> {
    let rel = path.strip_prefix(run_dir).ok()?;
    let components: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();

    match components.as_slice() {
        // agents/<id>/agent.json
        ["agents", agent_id, "agent.json"] => {
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
            Some(format!(
                "task {} status changed to {:?}",
                task.id, task.status
            ))
        }
        // messages/<file>.json
        ["messages", filename] if filename.ends_with(".json") => {
            let data = std::fs::read_to_string(path).ok()?;
            let msg: Message = serde_json::from_str(&data).ok()?;
            Some(format!(
                "new message {} from {} to {}",
                msg.id, msg.from, msg.to
            ))
        }
        // merge-queue.json
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

pub async fn wait_for_activity(
    repo_root: &Path,
    run_id: &str,
    timeout_secs: u64,
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

    // Use spawn_blocking so the std::sync::mpsc recv_timeout doesn't block the tokio runtime
    tokio::task::spawn_blocking(move || {
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

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(timeout_summary(
                    &repo_root_owned,
                    &run_id_owned,
                    timeout_dur,
                ));
            }

            match rx.recv_timeout(remaining) {
                Ok(paths) => {
                    for path in &paths {
                        if let Some(desc) = describe_event(&run_dir_owned, path, &agent_snapshots) {
                            return Ok(desc);
                        }
                    }
                    // All events were heartbeat-only, keep waiting
                    continue;
                }
                Err(_) => {
                    return Ok(timeout_summary(
                        &repo_root_owned,
                        &run_id_owned,
                        timeout_dur,
                    ));
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

        let result = wait_for_activity(dir.path(), run_id, 1).await.unwrap();
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

        let desc = describe_event(run_dir, &task_path, &HashMap::new()).unwrap();
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

        let desc = describe_event(run_dir, &msg_path, &HashMap::new()).unwrap();
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

        let desc = describe_event(run_dir, &queue_path, &HashMap::new()).unwrap();
        assert!(desc.contains("merge queue"), "unexpected: {desc}");
        assert!(desc.contains("1 entries"), "unexpected: {desc}");
    }

    #[test]
    fn describe_event_returns_none_for_irrelevant_file() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path();
        let path = run_dir.join("run.json");
        std::fs::write(&path, "{}").unwrap();

        assert!(describe_event(run_dir, &path, &HashMap::new()).is_none());
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

        assert!(describe_event(run_dir, &agent_path, &snapshots).is_none());
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

        let desc = describe_event(run_dir, &agent_path, &snapshots);
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

        let desc = describe_event(run_dir, &agent_path, &snapshots);
        assert!(desc.is_some(), "new agent should be reported");
        assert!(desc.unwrap().contains("w-2"));
    }
}
