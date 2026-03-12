use crate::types::*;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use super::{HiveState, atomic_write};

impl HiveState {
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
}
