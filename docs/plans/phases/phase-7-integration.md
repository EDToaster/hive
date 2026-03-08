# Phase 7: Integration and Polish

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire up hive start to spawn the coordinator, enforce role-based permissions, add message routing validation, and implement cleanup via hive stop.

**Prerequisite:** Phase 6 complete (TUI working).

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

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

---

## Done

All phases complete. Hive is a working end-to-end system.
