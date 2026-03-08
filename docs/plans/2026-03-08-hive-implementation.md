# Hive Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a stateless CLI + MCP server that orchestrates a swarm of Claude Code agents via git worktrees, task management, message passing, and a merge queue.

**Architecture:** Single Rust binary with three entry points: `hive <command>` (CLI), `hive mcp` (stdio MCP server), and `hive tui` (monitoring dashboard). All state lives in `.hive/` as JSON files + SQLite. No daemon — every invocation reads/writes `.hive/` directly.

**Tech Stack:** Rust, clap 4.5.60, rmcp 1.1.0, ratatui 0.30.0, rusqlite 0.38.0, tokio 1.50.0, serde/serde_json, chrono 0.4.44, uuid 1.22.0, notify 8.2.0, crossterm 0.29.0, schemars 1.2.1

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

## Phase 1: Project Scaffolding and Core Types

### Task 1.1: Initialize Rust project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore`

**Step 1: Create the Cargo project**

Run: `cargo init --name hive /Users/howard/src/hive`

**Step 2: Replace Cargo.toml with full dependency list**

```toml
[package]
name = "hive"
version = "0.1.0"
edition = "2024"

[dependencies]
clap = { version = "4.5.60", features = ["derive"] }
rmcp = { version = "1.1.0", features = ["server", "transport-io", "macros"] }
schemars = "1.2.1"
ratatui = "0.30.0"
crossterm = "0.29.0"
rusqlite = { version = "0.38.0", features = ["bundled"] }
tokio = { version = "1.50.0", features = ["full"] }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
chrono = { version = "0.4.44", features = ["serde"] }
uuid = { version = "1.22.0", features = ["v4", "serde"] }
notify = "8.2.0"
```

**Step 3: Update .gitignore**

Append to `.gitignore`:
```
/target
.hive/
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors (warnings OK)

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs .gitignore
git commit -m "feat: initialize rust project with dependencies"
```

---

### Task 1.2: Define core data types

**Files:**
- Create: `src/types.rs`
- Modify: `src/main.rs` (add module declaration)

**Step 1: Write the types module**

This defines all shared types used across the codebase. Every struct derives Serialize/Deserialize for JSON persistence.

```rust
// src/types.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --- Agent Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Coordinator,
    Lead,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Done,
    Failed,
    Stalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub parent: Option<String>,
    pub pid: Option<u32>,
    pub worktree: Option<String>,
    pub heartbeat: Option<DateTime<Utc>>,
    pub task_id: Option<String>,
}

// --- Task Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Active,
    Blocked,
    Review,
    Approved,
    Queued,
    Merged,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub urgency: Urgency,
    pub blocking: Vec<String>,
    pub blocked_by: Vec<String>,
    pub assigned_to: Option<String>,
    pub created_by: String,
    pub parent_task: Option<String>,
    pub branch: Option<String>,
    pub domain: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// --- Message Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageType {
    Info,
    Request,
    Status,
    TaskSuggestion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub to: String,
    pub timestamp: DateTime<Utc>,
    pub message_type: MessageType,
    pub body: String,
    pub refs: Vec<String>,
}

// --- Merge Queue ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub task_id: String,
    pub branch: String,
    pub submitted_by: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueue {
    pub entries: Vec<MergeQueueEntry>,
}

// --- Run ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub status: RunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Active,
    Completed,
    Failed,
}
```

**Step 2: Add module to main.rs**

Add `mod types;` to `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add src/types.rs src/main.rs
git commit -m "feat: define core data types for agents, tasks, messages, merge queue"
```

---

## Phase 2: State Layer

### Task 2.1: State directory management

**Files:**
- Create: `src/state.rs`
- Modify: `src/main.rs` (add module)

This module handles all reads/writes to the `.hive/` directory. Every other module goes through this layer — nothing else touches the filesystem directly.

**Step 1: Write the state module**

```rust
// src/state.rs
use crate::types::*;
use std::fs;
use std::path::{Path, PathBuf};

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
        fs::write(&active_path, run_id).map_err(|e| e.to_string())
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
        fs::write(&meta_path, json).map_err(|e| e.to_string())?;

        // Initialize empty merge queue
        let queue = MergeQueue { entries: vec![] };
        let queue_json = serde_json::to_string_pretty(&queue).map_err(|e| e.to_string())?;
        fs::write(run_dir.join("merge-queue.json"), queue_json).map_err(|e| e.to_string())?;

        self.set_active_run(run_id)
    }

    // --- Task CRUD ---

    pub fn tasks_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("tasks")
    }

    pub fn save_task(&self, run_id: &str, task: &Task) -> Result<(), String> {
        let path = self.tasks_dir(run_id).join(format!("{}.json", task.id));
        let json = serde_json::to_string_pretty(task).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
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
        fs::write(&path, json).map_err(|e| e.to_string())
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
        let path = self.messages_dir(run_id).join(format!("{}.json", message.id));
        let json = serde_json::to_string_pretty(message).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
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

    // --- Merge Queue ---

    pub fn load_merge_queue(&self, run_id: &str) -> Result<MergeQueue, String> {
        let path = self.run_dir(run_id).join("merge-queue.json");
        let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    }

    pub fn save_merge_queue(&self, run_id: &str, queue: &MergeQueue) -> Result<(), String> {
        let path = self.run_dir(run_id).join("merge-queue.json");
        let json = serde_json::to_string_pretty(queue).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    // --- Spec ---

    pub fn save_spec(&self, run_id: &str, spec_content: &str) -> Result<(), String> {
        let path = self.run_dir(run_id).join("spec.md");
        fs::write(&path, spec_content).map_err(|e| e.to_string())
    }

    pub fn load_spec(&self, run_id: &str) -> Result<String, String> {
        let path = self.run_dir(run_id).join("spec.md");
        fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    // --- Worktree path ---

    pub fn worktrees_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("worktrees")
    }

    pub fn worktree_path(&self, run_id: &str, agent_id: &str) -> PathBuf {
        self.worktrees_dir(run_id).join(agent_id)
    }
}
```

**Step 2: Add module to main.rs**

Add `mod state;` to `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add src/state.rs src/main.rs
git commit -m "feat: add state layer for .hive/ directory management"
```

---

### Task 2.2: SQLite logging layer

**Files:**
- Create: `src/logging.rs`
- Modify: `src/main.rs` (add module)

**Step 1: Write the logging module**

```rust
// src/logging.rs
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
                "SELECT agent_id, tool_name, COUNT(*), COALESCE(AVG(duration_ms), 0)
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
```

**Step 2: Add module to main.rs**

Add `mod logging;` to `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add src/logging.rs src/main.rs
git commit -m "feat: add sqlite logging layer for tool call tracking"
```

---

## Phase 3: Git and Agent Operations

### Task 3.1: Git operations module

**Files:**
- Create: `src/git.rs`
- Modify: `src/main.rs` (add module)

**Step 1: Write the git module**

This module shells out to `git` for all operations. Each function runs a command and returns structured output.

```rust
// src/git.rs
use std::path::Path;
use std::process::Command;

pub struct Git;

impl Git {
    fn run(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(format!("git {} failed: {stderr}", args.join(" ")))
        }
    }

    /// Create a new worktree with a new branch
    pub fn worktree_add(repo_root: &Path, worktree_path: &Path, branch: &str) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                "-b",
                branch,
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Remove a worktree
    pub fn worktree_remove(repo_root: &Path, worktree_path: &Path) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "remove",
                &worktree_path.to_string_lossy(),
                "--force",
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Prune stale worktree metadata
    pub fn worktree_prune(repo_root: &Path) -> Result<(), String> {
        Self::run(&["worktree", "prune"], repo_root)?;
        Ok(())
    }

    /// Merge a branch into the current branch
    pub fn merge(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["merge", branch, "--no-ff"], repo_root)?;
        Ok(())
    }

    /// Abort a merge in progress
    pub fn merge_abort(repo_root: &Path) -> Result<(), String> {
        Self::run(&["merge", "--abort"], repo_root)?;
        Ok(())
    }

    /// Check if there are merge conflicts
    pub fn has_conflicts(repo_root: &Path) -> Result<bool, String> {
        let output = Self::run(&["diff", "--name-only", "--diff-filter=U"], repo_root)?;
        Ok(!output.is_empty())
    }

    /// Get the current branch name
    pub fn current_branch(repo_root: &Path) -> Result<String, String> {
        Self::run(&["branch", "--show-current"], repo_root)
    }

    /// Checkout a branch
    pub fn checkout(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["checkout", branch], repo_root)?;
        Ok(())
    }

    /// Delete a branch
    pub fn branch_delete(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["branch", "-D", branch], repo_root)?;
        Ok(())
    }
}
```

**Step 2: Add module to main.rs**

Add `mod git;` to `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add src/git.rs src/main.rs
git commit -m "feat: add git operations module (worktree, merge, branch)"
```

---

### Task 3.2: Agent spawning module

**Files:**
- Create: `src/agent.rs`
- Modify: `src/main.rs` (add module)

This module handles the full agent spawn sequence: create worktree, write config files, launch claude code, register agent.

**Step 1: Write the agent module**

```rust
// src/agent.rs
use crate::git::Git;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use std::fs;
use std::process::Command;

pub struct AgentSpawner;

impl AgentSpawner {
    /// Full spawn sequence for a lead or worker agent.
    pub fn spawn(
        state: &HiveState,
        run_id: &str,
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
    ) -> Result<Agent, String> {
        let worktree_path = state.worktree_path(run_id, agent_id);
        let branch = format!("hive/{run_id}/{agent_id}");

        // Step 1: Create worktree
        Git::worktree_add(state.repo_root(), &worktree_path, &branch)?;

        // Step 2: Write .claude/hooks.json
        let claude_dir = worktree_path.join(".claude");
        fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;

        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "hive log-tool --run {run_id} --agent {agent_id} --tool $TOOL_NAME --status $TOOL_STATUS"
                        )
                    }]
                }]
            }
        });
        fs::write(
            claude_dir.join("hooks.json"),
            serde_json::to_string_pretty(&hooks_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 3: Write .claude/settings.local.json (MCP config)
        let settings_json = serde_json::json!({
            "mcpServers": {
                "hive": {
                    "command": "hive",
                    "args": ["mcp", "--run", run_id, "--agent", agent_id]
                }
            }
        });
        fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 4: Write CLAUDE.local.md
        let prompt = Self::generate_prompt(agent_id, role, parent, task_description);
        fs::write(worktree_path.join("CLAUDE.local.md"), &prompt)
            .map_err(|e| e.to_string())?;

        // Step 5: Launch claude code process
        let child = Command::new("claude")
            .arg("--print")
            .arg("--dangerously-skip-permissions")
            .arg(&prompt)
            .current_dir(&worktree_path)
            .spawn()
            .map_err(|e| format!("Failed to launch claude: {e}"))?;

        // Step 6: Register agent
        let agent = Agent {
            id: agent_id.to_string(),
            role,
            status: AgentStatus::Running,
            parent: parent.map(|s| s.to_string()),
            pid: Some(child.id()),
            worktree: Some(worktree_path.to_string_lossy().to_string()),
            heartbeat: Some(Utc::now()),
            task_id: None,
        };
        state.save_agent(run_id, &agent)?;

        Ok(agent)
    }

    fn generate_prompt(
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
    ) -> String {
        match role {
            AgentRole::Coordinator => format!(
                r#"You are the coordinator agent in a hive swarm.
Agent ID: {agent_id}
Role: coordinator

## Your Assignment
{task_description}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.
"#
            ),
            AgentRole::Lead => format!(
                r#"You are a lead agent in a hive swarm.
Agent ID: {agent_id}
Role: lead
Parent: {}

## Your Assignment
{task_description}

## Responsibilities
- Decompose your assignment into specific worker tasks.
- Spawn workers via hive_spawn_agent for each task.
- Review worker output when they submit for review.
- Send workers back with feedback if changes are needed.
- Submit approved branches to the merge queue via hive_submit_to_queue.
- Report progress to the coordinator via hive_send_message.

## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Worker => format!(
                r#"You are a worker agent in a hive swarm.
Agent ID: {agent_id}
Role: worker
Parent: {}

## Your Task
{task_description}

## Responsibilities
- Implement the task in your worktree.
- Run relevant tests and linters to verify your work.
- When done, call hive_update_task to set status to "review".
- If you discover an unrelated bug or issue, call hive_create_task
  with urgency and a description. It will be routed to your lead.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than your lead.
- Stay focused on your assigned task.
"#,
                parent.unwrap_or("unknown")
            ),
        }
    }

    /// Check if an agent process is still alive by PID
    pub fn is_alive(pid: u32) -> bool {
        // Send signal 0 to check if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}
```

Note: add `libc` dependency for process checking.

**Step 2: Add libc dependency**

Add to `Cargo.toml` under `[dependencies]`:
```toml
libc = "0.2"
```

**Step 3: Add module to main.rs**

Add `mod agent;` to `src/main.rs`.

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add src/agent.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: add agent spawn module (worktree, config, claude launch)"
```

---

## Phase 4: CLI

### Task 4.1: CLI skeleton with `hive init` and `hive start`

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs` (replace with CLI entry point)

**Step 1: Write the CLI module**

```rust
// src/cli.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hive", about = "Agentic swarm coordinator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize .hive/ in the current repo
    Init,

    /// Start a new run with a spec file
    Start {
        /// Path to the spec file
        spec: String,
    },

    /// Show current run status
    Status,

    /// List agents and their health
    Agents,

    /// List tasks and statuses
    Tasks {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,
    },

    /// View message history
    Messages {
        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,
    },

    /// Record a tool call event (called by agent hooks)
    LogTool {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        duration: Option<i64>,
    },

    /// Query the event log
    Logs {
        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,
    },

    /// Launch the monitoring dashboard
    Tui,

    /// Run as MCP server (stdio transport)
    Mcp {
        #[arg(long)]
        run: String,
        #[arg(long)]
        agent: String,
    },
}
```

**Step 2: Rewrite main.rs as the CLI entry point**

```rust
// src/main.rs
mod agent;
mod cli;
mod git;
mod logging;
mod state;
mod types;

use clap::Parser;
use cli::{Cli, Commands};
use logging::LogDb;
use state::HiveState;
use std::fs;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cmd_init(),
        Commands::Start { spec } => cmd_start(&spec),
        Commands::Status => cmd_status(),
        Commands::Agents => cmd_agents(),
        Commands::Tasks { status, assignee } => cmd_tasks(status, assignee),
        Commands::Messages { agent } => cmd_messages(agent),
        Commands::LogTool {
            run,
            agent,
            tool,
            status,
            duration,
        } => cmd_log_tool(&run, &agent, &tool, &status, duration),
        Commands::Logs { agent } => cmd_logs(agent),
        Commands::Tui => cmd_tui(),
        Commands::Mcp { run, agent } => cmd_mcp(&run, &agent),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn cmd_init() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;

    // Check we're in a git repo
    if !cwd.join(".git").exists() {
        return Err("Not a git repository. Run 'git init' first.".into());
    }

    let hive_dir = cwd.join(".hive");
    if hive_dir.exists() {
        return Err(".hive/ already exists. Already initialized.".into());
    }

    fs::create_dir_all(hive_dir.join("runs")).map_err(|e| e.to_string())?;

    // Write default config
    let config = "# Hive configuration\nstall_timeout_seconds: 300\n";
    fs::write(hive_dir.join("config.yaml"), config).map_err(|e| e.to_string())?;

    println!("Initialized .hive/ in {}", cwd.display());
    Ok(())
}

fn cmd_start(spec_path: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    let spec_content = fs::read_to_string(spec_path)
        .map_err(|e| format!("Cannot read spec file '{spec_path}': {e}"))?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;
    state.save_spec(&run_id, &spec_content)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    println!("Created run: {run_id}");
    println!("Spec saved to .hive/runs/{run_id}/spec.md");

    // TODO: Spawn coordinator agent
    println!("TODO: Spawn coordinator agent");

    Ok(())
}

fn cmd_status() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;
    let tasks = state.list_tasks(&run_id)?;
    let queue = state.load_merge_queue(&run_id)?;

    println!("Run: {run_id}");
    println!("Agents: {} total", agents.len());
    for agent in &agents {
        println!(
            "  {} ({:?}) - {:?}",
            agent.id, agent.role, agent.status
        );
    }
    println!("Tasks: {} total", tasks.len());
    let by_status = |s: types::TaskStatus| tasks.iter().filter(|t| t.status == s).count();
    println!(
        "  pending={} active={} review={} merged={} failed={}",
        by_status(types::TaskStatus::Pending),
        by_status(types::TaskStatus::Active),
        by_status(types::TaskStatus::Review),
        by_status(types::TaskStatus::Merged),
        by_status(types::TaskStatus::Failed),
    );
    println!("Merge queue: {} entries", queue.entries.len());

    Ok(())
}

fn cmd_agents() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;

    if agents.is_empty() {
        println!("No agents.");
        return Ok(());
    }

    for agent in &agents {
        let alive = agent
            .pid
            .map(|pid| {
                if agent::AgentSpawner::is_alive(pid) {
                    "alive"
                } else {
                    "dead"
                }
            })
            .unwrap_or("no pid");
        println!(
            "{:<20} role={:<12?} status={:<10?} parent={:<20} process={}",
            agent.id,
            agent.role,
            agent.status,
            agent.parent.as_deref().unwrap_or("-"),
            alive,
        );
    }
    Ok(())
}

fn cmd_tasks(status_filter: Option<String>, assignee_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let tasks = state.list_tasks(&run_id)?;

    let filtered: Vec<_> = tasks
        .iter()
        .filter(|t| {
            if let Some(ref s) = status_filter {
                let status_str = serde_json::to_value(&t.status)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                if status_str.as_deref() != Some(s.as_str()) {
                    return false;
                }
            }
            if let Some(ref a) = assignee_filter {
                if t.assigned_to.as_deref() != Some(a.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No tasks match filters.");
        return Ok(());
    }

    for task in &filtered {
        println!(
            "{:<12} {:?} [{:?}] assigned={} - {}",
            task.id,
            task.status,
            task.urgency,
            task.assigned_to.as_deref().unwrap_or("-"),
            task.title,
        );
    }
    Ok(())
}

fn cmd_messages(agent_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let messages = state.list_messages(&run_id)?;

    let filtered: Vec<_> = messages
        .iter()
        .filter(|m| {
            if let Some(ref a) = agent_filter {
                return m.from == *a || m.to == *a;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No messages.");
        return Ok(());
    }

    for msg in &filtered {
        println!(
            "[{}] {} -> {} ({:?}): {}",
            msg.timestamp.format("%H:%M:%S"),
            msg.from,
            msg.to,
            msg.message_type,
            msg.body,
        );
    }
    Ok(())
}

fn cmd_log_tool(
    run_id: &str,
    agent_id: &str,
    tool: &str,
    status: &str,
    duration: Option<i64>,
) -> Result<(), String> {
    let state = HiveState::discover()?;
    let log_path = state.run_dir(run_id).join("log.db");
    let db = LogDb::open(&log_path)?;

    // Look up agent role
    let agent = state.load_agent(run_id, agent_id).ok();
    let role = agent
        .map(|a| format!("{:?}", a.role).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());

    db.log_tool_call(
        run_id,
        agent_id,
        &role,
        "claude-code",
        tool,
        None,
        status,
        duration,
    )?;
    Ok(())
}

fn cmd_logs(agent_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let log_path = state.run_dir(&run_id).join("log.db");
    let db = LogDb::open(&log_path)?;

    let summary = db.agent_tool_summary(&run_id)?;

    let filtered: Vec<_> = summary
        .iter()
        .filter(|(aid, _, _, _)| {
            if let Some(ref a) = agent_filter {
                return aid == a;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No log entries.");
        return Ok(());
    }

    println!("{:<20} {:<25} {:>8} {:>10}", "Agent", "Tool", "Count", "Avg ms");
    println!("{}", "-".repeat(65));
    for (agent_id, tool_name, count, avg_ms) in &filtered {
        println!("{:<20} {:<25} {:>8} {:>10}", agent_id, tool_name, count, avg_ms);
    }
    Ok(())
}

fn cmd_tui() -> Result<(), String> {
    // TODO: Phase 6
    println!("TUI not yet implemented.");
    Ok(())
}

fn cmd_mcp(_run_id: &str, _agent_id: &str) -> Result<(), String> {
    // TODO: Phase 5
    println!("MCP server not yet implemented.");
    Ok(())
}
```

**Step 3: Verify it compiles and runs**

Run: `cargo build && cargo run -- --help`
Expected: prints help text with all subcommands listed

Run: `cargo run -- init` (from the hive repo root)
Expected: "Initialized .hive/ in /Users/howard/src/hive" (or error if already exists)

**Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add CLI skeleton with init, start, status, agents, tasks, messages, logs"
```

---

## Phase 5: MCP Server

### Task 5.1: MCP server with all hive tools

**Files:**
- Create: `src/mcp.rs`
- Modify: `src/main.rs` (wire up `cmd_mcp`)

**Step 1: Write the MCP server module**

Reference the `rmcp` crate API: use `#[tool_router]` macro to define tools, `Parameters<T>` for structured input, `CallToolResult` for output. Serve over stdio.

```rust
// src/mcp.rs
use crate::logging::LogDb;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::*;
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HiveMcp {
    run_id: String,
    agent_id: String,
    repo_root: String,
    tool_router: ToolRouter<Self>,
}

// --- Parameter types ---

#[derive(Deserialize, JsonSchema)]
pub struct SpawnAgentParams {
    /// Agent ID (unique name like "lead-backend" or "worker-001")
    pub agent_id: String,
    /// Role: "lead" or "worker"
    pub role: String,
    /// Task description for the agent
    pub task_description: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Short title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Urgency: low, normal, high, critical
    #[serde(default = "default_urgency")]
    pub urgency: String,
    /// Optional domain tag
    pub domain: Option<String>,
    /// Optional list of task IDs this blocks
    #[serde(default)]
    pub blocking: Vec<String>,
    /// Optional list of task IDs blocking this
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Optional parent task ID
    pub parent_task: Option<String>,
}

fn default_urgency() -> String {
    "normal".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateTaskParams {
    /// Task ID to update
    pub task_id: String,
    /// New status
    pub status: Option<String>,
    /// Agent ID to assign to
    pub assigned_to: Option<String>,
    /// Branch name
    pub branch: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTasksParams {
    /// Filter by status
    pub status: Option<String>,
    /// Filter by assignee
    pub assignee: Option<String>,
    /// Filter by domain
    pub domain: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// Recipient agent ID
    pub to: String,
    /// Message type: info, request, status, task-suggestion
    #[serde(default = "default_message_type")]
    pub message_type: String,
    /// Message body
    pub body: String,
    /// Optional task ID references
    #[serde(default)]
    pub refs: Vec<String>,
}

fn default_message_type() -> String {
    "info".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct SubmitToQueueParams {
    /// Task ID of the approved work
    pub task_id: String,
    /// Branch name to merge
    pub branch: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LogToolParams {
    /// Tool name
    pub tool: String,
    /// Status: success or error
    pub status: String,
    /// Duration in milliseconds
    pub duration_ms: Option<i64>,
    /// Optional args summary
    pub args_summary: Option<String>,
}

#[tool_router]
impl HiveMcp {
    pub fn new(run_id: String, agent_id: String, repo_root: String) -> Self {
        Self {
            run_id,
            agent_id,
            repo_root,
            tool_router: Self::tool_router(),
        }
    }

    fn state(&self) -> HiveState {
        HiveState::new(self.repo_root.clone().into())
    }

    #[tool(description = "Spawn a new agent (lead or worker) with a worktree and task")]
    async fn hive_spawn_agent(
        &self,
        params: Parameters<SpawnAgentParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let role = match p.role.as_str() {
            "lead" => AgentRole::Lead,
            "worker" => AgentRole::Worker,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid role. Use 'lead' or 'worker'.",
                )]))
            }
        };

        match crate::agent::AgentSpawner::spawn(
            &self.state(),
            &self.run_id,
            &p.agent_id,
            role,
            Some(&self.agent_id),
            &p.task_description,
        ) {
            Ok(agent) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Spawned agent '{}' (role={:?}, worktree={})",
                agent.id,
                agent.role,
                agent.worktree.unwrap_or_default()
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Create a new task with title, description, urgency, and optional blocking info")]
    async fn hive_create_task(
        &self,
        params: Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let urgency = match p.urgency.as_str() {
            "low" => Urgency::Low,
            "normal" => Urgency::Normal,
            "high" => Urgency::High,
            "critical" => Urgency::Critical,
            _ => Urgency::Normal,
        };

        let task_id = format!("task-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now();
        let task = Task {
            id: task_id.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
            status: TaskStatus::Pending,
            urgency,
            blocking: p.blocking.clone(),
            blocked_by: p.blocked_by.clone(),
            assigned_to: None,
            created_by: self.agent_id.clone(),
            parent_task: p.parent_task.clone(),
            branch: None,
            domain: p.domain.clone(),
            created_at: now,
            updated_at: now,
        };

        match self.state().save_task(&self.run_id, &task) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Created task '{task_id}': {}",
                p.title
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Update a task's status, assignment, or branch")]
    async fn hive_update_task(
        &self,
        params: Parameters<UpdateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let state = self.state();
        let mut task = match state.load_task(&self.run_id, &p.task_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        if let Some(ref status_str) = p.status {
            task.status = match status_str.as_str() {
                "pending" => TaskStatus::Pending,
                "active" => TaskStatus::Active,
                "blocked" => TaskStatus::Blocked,
                "review" => TaskStatus::Review,
                "approved" => TaskStatus::Approved,
                "queued" => TaskStatus::Queued,
                "merged" => TaskStatus::Merged,
                "failed" => TaskStatus::Failed,
                _ => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Invalid status: {status_str}"
                    ))]))
                }
            };
        }

        if let Some(ref assignee) = p.assigned_to {
            task.assigned_to = Some(assignee.clone());
        }

        if let Some(ref branch) = p.branch {
            task.branch = Some(branch.clone());
        }

        task.updated_at = Utc::now();

        match state.save_task(&self.run_id, &task) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Updated task '{}': status={:?}",
                task.id, task.status
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "List tasks, optionally filtered by status, assignee, or domain")]
    async fn hive_list_tasks(
        &self,
        params: Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let tasks = match self.state().list_tasks(&self.run_id) {
            Ok(t) => t,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let filtered: Vec<_> = tasks
            .iter()
            .filter(|t| {
                if let Some(ref s) = p.status {
                    let status_str = serde_json::to_value(&t.status)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                    if status_str.as_deref() != Some(s.as_str()) {
                        return false;
                    }
                }
                if let Some(ref a) = p.assignee {
                    if t.assigned_to.as_deref() != Some(a.as_str()) {
                        return false;
                    }
                }
                if let Some(ref d) = p.domain {
                    if t.domain.as_deref() != Some(d.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();

        let summary = serde_json::to_string_pretty(&filtered).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(description = "Send a message to another agent")]
    async fn hive_send_message(
        &self,
        params: Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let msg_type = match p.message_type.as_str() {
            "info" => MessageType::Info,
            "request" => MessageType::Request,
            "status" => MessageType::Status,
            "task-suggestion" => MessageType::TaskSuggestion,
            _ => MessageType::Info,
        };

        let msg_id = format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let message = Message {
            id: msg_id.clone(),
            from: self.agent_id.clone(),
            to: p.to.clone(),
            timestamp: Utc::now(),
            message_type: msg_type,
            body: p.body.clone(),
            refs: p.refs.clone(),
        };

        match self.state().save_message(&self.run_id, &message) {
            Ok(()) => {
                // TODO: inject message into target agent's Claude Code conversation
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Sent message '{msg_id}' to '{}'",
                    p.to
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Submit an approved branch to the merge queue")]
    async fn hive_submit_to_queue(
        &self,
        params: Parameters<SubmitToQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let state = self.state();
        let mut queue = match state.load_merge_queue(&self.run_id) {
            Ok(q) => q,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        queue.entries.push(MergeQueueEntry {
            task_id: p.task_id.clone(),
            branch: p.branch.clone(),
            submitted_by: self.agent_id.clone(),
            submitted_at: Utc::now(),
        });

        match state.save_merge_queue(&self.run_id, &queue) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Submitted branch '{}' for task '{}' to merge queue (position: {})",
                p.branch,
                p.task_id,
                queue.entries.len()
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Process the next item in the merge queue. Merges the branch into main and runs tests.")]
    async fn hive_merge_next(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let mut queue = match state.load_merge_queue(&self.run_id) {
            Ok(q) => q,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        if queue.entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "Merge queue is empty.",
            )]));
        }

        let entry = queue.entries.remove(0);

        // Attempt merge
        let repo_root = state.repo_root();
        match crate::git::Git::merge(repo_root, &entry.branch) {
            Ok(()) => {
                // Update task status
                if let Ok(mut task) = state.load_task(&self.run_id, &entry.task_id) {
                    task.status = TaskStatus::Merged;
                    task.updated_at = Utc::now();
                    let _ = state.save_task(&self.run_id, &task);
                }
                state.save_merge_queue(&self.run_id, &queue).ok();
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Merged branch '{}' for task '{}'. Queue remaining: {}",
                    entry.branch,
                    entry.task_id,
                    queue.entries.len()
                ))]))
            }
            Err(e) => {
                // Abort the failed merge
                let _ = crate::git::Git::merge_abort(repo_root);
                // Put entry back at front
                queue.entries.insert(0, entry.clone());
                state.save_merge_queue(&self.run_id, &queue).ok();
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Merge failed for branch '{}': {e}. Entry remains in queue.",
                    entry.branch
                ))]))
            }
        }
    }

    #[tool(description = "List all agents and their current status")]
    async fn hive_list_agents(&self) -> Result<CallToolResult, McpError> {
        let agents = match self.state().list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        let summary = serde_json::to_string_pretty(&agents).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }

    #[tool(description = "Check agent health by comparing heartbeats and verifying processes are alive")]
    async fn hive_check_agents(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let agents = match state.list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let mut report = Vec::new();
        for agent in &agents {
            let alive = agent
                .pid
                .map(|pid| crate::agent::AgentSpawner::is_alive(pid))
                .unwrap_or(false);
            let stalled = agent.heartbeat.is_some_and(|hb| {
                (Utc::now() - hb).num_seconds() > 300 // 5 min threshold
            });

            let status_note = if !alive && agent.status == AgentStatus::Running {
                "DEAD (process not found)"
            } else if stalled {
                "STALLED (no heartbeat > 5min)"
            } else {
                "OK"
            };

            report.push(format!(
                "{}: {:?} - {status_note}",
                agent.id, agent.status
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(
            report.join("\n"),
        )]))
    }

    #[tool(description = "Record a tool call event for observability")]
    async fn hive_log_tool(
        &self,
        params: Parameters<LogToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let state = self.state();
        let log_path = state.run_dir(&self.run_id).join("log.db");

        match LogDb::open(&log_path) {
            Ok(db) => {
                let agent = state.load_agent(&self.run_id, &self.agent_id).ok();
                let role = agent
                    .map(|a| format!("{:?}", a.role).to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string());

                match db.log_tool_call(
                    &self.run_id,
                    &self.agent_id,
                    &role,
                    "claude-code",
                    &p.tool,
                    p.args_summary.as_deref(),
                    &p.status,
                    p.duration_ms,
                ) {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text("Logged.")])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}

/// Start the MCP server on stdio
pub async fn run_mcp_server(run_id: &str, agent_id: &str) -> Result<(), String> {
    let repo_root = HiveState::discover()?.repo_root().to_string_lossy().to_string();
    let server = HiveMcp::new(run_id.to_string(), agent_id.to_string(), repo_root);

    let (stdin, stdout) = rmcp::transport::io::stdio();
    server
        .serve((stdin, stdout))
        .await
        .map_err(|e| format!("MCP server error: {e}"))?;

    Ok(())
}
```

**Step 2: Wire up cmd_mcp in main.rs**

Replace the `cmd_mcp` function:

```rust
fn cmd_mcp(run_id: &str, agent_id: &str) -> Result<(), String> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(crate::mcp::run_mcp_server(run_id, agent_id))
}
```

Add `mod mcp;` to the top of `main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles. There may be some API mismatches with rmcp that need adjusting — the exact `#[tool_router]` and `serve` API should be checked against rmcp 1.1.0 docs. Adjust as needed.

**Step 4: Commit**

```bash
git add src/mcp.rs src/main.rs
git commit -m "feat: add MCP server with all hive tools (spawn, task, message, merge, observe)"
```

---

## Phase 6: TUI Dashboard

### Task 6.1: Basic TUI with agent and task panels

**Files:**
- Create: `src/tui.rs`
- Modify: `src/main.rs` (wire up `cmd_tui`)

**Step 1: Write the TUI module**

Uses `ratatui` with `crossterm` backend. Reads `.hive/` state every tick (1 second). Displays panels for agents, tasks, merge queue, and messages.

```rust
// src/tui.rs
use crate::state::HiveState;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::io::stdout;
use std::time::{Duration, Instant};

pub fn run_tui() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    enable_raw_mode().map_err(|e| e.to_string())?;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;

    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;

    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();

    loop {
        // Read state
        let agents = state.list_agents(&run_id).unwrap_or_default();
        let tasks = state.list_tasks(&run_id).unwrap_or_default();
        let queue = state
            .load_merge_queue(&run_id)
            .unwrap_or(crate::types::MergeQueue {
                entries: vec![],
            });
        let messages = state.list_messages(&run_id).unwrap_or_default();

        terminal
            .draw(|frame| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),  // Title
                        Constraint::Min(6),     // Main content
                        Constraint::Length(8),   // Messages
                    ])
                    .split(frame.area());

                // Title bar
                let title = Paragraph::new(format!(" Hive TUI — Run: {run_id}"))
                    .style(Style::default().fg(Color::Cyan).bold())
                    .block(Block::default().borders(Borders::BOTTOM));
                frame.render_widget(title, chunks[0]);

                // Main content: agents | tasks | queue
                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(30),
                        Constraint::Percentage(45),
                        Constraint::Percentage(25),
                    ])
                    .split(chunks[1]);

                // Agents panel
                let agent_rows: Vec<Row> = agents
                    .iter()
                    .map(|a| {
                        let status_style = match a.status {
                            crate::types::AgentStatus::Running => Style::default().fg(Color::Green),
                            crate::types::AgentStatus::Done => Style::default().fg(Color::Blue),
                            crate::types::AgentStatus::Failed => Style::default().fg(Color::Red),
                            crate::types::AgentStatus::Stalled => Style::default().fg(Color::Yellow),
                        };
                        Row::new(vec![
                            Cell::from(a.id.clone()),
                            Cell::from(format!("{:?}", a.role)),
                            Cell::from(format!("{:?}", a.status)).style(status_style),
                        ])
                    })
                    .collect();
                let agent_table = Table::new(
                    agent_rows,
                    [
                        Constraint::Min(15),
                        Constraint::Min(10),
                        Constraint::Min(10),
                    ],
                )
                .header(Row::new(vec!["Agent", "Role", "Status"]).bold())
                .block(Block::default().title(" Agents ").borders(Borders::ALL));
                frame.render_widget(agent_table, main_chunks[0]);

                // Tasks panel
                let task_rows: Vec<Row> = tasks
                    .iter()
                    .map(|t| {
                        Row::new(vec![
                            Cell::from(t.id.clone()),
                            Cell::from(format!("{:?}", t.status)),
                            Cell::from(format!("{:?}", t.urgency)),
                            Cell::from(t.title.chars().take(30).collect::<String>()),
                        ])
                    })
                    .collect();
                let task_table = Table::new(
                    task_rows,
                    [
                        Constraint::Min(12),
                        Constraint::Min(10),
                        Constraint::Min(8),
                        Constraint::Min(20),
                    ],
                )
                .header(Row::new(vec!["ID", "Status", "Urgency", "Title"]).bold())
                .block(Block::default().title(" Tasks ").borders(Borders::ALL));
                frame.render_widget(task_table, main_chunks[1]);

                // Merge queue panel
                let queue_items: Vec<ListItem> = queue
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        ListItem::new(format!("{}. {} ({})", i + 1, e.branch, e.task_id))
                    })
                    .collect();
                let queue_list = List::new(queue_items)
                    .block(Block::default().title(" Merge Queue ").borders(Borders::ALL));
                frame.render_widget(queue_list, main_chunks[2]);

                // Messages panel (last N messages)
                let recent: Vec<ListItem> = messages
                    .iter()
                    .rev()
                    .take(6)
                    .map(|m| {
                        ListItem::new(format!(
                            "[{}] {} -> {}: {}",
                            m.timestamp.format("%H:%M:%S"),
                            m.from,
                            m.to,
                            m.body.chars().take(60).collect::<String>()
                        ))
                    })
                    .collect();
                let msg_list = List::new(recent)
                    .block(Block::default().title(" Messages (recent) ").borders(Borders::ALL));
                frame.render_widget(msg_list, chunks[2]);
            })
            .map_err(|e| e.to_string())?;

        // Handle input
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    break;
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    disable_raw_mode().map_err(|e| e.to_string())?;
    stdout()
        .execute(LeaveAlternateScreen)
        .map_err(|e| e.to_string())?;

    Ok(())
}
```

**Step 2: Wire up cmd_tui in main.rs**

Replace the `cmd_tui` function:

```rust
fn cmd_tui() -> Result<(), String> {
    crate::tui::run_tui()
}
```

Add `mod tui;` to the top of `main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Test manually**

From the hive repo (after running `hive init` and `hive start some-spec.md`):
Run: `cargo run -- tui`
Expected: TUI renders with empty panels. Press `q` to exit.

**Step 5: Commit**

```bash
git add src/tui.rs src/main.rs
git commit -m "feat: add TUI dashboard with agent, task, queue, and message panels"
```

---

## Phase 7: Integration and Polish

### Task 7.1: Wire up `hive start` to spawn the coordinator

**Files:**
- Modify: `src/main.rs` (update `cmd_start`)

**Step 1: Update cmd_start to spawn coordinator**

Replace the TODO in `cmd_start`:

```rust
fn cmd_start(spec_path: &str) -> Result<(), String> {
    let state = HiveState::discover()?;
    let spec_content = fs::read_to_string(spec_path)
        .map_err(|e| format!("Cannot read spec file '{spec_path}': {e}"))?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;
    state.save_spec(&run_id, &spec_content)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    println!("Created run: {run_id}");

    // Write coordinator CLAUDE.local.md to the base repo
    let coordinator_prompt = crate::agent::AgentSpawner::coordinator_prompt(&run_id, &spec_content);
    let repo_root = state.repo_root();
    fs::write(repo_root.join("CLAUDE.local.md"), &coordinator_prompt)
        .map_err(|e| e.to_string())?;

    // Write .claude/settings.local.json for coordinator MCP
    let claude_dir = repo_root.join(".claude");
    fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
    let settings = serde_json::json!({
        "mcpServers": {
            "hive": {
                "command": "hive",
                "args": ["mcp", "--run", &run_id, "--agent", "coordinator"]
            }
        }
    });
    fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Register coordinator agent (no PID yet — user launches claude manually)
    let coordinator = Agent {
        id: "coordinator".to_string(),
        role: AgentRole::Coordinator,
        status: AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: Some(chrono::Utc::now()),
        task_id: None,
    };
    state.save_agent(&run_id, &coordinator)?;

    println!("Coordinator configured. Launch Claude Code in this directory to begin.");
    println!("Run 'hive tui' in another terminal to monitor progress.");

    Ok(())
}
```

**Step 2: Add `coordinator_prompt` method to AgentSpawner**

Add to `src/agent.rs`:

```rust
impl AgentSpawner {
    pub fn coordinator_prompt(run_id: &str, spec_content: &str) -> String {
        format!(
            r#"You are the coordinator agent in a hive swarm.
Run ID: {run_id}
Agent ID: coordinator
Role: coordinator

## Spec
{spec_content}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.
"#
        )
    }
    // ... existing methods ...
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles

**Step 4: Commit**

```bash
git add src/main.rs src/agent.rs
git commit -m "feat: wire up hive start to configure coordinator agent"
```

---

### Task 7.2: Role-based permission enforcement in MCP

**Files:**
- Modify: `src/mcp.rs`

The MCP tools should enforce the permission table from the spec. For example, workers cannot call `hive_spawn_agent` or `hive_submit_to_queue`.

**Step 1: Add role lookup helper**

Add a helper method to `HiveMcp`:

```rust
fn agent_role(&self) -> AgentRole {
    self.state()
        .load_agent(&self.run_id, &self.agent_id)
        .map(|a| a.role)
        .unwrap_or(AgentRole::Worker) // default to most restricted
}

fn require_role(&self, allowed: &[AgentRole]) -> Result<(), CallToolResult> {
    let role = self.agent_role();
    if allowed.contains(&role) {
        Ok(())
    } else {
        Err(CallToolResult::error(vec![Content::text(format!(
            "Permission denied: {:?} cannot use this tool.",
            role
        ))]))
    }
}
```

**Step 2: Add permission checks to each tool**

At the top of each tool method, add the appropriate check:

- `hive_spawn_agent`: `require_role(&[Coordinator, Lead])?`
- `hive_submit_to_queue`: `require_role(&[Lead])?`
- `hive_merge_next`: `require_role(&[Coordinator])?`
- `hive_create_task`: all roles allowed (but workers create with type=suggestion)
- `hive_update_task`: all roles allowed
- `hive_list_tasks`: all roles allowed
- `hive_send_message`: all roles allowed (message routing validates recipient)
- `hive_list_agents`: all roles allowed
- `hive_check_agents`: `require_role(&[Coordinator, Lead])?`
- `hive_log_tool`: all roles allowed

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles

**Step 4: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: enforce role-based permissions on MCP tools"
```

---

### Task 7.3: Message routing validation

**Files:**
- Modify: `src/mcp.rs`

Workers should only send messages to their lead. Leads should only message their workers and the coordinator.

**Step 1: Add validation to `hive_send_message`**

After constructing the message, before saving:

```rust
// Validate routing
let sender_role = self.agent_role();
match sender_role {
    AgentRole::Worker => {
        // Workers can only message their parent lead
        let sender = self.state().load_agent(&self.run_id, &self.agent_id).ok();
        let parent = sender.and_then(|a| a.parent);
        if parent.as_deref() != Some(&p.to) {
            return Ok(CallToolResult::error(vec![Content::text(
                "Workers can only send messages to their lead."
            )]));
        }
    }
    AgentRole::Lead => {
        // Leads can message their workers or the coordinator
        let target = self.state().load_agent(&self.run_id, &p.to).ok();
        let valid = match target {
            Some(ref t) if t.parent.as_deref() == Some(&self.agent_id) => true,
            _ if p.to == "coordinator" => true,
            _ => false,
        };
        if !valid {
            return Ok(CallToolResult::error(vec![Content::text(
                "Leads can only message their workers or the coordinator."
            )]));
        }
    }
    AgentRole::Coordinator => {
        // Coordinator can message any lead
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: compiles

**Step 3: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: add message routing validation based on agent hierarchy"
```

---

### Task 7.4: Cleanup command

**Files:**
- Modify: `src/cli.rs` (add `Stop` command)
- Modify: `src/main.rs` (add `cmd_stop`)

**Step 1: Add Stop command to CLI**

Add to the `Commands` enum in `src/cli.rs`:

```rust
/// Stop the current run and clean up worktrees
Stop,
```

**Step 2: Implement cmd_stop**

```rust
fn cmd_stop() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let agents = state.list_agents(&run_id)?;

    // Kill agent processes
    for agent in &agents {
        if let Some(pid) = agent.pid {
            if crate::agent::AgentSpawner::is_alive(pid) {
                unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                println!("Stopped agent {} (pid {})", agent.id, pid);
            }
        }
    }

    // Remove worktrees
    for agent in &agents {
        if let Some(ref wt) = agent.worktree {
            let wt_path = std::path::Path::new(wt);
            if wt_path.exists() {
                match crate::git::Git::worktree_remove(state.repo_root(), wt_path) {
                    Ok(()) => println!("Removed worktree for {}", agent.id),
                    Err(e) => eprintln!("Warning: failed to remove worktree for {}: {e}", agent.id),
                }
            }
        }
    }

    crate::git::Git::worktree_prune(state.repo_root()).ok();

    // Clean up coordinator files
    let repo_root = state.repo_root();
    let _ = std::fs::remove_file(repo_root.join("CLAUDE.local.md"));
    let _ = std::fs::remove_file(repo_root.join(".claude/settings.local.json"));

    println!("Run {run_id} stopped.");
    Ok(())
}
```

**Step 3: Wire up in main.rs**

Add `Commands::Stop => cmd_stop(),` to the match.

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles

**Step 5: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add hive stop command for cleanup"
```

---

## Summary

| Phase | Tasks | What it delivers |
|-------|-------|-----------------|
| 1 | 1.1, 1.2 | Rust project with all types defined |
| 2 | 2.1, 2.2 | State layer: .hive/ filesystem + SQLite logging |
| 3 | 3.1, 3.2 | Git operations + agent spawn sequence |
| 4 | 4.1 | Full CLI with all commands |
| 5 | 5.1 | MCP server with all hive tools |
| 6 | 6.1 | TUI monitoring dashboard |
| 7 | 7.1-7.4 | Integration: coordinator wiring, permissions, routing, cleanup |

After Phase 7, hive is a working end-to-end system: `hive init` -> `hive start spec.md` -> coordinator uses MCP tools to spawn leads/workers -> agents work in worktrees -> leads submit to merge queue -> coordinator merges -> `hive tui` shows it all -> `hive stop` cleans up.
