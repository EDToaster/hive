use crate::types::*;
use std::fs;
use std::path::PathBuf;

use super::{HiveState, atomic_write};

impl HiveState {
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
}
