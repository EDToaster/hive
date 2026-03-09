use crate::types::*;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Configuration loaded from `.hive/config.yaml`.
pub struct HiveConfig {
    pub stall_timeout_seconds: i64,
    pub verify_command: Option<String>,
    pub max_retries: u32,
    pub budget_usd: Option<f64>,
}

impl Default for HiveConfig {
    fn default() -> Self {
        Self {
            stall_timeout_seconds: 300,
            verify_command: None,
            max_retries: 2,
            budget_usd: None,
        }
    }
}

/// Write to a temp file then rename, preventing partial writes.
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
    Ok(())
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

    /// Acquire an exclusive lock for a state file operation.
    /// Returns a guard that releases the lock on drop.
    pub fn lock_file(&self, name: &str) -> Result<std::fs::File, String> {
        let lock_path = self.hive_dir().join(format!("{name}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| format!("Failed to open lock file: {e}"))?;
        file.lock_exclusive()
            .map_err(|e| format!("Failed to acquire lock: {e}"))?;
        Ok(file)
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
            if let Some(value) = line.strip_prefix("max_retries:")
                && let Ok(v) = value.trim().parse::<u32>()
            {
                config.max_retries = v;
            }
            if let Some(value) = line.strip_prefix("budget_usd:")
                && let Ok(v) = value.trim().parse::<f64>()
            {
                config.budget_usd = Some(v);
            }
            if let Some(value) = line.strip_prefix("verify_command:") {
                let value = value.trim();
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(value);
                if !value.is_empty() {
                    config.verify_command = Some(value.to_string());
                }
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
        atomic_write(&active_path, run_id)
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
        atomic_write(&meta_path, &json)?;

        // Initialize empty merge queue
        let queue = MergeQueue { entries: vec![] };
        let queue_json = serde_json::to_string_pretty(&queue).map_err(|e| e.to_string())?;
        atomic_write(&run_dir.join("merge-queue.json"), &queue_json)?;

        self.set_active_run(run_id)
    }

    // --- Task CRUD ---

    pub fn tasks_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("tasks")
    }

    pub fn save_task(&self, run_id: &str, task: &Task) -> Result<(), String> {
        let path = self.tasks_dir(run_id).join(format!("{}.json", task.id));
        let json = serde_json::to_string_pretty(task).map_err(|e| e.to_string())?;
        atomic_write(&path, &json)
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
        atomic_write(&path, &json)
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
        atomic_write(&path, &json)
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

    pub fn load_messages_for_agent(
        &self,
        run_id: &str,
        agent_id: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Message>, String> {
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
                if msg.to != agent_id {
                    continue;
                }
                if let Some(since_ts) = since
                    && msg.timestamp <= since_ts
                {
                    continue;
                }
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
        atomic_write(&path, &json)
    }

    // --- Spec ---

    pub fn save_spec(&self, run_id: &str, spec_content: &str) -> Result<(), String> {
        let path = self.run_dir(run_id).join("spec.md");
        atomic_write(&path, spec_content)
    }

    pub fn load_spec(&self, run_id: &str) -> Result<String, String> {
        let path = self.run_dir(run_id).join("spec.md");
        fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    // --- Heartbeat ---

    pub fn update_agent_heartbeat(&self, run_id: &str, agent_id: &str) -> Result<(), String> {
        let mut agent = self.load_agent(run_id, agent_id)?;
        agent.heartbeat = Some(chrono::Utc::now());
        self.save_agent(run_id, &agent)
    }

    // --- Run Metadata ---

    pub fn load_run_metadata(&self, run_id: &str) -> Result<RunMetadata, String> {
        let path = self.run_dir(run_id).join("run.json");
        let data = fs::read_to_string(&path).map_err(|e| format!("Run {run_id}: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Run {run_id}: {e}"))
    }

    pub fn save_run_metadata(&self, run_id: &str, metadata: &RunMetadata) -> Result<(), String> {
        let path = self.run_dir(run_id).join("run.json");
        let json = serde_json::to_string_pretty(metadata).map_err(|e| e.to_string())?;
        atomic_write(&path, &json)
    }

    pub fn list_runs(&self) -> Result<Vec<RunMetadata>, String> {
        let runs_dir = self.runs_dir();
        if !runs_dir.exists() {
            return Ok(vec![]);
        }
        let mut runs = Vec::new();
        for entry in fs::read_dir(&runs_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().is_dir() {
                let run_json = entry.path().join("run.json");
                if run_json.exists() {
                    let data = fs::read_to_string(&run_json).map_err(|e| e.to_string())?;
                    if let Ok(meta) = serde_json::from_str::<RunMetadata>(&data) {
                        runs.push(meta);
                    }
                }
            }
        }
        runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(runs)
    }

    pub fn load_agent_cost(&self, run_id: &str, agent_id: &str) -> Option<AgentCost> {
        let output_path = self.agents_dir(run_id).join(agent_id).join("output.json");
        let data = fs::read_to_string(&output_path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&data).ok()?;

        let input_tokens = json.get("num_input_tokens")?.as_u64()?;
        let output_tokens = json.get("num_output_tokens")?.as_u64()?;
        let session_duration_secs = json
            .get("session_duration_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Claude Opus pricing: $15/M input, $75/M output
        let cost_usd = (input_tokens as f64 * 15.0 / 1_000_000.0)
            + (output_tokens as f64 * 75.0 / 1_000_000.0);

        Some(AgentCost {
            input_tokens,
            output_tokens,
            cost_usd,
            session_duration_secs,
        })
    }

    pub fn total_run_cost(&self, run_id: &str) -> f64 {
        let agents = match self.list_agents(run_id) {
            Ok(agents) => agents,
            Err(_) => return 0.0,
        };
        agents
            .iter()
            .filter_map(|agent| self.load_agent_cost(run_id, &agent.id))
            .map(|cost| cost.cost_usd)
            .sum()
    }

    // --- Worktree path ---

    pub fn worktrees_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("worktrees")
    }

    pub fn worktree_path(&self, run_id: &str, agent_id: &str) -> PathBuf {
        self.worktrees_dir(run_id).join(agent_id)
    }

    // --- Memory ---

    pub fn memory_dir(&self) -> PathBuf {
        self.hive_dir().join("memory")
    }

    pub fn load_operations(&self) -> Vec<OperationalEntry> {
        let path = self.memory_dir().join("operations.jsonl");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    pub fn save_operation(&self, entry: &OperationalEntry) -> Result<(), String> {
        let dir = self.memory_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("operations.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        writeln!(file, "{json}").map_err(|e| e.to_string())
    }

    pub fn load_conventions(&self) -> String {
        let path = self.memory_dir().join("conventions.md");
        fs::read_to_string(&path).unwrap_or_default()
    }

    pub fn save_conventions(&self, content: &str) -> Result<(), String> {
        let dir = self.memory_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        atomic_write(&dir.join("conventions.md"), content)
    }

    pub fn load_failures(&self) -> Vec<FailureEntry> {
        let path = self.memory_dir().join("failures.jsonl");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    pub fn save_failure(&self, entry: &FailureEntry) -> Result<(), String> {
        let dir = self.memory_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("failures.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        writeln!(file, "{json}").map_err(|e| e.to_string())
    }

    pub fn prune_memory(&self) -> Result<(), String> {
        // Prune operations to last 10
        let ops = self.load_operations();
        if ops.len() > 10 {
            let kept = &ops[ops.len() - 10..];
            let dir = self.memory_dir();
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let lines: Vec<String> = kept
                .iter()
                .map(|e| serde_json::to_string(e).unwrap())
                .collect();
            atomic_write(
                &dir.join("operations.jsonl"),
                &format!("{}\n", lines.join("\n")),
            )?;
        }

        // Prune failures to last 30
        let fails = self.load_failures();
        if fails.len() > 30 {
            let kept = &fails[fails.len() - 30..];
            let dir = self.memory_dir();
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let lines: Vec<String> = kept
                .iter()
                .map(|e| serde_json::to_string(e).unwrap())
                .collect();
            atomic_write(
                &dir.join("failures.jsonl"),
                &format!("{}\n", lines.join("\n")),
            )?;
        }

        Ok(())
    }

    // --- Hive Mind ---

    pub fn mind_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("mind")
    }

    pub fn save_discovery(&self, run_id: &str, discovery: &Discovery) -> Result<(), String> {
        let dir = self.mind_dir(run_id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("discoveries.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_string(discovery).map_err(|e| e.to_string())?;
        writeln!(file, "{json}").map_err(|e| e.to_string())
    }

    pub fn load_discoveries(&self, run_id: &str) -> Vec<Discovery> {
        let path = self.mind_dir(run_id).join("discoveries.jsonl");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    pub fn save_insight(&self, run_id: &str, insight: &Insight) -> Result<(), String> {
        let dir = self.mind_dir(run_id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("insights.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_string(insight).map_err(|e| e.to_string())?;
        writeln!(file, "{json}").map_err(|e| e.to_string())
    }

    pub fn load_insights(&self, run_id: &str) -> Vec<Insight> {
        let path = self.mind_dir(run_id).join("insights.jsonl");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    pub fn query_mind(&self, run_id: &str, query: &str) -> MindQueryResult {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        let discoveries = self.load_discoveries(run_id);
        let insights = self.load_insights(run_id);

        let matching_discoveries = discoveries
            .into_iter()
            .filter(|d| {
                let content_lower = d.content.to_lowercase();
                words.iter().any(|w| {
                    content_lower.contains(w)
                        || d.tags.iter().any(|t| t.to_lowercase().contains(w))
                        || d.file_paths.iter().any(|f| f.to_lowercase().contains(w))
                })
            })
            .collect();

        let matching_insights = insights
            .into_iter()
            .filter(|i| {
                let content_lower = i.content.to_lowercase();
                words.iter().any(|w| {
                    content_lower.contains(w) || i.tags.iter().any(|t| t.to_lowercase().contains(w))
                })
            })
            .collect();

        MindQueryResult {
            discoveries: matching_discoveries,
            insights: matching_insights,
        }
    }

    pub fn load_memory_for_prompt(&self, role: &AgentRole) -> String {
        if matches!(role, AgentRole::Postmortem) {
            return String::new();
        }

        let mut sections = Vec::new();

        let include_operations = matches!(role, AgentRole::Coordinator | AgentRole::Planner);
        let include_conventions = !matches!(role, AgentRole::Coordinator);
        let include_failures = matches!(
            role,
            AgentRole::Lead
                | AgentRole::Worker
                | AgentRole::Reviewer
                | AgentRole::Explorer
                | AgentRole::Evaluator
        );

        if include_operations {
            let ops = self.load_operations();
            if !ops.is_empty() {
                let mut s = String::from("### Recent Operations\n");
                for op in &ops {
                    s.push_str(&format!(
                        "- Run {}: {} tasks, {} failed, {} agents, ${:.2}\n",
                        op.run_id,
                        op.tasks_total,
                        op.tasks_failed,
                        op.agents_spawned,
                        op.total_cost_usd
                    ));
                    if !op.learnings.is_empty() {
                        s.push_str(&format!("  Learnings: {}\n", op.learnings.join(", ")));
                    }
                }
                sections.push(s);
            }
        }

        if include_conventions {
            let conv = self.load_conventions();
            if !conv.is_empty() {
                sections.push(format!("### Conventions\n{conv}\n"));
            }
        }

        if include_failures {
            let fails = self.load_failures();
            if !fails.is_empty() {
                let mut s = String::from("### Known Failure Patterns\n");
                for f in &fails {
                    s.push_str(&format!(
                        "- Pattern: {} — Context: {}\n",
                        f.pattern, f.context
                    ));
                }
                sections.push(s);
            }
        }

        if sections.is_empty() {
            return String::new();
        }

        format!("## Project Memory\n\n{}", sections.join("\n"))
    }

    // --- Planner Spec ---

    pub fn save_planner_spec(&self, run_id: &str, spec: &str) -> Result<(), String> {
        self.save_spec(run_id, spec)
    }

    pub fn load_planner_spec(&self, run_id: &str) -> Option<String> {
        self.load_spec(run_id).ok()
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
            review_count: 0,
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
    fn test_load_memory_for_prompt_planner() {
        let dir = TempDir::new().unwrap();
        let state = make_state(dir.path());
        state.save_operation(&make_operation("run-1", 5)).unwrap();
        state.save_conventions("Use snake_case.").unwrap();
        let prompt = state.load_memory_for_prompt(&AgentRole::Planner);
        assert!(prompt.contains("### Recent Operations"));
        assert!(prompt.contains("### Conventions"));
        assert!(!prompt.contains("### Known Failure Patterns"));
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
    fn test_planner_spec_save_and_load() {
        let dir = TempDir::new().unwrap();
        let state = make_state(dir.path());
        state.create_run("run-1").unwrap();
        state
            .save_planner_spec("run-1", "# My Spec\nDo stuff.")
            .unwrap();
        let loaded = state.load_planner_spec("run-1");
        assert_eq!(loaded.as_deref(), Some("# My Spec\nDo stuff."));
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

        // Write output.json for agent-1: 1000 input, 500 output
        let output1 = r#"{"num_input_tokens": 1000, "num_output_tokens": 500, "session_duration_seconds": 60}"#;
        std::fs::write(
            state
                .agents_dir("run-1")
                .join("agent-1")
                .join("output.json"),
            output1,
        )
        .unwrap();

        // Write output.json for agent-2: 2000 input, 1000 output
        let output2 = r#"{"num_input_tokens": 2000, "num_output_tokens": 1000, "session_duration_seconds": 120}"#;
        std::fs::write(
            state
                .agents_dir("run-1")
                .join("agent-2")
                .join("output.json"),
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
}
