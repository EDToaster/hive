mod agents;
mod memory;
mod messages;
mod queue;
mod tasks;

#[cfg(test)]
mod tests;

use crate::types::*;
use std::fs;
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
        use fs2::FileExt;
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
}
