use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;

pub struct LogDb {
    conn: Connection,
}

impl LogDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_calls (
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
            CREATE INDEX IF NOT EXISTS idx_tool_calls_tool ON tool_calls(run_id, tool_name);",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
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
}
