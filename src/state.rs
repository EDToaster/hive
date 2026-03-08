use crate::types::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration loaded from `.hive/config.yaml`.
pub struct HiveConfig {
    pub stall_timeout_seconds: i64,
}

impl Default for HiveConfig {
    fn default() -> Self {
        Self {
            stall_timeout_seconds: 300,
        }
    }
}

/// Root handle for all .hive/ state operations.
/// All methods are stateless — they read/write the filesystem on every call.
pub struct HiveState {
    /// Path to the repo root (parent of .hive/)
    repo_root: PathBuf,
}

impl HiveState {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }

    /// Find the repo root by walking up from cwd looking for .hive/
    pub fn discover() -> Result<Self, String> {
        let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
        loop {
            if dir.join(".hive").is_dir() {
                return Ok(Self::new(dir));
            }
            if !dir.pop() {
                return Err("Not inside a hive project. Run 'hive init' first.".into());
            }
        }
    }

    pub fn hive_dir(&self) -> PathBuf {
        self.repo_root.join(".hive")
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Load config from `.hive/config.yaml`. Returns defaults if file is missing or unparseable.
    pub fn load_config(&self) -> HiveConfig {
        let path = self.hive_dir().join("config.yaml");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return HiveConfig::default(),
        };
        let mut config = HiveConfig::default();
        for line in content.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("stall_timeout_seconds:")
                && let Ok(v) = value.trim().parse::<i64>()
            {
                config.stall_timeout_seconds = v;
            }
        }
        config
    }

    // --- Run Management ---

    pub fn runs_dir(&self) -> PathBuf {
        self.hive_dir().join("runs")
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runs_dir().join(run_id)
    }

    pub fn active_run_id(&self) -> Result<String, String> {
        let active_path = self.hive_dir().join("active_run");
        fs::read_to_string(&active_path)
            .map(|s| s.trim().to_string())
            .map_err(|_| "No active run. Start one with 'hive start'.".into())
    }

    pub fn set_active_run(&self, run_id: &str) -> Result<(), String> {
        let active_path = self.hive_dir().join("active_run");
        fs::write(&active_path, run_id).map_err(|e| e.to_string())
    }

    pub fn create_run(&self, run_id: &str) -> Result<(), String> {
        let run_dir = self.run_dir(run_id);
        fs::create_dir_all(run_dir.join("tasks")).map_err(|e| e.to_string())?;
        fs::create_dir_all(run_dir.join("agents")).map_err(|e| e.to_string())?;
        fs::create_dir_all(run_dir.join("messages")).map_err(|e| e.to_string())?;
        fs::create_dir_all(run_dir.join("worktrees")).map_err(|e| e.to_string())?;

        let metadata = RunMetadata {
            id: run_id.to_string(),
            created_at: chrono::Utc::now(),
            status: RunStatus::Active,
        };
        let meta_path = run_dir.join("run.json");
        let json = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())?;
        fs::write(&meta_path, json).map_err(|e| e.to_string())?;

        // Initialize empty merge queue
        let queue = MergeQueue { entries: vec![] };
        let queue_json = serde_json::to_string_pretty(&queue).map_err(|e| e.to_string())?;
        fs::write(run_dir.join("merge-queue.json"), queue_json).map_err(|e| e.to_string())?;

        self.set_active_run(run_id)
    }

    // --- Task CRUD ---

    pub fn tasks_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("tasks")
    }

    pub fn save_task(&self, run_id: &str, task: &Task) -> Result<(), String> {
        let path = self.tasks_dir(run_id).join(format!("{}.json", task.id));
        let json = serde_json::to_string_pretty(task).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn load_task(&self, run_id: &str, task_id: &str) -> Result<Task, String> {
        let path = self.tasks_dir(run_id).join(format!("{task_id}.json"));
        let data = fs::read_to_string(&path).map_err(|e| format!("Task {task_id}: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Task {task_id}: {e}"))
    }

    pub fn list_tasks(&self, run_id: &str) -> Result<Vec<Task>, String> {
        let dir = self.tasks_dir(run_id);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let task: Task = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                tasks.push(task);
            }
        }
        Ok(tasks)
    }

    // --- Agent CRUD ---

    pub fn agents_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("agents")
    }

    pub fn save_agent(&self, run_id: &str, agent: &Agent) -> Result<(), String> {
        let agent_dir = self.agents_dir(run_id).join(&agent.id);
        fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;
        let path = agent_dir.join("agent.json");
        let json = serde_json::to_string_pretty(agent).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn load_agent(&self, run_id: &str, agent_id: &str) -> Result<Agent, String> {
        let path = self.agents_dir(run_id).join(agent_id).join("agent.json");
        let data = fs::read_to_string(&path).map_err(|e| format!("Agent {agent_id}: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Agent {agent_id}: {e}"))
    }

    pub fn list_agents(&self, run_id: &str) -> Result<Vec<Agent>, String> {
        let dir = self.agents_dir(run_id);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut agents = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().is_dir() {
                let path = entry.path().join("agent.json");
                if path.exists() {
                    let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                    let agent: Agent = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    agents.push(agent);
                }
            }
        }
        Ok(agents)
    }

    // --- Message CRUD ---

    pub fn messages_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("messages")
    }

    pub fn save_message(&self, run_id: &str, message: &Message) -> Result<(), String> {
        let path = self
            .messages_dir(run_id)
            .join(format!("{}.json", message.id));
        let json = serde_json::to_string_pretty(message).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn list_messages(&self, run_id: &str) -> Result<Vec<Message>, String> {
        let dir = self.messages_dir(run_id);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut messages = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let msg: Message = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                messages.push(msg);
            }
        }
        messages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(messages)
    }

    // --- Merge Queue ---

    pub fn load_merge_queue(&self, run_id: &str) -> Result<MergeQueue, String> {
        let path = self.run_dir(run_id).join("merge-queue.json");
        let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    }

    pub fn save_merge_queue(&self, run_id: &str, queue: &MergeQueue) -> Result<(), String> {
        let path = self.run_dir(run_id).join("merge-queue.json");
        let json = serde_json::to_string_pretty(queue).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    // --- Spec ---

    pub fn save_spec(&self, run_id: &str, spec_content: &str) -> Result<(), String> {
        let path = self.run_dir(run_id).join("spec.md");
        fs::write(&path, spec_content).map_err(|e| e.to_string())
    }

    pub fn load_spec(&self, run_id: &str) -> Result<String, String> {
        let path = self.run_dir(run_id).join("spec.md");
        fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    // --- Worktree path ---

    pub fn worktrees_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("worktrees")
    }

    pub fn worktree_path(&self, run_id: &str, agent_id: &str) -> PathBuf {
        self.worktrees_dir(run_id).join(agent_id)
    }
}

#[cfg(test)]
mod tests {
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
}
