use super::*;
use tempfile::TempDir;

fn make_state(dir: &std::path::Path) -> HiveState {
    std::fs::create_dir_all(dir.join(".hive")).unwrap();
    HiveState::new(dir.to_path_buf())
}

fn make_task(id: &str, status: TaskStatus) -> Task {
    let now = chrono::Utc::now();
    Task {
        id: id.into(),
        title: format!("Task {id}"),
        description: "A test task".into(),
        status,
        urgency: Urgency::Normal,
        blocking: vec![],
        blocked_by: vec![],
        assigned_to: None,
        created_by: "test".into(),
        parent_task: None,
        branch: None,
        domain: None,
        review_count: 0,
        submitted_by: None,
        created_at: now,
        updated_at: now,
    }
}

fn make_agent(id: &str, role: AgentRole, status: AgentStatus) -> Agent {
    Agent {
        id: id.into(),
        role,
        status,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
        branched_from: None,
        wait_cursor: None,
    }
}

fn make_message(
    id: &str,
    from: &str,
    to: &str,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> Message {
    Message {
        id: id.into(),
        from: from.into(),
        to: to.into(),
        timestamp,
        message_type: MessageType::Info,
        body: "hello".into(),
        refs: vec![],
    }
}

// --- Run management ---

#[test]
fn create_run_creates_directory_structure() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("test-run-1").unwrap();

    let run_dir = state.run_dir("test-run-1");
    assert!(run_dir.join("tasks").is_dir());
    assert!(run_dir.join("agents").is_dir());
    assert!(run_dir.join("messages").is_dir());
    assert!(run_dir.join("worktrees").is_dir());
}

#[test]
fn create_run_writes_run_json() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-abc").unwrap();

    let data = std::fs::read_to_string(state.run_dir("run-abc").join("run.json")).unwrap();
    let meta: RunMetadata = serde_json::from_str(&data).unwrap();
    assert_eq!(meta.id, "run-abc");
    assert_eq!(meta.status, RunStatus::Active);
}

#[test]
fn create_run_initializes_empty_merge_queue() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let queue = state.load_merge_queue("run-1").unwrap();
    assert!(queue.entries.is_empty());
}

#[test]
fn active_run_read_write() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());

    assert!(state.active_run_id().is_err());

    state.set_active_run("run-123").unwrap();
    assert_eq!(state.active_run_id().unwrap(), "run-123");
}

// --- Task CRUD ---

#[test]
fn task_save_and_load() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let task = make_task("task-1", TaskStatus::Pending);
    state.save_task("run-1", &task).unwrap();

    let loaded = state.load_task("run-1", "task-1").unwrap();
    assert_eq!(loaded.id, "task-1");
    assert_eq!(loaded.status, TaskStatus::Pending);
}

#[test]
fn task_update_overwrites() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let task = make_task("task-1", TaskStatus::Pending);
    state.save_task("run-1", &task).unwrap();

    let mut updated = task;
    updated.status = TaskStatus::Active;
    state.save_task("run-1", &updated).unwrap();

    let loaded = state.load_task("run-1", "task-1").unwrap();
    assert_eq!(loaded.status, TaskStatus::Active);
}

#[test]
fn list_tasks_returns_all() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    for i in 1..=3 {
        state
            .save_task(
                "run-1",
                &make_task(&format!("task-{i}"), TaskStatus::Pending),
            )
            .unwrap();
    }

    let tasks = state.list_tasks("run-1").unwrap();
    assert_eq!(tasks.len(), 3);
}

#[test]
fn list_tasks_empty_dir_returns_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let tasks = state.list_tasks("run-1").unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn list_tasks_nonexistent_run_returns_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let tasks = state.list_tasks("no-such-run").unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn load_nonexistent_task_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    assert!(state.load_task("run-1", "nonexistent").is_err());
}

// --- Agent CRUD ---

#[test]
fn agent_save_and_load() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Running);
    state.save_agent("run-1", &agent).unwrap();

    let loaded = state.load_agent("run-1", "agent-1").unwrap();
    assert_eq!(loaded.id, "agent-1");
    assert_eq!(loaded.role, AgentRole::Worker);
    assert_eq!(loaded.status, AgentStatus::Running);
}

#[test]
fn agent_save_creates_subdirectory() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("lead-1", AgentRole::Lead, AgentStatus::Running);
    state.save_agent("run-1", &agent).unwrap();

    assert!(state.agents_dir("run-1").join("lead-1").is_dir());
    assert!(
        state
            .agents_dir("run-1")
            .join("lead-1")
            .join("agent.json")
            .exists()
    );
}

#[test]
fn list_agents_reads_subdirectories() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    for id in ["agent-1", "agent-2"] {
        state
            .save_agent(
                "run-1",
                &make_agent(id, AgentRole::Worker, AgentStatus::Running),
            )
            .unwrap();
    }

    let agents = state.list_agents("run-1").unwrap();
    assert_eq!(agents.len(), 2);
}

#[test]
fn list_agents_empty_returns_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agents = state.list_agents("run-1").unwrap();
    assert!(agents.is_empty());
}

// --- Message CRUD ---

#[test]
fn message_save_and_list() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let msg = make_message("msg-1", "lead-1", "coord", chrono::Utc::now());
    state.save_message("run-1", &msg).unwrap();

    let messages = state.list_messages("run-1").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "msg-1");
    assert_eq!(messages[0].from, "lead-1");
}

#[test]
fn list_messages_sorted_by_timestamp() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let t1 = "2026-03-08T10:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let t2 = "2026-03-08T11:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let t3 = "2026-03-08T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();

    // Save out of order
    state
        .save_message("run-1", &make_message("msg-3", "a", "b", t3))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-1", "a", "b", t1))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-2", "a", "b", t2))
        .unwrap();

    let messages = state.list_messages("run-1").unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id, "msg-1");
    assert_eq!(messages[1].id, "msg-2");
    assert_eq!(messages[2].id, "msg-3");
}

// --- Merge Queue ---

#[test]
fn merge_queue_save_and_load() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let queue = MergeQueue {
        entries: vec![
            MergeQueueEntry {
                task_id: "task-1".into(),
                branch: "hive/run-1/lead-1".into(),
                submitted_by: "lead-1".into(),
                submitted_at: chrono::Utc::now(),
            },
            MergeQueueEntry {
                task_id: "task-2".into(),
                branch: "hive/run-1/lead-2".into(),
                submitted_by: "lead-2".into(),
                submitted_at: chrono::Utc::now(),
            },
        ],
    };
    state.save_merge_queue("run-1", &queue).unwrap();

    let loaded = state.load_merge_queue("run-1").unwrap();
    assert_eq!(loaded.entries.len(), 2);
    assert_eq!(loaded.entries[0].task_id, "task-1");
    assert_eq!(loaded.entries[1].task_id, "task-2");
}

// --- Spec ---

#[test]
fn spec_save_and_load() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let spec = "# My Project Spec\n\nBuild something cool.";
    state.save_spec("run-1", spec).unwrap();

    let loaded = state.load_spec("run-1").unwrap();
    assert_eq!(loaded, spec);
}

// --- Heartbeat ---

#[test]
fn update_agent_heartbeat_sets_timestamp() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Running);
    state.save_agent("run-1", &agent).unwrap();

    // Agent starts with no heartbeat
    let loaded = state.load_agent("run-1", "agent-1").unwrap();
    assert!(loaded.heartbeat.is_none());

    // Update heartbeat
    state.update_agent_heartbeat("run-1", "agent-1").unwrap();

    let loaded = state.load_agent("run-1", "agent-1").unwrap();
    assert!(loaded.heartbeat.is_some());
}

// --- Path helpers ---

// --- Config ---

#[test]
fn load_config_returns_defaults_when_no_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let config = state.load_config();
    assert_eq!(config.stall_timeout_seconds, 300);
}

#[test]
fn load_config_reads_stall_timeout() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "# Hive configuration\nstall_timeout_seconds: 600\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(config.stall_timeout_seconds, 600);
}

#[test]
fn load_config_handles_malformed_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "garbage content\n").unwrap();
    let config = state.load_config();
    assert_eq!(config.stall_timeout_seconds, 300);
}

// --- Path helpers ---

#[test]
fn path_structure_is_correct() {
    let state = HiveState::new("/tmp/myrepo".into());
    assert_eq!(
        state.hive_dir(),
        std::path::PathBuf::from("/tmp/myrepo/.hive")
    );
    assert_eq!(
        state.run_dir("run-1"),
        std::path::PathBuf::from("/tmp/myrepo/.hive/runs/run-1")
    );
    assert_eq!(
        state.worktree_path("run-1", "agent-1"),
        std::path::PathBuf::from("/tmp/myrepo/.hive/runs/run-1/worktrees/agent-1")
    );
}

// --- Run Metadata ---

#[test]
fn load_run_metadata_returns_metadata() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let meta = state.load_run_metadata("run-1").unwrap();
    assert_eq!(meta.id, "run-1");
    assert_eq!(meta.status, RunStatus::Active);
}

#[test]
fn load_run_metadata_nonexistent_run_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    assert!(state.load_run_metadata("no-such-run").is_err());
}

// --- load_messages_for_agent ---

#[test]
fn load_messages_for_agent_filters_by_recipient() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let now = chrono::Utc::now();
    state
        .save_message("run-1", &make_message("msg-1", "lead-1", "worker-1", now))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-2", "lead-1", "worker-2", now))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-3", "coord", "worker-1", now))
        .unwrap();

    let msgs = state
        .load_messages_for_agent("run-1", "worker-1", None)
        .unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(msgs.iter().all(|m| m.to == "worker-1"));
}

#[test]
fn load_messages_for_agent_filters_by_since() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let t1 = "2026-03-08T10:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let t2 = "2026-03-08T11:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let t3 = "2026-03-08T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();

    state
        .save_message("run-1", &make_message("msg-1", "a", "worker-1", t1))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-2", "a", "worker-1", t2))
        .unwrap();
    state
        .save_message("run-1", &make_message("msg-3", "a", "worker-1", t3))
        .unwrap();

    let msgs = state
        .load_messages_for_agent("run-1", "worker-1", Some(t1))
        .unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].id, "msg-2");
    assert_eq!(msgs[1].id, "msg-3");
}

#[test]
fn load_config_reads_verify_command() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "stall_timeout_seconds: 300\nverify_command: cargo test --all-targets\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(
        config.verify_command.as_deref(),
        Some("cargo test --all-targets")
    );
}

#[test]
fn load_config_verify_command_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "stall_timeout_seconds: 300\n",
    )
    .unwrap();
    let config = state.load_config();
    assert!(config.verify_command.is_none());
}

#[test]
fn load_config_verify_command_with_ampersand() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "verify_command: cargo test && cargo clippy\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(
        config.verify_command.as_deref(),
        Some("cargo test && cargo clippy")
    );
}

#[test]
fn load_config_verify_command_with_quotes() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "verify_command: \"cargo test && cargo clippy\"\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(
        config.verify_command.as_deref(),
        Some("cargo test && cargo clippy")
    );
}

#[test]
fn load_messages_for_agent_returns_empty_when_none_match() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let now = chrono::Utc::now();
    state
        .save_message("run-1", &make_message("msg-1", "a", "worker-2", now))
        .unwrap();

    let msgs = state
        .load_messages_for_agent("run-1", "worker-1", None)
        .unwrap();
    assert!(msgs.is_empty());
}

// --- max_retries config ---

#[test]
fn load_config_reads_max_retries() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "max_retries: 3\n").unwrap();
    let config = state.load_config();
    assert_eq!(config.max_retries, 3);
}

#[test]
fn load_config_max_retries_defaults_to_2() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "stall_timeout_seconds: 300\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(config.max_retries, 2);
}

// --- retry_count ---

// --- lock_file ---

#[test]
fn lock_file_creates_lockfile() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let _lock = state.lock_file("test-lock").unwrap();
    assert!(state.hive_dir().join("test-lock.lock").exists());
}

#[test]
fn lock_file_prevents_concurrent_access() {
    use std::sync::{Arc, Mutex};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();

    // Create .hive dir
    std::fs::create_dir_all(root.join(".hive")).unwrap();

    let counter = Arc::new(Mutex::new(0u32));
    let max_concurrent = Arc::new(Mutex::new(0u32));

    let mut handles = vec![];
    for _ in 0..4 {
        let root = root.clone();
        let counter = Arc::clone(&counter);
        let max_concurrent = Arc::clone(&max_concurrent);
        handles.push(std::thread::spawn(move || {
            let state = HiveState::new(root);
            let _lock = state.lock_file("concurrent").unwrap();

            // Increment counter (simulates entering critical section)
            let mut c = counter.lock().unwrap();
            *c += 1;
            let current = *c;
            let mut max = max_concurrent.lock().unwrap();
            if current > *max {
                *max = current;
            }
            drop(max);
            drop(c);

            // Small sleep to allow overlap if locking is broken
            std::thread::sleep(std::time::Duration::from_millis(10));

            // Decrement counter (simulates leaving critical section)
            let mut c = counter.lock().unwrap();
            *c -= 1;
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // If locking works, max concurrent should be 1
    assert_eq!(*max_concurrent.lock().unwrap(), 1);
}

// --- atomic_write ---

#[test]
fn atomic_write_produces_valid_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.json");
    atomic_write(&path, "hello world").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn atomic_write_doesnt_corrupt_on_partial_failure() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("original.json");

    // Write initial content
    std::fs::write(&path, "original content").unwrap();

    // Attempt atomic_write to a path where the tmp file can't be created
    // (nonexistent parent directory)
    let bad_path = dir.path().join("nonexistent").join("file.json");
    assert!(atomic_write(&bad_path, "new content").is_err());

    // Original file should still be intact
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "original content");
}

#[test]
fn agent_retry_count_serialization_roundtrip() {
    let agent = Agent {
        id: "agent-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 1,
        model: None,
        branched_from: None,
        wait_cursor: None,
    };
    let json = serde_json::to_string(&agent).unwrap();
    let back: Agent = serde_json::from_str(&json).unwrap();
    assert_eq!(back.retry_count, 1);
}

// --- budget_usd config ---

#[test]
fn load_config_reads_budget_usd() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "budget_usd: 25.0\n").unwrap();
    let config = state.load_config();
    assert_eq!(config.budget_usd, Some(25.0));
}

#[test]
fn load_config_budget_usd_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "stall_timeout_seconds: 300\n",
    )
    .unwrap();
    let config = state.load_config();
    assert!(config.budget_usd.is_none());
}

#[test]
fn load_config_budget_usd_ignores_invalid() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "budget_usd: not-a-number\n",
    )
    .unwrap();
    let config = state.load_config();
    assert!(config.budget_usd.is_none());
}

// --- Memory CRUD ---

fn make_operation(run_id: &str, tasks_total: u32) -> OperationalEntry {
    OperationalEntry {
        run_id: run_id.into(),
        created_at: chrono::Utc::now(),
        tasks_total,
        tasks_failed: 0,
        agents_spawned: 2,
        total_cost_usd: 1.50,
        learnings: vec!["test learning".into()],
        spec_quality: "good".into(),
        team_sizing: "appropriate".into(),
    }
}

fn make_failure(pattern: &str) -> FailureEntry {
    FailureEntry {
        run_id: "run-test".into(),
        created_at: chrono::Utc::now(),
        pattern: pattern.into(),
        context: "test context".into(),
        run_number: 1,
    }
}

#[test]
fn test_save_and_load_operations() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_operation(&make_operation("run-1", 5)).unwrap();
    state.save_operation(&make_operation("run-2", 3)).unwrap();
    let ops = state.load_operations();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].run_id, "run-1");
    assert_eq!(ops[0].tasks_total, 5);
    assert_eq!(ops[1].run_id, "run-2");
}

#[test]
fn test_save_and_load_failures() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_failure(&make_failure("timeout")).unwrap();
    state.save_failure(&make_failure("oom")).unwrap();
    let fails = state.load_failures();
    assert_eq!(fails.len(), 2);
    assert_eq!(fails[0].pattern, "timeout");
    assert_eq!(fails[1].pattern, "oom");
}

#[test]
fn test_save_and_load_conventions() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_conventions("Use snake_case.").unwrap();
    let conv = state.load_conventions();
    assert_eq!(conv, "Use snake_case.");
}

#[test]
fn test_load_operations_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let ops = state.load_operations();
    assert!(ops.is_empty());
}

#[test]
fn test_load_failures_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let fails = state.load_failures();
    assert!(fails.is_empty());
}

#[test]
fn test_load_conventions_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let conv = state.load_conventions();
    assert!(conv.is_empty());
}

#[test]
fn test_prune_operations() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    for i in 0..15 {
        state
            .save_operation(&make_operation(&format!("run-{i}"), i))
            .unwrap();
    }
    assert_eq!(state.load_operations().len(), 15);
    state.prune_memory().unwrap();
    let ops = state.load_operations();
    assert_eq!(ops.len(), 10);
    assert_eq!(ops[0].run_id, "run-5");
    assert_eq!(ops[9].run_id, "run-14");
}

#[test]
fn test_prune_failures() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    for i in 0..35 {
        state
            .save_failure(&make_failure(&format!("pattern-{i}")))
            .unwrap();
    }
    assert_eq!(state.load_failures().len(), 35);
    state.prune_memory().unwrap();
    let fails = state.load_failures();
    assert_eq!(fails.len(), 30);
    assert_eq!(fails[0].pattern, "pattern-5");
    assert_eq!(fails[29].pattern, "pattern-34");
}

#[test]
fn test_load_memory_for_prompt_coordinator() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_operation(&make_operation("run-1", 5)).unwrap();
    let prompt = state.load_memory_for_prompt(&AgentRole::Coordinator);
    assert!(prompt.contains("## Project Memory"));
    assert!(prompt.contains("### Recent Operations"));
    assert!(prompt.contains("run-1"));
}

#[test]
fn test_load_memory_for_prompt_worker() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_conventions("Use snake_case.").unwrap();
    state.save_failure(&make_failure("timeout")).unwrap();
    let prompt = state.load_memory_for_prompt(&AgentRole::Worker);
    assert!(prompt.contains("### Conventions"));
    assert!(prompt.contains("Use snake_case."));
    assert!(prompt.contains("### Known Failure Patterns"));
    assert!(prompt.contains("timeout"));
    assert!(!prompt.contains("### Recent Operations"));
}

#[test]
fn test_load_memory_for_prompt_postmortem() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_operation(&make_operation("run-1", 5)).unwrap();
    let prompt = state.load_memory_for_prompt(&AgentRole::Postmortem);
    assert!(prompt.is_empty());
}

#[test]
fn test_load_memory_for_prompt_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let prompt = state.load_memory_for_prompt(&AgentRole::Coordinator);
    assert!(prompt.is_empty());
}

#[test]
fn test_spec_save_and_load() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    state.save_spec("run-1", "# My Spec\nDo stuff.").unwrap();
    let loaded = state.load_spec("run-1");
    assert_eq!(loaded.as_deref().ok(), Some("# My Spec\nDo stuff."));
}

#[test]
fn total_run_cost_sums_agent_costs() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Create two agents
    let agent1 = make_agent("agent-1", AgentRole::Worker, AgentStatus::Done);
    let agent2 = make_agent("agent-2", AgentRole::Worker, AgentStatus::Done);
    state.save_agent("run-1", &agent1).unwrap();
    state.save_agent("run-1", &agent2).unwrap();

    // Write output.jsonl for agent-1: 1000 input, 500 output
    let output1 = "{\"type\":\"assistant\",\"message\":\"hello\"}\n{\"num_input_tokens\": 1000, \"num_output_tokens\": 500, \"session_duration_seconds\": 60}\n";
    std::fs::write(
        state
            .agents_dir("run-1")
            .join("agent-1")
            .join("output.jsonl"),
        output1,
    )
    .unwrap();

    // Write output.jsonl for agent-2: 2000 input, 1000 output
    let output2 = "{\"type\":\"assistant\",\"message\":\"hello\"}\n{\"num_input_tokens\": 2000, \"num_output_tokens\": 1000, \"session_duration_seconds\": 120}\n";
    std::fs::write(
        state
            .agents_dir("run-1")
            .join("agent-2")
            .join("output.jsonl"),
        output2,
    )
    .unwrap();

    let total = state.total_run_cost("run-1");

    // agent-1: 1000 * 15.0 / 1_000_000 + 500 * 75.0 / 1_000_000 = 0.015 + 0.0375 = 0.0525
    // agent-2: 2000 * 15.0 / 1_000_000 + 1000 * 75.0 / 1_000_000 = 0.03 + 0.075 = 0.105
    // total: 0.1575
    let expected = 0.0525 + 0.105;
    assert!(
        (total - expected).abs() < 1e-10,
        "Expected {expected}, got {total}"
    );
}

// --- Hive Mind ---

fn make_discovery(id: &str, content: &str) -> Discovery {
    Discovery {
        id: id.into(),
        run_id: "run-1".into(),
        agent_id: "explorer-1".into(),
        timestamp: chrono::Utc::now(),
        content: content.into(),
        file_paths: vec![],
        confidence: Confidence::Medium,
        tags: vec![],
    }
}

fn make_insight(id: &str, content: &str) -> Insight {
    Insight {
        id: id.into(),
        run_id: "run-1".into(),
        timestamp: chrono::Utc::now(),
        content: content.into(),
        discovery_ids: vec!["disc-1".into()],
        tags: vec![],
    }
}

#[test]
fn test_mind_dir_path() {
    let state = HiveState::new("/tmp/myrepo".into());
    assert_eq!(
        state.mind_dir("run-1"),
        std::path::PathBuf::from("/tmp/myrepo/.hive/runs/run-1/mind")
    );
}

#[test]
fn test_save_load_discoveries() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let d1 = make_discovery("disc-1", "Found pattern A");
    let d2 = make_discovery("disc-2", "Found pattern B");
    state.save_discovery("run-1", &d1).unwrap();
    state.save_discovery("run-1", &d2).unwrap();

    let loaded = state.load_discoveries("run-1");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, "disc-1");
    assert_eq!(loaded[0].content, "Found pattern A");
    assert_eq!(loaded[1].id, "disc-2");
}

#[test]
fn test_save_load_insights() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let i1 = make_insight("ins-1", "Key insight A");
    let i2 = make_insight("ins-2", "Key insight B");
    state.save_insight("run-1", &i1).unwrap();
    state.save_insight("run-1", &i2).unwrap();

    let loaded = state.load_insights("run-1");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, "ins-1");
    assert_eq!(loaded[0].content, "Key insight A");
    assert_eq!(loaded[1].id, "ins-2");
}

#[test]
fn test_query_mind_matches_content() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    state
        .save_discovery(
            "run-1",
            &make_discovery("disc-1", "Found a caching pattern"),
        )
        .unwrap();
    state
        .save_discovery("run-1", &make_discovery("disc-2", "Database optimization"))
        .unwrap();

    let result = state.query_mind("run-1", "caching");
    assert_eq!(result.discoveries.len(), 1);
    assert_eq!(result.discoveries[0].id, "disc-1");
}

#[test]
fn test_query_mind_matches_tags() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let mut d = make_discovery("disc-1", "Some content");
    d.tags = vec!["architecture".into(), "performance".into()];
    state.save_discovery("run-1", &d).unwrap();

    let result = state.query_mind("run-1", "architecture");
    assert_eq!(result.discoveries.len(), 1);
    assert_eq!(result.discoveries[0].id, "disc-1");
}

#[test]
fn test_query_mind_matches_file_paths() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let mut d = make_discovery("disc-1", "Some content");
    d.file_paths = vec!["src/main.rs".into()];
    state.save_discovery("run-1", &d).unwrap();

    let result = state.query_mind("run-1", "main.rs");
    assert_eq!(result.discoveries.len(), 1);
    assert_eq!(result.discoveries[0].id, "disc-1");
}

#[test]
fn test_query_mind_matches_insights() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    state
        .save_insight("run-1", &make_insight("ins-1", "Caching is essential"))
        .unwrap();
    state
        .save_insight("run-1", &make_insight("ins-2", "Database needs indexing"))
        .unwrap();

    let result = state.query_mind("run-1", "caching");
    assert_eq!(result.insights.len(), 1);
    assert_eq!(result.insights[0].id, "ins-1");
}

#[test]
fn test_query_mind_empty_results() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    state
        .save_discovery("run-1", &make_discovery("disc-1", "Some content"))
        .unwrap();

    let result = state.query_mind("run-1", "nonexistent");
    assert!(result.discoveries.is_empty());
    assert!(result.insights.is_empty());
}

#[test]
fn test_query_mind_case_insensitive() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    state
        .save_discovery(
            "run-1",
            &make_discovery("disc-1", "Found a CACHING pattern"),
        )
        .unwrap();

    let result = state.query_mind("run-1", "caching");
    assert_eq!(result.discoveries.len(), 1);

    let result = state.query_mind("run-1", "CACHING");
    assert_eq!(result.discoveries.len(), 1);
}

#[test]
fn test_memory_prompt_explorer_gets_conventions_and_failures() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_conventions("Use snake_case.").unwrap();
    state.save_failure(&make_failure("timeout")).unwrap();
    let prompt = state.load_memory_for_prompt(&AgentRole::Explorer);
    assert!(prompt.contains("### Conventions"));
    assert!(prompt.contains("Use snake_case."));
    assert!(prompt.contains("### Known Failure Patterns"));
    assert!(prompt.contains("timeout"));
    assert!(!prompt.contains("### Recent Operations"));
}

#[test]
fn test_memory_prompt_evaluator_gets_conventions_and_failures() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.save_conventions("Use snake_case.").unwrap();
    state.save_failure(&make_failure("timeout")).unwrap();
    let prompt = state.load_memory_for_prompt(&AgentRole::Evaluator);
    assert!(prompt.contains("### Conventions"));
    assert!(prompt.contains("Use snake_case."));
    assert!(prompt.contains("### Known Failure Patterns"));
    assert!(prompt.contains("timeout"));
    assert!(!prompt.contains("### Recent Operations"));
}

// =================================================================
// Adversarial tests: corrupted state, malformed JSON, edge cases
// =================================================================

#[test]
fn load_task_with_corrupted_json_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Write garbage to a task file
    let task_path = state.tasks_dir("run-1").join("task-corrupt.json");
    std::fs::write(&task_path, "{{not valid json}}").unwrap();

    let result = state.load_task("run-1", "task-corrupt");
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("task-corrupt"),
        "Error should mention the task ID: {err_msg}"
    );
}

#[test]
fn list_tasks_with_one_corrupted_file_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Save a valid task
    state
        .save_task("run-1", &make_task("task-good", TaskStatus::Pending))
        .unwrap();

    // Write a corrupted task file
    let task_path = state.tasks_dir("run-1").join("task-bad.json");
    std::fs::write(&task_path, "not json").unwrap();

    // list_tasks should fail because it doesn't skip corrupted files
    let result = state.list_tasks("run-1");
    assert!(
        result.is_err(),
        "list_tasks should propagate JSON parse errors"
    );
}

#[test]
fn load_agent_with_corrupted_json_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Create agent dir with corrupted agent.json
    let agent_dir = state.agents_dir("run-1").join("agent-corrupt");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("agent.json"), "CORRUPTED").unwrap();

    let result = state.load_agent("run-1", "agent-corrupt");
    assert!(result.is_err());
}

#[test]
fn list_agents_with_one_corrupted_file_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Save a valid agent
    state
        .save_agent(
            "run-1",
            &make_agent("agent-good", AgentRole::Worker, AgentStatus::Running),
        )
        .unwrap();

    // Create corrupted agent
    let agent_dir = state.agents_dir("run-1").join("agent-bad");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("agent.json"), "not json").unwrap();

    let result = state.list_agents("run-1");
    assert!(
        result.is_err(),
        "list_agents should propagate JSON parse errors"
    );
}

#[test]
fn list_messages_with_corrupted_file_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let msg_path = state.messages_dir("run-1").join("msg-bad.json");
    std::fs::write(&msg_path, "{{bad json}}").unwrap();

    let result = state.list_messages("run-1");
    assert!(result.is_err());
}

#[test]
fn load_merge_queue_with_corrupted_json() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Overwrite merge-queue.json with garbage
    let queue_path = state.run_dir("run-1").join("merge-queue.json");
    std::fs::write(&queue_path, "GARBAGE").unwrap();

    let result = state.load_merge_queue("run-1");
    assert!(result.is_err());
}

#[test]
fn load_run_metadata_with_corrupted_json() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Overwrite run.json with garbage
    let run_path = state.run_dir("run-1").join("run.json");
    std::fs::write(&run_path, "not-json").unwrap();

    let result = state.load_run_metadata("run-1");
    assert!(result.is_err());
}

#[test]
fn load_task_nonexistent_run_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let result = state.load_task("nonexistent-run", "task-1");
    assert!(result.is_err());
}

#[test]
fn load_agent_nonexistent_run_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let result = state.load_agent("nonexistent-run", "agent-1");
    assert!(result.is_err());
}

#[test]
fn update_heartbeat_nonexistent_agent_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let result = state.update_agent_heartbeat("run-1", "ghost-agent");
    assert!(result.is_err());
}

#[test]
fn load_spec_nonexistent_fails() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    // No spec saved
    let result = state.load_spec("run-1");
    assert!(result.is_err());
}

#[test]
fn load_spec_nonexistent_returns_err() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    assert!(state.load_spec("run-1").is_err());
}

// --- Special characters in IDs ---

#[test]
fn task_with_special_chars_in_id() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Forward slashes in task IDs would create subdirectories
    let task = make_task("task/with/slashes", TaskStatus::Pending);
    let result = state.save_task("run-1", &task);
    // This might succeed or fail depending on OS filesystem behavior
    // On Unix, it would try to create subdirectories
    // The important thing is it doesn't panic
    if result.is_ok() {
        // If save succeeds, load should also succeed
        let loaded = state.load_task("run-1", "task/with/slashes");
        assert!(loaded.is_ok());
    }
}

#[test]
fn agent_with_dot_dot_in_id() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Path traversal attempt in agent ID
    let agent = make_agent("../../etc/passwd", AgentRole::Worker, AgentStatus::Running);
    // This creates a directory at agents/../../etc/passwd/ which is actually
    // a traversal. The test verifies the code doesn't prevent this.
    let result = state.save_agent("run-1", &agent);
    // Whether this succeeds depends on the path resolution, but it shouldn't panic.
    let _ = result;
}

#[test]
fn task_with_empty_id() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let task = make_task("", TaskStatus::Pending);
    // Empty ID creates ".json" file — save should succeed
    let result = state.save_task("run-1", &task);
    assert!(result.is_ok());

    let loaded = state.load_task("run-1", "");
    assert!(loaded.is_ok());
    assert_eq!(loaded.unwrap().id, "");
}

#[test]
fn agent_with_empty_id() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("", AgentRole::Worker, AgentStatus::Running);
    // Empty ID creates agents// directory
    let result = state.save_agent("run-1", &agent);
    // On most systems this will create an agent at agents//agent.json
    // which resolves to agents/agent.json
    let _ = result;
}

// --- Config edge cases ---

#[test]
fn load_config_with_negative_stall_timeout() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "stall_timeout_seconds: -100\n",
    )
    .unwrap();
    let config = state.load_config();
    // Negative value parses fine into i64
    assert_eq!(config.stall_timeout_seconds, -100);
}

#[test]
fn load_config_with_zero_max_retries() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "max_retries: 0\n").unwrap();
    let config = state.load_config();
    assert_eq!(config.max_retries, 0);
}

#[test]
fn load_config_with_empty_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "").unwrap();
    let config = state.load_config();
    // Should return all defaults
    assert_eq!(config.stall_timeout_seconds, 300);
    assert_eq!(config.max_retries, 2);
    assert!(config.budget_usd.is_none());
    assert!(config.verify_command.is_none());
}

#[test]
fn load_config_with_binary_content() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        [0xFF, 0xFE, 0x00, 0x01, 0x02],
    )
    .unwrap();
    // Should not panic, just return defaults (file may or may not be readable as UTF-8)
    let config = state.load_config();
    assert_eq!(config.stall_timeout_seconds, 300);
}

#[test]
fn load_config_with_negative_budget() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "budget_usd: -50.0\n").unwrap();
    let config = state.load_config();
    // Negative budget parses fine — no validation
    assert_eq!(config.budget_usd, Some(-50.0));
}

#[test]
fn load_config_verify_command_empty_string_treated_as_none() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(state.hive_dir().join("config.yaml"), "verify_command: \n").unwrap();
    let config = state.load_config();
    assert!(config.verify_command.is_none());
}

// --- JSONL corruption resilience ---

#[test]
fn load_operations_with_corrupted_lines_skips_bad() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::create_dir_all(state.memory_dir()).unwrap();

    let valid_op = serde_json::to_string(&make_operation("run-1", 5)).unwrap();
    let content = format!("{valid_op}\nBAD LINE\n{valid_op}\n\n");
    std::fs::write(state.memory_dir().join("operations.jsonl"), &content).unwrap();

    let ops = state.load_operations();
    // filter_map(|l| serde_json::from_str(l).ok()) skips bad lines
    assert_eq!(ops.len(), 2);
}

#[test]
fn load_failures_with_corrupted_lines_skips_bad() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::create_dir_all(state.memory_dir()).unwrap();

    let valid_fail = serde_json::to_string(&make_failure("pattern-1")).unwrap();
    let content = format!("{valid_fail}\n{{bad}}\n{valid_fail}\n");
    std::fs::write(state.memory_dir().join("failures.jsonl"), &content).unwrap();

    let fails = state.load_failures();
    assert_eq!(fails.len(), 2);
}

#[test]
fn load_discoveries_with_corrupted_lines_skips_bad() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    std::fs::create_dir_all(state.mind_dir("run-1")).unwrap();

    let valid_disc = serde_json::to_string(&make_discovery("d1", "Found something")).unwrap();
    let content = format!("{valid_disc}\nGARBAGE\n{valid_disc}\n");
    std::fs::write(state.mind_dir("run-1").join("discoveries.jsonl"), &content).unwrap();

    let discoveries = state.load_discoveries("run-1");
    assert_eq!(discoveries.len(), 2);
}

#[test]
fn load_insights_with_corrupted_lines_skips_bad() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    std::fs::create_dir_all(state.mind_dir("run-1")).unwrap();

    let valid_ins = serde_json::to_string(&make_insight("i1", "Key insight")).unwrap();
    let content = format!("{valid_ins}\n{{broken json\n{valid_ins}\n");
    std::fs::write(state.mind_dir("run-1").join("insights.jsonl"), &content).unwrap();

    let insights = state.load_insights("run-1");
    assert_eq!(insights.len(), 2);
}

// --- Cost tracking edge cases ---

#[test]
fn load_agent_cost_empty_output_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Done);
    state.save_agent("run-1", &agent).unwrap();

    // Empty output.jsonl
    std::fs::write(
        state
            .agents_dir("run-1")
            .join("agent-1")
            .join("output.jsonl"),
        "",
    )
    .unwrap();

    let cost = state.load_agent_cost("run-1", "agent-1");
    assert!(cost.is_none());
}

#[test]
fn load_agent_cost_no_token_lines() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Done);
    state.save_agent("run-1", &agent).unwrap();

    // output.jsonl with no token count lines
    std::fs::write(
        state
            .agents_dir("run-1")
            .join("agent-1")
            .join("output.jsonl"),
        "{\"type\":\"assistant\",\"message\":\"hello\"}\n{\"type\":\"done\"}\n",
    )
    .unwrap();

    let cost = state.load_agent_cost("run-1", "agent-1");
    assert!(cost.is_none());
}

#[test]
fn load_agent_cost_with_corrupted_output_lines() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Done);
    state.save_agent("run-1", &agent).unwrap();

    // Mix of valid and invalid JSON lines, with token data early
    let content =
        "not json at all\n{\"num_input_tokens\": 100, \"num_output_tokens\": 50}\nmore garbage\n";
    std::fs::write(
        state
            .agents_dir("run-1")
            .join("agent-1")
            .join("output.jsonl"),
        content,
    )
    .unwrap();

    // Should still find the valid token line
    let cost = state.load_agent_cost("run-1", "agent-1");
    assert!(cost.is_some());
    let cost = cost.unwrap();
    assert_eq!(cost.input_tokens, 100);
    assert_eq!(cost.output_tokens, 50);
}

#[test]
fn load_agent_cost_missing_output_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let agent = make_agent("agent-1", AgentRole::Worker, AgentStatus::Done);
    state.save_agent("run-1", &agent).unwrap();
    // Don't create output.jsonl

    let cost = state.load_agent_cost("run-1", "agent-1");
    assert!(cost.is_none());
}

#[test]
fn total_run_cost_nonexistent_run_returns_zero() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    let cost = state.total_run_cost("nonexistent-run");
    assert_eq!(cost, 0.0);
}

#[test]
fn total_run_cost_agents_with_no_output_returns_zero() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Save agents without output files
    state
        .save_agent(
            "run-1",
            &make_agent("agent-1", AgentRole::Worker, AgentStatus::Done),
        )
        .unwrap();

    let cost = state.total_run_cost("run-1");
    assert_eq!(cost, 0.0);
}

// --- list_runs edge cases ---

#[test]
fn list_runs_with_no_runs_dir() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    // Don't create any runs
    let runs = state.list_runs().unwrap();
    assert!(runs.is_empty());
}

#[test]
fn list_runs_skips_corrupted_run_json() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("good-run").unwrap();

    // Create a corrupted run
    let bad_run_dir = state.runs_dir().join("bad-run");
    std::fs::create_dir_all(&bad_run_dir).unwrap();
    std::fs::write(bad_run_dir.join("run.json"), "NOT JSON").unwrap();

    let runs = state.list_runs().unwrap();
    // list_runs uses `if let Ok(meta)` so it silently skips corrupted runs
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "good-run");
}

#[test]
fn list_runs_skips_dir_without_run_json() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("good-run").unwrap();

    // Create a dir without run.json
    std::fs::create_dir_all(state.runs_dir().join("empty-dir")).unwrap();

    let runs = state.list_runs().unwrap();
    assert_eq!(runs.len(), 1);
}

// --- query_mind edge cases ---

#[test]
fn query_mind_empty_query_matches_nothing() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    state
        .save_discovery("run-1", &make_discovery("d1", "some content"))
        .unwrap();

    let result = state.query_mind("run-1", "");
    // Empty query has no words, so nothing matches
    assert!(result.discoveries.is_empty());
}

#[test]
fn query_mind_nonexistent_run_returns_empty() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());

    let result = state.query_mind("nonexistent-run", "test");
    assert!(result.discoveries.is_empty());
    assert!(result.insights.is_empty());
}

// --- Non-JSON files in task/message dirs ---

#[test]
fn list_tasks_ignores_non_json_files() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    // Save a valid task
    state
        .save_task("run-1", &make_task("task-1", TaskStatus::Pending))
        .unwrap();

    // Add non-JSON files
    std::fs::write(state.tasks_dir("run-1").join("README.md"), "ignore me").unwrap();
    std::fs::write(state.tasks_dir("run-1").join(".gitkeep"), "").unwrap();
    std::fs::write(
        state.tasks_dir("run-1").join("task-1.json.tmp"),
        "temp file",
    )
    .unwrap();

    let tasks = state.list_tasks("run-1").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");
}

#[test]
fn list_messages_ignores_non_json_files() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let msg = make_message("msg-1", "a", "b", chrono::Utc::now());
    state.save_message("run-1", &msg).unwrap();

    // Add non-JSON files
    std::fs::write(state.messages_dir("run-1").join("not-a-message.txt"), "x").unwrap();

    let messages = state.list_messages("run-1").unwrap();
    assert_eq!(messages.len(), 1);
}

// --- Atomic write edge cases ---

#[test]
fn atomic_write_to_nonexistent_directory_fails() {
    let dir = TempDir::new().unwrap();
    let path = dir
        .path()
        .join("nonexistent")
        .join("deep")
        .join("file.json");
    let result = atomic_write(&path, "content");
    assert!(result.is_err());
}

#[test]
fn atomic_write_overwrites_existing_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.json");
    atomic_write(&path, "first").unwrap();
    atomic_write(&path, "second").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "second");
}

#[test]
fn atomic_write_with_empty_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.json");
    atomic_write(&path, "").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "");
}

#[test]
fn atomic_write_with_large_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("large.json");
    let large = "x".repeat(1_000_000);
    atomic_write(&path, &large).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content.len(), 1_000_000);
}

// --- Prune with no memory dir ---

#[test]
fn prune_memory_with_no_memory_dir_succeeds() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    // No memory dir exists
    let result = state.prune_memory();
    assert!(result.is_ok());
}

// --- Multiple create_run ---

#[test]
fn create_run_twice_with_same_id_overwrites() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();
    // Save a task
    state
        .save_task("run-1", &make_task("task-1", TaskStatus::Active))
        .unwrap();

    // Create same run again — it recreates the dirs but doesn't clear existing files
    state.create_run("run-1").unwrap();

    // The task file should still exist (create_run uses create_dir_all which is idempotent)
    let tasks = state.list_tasks("run-1").unwrap();
    assert_eq!(tasks.len(), 1);

    // But the run.json and merge-queue.json are overwritten
    let meta = state.load_run_metadata("run-1").unwrap();
    assert_eq!(meta.status, RunStatus::Active);
}

// --- load_messages_for_agent with corrupted messages ---

#[test]
fn load_messages_for_agent_fails_on_corrupted_file() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    state.create_run("run-1").unwrap();

    let msg_path = state.messages_dir("run-1").join("msg-bad.json");
    std::fs::write(&msg_path, "NOT JSON").unwrap();

    let result = state.load_messages_for_agent("run-1", "worker-1", None);
    assert!(result.is_err());
}

#[test]
fn load_config_parses_model_entries() {
    let dir = TempDir::new().unwrap();
    let state = HiveState::new(dir.path().to_path_buf());
    std::fs::create_dir_all(state.hive_dir()).unwrap();
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "model_worker: haiku\nmodel_lead: opus\nfallback_model: sonnet\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(
        config.models.model_for_role(AgentRole::Worker),
        ModelTier::Haiku
    );
    assert_eq!(
        config.models.model_for_role(AgentRole::Lead),
        ModelTier::Opus
    );
    // Unset roles fall back to defaults
    assert_eq!(
        config.models.model_for_role(AgentRole::Coordinator),
        ModelTier::Opus
    );
    assert_eq!(config.fallback_model, Some("sonnet".to_string()));
}

#[test]
fn load_config_ignores_invalid_model_names() {
    let dir = TempDir::new().unwrap();
    let state = HiveState::new(dir.path().to_path_buf());
    std::fs::create_dir_all(state.hive_dir()).unwrap();
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "model_worker: gpt4\nmodel_lead: sonnet\n",
    )
    .unwrap();
    let config = state.load_config();
    // gpt4 is invalid, should fall back to default (sonnet)
    assert_eq!(
        config.models.model_for_role(AgentRole::Worker),
        ModelTier::Sonnet
    );
    // sonnet is valid
    assert_eq!(
        config.models.model_for_role(AgentRole::Lead),
        ModelTier::Sonnet
    );
}

#[test]
fn resolve_model_priority_chain() {
    let dir = TempDir::new().unwrap();
    let state = HiveState::new(dir.path().to_path_buf());
    std::fs::create_dir_all(state.hive_dir()).unwrap();
    std::fs::write(
        state.hive_dir().join("config.yaml"),
        "model_worker: haiku\n",
    )
    .unwrap();
    let config = state.load_config();

    // Per-spawn override wins
    assert_eq!(
        config.resolve_model(AgentRole::Worker, Some("opus")),
        "opus"
    );
    // Role config wins over default
    assert_eq!(
        config.resolve_model(AgentRole::Worker, None),
        "claude-haiku-4-5"
    );
    // No override, no config → role default
    assert_eq!(
        config.resolve_model(AgentRole::Lead, None),
        "claude-sonnet-4-6"
    );
}

#[test]
fn load_config_worktree_strategies() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        dir.path().join(".hive/config.yaml"),
        "worktree_lead: full
worktree_worker: sparse
worktree_postmortem: no_checkout
",
    )
    .unwrap();
    let config = state.load_config();

    // Explicit config overrides
    assert_eq!(config.worktrees.lead, Some(WorktreeStrategy::Full));
    assert_eq!(
        config.worktrees.worker,
        Some(WorktreeStrategy::Sparse {
            paths: vec!["src".to_string()]
        })
    );
    assert_eq!(
        config.worktrees.postmortem,
        Some(WorktreeStrategy::NoCheckout)
    );

    // Unset fields remain None (resolved via default_for_role)
    assert!(config.worktrees.explorer.is_none());
}

#[test]
fn worktree_config_resolve_uses_config_then_default() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path());
    std::fs::write(
        dir.path().join(".hive/config.yaml"),
        "worktree_worker: full
",
    )
    .unwrap();
    let config = state.load_config();

    // Worker config overrides default (full instead of sparse)
    assert_eq!(
        config.worktrees.resolve(AgentRole::Worker),
        WorktreeStrategy::Full
    );

    // Lead has no config entry, falls back to role default (Full)
    assert_eq!(
        config.worktrees.resolve(AgentRole::Lead),
        WorktreeStrategy::Full
    );

    // Explorer has no config entry, falls back to role default (Sparse)
    assert_eq!(
        config.worktrees.resolve(AgentRole::Explorer),
        WorktreeStrategy::Sparse {
            paths: vec!["src".to_string()]
        }
    );
}

#[test]
fn load_config_global_worktree_strategy_full() {
    let dir = tempfile::tempdir().unwrap();
    let state = make_state(dir.path());
    fs::write(
        dir.path().join(".hive/config.yaml"),
        "worktree_strategy: full\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(config.global_worktree, Some(WorktreeStrategy::Full));
}

#[test]
fn load_config_global_worktree_strategy_sparse() {
    let dir = tempfile::tempdir().unwrap();
    let state = make_state(dir.path());
    fs::write(
        dir.path().join(".hive/config.yaml"),
        "worktree_strategy: sparse\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(
        config.global_worktree,
        Some(WorktreeStrategy::Sparse {
            paths: vec!["src".to_string()]
        })
    );
}

#[test]
fn load_config_global_worktree_strategy_auto_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let state = make_state(dir.path());
    fs::write(
        dir.path().join(".hive/config.yaml"),
        "worktree_strategy: auto\n",
    )
    .unwrap();
    let config = state.load_config();
    assert_eq!(config.global_worktree, None, "auto should resolve to None");
}

#[test]
fn load_config_global_worktree_absent_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let state = make_state(dir.path());
    fs::write(dir.path().join(".hive/config.yaml"), "budget_usd: 10.0\n").unwrap();
    let config = state.load_config();
    assert_eq!(
        config.global_worktree, None,
        "no worktree_strategy key → None"
    );
}
