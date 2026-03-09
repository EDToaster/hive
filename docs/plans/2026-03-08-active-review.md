# Active Review Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a reviewer agent role that automatically reviews every branch submission before it enters the merge queue, enforcing quality through read-only code review with hook-based write blocking.

**Architecture:** When a lead calls `hive_submit_to_queue`, instead of directly queuing the branch, hive spawns a reviewer agent in the submitter's worktree. The reviewer evaluates the diff against the task description and submits a verdict (approve/request-changes/reject). Approved branches enter the merge queue. Request-changes sends feedback to the original agent. Reviewers are read-only — enforced by hooks that block write tools.

**Tech Stack:** Rust, rmcp MCP framework, serde, chrono, existing hive infrastructure.

---

### Task 1: Add Reviewer to AgentRole enum

**Files:**
- Modify: `src/types.rs:6-12` (AgentRole enum)
- Modify: `src/types.rs:66-81` (Task struct — add review_count field)
- Modify: `src/types.rs:107-118` (MergeQueueEntry — add reviewer tracking)

**Step 1: Add Reviewer variant and review_count field**

In `src/types.rs`, add `Reviewer` to the `AgentRole` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Coordinator,
    Lead,
    Worker,
    Reviewer,
}
```

Add `review_count` to the `Task` struct (with `#[serde(default)]` for backward compat):

```rust
pub struct Task {
    // ... existing fields ...
    #[serde(default)]
    pub review_count: u32,
}
```

**Step 2: Update existing tests**

Update `agent_role_serializes_lowercase` test to include Reviewer:

```rust
assert_eq!(serde_json::to_string(&AgentRole::Reviewer).unwrap(), "\"reviewer\"");
let role: AgentRole = serde_json::from_str("\"reviewer\"").unwrap();
assert_eq!(role, AgentRole::Reviewer);
```

Add test for `review_count` backward compat:

```rust
#[test]
fn task_without_review_count_defaults_to_zero() {
    // Serialize a task without review_count, verify it deserializes with 0
    let json = r#"{"id":"t1","title":"test","description":"d","status":"pending","urgency":"normal","blocking":[],"blocked_by":[],"assigned_to":null,"created_by":"coord","parent_task":null,"branch":null,"domain":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;
    let task: Task = serde_json::from_str(json).unwrap();
    assert_eq!(task.review_count, 0);
}
```

**Step 3: Run tests**

Run: `cargo test --all-targets`
Expected: All 123+ tests pass, including new ones.

**Step 4: Commit**

```bash
git add src/types.rs
git commit -m "feat: add Reviewer role and review_count to Task"
```

---

### Task 2: Add reviewer prompt and read-only hooks to agent.rs

**Files:**
- Modify: `src/agent.rs:228-378` (generate_prompt — add Reviewer match arm)
- Modify: `src/agent.rs:28-66` (spawn — add reviewer hooks variant)

**Step 1: Add reviewer prompt to generate_prompt**

Add a new match arm for `AgentRole::Reviewer` in the `generate_prompt` function at `src/agent.rs:234`. The reviewer prompt should include:

```rust
AgentRole::Reviewer => format!(
    r#"You are a reviewer agent in a hive swarm.
Agent ID: {agent_id}
Role: reviewer
Parent: {}

## Your Review Task
{task_description}

## Responsibilities
- Review the code changes on this branch against the task description.
- Evaluate: correctness, completeness, code quality, scope discipline.
- Check that tests were added/updated and pass.
- Check that no unrelated files were modified.
- Submit your verdict via hive_review_verdict.

## Verdict Options
- **approve**: Code correctly implements the task, tests pass, no issues.
- **request-changes**: Code has specific issues that need fixing. Provide clear, actionable feedback.
- **reject**: Fundamentally wrong approach or task cannot be completed this way.

## Constraints
- You are READ-ONLY. Do NOT modify any files. Do NOT use Edit, Write, or Bash to change files.
- Only use Read, Glob, Grep to examine code.
- Use hive MCP tools only: hive_review_verdict, hive_read_messages, hive_list_tasks.
- Review the diff by reading the changed files and comparing to the task intent.
- Be thorough but concise. Focus on correctness over style.
- After submitting your verdict, stop immediately.
"#,
    parent.unwrap_or("coordinator")
),
```

**Step 2: Add read-only hooks for reviewer agents**

In the `spawn` function at `src/agent.rs:28-66`, modify the hooks generation to use a different hooks config when `role == AgentRole::Reviewer`. The reviewer hooks should block write operations:

```rust
let settings_json = if role == AgentRole::Reviewer {
    serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Edit|Write|NotebookEdit",
                "hooks": [{
                    "type": "command",
                    "command": "echo 'BLOCKED: Reviewer agents are read-only. Do not modify files.' >&2 && exit 2"
                }]
            }, {
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": "if echo \"$TOOL_INPUT\" | jq -r '.command' | grep -qE '(>|>>|tee |rm |mv |cp |chmod |sed -i|mkdir|touch|git add|git commit|git push|cargo fmt)'; then echo 'BLOCKED: Reviewer agents are read-only.' >&2 && exit 2; fi"
                }]
            }],
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [
                    {
                        "type": "command",
                        "command": format!(
                            "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent {agent_id} --tool {{}} --status success"
                        )
                    },
                    {
                        "type": "command",
                        "command": format!(
                            "hive heartbeat --run {run_id} --agent {agent_id}"
                        )
                    }
                ]
            }],
            "Stop": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "hive read-messages --agent {agent_id} --run {run_id} --unread --stop-hook"
                    )
                }]
            }]
        }
    })
} else {
    // ... existing hooks ...
};
```

**Step 3: Add tests**

```rust
#[test]
fn reviewer_prompt_contains_readonly_constraints() {
    let prompt = AgentSpawner::generate_prompt(
        "reviewer-1",
        AgentRole::Reviewer,
        Some("lead-1"),
        "Review the changes for task-123",
    );
    assert!(prompt.contains("Agent ID: reviewer-1"));
    assert!(prompt.contains("Role: reviewer"));
    assert!(prompt.contains("READ-ONLY"));
    assert!(prompt.contains("hive_review_verdict"));
    assert!(prompt.contains("Do NOT modify any files"));
}
```

**Step 4: Run tests**

Run: `cargo test --all-targets`
Expected: PASS

**Step 5: Commit**

```bash
git add src/agent.rs
git commit -m "feat: add reviewer prompt with read-only hook enforcement"
```

---

### Task 3: Add hive_review_verdict MCP tool

**Files:**
- Modify: `src/mcp.rs` (add ReviewVerdictParams, hive_review_verdict tool, add InReview to task status flow)

**Step 1: Add the parameter type**

After the existing param types (around line 60 in mcp.rs), add:

```rust
#[derive(Deserialize, JsonSchema)]
pub struct ReviewVerdictParams {
    /// Task ID being reviewed
    pub task_id: String,
    /// Verdict: "approve", "request-changes", or "reject"
    pub verdict: String,
    /// Feedback message (required for request-changes and reject)
    pub feedback: Option<String>,
}
```

**Step 2: Add the tool implementation**

Add the `hive_review_verdict` tool to the HiveMcp impl block. This tool:
- Is only callable by Reviewer agents
- On **approve**: sets task status to Queued, adds branch to merge queue, notifies coordinator
- On **request-changes**: increments task review_count, sends feedback message to the agent that submitted the work, sets task status back to Active so the agent can resume
- On **reject**: marks task as Failed, notifies the lead

```rust
#[tool(description = "Submit a review verdict for a task. Reviewer-only.")]
async fn hive_review_verdict(
    &self,
    params: Parameters<ReviewVerdictParams>,
) -> Result<CallToolResult, McpError> {
    if let Err(result) = self.require_role(&[AgentRole::Reviewer]) {
        return Ok(result);
    }
    let p = &params.0;
    let state = self.state();

    let mut task = match state.load_task(&self.run_id, &p.task_id) {
        Ok(t) => t,
        Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
    };

    match p.verdict.as_str() {
        "approve" => {
            task.status = TaskStatus::Queued;
            task.updated_at = Utc::now();
            state.save_task(&self.run_id, &task).map_err(|e| McpError::internal_error(e, None))?;

            // Add to merge queue
            let branch = task.branch.clone().unwrap_or_default();
            let submitted_by = task.assigned_to.clone().unwrap_or_default();
            let _lock = state.lock_file("merge-queue").map_err(|e| McpError::internal_error(e, None))?;
            let mut queue = state.load_merge_queue(&self.run_id).unwrap_or(MergeQueue { entries: vec![] });
            queue.entries.push(MergeQueueEntry {
                task_id: p.task_id.clone(),
                branch: branch.clone(),
                submitted_by: submitted_by.clone(),
                submitted_at: Utc::now(),
            });
            state.save_merge_queue(&self.run_id, &queue).map_err(|e| McpError::internal_error(e, None))?;

            // Notify coordinator
            let msg = format!("Review approved for task '{}'. Branch '{}' added to merge queue.", p.task_id, branch);
            let _ = state.save_message(&self.run_id, &Message {
                id: uuid::Uuid::new_v4().to_string(),
                from: self.agent_id.clone(),
                to: "coordinator".to_string(),
                timestamp: Utc::now(),
                message_type: MessageType::Status,
                body: msg.clone(),
                refs: vec![p.task_id.clone()],
            });

            Ok(CallToolResult::success(vec![Content::text(msg)]))
        }
        "request-changes" => {
            let feedback = p.feedback.as_deref().unwrap_or("Changes requested (no details provided)");
            task.review_count += 1;
            task.status = TaskStatus::Active;
            task.updated_at = Utc::now();
            state.save_task(&self.run_id, &task).map_err(|e| McpError::internal_error(e, None))?;

            // Send feedback to the agent that worked on this task
            if let Some(ref assigned) = task.assigned_to {
                let _ = state.save_message(&self.run_id, &Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: self.agent_id.clone(),
                    to: assigned.clone(),
                    timestamp: Utc::now(),
                    message_type: MessageType::Request,
                    body: format!("Review feedback for task '{}' (review cycle {}):\n{}", p.task_id, task.review_count, feedback),
                    refs: vec![p.task_id.clone()],
                });
            }

            Ok(CallToolResult::success(vec![Content::text(format!(
                "Requested changes for task '{}' (review cycle {}). Feedback sent to {}.",
                p.task_id, task.review_count, task.assigned_to.as_deref().unwrap_or("unknown")
            ))]))
        }
        "reject" => {
            let feedback = p.feedback.as_deref().unwrap_or("Rejected (no details provided)");
            task.status = TaskStatus::Failed;
            task.updated_at = Utc::now();
            state.save_task(&self.run_id, &task).map_err(|e| McpError::internal_error(e, None))?;

            // Notify the lead
            if let Some(ref assigned) = task.assigned_to {
                // Find the agent to get its parent (the lead)
                if let Ok(agent) = state.load_agent(&self.run_id, assigned) {
                    if let Some(ref parent) = agent.parent {
                        let _ = state.save_message(&self.run_id, &Message {
                            id: uuid::Uuid::new_v4().to_string(),
                            from: self.agent_id.clone(),
                            to: parent.clone(),
                            timestamp: Utc::now(),
                            message_type: MessageType::Status,
                            body: format!("Task '{}' rejected by reviewer:\n{}", p.task_id, feedback),
                            refs: vec![p.task_id.clone()],
                        });
                    }
                }
            }

            Ok(CallToolResult::success(vec![Content::text(format!(
                "Task '{}' rejected: {}", p.task_id, feedback
            ))]))
        }
        _ => Ok(CallToolResult::error(vec![Content::text(
            "Invalid verdict. Use 'approve', 'request-changes', or 'reject'."
        )])),
    }
}
```

**Step 3: Run tests**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: PASS

**Step 4: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: add hive_review_verdict MCP tool"
```

---

### Task 4: Modify hive_submit_to_queue to spawn reviewer

**Files:**
- Modify: `src/mcp.rs:509-540` (hive_submit_to_queue)
- Modify: `src/agent.rs` (add spawn_reviewer helper or extend spawn)

**Step 1: Modify hive_submit_to_queue**

Instead of directly adding to the merge queue, spawn a reviewer agent. The revised flow:

```rust
#[tool(description = "Submit a branch for review before merging. Spawns a reviewer agent to evaluate the changes.")]
async fn hive_submit_to_queue(
    &self,
    params: Parameters<SubmitToQueueParams>,
) -> Result<CallToolResult, McpError> {
    if let Err(result) = self.require_role(&[AgentRole::Lead]) {
        return Ok(result);
    }
    let p = &params.0;
    let state = self.state();

    // Load the task and update its status + branch
    let _task_lock = state.lock_file(&format!("task-{}", p.task_id)).map_err(|e| McpError::internal_error(e, None))?;
    let mut task = match state.load_task(&self.run_id, &p.task_id) {
        Ok(t) => t,
        Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
    };

    // Check review cycle limit (max 3)
    if task.review_count >= 3 {
        task.status = TaskStatus::Failed;
        task.updated_at = Utc::now();
        let _ = state.save_task(&self.run_id, &task);
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "Task '{}' has exceeded the maximum review cycles (3). Marked as failed.",
            p.task_id
        ))]));
    }

    task.status = TaskStatus::Review;
    task.branch = Some(p.branch.clone());
    task.updated_at = Utc::now();
    state.save_task(&self.run_id, &task).map_err(|e| McpError::internal_error(e, None))?;

    // Spawn reviewer agent in the submitting agent's worktree
    let reviewer_id = format!("reviewer-{}", &p.task_id[..8.min(p.task_id.len())]);
    let review_description = format!(
        "Review task '{}': {}\n\nBranch: {}\nTask description: {}\n\nExamine the diff on this branch against main. Run `git log main..HEAD --oneline` and `git diff main...HEAD --stat` to see what changed. Then read the changed files and evaluate.",
        p.task_id, task.title, p.branch, task.description
    );

    // Spawn the reviewer in the same worktree as the submitting lead
    match crate::agent::AgentSpawner::spawn(
        &state,
        &self.run_id,
        &reviewer_id,
        AgentRole::Reviewer,
        Some(&self.agent_id),
        &review_description,
    ) {
        Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
            "Spawned reviewer '{}' for task '{}'. Awaiting review verdict.",
            reviewer_id, p.task_id
        ))])),
        Err(e) => {
            // If reviewer spawn fails, fall back to direct queue entry
            task.status = TaskStatus::Queued;
            task.updated_at = Utc::now();
            let _ = state.save_task(&self.run_id, &task);

            let _lock = state.lock_file("merge-queue").map_err(|e| McpError::internal_error(e, None))?;
            let mut queue = state.load_merge_queue(&self.run_id).unwrap_or(MergeQueue { entries: vec![] });
            queue.entries.push(MergeQueueEntry {
                task_id: p.task_id.clone(),
                branch: p.branch.clone(),
                submitted_by: self.agent_id.clone(),
                submitted_at: Utc::now(),
            });
            let _ = state.save_merge_queue(&self.run_id, &queue);

            Ok(CallToolResult::success(vec![Content::text(format!(
                "Warning: Failed to spawn reviewer ({e}). Branch submitted directly to merge queue as fallback."
            ))]))
        }
    }
}
```

**Step 2: Update role permissions in require_role calls**

The Reviewer role needs permission to call `hive_review_verdict`, `hive_read_messages`, `hive_list_tasks`, and `hive_check_agents`. Check all `require_role` calls and add `AgentRole::Reviewer` where appropriate:
- `hive_list_tasks`: add Reviewer (already allows all roles via no check — verify this)
- `hive_read_messages`: add Reviewer
- `hive_review_verdict`: Reviewer only (already done in Task 3)

**Step 3: Run tests**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: PASS

**Step 4: Commit**

```bash
git add src/mcp.rs src/agent.rs
git commit -m "feat: spawn reviewer on queue submission instead of direct merge"
```

---

### Task 5: Update TUI for review status

**Files:**
- Modify: `src/tui.rs:218-237` (task_status_color, task_status_bullet)
- Modify: `src/tui.rs` (agent role display)

**Step 1: Add Reviewer role to agent display**

The TUI already renders agent roles. Verify that the `AgentRole::Reviewer` variant displays correctly in the agents panel. The `Display` or `Debug` impl on AgentRole should already handle this since it's serialized as `"reviewer"`. Check the agent list rendering code and add any necessary color mapping (use Cyan for reviewer agents to distinguish from other roles).

**Step 2: Verify task status rendering**

The existing `TaskStatus::Review` already renders with yellow color and `"⊙ review"` bullet. This is sufficient — the reviewer feature uses the existing Review status. No new status variants needed.

**Step 3: Add review_count to task detail display**

If the TUI shows task details, add the review_count when > 0:
```
◉ active  Task title (review cycle 2)
```

**Step 4: Run tests**

Run: `cargo test --all-targets`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tui.rs
git commit -m "feat: add reviewer role display and review cycle count to TUI"
```

---

### Task 6: Integration — wire everything together and test end-to-end

**Files:**
- Modify: `src/mcp.rs` (ensure reviewer can be spawned by leads via hive_spawn_agent)
- Modify: `src/agent.rs` (ensure reviewer gets task_id set on Agent struct)

**Step 1: Update hive_spawn_agent permissions**

In `hive_spawn_agent`, the hierarchy check currently only allows:
- Coordinator → Lead
- Lead → Worker

Add a new allowed pair:
- Lead → Reviewer (for the submit_to_queue flow — though this is spawned internally, not via MCP)

Actually, since `hive_submit_to_queue` calls `AgentSpawner::spawn` directly (not via the MCP tool), the MCP permission check doesn't apply. But if a lead tries to manually spawn a reviewer via `hive_spawn_agent`, it should be allowed. Update the permission check:

```rust
let allowed = matches!(
    (caller_role, role),
    (AgentRole::Coordinator, AgentRole::Lead)
        | (AgentRole::Lead, AgentRole::Worker)
        | (AgentRole::Lead, AgentRole::Reviewer)
);
```

**Step 2: Set task_id on reviewer agent**

When spawning the reviewer in `hive_submit_to_queue`, set the `task_id` field on the Agent struct so the TUI and status commands can show which task each reviewer is reviewing. This requires modifying the Agent after spawn:

```rust
// After successful spawn, update the agent's task_id
if let Ok(mut reviewer_agent) = state.load_agent(&self.run_id, &reviewer_id) {
    reviewer_agent.task_id = Some(p.task_id.clone());
    let _ = state.save_agent(&self.run_id, &reviewer_agent);
}
```

**Step 3: Run full test suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All tests pass, no clippy warnings.

**Step 4: Commit**

```bash
git add src/mcp.rs src/agent.rs
git commit -m "feat: wire up reviewer permissions and task tracking"
```

---

## Lead Decomposition (for hive run)

| Lead | Tasks | Primary Files |
|------|-------|---------------|
| lead-types | Task 1 | `src/types.rs` |
| lead-agent | Task 2 | `src/agent.rs` |
| lead-mcp | Tasks 3, 4, 6 | `src/mcp.rs`, `src/agent.rs` (permissions only) |
| lead-tui | Task 5 | `src/tui.rs` |

**Merge order:**
1. lead-types (no dependencies — adds Reviewer role + review_count)
2. lead-agent (depends on types — needs Reviewer variant)
3. lead-mcp (depends on types + agent — needs Reviewer role, prompt, and spawn)
4. lead-tui (depends on types — needs Reviewer role for display)

lead-types merges first. lead-agent and lead-tui can merge in parallel after types. lead-mcp merges last.
