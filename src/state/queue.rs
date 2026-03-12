use crate::types::*;
use std::fs;

use super::{HiveState, atomic_write};

impl HiveState {
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

    // --- Planner Spec ---

    pub fn save_planner_spec(&self, run_id: &str, spec: &str) -> Result<(), String> {
        self.save_spec(run_id, spec)
    }

    pub fn load_planner_spec(&self, run_id: &str) -> Option<String> {
        self.load_spec(run_id).ok()
    }
}
