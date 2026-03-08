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
