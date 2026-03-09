# Fix Lead Delegation Regression

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the regression where lead agents implement code themselves instead of spawning workers, by reverting the lead prompt to the proven run-8 style, adding Bash write-blocking to lead hooks, and reducing spawn friction with an inline `task` parameter.

**Architecture:** Three changes — (1) revert lead prompt to action-oriented delegation style, (2) add Bash write-command blocking to lead hooks (matching the reviewer pattern), (3) add optional `task` text field to `SpawnAgentParams` that auto-creates a task, making `task_id` and `task` mutually exclusive.

**Tech Stack:** Rust, serde/serde_json, rmcp MCP tools, chrono, uuid.

---

### Task 1: Revert Lead Prompt to Run-8 Delegation Style

**Files:**
- Modify: `src/agent.rs:339-395` (lead prompt in `generate_prompt`)
- Modify: `src/agent.rs:133-137` (lead task_description in `spawn`)
- Modify: `src/agent.rs:802-815` (test `lead_prompt_has_delegation_review_health`)

**Step 1: Write the failing test**

Update the test at line 802 to assert the old proven wording instead of the current wording:

```rust
#[test]
fn lead_prompt_has_delegation_review_health() {
    let prompt = AgentSpawner::generate_prompt(
        "lead-1",
        AgentRole::Lead,
        Some("coord-1"),
        "Handle backend",
        "",
    );
    assert!(prompt.contains("## Delegation Protocol"));
    assert!(prompt.contains("ALWAYS spawn workers for implementation"));
    assert!(prompt.contains("write a detailed implementation plan"));
    assert!(!prompt.contains("FORBIDDEN"));
    assert!(!prompt.contains("blocked by hooks"));
    assert!(prompt.contains("## Code Review Protocol"));
    assert!(prompt.contains("hive_review_agent"));
    assert!(prompt.contains("## Health Monitoring"));
    assert!(prompt.contains("hive_check_agents every 60 seconds"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test lead_prompt_has_delegation_review_health -- --nocapture`
Expected: FAIL — current prompt has "FORBIDDEN" and lacks "write a detailed implementation plan"

**Step 3: Revert the lead prompt in `generate_prompt`**

Replace the Delegation Protocol and Constraints sections in the `AgentRole::Lead` match arm (lines 362-392) with:

```rust
## Delegation Protocol
- ALWAYS spawn workers for implementation. You are a manager, not an implementer.
- Read the relevant source files to understand the codebase, then write a detailed implementation plan.
- Create subtasks via hive_create_task, then spawn one worker per task via hive_spawn_agent.
- Spawn one worker per logical unit of work (usually one file or one feature).
- After spawning workers, use hive_wait_for_activity and hive_check_agents to monitor.
- When workers finish, review their work with hive_review_agent before submitting.
```

And revert the Constraints section to:

```rust
## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
- When you have nothing to do, stop and wait to be resumed. Do not loop.
```

Remove: "You MUST NOT implement code yourself", "FORBIDDEN", "blocked by hooks" lines.

**Step 4: Revert the lead task_description in `spawn`**

Replace lines 133-137 with the simpler framing that doesn't duplicate delegation instructions (those belong in the prompt, not the task description):

```rust
let task_description = if matches!(role, AgentRole::Lead) {
    format!(
        "You are managing task `{}`: `{}`.\n\n{}",
        task.id, task.title, task.description
    )
} else {
    format!(
        "Your task is `{}`: `{}`.\n\n{}\n\nWhen done, call `hive_update_task(status=review)`.",
        task.id, task.title, task.description
    )
};
```

**Step 5: Run test to verify it passes**

Run: `cargo test lead_prompt_has_delegation_review_health -- --nocapture`
Expected: PASS

**Step 6: Run full test suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass, zero warnings

**Step 7: Commit**

```bash
git add src/agent.rs
git commit -m "$(cat <<'EOF'
fix: revert lead prompt to proven run-8 delegation style

The lead prompt was changed to use threatening language ("FORBIDDEN",
"blocked by hooks") which was less effective than the original
action-oriented style ("ALWAYS spawn workers", "write a detailed
implementation plan"). Revert to the style that produced 23 agents
with proper delegation in run 8.
EOF
)"
```

---

### Task 2: Add Bash Write-Blocking to Lead Hooks

**Files:**
- Modify: `src/agent.rs:86-100` (lead settings_json block)

**Step 1: Write the failing test**

Add a new test that verifies the lead hooks block Bash write commands:

```rust
#[test]
fn lead_hooks_block_bash_writes() {
    // Verify the lead settings_json contains a Bash PreToolUse hook
    // We test the JSON structure directly since we can't run hooks in tests
    let settings_json = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Edit|Write|NotebookEdit",
                    "hooks": [{
                        "type": "command",
                        "command": "echo 'BLOCKED' >&2 && exit 2"
                    }]
                },
                {
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "if echo \"$TOOL_INPUT\" | jq -r '.command' | grep -qE '(>|>>|tee |sed -i|perl -.*-i|awk .*-i)'; then echo 'BLOCKED: Lead agents must delegate implementation to workers.' >&2 && exit 2; fi"
                    }]
                }
            ]
        }
    });
    let pre_tool = settings_json["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre_tool.len(), 2);
    assert_eq!(pre_tool[1]["matcher"], "Bash");
    let cmd = pre_tool[1]["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.contains("BLOCKED"));
    assert!(cmd.contains("sed -i"));
    assert!(cmd.contains("tee "));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test lead_hooks_block_bash_writes -- --nocapture`
Expected: FAIL — test doesn't exist yet

**Step 3: Add Bash write-blocking to lead hooks**

In the lead `settings_json` block (line 86), add a second PreToolUse entry for Bash, matching the pattern used by reviewers but tailored for leads:

```rust
} else if matches!(role, AgentRole::Lead) {
    // Lead agents: block Edit/Write and destructive Bash to enforce delegation
    serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Edit|Write|NotebookEdit",
                    "hooks": [{
                        "type": "command",
                        "command": "echo 'BLOCKED: Lead agents must delegate implementation to workers via hive_spawn_agent.' >&2 && exit 2"
                    }]
                },
                {
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "if echo \"$TOOL_INPUT\" | jq -r '.command' | grep -qE '(>|>>|tee |sed -i|perl -.*-i|awk .*-i)'; then echo 'BLOCKED: Lead agents must delegate implementation to workers via hive_spawn_agent.' >&2 && exit 2; fi"
                        )
                    }]
                }
            ],
            "PostToolUse": post_tool_hooks,
            "Stop": stop_hooks
        }
    })
}
```

**Step 4: Run tests to verify**

Run: `cargo test lead_hooks_block_bash_writes -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass

**Step 6: Commit**

```bash
git add src/agent.rs
git commit -m "$(cat <<'EOF'
fix: add Bash write-blocking to lead hooks

Leads were bypassing Edit/Write hooks by using Bash commands like
sed -i, shell redirection (>), and tee. Add a Bash PreToolUse hook
that blocks common write patterns, matching the reviewer hook pattern.
EOF
)"
```

---

### Task 3: Add Inline `task` Field to `hive_spawn_agent`

**Files:**
- Modify: `src/mcp.rs:30-37` (SpawnAgentParams)
- Modify: `src/mcp.rs:222-310` (hive_spawn_agent implementation)
- Modify: `src/mcp.rs` (existing spawn tests)

**Step 1: Write the failing test**

Add a test that validates the mutually-exclusive `task` / `task_id` fields and the auto-create behavior:

```rust
#[tokio::test]
async fn spawn_with_inline_task_creates_task_and_spawns() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    // Create inline — no pre-existing task needed
    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-test".into(),
        role: "lead".into(),
        task_id: None,
        task: Some("Implement the widget feature".into()),
    });
    // This will fail at the spawn step (no git repo) but should get past validation
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    // The task should have been created even if spawn fails
    let tasks = mcp.state().list_tasks(&mcp.run_id).unwrap_or_default();
    assert!(tasks.iter().any(|t| t.title == "Implement the widget feature"));
}

#[tokio::test]
async fn spawn_rejects_both_task_id_and_task() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-test".into(),
        role: "lead".into(),
        task_id: Some("task-123".into()),
        task: Some("Some text".into()),
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn spawn_rejects_neither_task_id_nor_task() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-test".into(),
        role: "lead".into(),
        task_id: None,
        task: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test spawn_with_inline_task spawn_rejects_both spawn_rejects_neither -- --nocapture`
Expected: FAIL — `SpawnAgentParams` doesn't have `task` field yet

**Step 3: Update `SpawnAgentParams`**

Replace the struct at line 30:

```rust
#[derive(Deserialize, JsonSchema)]
pub struct SpawnAgentParams {
    /// Agent ID (unique name like "lead-backend" or "worker-001")
    pub agent_id: String,
    /// Role: "lead" or "worker"
    pub role: String,
    /// Task ID to assign (mutually exclusive with `task`)
    pub task_id: Option<String>,
    /// Inline task description — auto-creates a task (mutually exclusive with `task_id`)
    pub task: Option<String>,
}
```

**Step 4: Update `hive_spawn_agent` implementation**

Replace the task loading section (lines 267-292) with validation + auto-create logic:

```rust
// Validate mutually exclusive task_id / task
let task = match (&p.task_id, &p.task) {
    (Some(_), Some(_)) => {
        return Ok(CallToolResult::error(vec![Content::text(
            "Provide either 'task_id' or 'task', not both.",
        )]));
    }
    (None, None) => {
        return Ok(CallToolResult::error(vec![Content::text(
            "Provide either 'task_id' (existing task) or 'task' (auto-create).",
        )]));
    }
    (Some(tid), None) => {
        // Existing task
        match state.load_task(&self.run_id, tid) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Failed to load task '{tid}': {e}"
                ))]));
            }
        }
    }
    (None, Some(desc)) => {
        // Auto-create task
        let task_id = format!("task-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = Utc::now();
        Task {
            id: task_id,
            title: desc.clone(),
            description: desc.clone(),
            status: TaskStatus::Pending,
            urgency: Urgency::Normal,
            blocking: vec![],
            blocked_by: vec![],
            assigned_to: None,
            created_by: self.agent_id.clone(),
            parent_task: None,
            branch: None,
            domain: None,
            review_count: 0,
            created_at: now,
            updated_at: now,
        }
    }
};
let mut task = task;
```

Keep the existing assignment and save logic after this block unchanged.

**Step 5: Update existing spawn tests**

All existing tests that construct `SpawnAgentParams` with `task_id: "..."` need to change to `task_id: Some("...".into()), task: None`. Search for all `SpawnAgentParams {` in the test module and update. There are approximately 3 existing tests (`spawn_rejects_reviewer_role`, `spawn_rejects_planner_role`, `spawn_hierarchy_coordinator_spawns_lead`).

**Step 6: Run tests**

Run: `cargo test --all-targets`
Expected: All pass including new tests (the inline-task test may partially fail at git worktree step but should validate task creation)

**Step 7: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: Zero warnings

**Step 8: Commit**

```bash
git add src/mcp.rs
git commit -m "$(cat <<'EOF'
feat: add inline task field to hive_spawn_agent

Add optional `task` text field to SpawnAgentParams, mutually exclusive
with `task_id`. When `task` is provided, a task is auto-created before
spawning the agent. This reduces the friction of spawning workers from
two MCP calls (create_task + spawn_agent) to one.
EOF
)"
```

---

### Task 4: Update Lead Prompt to Mention Inline Task Shortcut

**Files:**
- Modify: `src/agent.rs:362-370` (Delegation Protocol in lead prompt)

**Step 1: Update the delegation protocol**

After completing Task 3, update the lead prompt's Delegation Protocol to mention the shortcut:

```
## Delegation Protocol
- ALWAYS spawn workers for implementation. You are a manager, not an implementer.
- Read the relevant source files to understand the codebase, then write a detailed implementation plan.
- Spawn one worker per logical unit of work via hive_spawn_agent with an inline `task` description.
- After spawning workers, use hive_wait_for_activity and hive_check_agents to monitor.
- When workers finish, review their work with hive_review_agent before submitting.
```

**Step 2: Run tests**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass (update any prompt assertion tests if they check for exact wording)

**Step 3: Commit**

```bash
git add src/agent.rs
git commit -m "$(cat <<'EOF'
fix: update lead prompt to mention inline task shortcut

Tell leads to use the simpler hive_spawn_agent(task=...) form
instead of the two-step create_task + spawn_agent workflow.
EOF
)"
```

---

## Ordering

```
Task 1 (Revert Lead Prompt) — no dependencies
Task 2 (Bash Write-Blocking Hooks) — no dependencies
Task 3 (Inline Task Field) — no dependencies
Task 4 (Update Prompt for Shortcut) — depends on Task 3
```

Tasks 1, 2, and 3 can be done in parallel. Task 4 is a small follow-up after Task 3.
