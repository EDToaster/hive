use crate::types::*;
use std::fs;
use std::path::PathBuf;

use super::{HiveState, atomic_write};

impl HiveState {
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

    // --- Heartbeat ---

    pub fn update_agent_heartbeat(&self, run_id: &str, agent_id: &str) -> Result<(), String> {
        let mut agent = self.load_agent(run_id, agent_id)?;
        agent.heartbeat = Some(chrono::Utc::now());
        self.save_agent(run_id, &agent)
    }

    // --- Agent Cost ---

    pub fn load_agent_cost(&self, run_id: &str, agent_id: &str) -> Option<AgentCost> {
        let output_path = self.agents_dir(run_id).join(agent_id).join("output.jsonl");
        let data = fs::read_to_string(&output_path).ok()?;

        // NDJSON (stream-json): scan lines in reverse for the result message with token counts
        let json = data.lines().rev().find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            if v.get("num_input_tokens").is_some() {
                Some(v)
            } else {
                None
            }
        })?;

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
}
