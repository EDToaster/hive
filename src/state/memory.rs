use crate::types::*;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use super::{HiveState, atomic_write};

impl HiveState {
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

        let include_operations = matches!(role, AgentRole::Coordinator);
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
}
