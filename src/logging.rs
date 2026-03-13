use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ToolCallRow {
    pub timestamp: String,
    pub agent_id: String,
    pub tool_name: String,
    pub args_summary: Option<String>,
    pub status: String,
    pub duration_ms: Option<i64>,
}

/// A row from the append-only event log used by wait_for_activity.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: u64,
    pub event_type: String,
    pub entity_id: String,
    pub summary: String,
    pub timestamp: String,
}

pub struct LogDb {
    conn: Connection,
}

impl LogDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
            PRAGMA busy_timeout=5000;
            CREATE TABLE IF NOT EXISTS tool_calls (
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
            CREATE INDEX IF NOT EXISTS idx_tool_calls_tool ON tool_calls(run_id, tool_name);
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                summary TEXT NOT NULL,
                timestamp TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_run ON events(run_id, id);",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Append an event to the log. Returns the new event ID.
    pub fn append_event(
        &self,
        run_id: &str,
        event_type: &str,
        entity_id: &str,
        summary: &str,
    ) -> Result<u64, String> {
        let timestamp = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO events (run_id, event_type, entity_id, summary, timestamp) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![run_id, event_type, entity_id, summary, timestamp],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Return events after `cursor` for the given run, up to `limit` rows.
    pub fn events_since(
        &self,
        run_id: &str,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, event_type, entity_id, summary, timestamp \
                 FROM events WHERE run_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                rusqlite::params![run_id, cursor as i64, limit as i64],
                |row| {
                    Ok(EventRow {
                        id: row.get::<_, i64>(0)? as u64,
                        event_type: row.get(1)?,
                        entity_id: row.get(2)?,
                        summary: row.get(3)?,
                        timestamp: row.get(4)?,
                    })
                },
            )
            .map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| e.to_string())?);
        }
        Ok(results)
    }

    /// Return the maximum event ID for a run (0 if no events).
    #[allow(dead_code)]
    pub fn max_event_id(&self, run_id: &str) -> Result<u64, String> {
        let result: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(id), 0) FROM events WHERE run_id = ?1",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(result as u64)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_tool_call(
        &self,
        run_id: &str,
        agent_id: &str,
        agent_role: &str,
        source: &str,
        tool_name: &str,
        args_summary: Option<&str>,
        status: &str,
        duration_ms: Option<i64>,
    ) -> Result<(), String> {
        let timestamp = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO tool_calls (run_id, agent_id, agent_role, source, tool_name, args_summary, status, duration_ms, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    run_id,
                    agent_id,
                    agent_role,
                    source,
                    tool_name,
                    args_summary,
                    status,
                    duration_ms,
                    timestamp,
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn agent_tool_summary(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, String, i64, i64)>, String> {
        // Returns: (agent_id, tool_name, count, avg_duration_ms)
        let mut stmt = self
            .conn
            .prepare(
                "SELECT agent_id, tool_name, COUNT(*), CAST(COALESCE(AVG(duration_ms), 0) AS INTEGER)
                 FROM tool_calls WHERE run_id = ?1
                 GROUP BY agent_id, tool_name
                 ORDER BY agent_id, COUNT(*) DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| e.to_string())?);
        }
        Ok(results)
    }

    #[allow(dead_code)]
    pub fn recent_tool_calls(
        &self,
        run_id: &str,
        limit: usize,
        agent_id: Option<&str>,
    ) -> Result<Vec<ToolCallRow>, String> {
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match agent_id {
            Some(aid) => (
                "SELECT timestamp, agent_id, tool_name, args_summary, status, duration_ms \
                 FROM tool_calls WHERE run_id = ?1 AND agent_id = ?2 \
                 ORDER BY timestamp DESC LIMIT ?3"
                    .to_string(),
                vec![
                    Box::new(run_id.to_string()),
                    Box::new(aid.to_string()),
                    Box::new(limit as i64),
                ],
            ),
            None => (
                "SELECT timestamp, agent_id, tool_name, args_summary, status, duration_ms \
                 FROM tool_calls WHERE run_id = ?1 \
                 ORDER BY timestamp DESC LIMIT ?2"
                    .to_string(),
                vec![Box::new(run_id.to_string()), Box::new(limit as i64)],
            ),
        };

        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(ToolCallRow {
                    timestamp: row.get(0)?,
                    agent_id: row.get(1)?,
                    tool_name: row.get(2)?,
                    args_summary: row.get(3)?,
                    status: row.get(4)?,
                    duration_ms: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| e.to_string())?);
        }
        results.reverse();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_table() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("log.db");
        let _db1 = LogDb::open(&path).unwrap();
        let _db2 = LogDb::open(&path).unwrap();
    }

    #[test]
    fn log_tool_call_inserts_row() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "hive_update_task",
            Some("task_id=task-1"),
            "success",
            Some(150),
        )
        .unwrap();

        let (tool_name, status): (String, String) = db
            .conn
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
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "lead",
            "hook",
            "hive_spawn_agent",
            None,
            "success",
            None,
        )
        .unwrap();

        let args_summary: Option<String> = db
            .conn
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
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        // Agent-1 calls tool_a 3 times, tool_b 1 time
        for _ in 0..3 {
            db.log_tool_call(
                "run-1",
                "agent-1",
                "worker",
                "mcp",
                "tool_a",
                None,
                "success",
                Some(100),
            )
            .unwrap();
        }
        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_b",
            None,
            "success",
            Some(200),
        )
        .unwrap();

        // Agent-2 calls tool_a 2 times
        for _ in 0..2 {
            db.log_tool_call(
                "run-1",
                "agent-2",
                "lead",
                "mcp",
                "tool_a",
                None,
                "success",
                Some(50),
            )
            .unwrap();
        }

        let results = db.agent_tool_summary("run-1").unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], ("agent-1".into(), "tool_a".into(), 3, 100));
        assert_eq!(results[1], ("agent-1".into(), "tool_b".into(), 1, 200));
        assert_eq!(results[2], ("agent-2".into(), "tool_a".into(), 2, 50));
    }

    #[test]
    fn agent_tool_summary_empty_run() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();
        let results = db.agent_tool_summary("nonexistent-run").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn agent_tool_summary_ignores_other_runs() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            None,
            "success",
            Some(100),
        )
        .unwrap();
        db.log_tool_call(
            "run-2",
            "agent-1",
            "worker",
            "mcp",
            "tool_b",
            None,
            "success",
            Some(200),
        )
        .unwrap();

        let results = db.agent_tool_summary("run-1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "tool_a");
    }

    #[test]
    fn recent_tool_calls_empty_db() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();
        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn recent_tool_calls_respects_limit() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        for i in 0..5 {
            db.log_tool_call(
                "run-1",
                "agent-1",
                "worker",
                "mcp",
                &format!("tool_{}", i),
                None,
                "success",
                Some(100),
            )
            .unwrap();
        }

        let results = db.recent_tool_calls("run-1", 3, None).unwrap();
        assert_eq!(results.len(), 3);
        // Should be the 3 most recent, in chronological order
        assert_eq!(results[0].tool_name, "tool_2");
        assert_eq!(results[1].tool_name, "tool_3");
        assert_eq!(results[2].tool_name, "tool_4");
    }

    #[test]
    fn recent_tool_calls_filters_by_agent() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            None,
            "success",
            Some(100),
        )
        .unwrap();
        db.log_tool_call(
            "run-1",
            "agent-2",
            "worker",
            "mcp",
            "tool_b",
            None,
            "success",
            Some(100),
        )
        .unwrap();
        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_c",
            None,
            "success",
            Some(100),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 10, Some("agent-1")).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.agent_id == "agent-1"));
    }

    #[test]
    fn recent_tool_calls_chronological_order() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        for _ in 0..3 {
            db.log_tool_call(
                "run-1",
                "agent-1",
                "worker",
                "mcp",
                "tool_a",
                None,
                "success",
                Some(100),
            )
            .unwrap();
        }

        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert_eq!(results.len(), 3);
        for i in 0..results.len() - 1 {
            assert!(results[i].timestamp <= results[i + 1].timestamp);
        }
    }

    #[test]
    fn recent_tool_calls_ignores_other_runs() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            None,
            "success",
            Some(100),
        )
        .unwrap();
        db.log_tool_call(
            "run-2",
            "agent-1",
            "worker",
            "mcp",
            "tool_b",
            None,
            "success",
            Some(100),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "tool_a");
    }

    // =================================================================
    // Adversarial tests: special chars, empty strings, large inputs
    // =================================================================

    #[test]
    fn log_tool_call_with_special_chars() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-with-'quotes'",
            "worker",
            "mcp",
            "tool_with_\"doublequotes\"",
            Some("args with\nnewlines\tand\ttabs; DROP TABLE tool_calls;--"),
            "success",
            Some(100),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-with-'quotes'");
        assert_eq!(results[0].tool_name, "tool_with_\"doublequotes\"");
        assert!(
            results[0]
                .args_summary
                .as_ref()
                .unwrap()
                .contains("DROP TABLE")
        );
    }

    #[test]
    fn log_tool_call_with_unicode() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-日本語",
            "agent-🚀",
            "探索者",
            "mcp",
            "outil_français",
            Some("描述 with émojis 🎉"),
            "succès",
            Some(42),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-日本語", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-🚀");
    }

    #[test]
    fn log_tool_call_with_empty_strings() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call("", "", "", "", "", Some(""), "", Some(0))
            .unwrap();

        let results = db.recent_tool_calls("", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "");
        assert_eq!(results[0].tool_name, "");
    }

    #[test]
    fn log_tool_call_with_very_long_args() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        let long_args = "x".repeat(100_000);
        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            Some(&long_args),
            "success",
            Some(100),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].args_summary.as_ref().unwrap().len(), 100_000);
    }

    #[test]
    fn log_tool_call_with_negative_duration() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            None,
            "success",
            Some(-999),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].duration_ms, Some(-999));
    }

    #[test]
    fn agent_tool_summary_with_null_durations() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        // All calls with None duration
        for _ in 0..3 {
            db.log_tool_call(
                "run-1", "agent-1", "worker", "mcp", "tool_a", None, "success", None,
            )
            .unwrap();
        }

        let results = db.agent_tool_summary("run-1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, 3); // count
        assert_eq!(results[0].3, 0); // avg duration defaults to 0 for NULLs
    }

    #[test]
    fn recent_tool_calls_with_zero_limit() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        db.log_tool_call(
            "run-1",
            "agent-1",
            "worker",
            "mcp",
            "tool_a",
            None,
            "success",
            Some(100),
        )
        .unwrap();

        let results = db.recent_tool_calls("run-1", 0, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn open_in_nonexistent_directory_fails() {
        let result = LogDb::open(Path::new("/nonexistent/deep/path/log.db"));
        assert!(result.is_err());
    }

    #[test]
    fn log_many_tool_calls_in_sequence() {
        let dir = TempDir::new().unwrap();
        let db = LogDb::open(&dir.path().join("log.db")).unwrap();

        for i in 0..100 {
            db.log_tool_call(
                "run-1",
                &format!("agent-{}", i % 5),
                "worker",
                "mcp",
                &format!("tool_{}", i % 10),
                None,
                "success",
                Some(i as i64),
            )
            .unwrap();
        }

        let summary = db.agent_tool_summary("run-1").unwrap();
        assert!(!summary.is_empty());
        let total_count: i64 = summary.iter().map(|s| s.2).sum();
        assert_eq!(total_count, 100);
    }
}
