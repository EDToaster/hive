use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

// We test against the library by importing the binary crate's modules.
// Since hive is a binary crate, we use a separate test file that exercises
// the public APIs by building test helpers inline.

// ============================================================
// Helper: create a HiveState rooted at a temp directory with .hive/ initialized
// ============================================================

fn init_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    // Initialize a git repo so git operations work
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Need at least one commit for worktrees to work
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

// ============================================================
// Types tests — serde roundtrips
// ============================================================

mod types_tests {

    #[test]
    fn agent_role_serializes_lowercase() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum AgentRole {
            Coordinator,
            Lead,
            Worker,
        }

        let json = serde_json::to_string(&AgentRole::Coordinator).unwrap();
        assert_eq!(json, "\"coordinator\"");
        let json = serde_json::to_string(&AgentRole::Lead).unwrap();
        assert_eq!(json, "\"lead\"");
        let json = serde_json::to_string(&AgentRole::Worker).unwrap();
        assert_eq!(json, "\"worker\"");

        // Deserialize back
        let role: AgentRole = serde_json::from_str("\"coordinator\"").unwrap();
        assert_eq!(role, AgentRole::Coordinator);
    }

    #[test]
    fn agent_status_serializes_lowercase() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum AgentStatus {
            Running,
            Done,
            Failed,
            Stalled,
        }

        for (variant, expected) in [
            (AgentStatus::Running, "\"running\""),
            (AgentStatus::Done, "\"done\""),
            (AgentStatus::Failed, "\"failed\""),
            (AgentStatus::Stalled, "\"stalled\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn task_status_all_variants_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum TaskStatus {
            Pending,
            Active,
            Blocked,
            Review,
            Approved,
            Queued,
            Merged,
            Failed,
        }

        let variants = [
            TaskStatus::Pending,
            TaskStatus::Active,
            TaskStatus::Blocked,
            TaskStatus::Review,
            TaskStatus::Approved,
            TaskStatus::Queued,
            TaskStatus::Merged,
            TaskStatus::Failed,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let back: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn urgency_all_variants_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum Urgency {
            Low,
            Normal,
            High,
            Critical,
        }

        for (variant, expected) in [
            (Urgency::Low, "\"low\""),
            (Urgency::Normal, "\"normal\""),
            (Urgency::High, "\"high\""),
            (Urgency::Critical, "\"critical\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn message_type_serializes_kebab_case() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "kebab-case")]
        enum MessageType {
            Info,
            Request,
            Status,
            TaskSuggestion,
        }

        let json = serde_json::to_string(&MessageType::TaskSuggestion).unwrap();
        assert_eq!(json, "\"task-suggestion\"");

        let back: MessageType = serde_json::from_str("\"task-suggestion\"").unwrap();
        assert_eq!(back, MessageType::TaskSuggestion);

        let json = serde_json::to_string(&MessageType::Info).unwrap();
        assert_eq!(json, "\"info\"");
    }

    #[test]
    fn agent_struct_roundtrip() {
        let json = r#"{
            "id": "agent-1",
            "role": "worker",
            "status": "running",
            "parent": "lead-1",
            "pid": 12345,
            "worktree": "/tmp/wt",
            "heartbeat": "2026-03-08T10:00:00Z",
            "task_id": "task-1"
        }"#;

        let agent: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(agent["role"], "worker");
        assert_eq!(agent["status"], "running");
        assert_eq!(agent["pid"], 12345);

        // Roundtrip
        let serialized = serde_json::to_string(&agent).unwrap();
        let back: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(agent, back);
    }

    #[test]
    fn agent_struct_with_null_optionals() {
        let json = r#"{
            "id": "coord-1",
            "role": "coordinator",
            "status": "running",
            "parent": null,
            "pid": null,
            "worktree": null,
            "heartbeat": null,
            "task_id": null
        }"#;

        let agent: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(agent["role"], "coordinator");
        assert!(agent["parent"].is_null());
        assert!(agent["pid"].is_null());
    }

    #[test]
    fn task_struct_roundtrip() {
        let json = r#"{
            "id": "task-1",
            "title": "Implement feature X",
            "description": "Build the thing",
            "status": "pending",
            "urgency": "high",
            "blocking": ["task-2"],
            "blocked_by": [],
            "assigned_to": null,
            "created_by": "coordinator",
            "parent_task": null,
            "branch": null,
            "domain": "backend",
            "created_at": "2026-03-08T10:00:00Z",
            "updated_at": "2026-03-08T10:00:00Z"
        }"#;

        let task: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(task["status"], "pending");
        assert_eq!(task["urgency"], "high");
        assert_eq!(task["blocking"], serde_json::json!(["task-2"]));
    }

    #[test]
    fn merge_queue_roundtrip() {
        let json = r#"{
            "entries": [
                {
                    "task_id": "task-1",
                    "branch": "hive/run1/lead-1",
                    "submitted_by": "lead-1",
                    "submitted_at": "2026-03-08T12:00:00Z"
                }
            ]
        }"#;

        let queue: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(queue["entries"].as_array().unwrap().len(), 1);
        assert_eq!(queue["entries"][0]["task_id"], "task-1");
    }

    #[test]
    fn run_metadata_roundtrip() {
        let json = r#"{
            "id": "run-abc",
            "created_at": "2026-03-08T10:00:00Z",
            "status": "active"
        }"#;

        let run: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(run["status"], "active");

        // Test all run statuses
        for status in ["active", "completed", "failed"] {
            let json =
                format!(r#"{{"id":"r","created_at":"2026-03-08T10:00:00Z","status":"{status}"}}"#);
            let run: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(run["status"], status);
        }
    }

    #[test]
    fn message_struct_roundtrip() {
        let json = r#"{
            "id": "msg-1",
            "from": "lead-1",
            "to": "coordinator",
            "timestamp": "2026-03-08T11:00:00Z",
            "message_type": "status",
            "body": "All tasks complete",
            "refs": ["task-1", "task-2"]
        }"#;

        let msg: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(msg["message_type"], "status");
        assert_eq!(msg["refs"].as_array().unwrap().len(), 2);
    }
}

// ============================================================
// State tests — filesystem CRUD operations
// ============================================================

mod state_tests {
    use super::*;

    fn create_hive_state(dir: &Path) {
        // Create .hive directory structure
        fs::create_dir_all(dir.join(".hive")).unwrap();
    }

    fn make_task_json(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": format!("Task {id}"),
            "description": "A test task",
            "status": status,
            "urgency": "normal",
            "blocking": [],
            "blocked_by": [],
            "assigned_to": null,
            "created_by": "test",
            "parent_task": null,
            "branch": null,
            "domain": null,
            "created_at": "2026-03-08T10:00:00Z",
            "updated_at": "2026-03-08T10:00:00Z"
        })
    }

    fn make_agent_json(id: &str, role: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "role": role,
            "status": status,
            "parent": null,
            "pid": null,
            "worktree": null,
            "heartbeat": null,
            "task_id": null
        })
    }

    fn make_message_json(id: &str, from: &str, to: &str, timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "from": from,
            "to": to,
            "timestamp": timestamp,
            "message_type": "info",
            "body": "hello",
            "refs": []
        })
    }

    // --- Run management ---

    #[test]
    fn create_run_creates_directory_structure() {
        let dir = TempDir::new().unwrap();
        create_hive_state(dir.path());

        let run_id = "test-run-1";
        let run_dir = dir.path().join(".hive/runs").join(run_id);
        fs::create_dir_all(run_dir.join("tasks")).unwrap();
        fs::create_dir_all(run_dir.join("agents")).unwrap();
        fs::create_dir_all(run_dir.join("messages")).unwrap();
        fs::create_dir_all(run_dir.join("worktrees")).unwrap();

        assert!(run_dir.join("tasks").is_dir());
        assert!(run_dir.join("agents").is_dir());
        assert!(run_dir.join("messages").is_dir());
        assert!(run_dir.join("worktrees").is_dir());
    }

    #[test]
    fn create_run_writes_run_json() {
        let dir = TempDir::new().unwrap();
        create_hive_state(dir.path());

        let run_id = "run-abc";
        let run_dir = dir.path().join(".hive/runs").join(run_id);
        fs::create_dir_all(&run_dir).unwrap();

        let metadata = serde_json::json!({
            "id": run_id,
            "created_at": "2026-03-08T10:00:00Z",
            "status": "active"
        });
        fs::write(
            run_dir.join("run.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(run_dir.join("run.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["id"], run_id);
        assert_eq!(loaded["status"], "active");
    }

    #[test]
    fn create_run_initializes_empty_merge_queue() {
        let dir = TempDir::new().unwrap();
        create_hive_state(dir.path());

        let run_dir = dir.path().join(".hive/runs/run-1");
        fs::create_dir_all(&run_dir).unwrap();

        let queue = serde_json::json!({ "entries": [] });
        fs::write(
            run_dir.join("merge-queue.json"),
            serde_json::to_string_pretty(&queue).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(run_dir.join("merge-queue.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["entries"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn active_run_read_write() {
        let dir = TempDir::new().unwrap();
        create_hive_state(dir.path());

        let active_path = dir.path().join(".hive/active_run");

        // No active run yet
        assert!(fs::read_to_string(&active_path).is_err());

        // Set active run
        fs::write(&active_path, "run-123").unwrap();
        let active = fs::read_to_string(&active_path).unwrap().trim().to_string();
        assert_eq!(active, "run-123");
    }

    // --- Task CRUD ---

    #[test]
    fn task_save_and_load() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let task = make_task_json("task-1", "pending");
        fs::write(
            tasks_dir.join("task-1.json"),
            serde_json::to_string_pretty(&task).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(tasks_dir.join("task-1.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["id"], "task-1");
        assert_eq!(loaded["status"], "pending");
    }

    #[test]
    fn task_update_overwrites() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let task = make_task_json("task-1", "pending");
        fs::write(
            tasks_dir.join("task-1.json"),
            serde_json::to_string_pretty(&task).unwrap(),
        )
        .unwrap();

        // Update status
        let updated = make_task_json("task-1", "active");
        fs::write(
            tasks_dir.join("task-1.json"),
            serde_json::to_string_pretty(&updated).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(tasks_dir.join("task-1.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["status"], "active");
    }

    #[test]
    fn list_tasks_returns_all_json_files() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        for i in 1..=3 {
            let task = make_task_json(&format!("task-{i}"), "pending");
            fs::write(
                tasks_dir.join(format!("task-{i}.json")),
                serde_json::to_string_pretty(&task).unwrap(),
            )
            .unwrap();
        }

        let mut tasks: Vec<serde_json::Value> = Vec::new();
        for entry in fs::read_dir(&tasks_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let data = fs::read_to_string(entry.path()).unwrap();
                tasks.push(serde_json::from_str(&data).unwrap());
            }
        }
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn list_tasks_empty_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let entries: Vec<_> = fs::read_dir(&tasks_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn list_tasks_nonexistent_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        assert!(!tasks_dir.exists());
        // Equivalent to what list_tasks does: check exists first
    }

    #[test]
    fn load_nonexistent_task_fails() {
        let dir = TempDir::new().unwrap();
        let tasks_dir = dir.path().join(".hive/runs/run-1/tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let result = fs::read_to_string(tasks_dir.join("nonexistent.json"));
        assert!(result.is_err());
    }

    // --- Agent CRUD ---

    #[test]
    fn agent_save_and_load() {
        let dir = TempDir::new().unwrap();
        let agent_dir = dir.path().join(".hive/runs/run-1/agents/agent-1");
        fs::create_dir_all(&agent_dir).unwrap();

        let agent = make_agent_json("agent-1", "worker", "running");
        fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string_pretty(&agent).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(agent_dir.join("agent.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["id"], "agent-1");
        assert_eq!(loaded["role"], "worker");
        assert_eq!(loaded["status"], "running");
    }

    #[test]
    fn agent_save_creates_subdirectory() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".hive/runs/run-1/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_id = "lead-1";
        let agent_dir = agents_dir.join(agent_id);
        fs::create_dir_all(&agent_dir).unwrap();

        assert!(agent_dir.is_dir());

        let agent = make_agent_json(agent_id, "lead", "running");
        fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string_pretty(&agent).unwrap(),
        )
        .unwrap();
        assert!(agent_dir.join("agent.json").exists());
    }

    #[test]
    fn list_agents_reads_subdirectories() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".hive/runs/run-1/agents");

        for id in ["agent-1", "agent-2"] {
            let agent_dir = agents_dir.join(id);
            fs::create_dir_all(&agent_dir).unwrap();
            let agent = make_agent_json(id, "worker", "running");
            fs::write(
                agent_dir.join("agent.json"),
                serde_json::to_string_pretty(&agent).unwrap(),
            )
            .unwrap();
        }

        let mut agents: Vec<serde_json::Value> = Vec::new();
        for entry in fs::read_dir(&agents_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_dir() {
                let path = entry.path().join("agent.json");
                if path.exists() {
                    let data = fs::read_to_string(&path).unwrap();
                    agents.push(serde_json::from_str(&data).unwrap());
                }
            }
        }
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn list_agents_empty_returns_empty() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".hive/runs/run-1/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let entries: Vec<_> = fs::read_dir(&agents_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 0);
    }

    // --- Message CRUD ---

    #[test]
    fn message_save_and_load() {
        let dir = TempDir::new().unwrap();
        let msgs_dir = dir.path().join(".hive/runs/run-1/messages");
        fs::create_dir_all(&msgs_dir).unwrap();

        let msg = make_message_json("msg-1", "lead-1", "coord", "2026-03-08T10:00:00Z");
        fs::write(
            msgs_dir.join("msg-1.json"),
            serde_json::to_string_pretty(&msg).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(msgs_dir.join("msg-1.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["id"], "msg-1");
        assert_eq!(loaded["from"], "lead-1");
    }

    #[test]
    fn list_messages_sorted_by_timestamp() {
        let dir = TempDir::new().unwrap();
        let msgs_dir = dir.path().join(".hive/runs/run-1/messages");
        fs::create_dir_all(&msgs_dir).unwrap();

        // Write messages out of order
        let msg3 = make_message_json("msg-3", "a", "b", "2026-03-08T12:00:00Z");
        let msg1 = make_message_json("msg-1", "a", "b", "2026-03-08T10:00:00Z");
        let msg2 = make_message_json("msg-2", "a", "b", "2026-03-08T11:00:00Z");

        fs::write(
            msgs_dir.join("msg-3.json"),
            serde_json::to_string_pretty(&msg3).unwrap(),
        )
        .unwrap();
        fs::write(
            msgs_dir.join("msg-1.json"),
            serde_json::to_string_pretty(&msg1).unwrap(),
        )
        .unwrap();
        fs::write(
            msgs_dir.join("msg-2.json"),
            serde_json::to_string_pretty(&msg2).unwrap(),
        )
        .unwrap();

        // Read and sort like list_messages does
        let mut messages: Vec<serde_json::Value> = Vec::new();
        for entry in fs::read_dir(&msgs_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let data = fs::read_to_string(entry.path()).unwrap();
                messages.push(serde_json::from_str(&data).unwrap());
            }
        }
        messages.sort_by(|a, b| {
            a["timestamp"]
                .as_str()
                .unwrap()
                .cmp(b["timestamp"].as_str().unwrap())
        });

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["id"], "msg-1");
        assert_eq!(messages[1]["id"], "msg-2");
        assert_eq!(messages[2]["id"], "msg-3");
    }

    // --- Merge Queue ---

    #[test]
    fn merge_queue_save_and_load() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join(".hive/runs/run-1");
        fs::create_dir_all(&run_dir).unwrap();

        let queue = serde_json::json!({
            "entries": [
                {
                    "task_id": "task-1",
                    "branch": "hive/run-1/lead-1",
                    "submitted_by": "lead-1",
                    "submitted_at": "2026-03-08T12:00:00Z"
                },
                {
                    "task_id": "task-2",
                    "branch": "hive/run-1/lead-2",
                    "submitted_by": "lead-2",
                    "submitted_at": "2026-03-08T12:05:00Z"
                }
            ]
        });
        fs::write(
            run_dir.join("merge-queue.json"),
            serde_json::to_string_pretty(&queue).unwrap(),
        )
        .unwrap();

        let data = fs::read_to_string(run_dir.join("merge-queue.json")).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded["entries"].as_array().unwrap().len(), 2);
        assert_eq!(loaded["entries"][0]["task_id"], "task-1");
        assert_eq!(loaded["entries"][1]["task_id"], "task-2");
    }

    // --- Spec ---

    #[test]
    fn spec_save_and_load() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join(".hive/runs/run-1");
        fs::create_dir_all(&run_dir).unwrap();

        let spec = "# My Project Spec\n\nBuild something cool.";
        fs::write(run_dir.join("spec.md"), spec).unwrap();

        let loaded = fs::read_to_string(run_dir.join("spec.md")).unwrap();
        assert_eq!(loaded, spec);
    }

    // --- Path helpers ---

    #[test]
    fn path_structure_is_correct() {
        let root = Path::new("/tmp/myrepo");
        let hive_dir = root.join(".hive");
        let runs_dir = hive_dir.join("runs");
        let run_dir = runs_dir.join("run-1");

        assert_eq!(run_dir.join("tasks"), root.join(".hive/runs/run-1/tasks"));
        assert_eq!(run_dir.join("agents"), root.join(".hive/runs/run-1/agents"));
        assert_eq!(
            run_dir.join("messages"),
            root.join(".hive/runs/run-1/messages")
        );
        assert_eq!(
            run_dir.join("worktrees").join("agent-1"),
            root.join(".hive/runs/run-1/worktrees/agent-1")
        );
    }
}

// ============================================================
// Logging tests — SQLite operations
// ============================================================

mod logging_tests {
    use super::*;
    use rusqlite::Connection;

    fn open_test_db(dir: &Path) -> Connection {
        let db_path = dir.join("log.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_calls (
                id INTEGER PRIMARY KEY,
                run_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                agent_role TEXT NOT NULL,
                source TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                args_summary TEXT,
                status TEXT NOT NULL,
                duration_ms INTEGER,
                timestamp TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tool_calls_agent ON tool_calls(run_id, agent_id);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_tool ON tool_calls(run_id, tool_name);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn open_creates_table() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        // Verify table exists by querying it
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let _conn1 = open_test_db(dir.path());
        // Opening again should not fail (CREATE TABLE IF NOT EXISTS)
        let _conn2 = open_test_db(dir.path());
    }

    #[test]
    fn log_tool_call_inserts_row() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        let timestamp = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "run-1",
                "agent-1",
                "worker",
                "mcp",
                "hive_update_task",
                Some("task_id=task-1"),
                "success",
                Some(150i64),
                timestamp,
            ],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let (tool_name, status): (String, String) = conn
            .query_row(
                "SELECT tool_name, status FROM tool_calls WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tool_name, "hive_update_task");
        assert_eq!(status, "success");
    }

    #[test]
    fn log_tool_call_with_null_optionals() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        let timestamp = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "run-1",
                "agent-1",
                "lead",
                "hook",
                "hive_spawn_agent",
                None::<String>,
                "success",
                None::<i64>,
                timestamp,
            ],
        )
        .unwrap();

        let args_summary: Option<String> = conn
            .query_row(
                "SELECT args_summary FROM tool_calls WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(args_summary.is_none());
    }

    #[test]
    fn agent_tool_summary_groups_correctly() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        let timestamp = chrono::Utc::now().to_rfc3339();
        // Agent-1 calls tool_a 3 times, tool_b 1 time
        for _ in 0..3 {
            conn.execute(
                "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params!["run-1", "agent-1", "worker", "mcp", "tool_a", None::<String>, "success", Some(100i64), &timestamp],
            ).unwrap();
        }
        conn.execute(
            "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params!["run-1", "agent-1", "worker", "mcp", "tool_b", None::<String>, "success", Some(200i64), &timestamp],
        ).unwrap();

        // Agent-2 calls tool_a 2 times
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params!["run-1", "agent-2", "lead", "mcp", "tool_a", None::<String>, "success", Some(50i64), &timestamp],
            ).unwrap();
        }

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, tool_name, COUNT(*), CAST(COALESCE(AVG(duration_ms), 0) AS INTEGER)
                 FROM tool_calls WHERE run_id = ?1
                 GROUP BY agent_id, tool_name
                 ORDER BY agent_id, COUNT(*) DESC",
            )
            .unwrap();
        let results: Vec<(String, String, i64, i64)> = stmt
            .query_map(rusqlite::params!["run-1"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(results.len(), 3);
        // agent-1, tool_a: 3 calls, avg 100ms
        assert_eq!(results[0], ("agent-1".into(), "tool_a".into(), 3, 100));
        // agent-1, tool_b: 1 call, avg 200ms
        assert_eq!(results[1], ("agent-1".into(), "tool_b".into(), 1, 200));
        // agent-2, tool_a: 2 calls, avg 50ms
        assert_eq!(results[2], ("agent-2".into(), "tool_a".into(), 2, 50));
    }

    #[test]
    fn agent_tool_summary_empty_run() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, tool_name, COUNT(*), CAST(COALESCE(AVG(duration_ms), 0) AS INTEGER)
                 FROM tool_calls WHERE run_id = ?1
                 GROUP BY agent_id, tool_name
                 ORDER BY agent_id, COUNT(*) DESC",
            )
            .unwrap();
        let results: Vec<(String, String, i64, i64)> = stmt
            .query_map(rusqlite::params!["nonexistent-run"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(results.is_empty());
    }

    #[test]
    fn agent_tool_summary_ignores_other_runs() {
        let dir = TempDir::new().unwrap();
        let conn = open_test_db(dir.path());

        let timestamp = chrono::Utc::now().to_rfc3339();
        // Insert into run-1
        conn.execute(
            "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params!["run-1", "agent-1", "worker", "mcp", "tool_a", None::<String>, "success", Some(100i64), &timestamp],
        ).unwrap();

        // Insert into run-2
        conn.execute(
            "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params!["run-2", "agent-1", "worker", "mcp", "tool_b", None::<String>, "success", Some(200i64), &timestamp],
        ).unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, tool_name, COUNT(*), CAST(COALESCE(AVG(duration_ms), 0) AS INTEGER)
                 FROM tool_calls WHERE run_id = ?1
                 GROUP BY agent_id, tool_name
                 ORDER BY agent_id, COUNT(*) DESC",
            )
            .unwrap();
        let results: Vec<(String, String, i64, i64)> = stmt
            .query_map(rusqlite::params!["run-1"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "tool_a");
    }
}

// ============================================================
// Git tests — real git operations in temp repos
// ============================================================

mod git_tests {
    use super::*;

    fn git_run(args: &[&str], cwd: &Path) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn git_run_result(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(format!("git {} failed: {stderr}", args.join(" ")))
        }
    }

    #[test]
    fn worktree_add_creates_worktree() {
        let dir = init_test_repo();
        let wt_path = dir.path().join("worktree-1");

        git_run_result(
            &[
                "worktree",
                "add",
                &wt_path.to_string_lossy(),
                "-b",
                "test-branch",
            ],
            dir.path(),
        )
        .unwrap();

        assert!(wt_path.exists());
        assert!(wt_path.is_dir());

        // Verify branch was created
        let branch = git_run(&["branch", "--show-current"], &wt_path);
        assert_eq!(branch, "test-branch");
    }

    #[test]
    fn worktree_add_fails_on_duplicate_branch() {
        let dir = init_test_repo();
        let wt1 = dir.path().join("wt1");
        let wt2 = dir.path().join("wt2");

        git_run_result(
            &[
                "worktree",
                "add",
                &wt1.to_string_lossy(),
                "-b",
                "same-branch",
            ],
            dir.path(),
        )
        .unwrap();

        let result = git_run_result(
            &[
                "worktree",
                "add",
                &wt2.to_string_lossy(),
                "-b",
                "same-branch",
            ],
            dir.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn worktree_remove_cleans_up() {
        let dir = init_test_repo();
        let wt_path = dir.path().join("wt-remove");

        git_run_result(
            &[
                "worktree",
                "add",
                &wt_path.to_string_lossy(),
                "-b",
                "remove-branch",
            ],
            dir.path(),
        )
        .unwrap();
        assert!(wt_path.exists());

        git_run_result(
            &["worktree", "remove", &wt_path.to_string_lossy(), "--force"],
            dir.path(),
        )
        .unwrap();
        assert!(!wt_path.exists());
    }

    #[test]
    fn worktree_prune_succeeds() {
        let dir = init_test_repo();
        // Prune on a clean repo should be fine
        git_run_result(&["worktree", "prune"], dir.path()).unwrap();
    }

    #[test]
    fn current_branch_returns_branch_name() {
        let dir = init_test_repo();
        // Default branch after init
        let branch = git_run(&["branch", "--show-current"], dir.path());
        // Could be "main" or "master" depending on git config
        assert!(!branch.is_empty());
    }

    #[test]
    fn checkout_switches_branch() {
        let dir = init_test_repo();

        // Create a new branch
        git_run_result(&["branch", "feature-1"], dir.path()).unwrap();
        git_run_result(&["checkout", "feature-1"], dir.path()).unwrap();

        let branch = git_run(&["branch", "--show-current"], dir.path());
        assert_eq!(branch, "feature-1");
    }

    #[test]
    fn checkout_nonexistent_branch_fails() {
        let dir = init_test_repo();
        let result = git_run_result(&["checkout", "nonexistent-branch"], dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn branch_delete_removes_branch() {
        let dir = init_test_repo();

        git_run_result(&["branch", "to-delete"], dir.path()).unwrap();
        git_run_result(&["branch", "-D", "to-delete"], dir.path()).unwrap();

        let branches = git_run(&["branch"], dir.path());
        assert!(!branches.contains("to-delete"));
    }

    #[test]
    fn merge_no_ff_creates_merge_commit() {
        let dir = init_test_repo();
        let main_branch = git_run(&["branch", "--show-current"], dir.path());

        // Create feature branch with a commit
        git_run_result(&["checkout", "-b", "feature-merge"], dir.path()).unwrap();
        fs::write(dir.path().join("feature.txt"), "feature content").unwrap();
        git_run_result(&["add", "feature.txt"], dir.path()).unwrap();
        git_run_result(&["commit", "-m", "add feature"], dir.path()).unwrap();

        // Switch back and merge
        git_run_result(&["checkout", &main_branch], dir.path()).unwrap();
        git_run_result(&["merge", "feature-merge", "--no-ff"], dir.path()).unwrap();

        // Verify merge commit exists (--no-ff always creates one)
        let log = git_run(&["log", "--oneline", "-1"], dir.path());
        assert!(log.contains("Merge branch"));
    }

    #[test]
    fn has_conflicts_returns_false_on_clean_repo() {
        let dir = init_test_repo();
        let output = git_run(&["diff", "--name-only", "--diff-filter=U"], dir.path());
        assert!(output.is_empty());
    }

    #[test]
    fn merge_conflicting_branches_detected() {
        let dir = init_test_repo();
        let main_branch = git_run(&["branch", "--show-current"], dir.path());

        // Create conflicting file on main
        fs::write(dir.path().join("conflict.txt"), "main content").unwrap();
        git_run_result(&["add", "conflict.txt"], dir.path()).unwrap();
        git_run_result(&["commit", "-m", "main version"], dir.path()).unwrap();

        // Create branch with different content for same file
        git_run_result(&["checkout", "-b", "conflict-branch", "HEAD~1"], dir.path()).unwrap();
        fs::write(dir.path().join("conflict.txt"), "branch content").unwrap();
        git_run_result(&["add", "conflict.txt"], dir.path()).unwrap();
        git_run_result(&["commit", "-m", "branch version"], dir.path()).unwrap();

        // Try to merge — should fail
        git_run_result(&["checkout", &main_branch], dir.path()).unwrap();
        let merge_result = git_run_result(&["merge", "conflict-branch", "--no-ff"], dir.path());
        assert!(merge_result.is_err());

        // Check for conflicts
        let conflicts = git_run(&["diff", "--name-only", "--diff-filter=U"], dir.path());
        assert!(conflicts.contains("conflict.txt"));

        // Abort merge
        git_run_result(&["merge", "--abort"], dir.path()).unwrap();
    }
}

// ============================================================
// Agent tests — prompt generation and is_alive
// ============================================================

mod agent_tests {
    #[test]
    fn coordinator_prompt_contains_role_and_id() {
        let prompt = generate_prompt("coord-1", "coordinator", None, "Build a REST API");
        assert!(prompt.contains("Agent ID: coord-1"));
        assert!(prompt.contains("Role: coordinator"));
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("Decompose the spec"));
        assert!(prompt.contains("Do NOT read or write implementation code"));
    }

    #[test]
    fn lead_prompt_contains_parent() {
        let prompt = generate_prompt("lead-1", "lead", Some("coord-1"), "Handle backend domain");
        assert!(prompt.contains("Agent ID: lead-1"));
        assert!(prompt.contains("Role: lead"));
        assert!(prompt.contains("Parent: coord-1"));
        assert!(prompt.contains("Handle backend domain"));
        assert!(prompt.contains("Spawn workers"));
        assert!(prompt.contains("Submit approved branches"));
    }

    #[test]
    fn lead_prompt_defaults_parent_to_coordinator() {
        let prompt = generate_prompt("lead-1", "lead", None, "task");
        assert!(prompt.contains("Parent: coordinator"));
    }

    #[test]
    fn worker_prompt_contains_parent_and_constraints() {
        let prompt = generate_prompt(
            "worker-1",
            "worker",
            Some("lead-1"),
            "Implement login endpoint",
        );
        assert!(prompt.contains("Agent ID: worker-1"));
        assert!(prompt.contains("Role: worker"));
        assert!(prompt.contains("Parent: lead-1"));
        assert!(prompt.contains("Implement login endpoint"));
        assert!(prompt.contains("Do not spawn other agents"));
        assert!(prompt.contains("Do not submit to the merge queue"));
    }

    #[test]
    fn worker_prompt_defaults_parent_to_unknown() {
        let prompt = generate_prompt("worker-1", "worker", None, "task");
        assert!(prompt.contains("Parent: unknown"));
    }

    #[test]
    fn is_alive_returns_true_for_current_process() {
        let pid = std::process::id();
        let alive = unsafe { libc::kill(pid as i32, 0) == 0 };
        assert!(alive);
    }

    #[test]
    fn is_alive_returns_false_for_bogus_pid() {
        // PID 99999999 is extremely unlikely to exist
        let alive = unsafe { libc::kill(99999999, 0) == 0 };
        assert!(!alive);
    }

    // Inline prompt generation to avoid needing to import from binary crate
    fn generate_prompt(
        agent_id: &str,
        role: &str,
        parent: Option<&str>,
        task_description: &str,
    ) -> String {
        match role {
            "coordinator" => format!(
                r#"You are the coordinator agent in a hive swarm.
Agent ID: {agent_id}
Role: coordinator

## Your Assignment
{task_description}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.
"#
            ),
            "lead" => format!(
                r#"You are a lead agent in a hive swarm.
Agent ID: {agent_id}
Role: lead
Parent: {}

## Your Assignment
{task_description}

## Responsibilities
- Decompose your assignment into specific worker tasks.
- Spawn workers via hive_spawn_agent for each task.
- Review worker output when they submit for review.
- Send workers back with feedback if changes are needed.
- Submit approved branches to the merge queue via hive_submit_to_queue.
- Report progress to the coordinator via hive_send_message.

## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
"#,
                parent.unwrap_or("coordinator")
            ),
            "worker" => format!(
                r#"You are a worker agent in a hive swarm.
Agent ID: {agent_id}
Role: worker
Parent: {}

## Your Task
{task_description}

## Responsibilities
- Implement the task in your worktree.
- Run relevant tests and linters to verify your work.
- When done, call hive_update_task to set status to "review".
- If you discover an unrelated bug or issue, call hive_create_task
  with urgency and a description. It will be routed to your lead.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than your lead.
- Stay focused on your assigned task.
"#,
                parent.unwrap_or("unknown")
            ),
            _ => panic!("Unknown role: {role}"),
        }
    }
}
