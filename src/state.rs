use crate::types::*;
use std::fs;
use std::path::{Path, PathBuf};

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
