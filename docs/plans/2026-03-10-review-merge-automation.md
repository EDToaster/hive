# Review-to-Merge Automation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the broken post-review flow: auto-merge on approve, and keep leads alive to handle request-changes.

**Architecture:** Extract merge logic from `hive_merge_next` into a role-agnostic helper, call it from `hive_review_verdict` on approve. Update `hive_submit_to_queue` response and lead prompt to keep leads alive through the review cycle.

**Tech Stack:** Rust, `rmcp` MCP framework, `serde_json`

---

### Task 1: Extract merge logic into a reusable helper

Currently `hive_merge_next` (line 1065) contains all merge logic behind a coordinator role check. We need a private method that both `hive_merge_next` and `hive_review_verdict` can call.

**Files:**
- Modify: `src/mcp.rs:1065-1220` (hive_merge_next)

**Step 1: Write the failing test**

Add a test that calls `hive_review_verdict` with "approve" as a reviewer and verifies the task ends up with status `Merged` (not just `Queued`). This test will fail because currently approve only adds to queue.

```rust
#[tokio::test]
async fn review_verdict_approve_auto_merges() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1234", AgentRole::Reviewer);
    let state = mcp.state();
    let root = state.repo_root().to_path_buf();

    // Initialize a git repo so merge operations work
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["checkout", "-b", "main"])
        .current_dir(&root)
        .output()
        .unwrap();
    // Create initial commit on main
    std::fs::write(root.join("README.md"), "# Test").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "init"])
        .current_dir(&root)
        .output()
        .unwrap();

    // Create a branch with a change
    std::process::Command::new("git")
        .args(["checkout", "-b", "hive/test/lead-1"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::fs::write(root.join("feature.txt"), "new feature").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "feat: add feature"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["checkout", "main"])
        .current_dir(&root)
        .output()
        .unwrap();

    // Create task in Review status (as it would be after submit_to_queue)
    let mut task = make_task("task-1234", None, TaskStatus::Review);
    task.branch = Some("hive/test/lead-1".into());
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    // Save the lead agent so notification can find it
    let lead_agent = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-1234".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent("test-run", &lead_agent).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1234".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false), "verdict should succeed");

    // Task should be Merged, not just Queued
    let task = state.load_task("test-run", "task-1234").unwrap();
    assert_eq!(task.status, TaskStatus::Merged, "task should be auto-merged");

    // Merge queue should be empty (entry was processed)
    let queue = state.load_merge_queue("test-run").unwrap();
    assert!(queue.entries.is_empty(), "queue should be empty after auto-merge");
}
```

**Step 2: Run the test to verify it fails**

Run: `cargo test review_verdict_approve_auto_merges -- --nocapture`
Expected: FAIL — task status is `Queued`, not `Merged`

**Step 3: Extract `process_next_merge` from `hive_merge_next`**

Create a private method that contains the merge logic without the role check. `hive_merge_next` becomes a thin wrapper that checks the role then delegates.

In `src/mcp.rs`, add this method to `impl HiveMcp` (before `hive_merge_next`):

```rust
/// Process the next entry in the merge queue. Role-agnostic — callers must
/// ensure authorization before calling. Returns a user-facing result message.
fn process_next_merge(&self) -> Result<CallToolResult, McpError> {
    let state = self.state();
    let _lock = state
        .lock_file("merge-queue")
        .map_err(|e| McpError::internal_error(e, None))?;
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
    let repo_root = state.repo_root().to_path_buf();

    let mark_failed = |state: &HiveState, run_id: &str, task_id: &str| {
        if let Ok(mut task) = state.load_task(run_id, task_id) {
            task.status = TaskStatus::Failed;
            task.updated_at = Utc::now();
            let _ = state.save_task(run_id, &task);
        }
    };

    if let Err(e) = crate::git::Git::checkout(&repo_root, "main") {
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "Failed to checkout main before merge: {e}"
        ))]));
    }

    let merge_result = crate::git::Git::merge(&repo_root, &entry.branch);

    let merge_result = if let Err(merge_err) = merge_result {
        let _ = crate::git::Git::merge_abort(&repo_root);

        match crate::git::Git::rebase(&repo_root, &entry.branch, "main") {
            Ok(()) => {
                if let Err(e) = crate::git::Git::checkout(&repo_root, "main") {
                    mark_failed(&state, &self.run_id, &entry.task_id);
                    state.save_merge_queue(&self.run_id, &queue).ok();
                    let msg = format!(
                        "Rebase of '{}' succeeded but checkout main failed: {e}",
                        entry.branch
                    );
                    Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                    return Ok(CallToolResult::error(vec![Content::text(msg)]));
                }
                crate::git::Git::merge(&repo_root, &entry.branch)
            }
            Err(rebase_err) => {
                let _ = crate::git::Git::rebase_abort(&repo_root);
                let _ = crate::git::Git::checkout(&repo_root, "main");
                mark_failed(&state, &self.run_id, &entry.task_id);
                state.save_merge_queue(&self.run_id, &queue).ok();
                let msg = format!(
                    "Merge failed for branch '{}': {merge_err}. Auto-rebase also failed: {rebase_err}. Task marked as failed.",
                    entry.branch
                );
                Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
                return Ok(CallToolResult::error(vec![Content::text(msg)]));
            }
        }
    } else {
        merge_result
    };

    match merge_result {
        Ok(()) => {
            let config = state.load_config();
            if let Some(ref verify_cmd) = config.verify_command
                && let Err(verify_err) =
                    crate::git::Git::run_shell_command(&repo_root, verify_cmd)
            {
                if let Err(reset_err) = crate::git::Git::reset_hard(&repo_root, "HEAD~1") {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "CRITICAL: Verification failed for '{}' AND reset failed: {reset_err}. Repository may be in inconsistent state. Manual intervention required.",
                        entry.branch
                    ))]));
                }
                mark_failed(&state, &self.run_id, &entry.task_id);
                state.save_merge_queue(&self.run_id, &queue).ok();
                let truncated_output = if verify_err.len() > 500 {
                    format!("{}...(truncated)", &verify_err[..500])
                } else {
                    verify_err.clone()
                };
                let msg = format!(
                    "Verification failed for branch '{}' (task '{}'): {verify_err}",
                    entry.branch, entry.task_id
                );
                let notify_msg = format!(
                    "Verification failed for branch '{}' (task '{}').\nOutput:\n{truncated_output}",
                    entry.branch, entry.task_id
                );
                Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &notify_msg);
                return Ok(CallToolResult::error(vec![Content::text(msg)]));
            }

            let mut warnings = Vec::new();

            match state.load_task(&self.run_id, &entry.task_id) {
                Ok(mut task) => {
                    task.status = TaskStatus::Merged;
                    task.updated_at = Utc::now();
                    if let Err(e) = state.save_task(&self.run_id, &task) {
                        warnings.push(format!("Warning: failed to update task status: {e}"));
                    }
                }
                Err(e) => warnings.push(format!("Warning: failed to load task: {e}")),
            }
            if let Err(e) = state.save_merge_queue(&self.run_id, &queue) {
                warnings.push(format!("Warning: failed to save merge queue: {e}"));
            }

            let mut msg = format!(
                "Merged branch '{}' for task '{}'. Queue remaining: {}",
                entry.branch,
                entry.task_id,
                queue.entries.len()
            );
            for w in &warnings {
                msg.push_str(&format!("\n{w}"));
            }

            Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);

            Ok(CallToolResult::success(vec![Content::text(msg)]))
        }
        Err(e) => {
            let _ = crate::git::Git::merge_abort(&repo_root);
            mark_failed(&state, &self.run_id, &entry.task_id);
            state.save_merge_queue(&self.run_id, &queue).ok();
            let msg = format!(
                "Merge failed for branch '{}' after rebase: {e}. Task '{}' marked as failed.",
                entry.branch, entry.task_id
            );
            Self::notify_submitter(&state, &self.run_id, &entry.submitted_by, &msg);
            Ok(CallToolResult::error(vec![Content::text(msg)]))
        }
    }
}
```

Then simplify `hive_merge_next` to:

```rust
async fn hive_merge_next(&self) -> Result<CallToolResult, McpError> {
    if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
        return Ok(result);
    }
    self.process_next_merge()
}
```

**Step 4: Call `process_next_merge` from `hive_review_verdict` approve path**

In the "approve" arm of `hive_review_verdict`, after adding to queue and saving, drop the lock and call `process_next_merge`. Replace lines 810-855 with:

```rust
"approve" => {
    task.status = TaskStatus::Queued;
    task.updated_at = Utc::now();
    state
        .save_task(&self.run_id, &task)
        .map_err(|e| McpError::internal_error(e, None))?;

    // Add to merge queue in its own scope so the lock drops before process_next_merge
    {
        let branch = task.branch.clone().unwrap_or_default();
        let submitted_by = task.assigned_to.clone().unwrap_or_default();
        let _lock = state
            .lock_file("merge-queue")
            .map_err(|e| McpError::internal_error(e, None))?;
        let mut queue = state
            .load_merge_queue(&self.run_id)
            .unwrap_or(MergeQueue { entries: vec![] });
        queue.entries.push(MergeQueueEntry {
            task_id: p.task_id.clone(),
            branch: branch.clone(),
            submitted_by: submitted_by.clone(),
            submitted_at: Utc::now(),
        });
        state
            .save_merge_queue(&self.run_id, &queue)
            .map_err(|e| McpError::internal_error(e, None))?;
    }

    // Auto-process the merge queue
    let merge_result = self.process_next_merge()?;

    // Notify coordinator of the outcome
    let result_text = merge_result
        .content
        .first()
        .and_then(|c| match c {
            Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let _ = state.save_message(
        &self.run_id,
        &Message {
            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
            from: self.agent_id.clone(),
            to: "coordinator".to_string(),
            timestamp: Utc::now(),
            message_type: MessageType::Status,
            body: format!(
                "Review approved for task '{}': {}",
                p.task_id, result_text
            ),
            refs: vec![p.task_id.clone()],
        },
    );

    Ok(merge_result)
}
```

**Step 5: Run the test to verify it passes**

Run: `cargo test review_verdict_approve_auto_merges -- --nocapture`
Expected: PASS

**Step 6: Run all tests and lint**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass, no warnings

**Step 7: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: auto-merge on review approve — extract process_next_merge helper"
```

---

### Task 2: Update `hive_submit_to_queue` response to instruct lead to wait

The current response just says "Spawned reviewer, awaiting verdict" which gives the lead no reason to stay alive. Change it to explicitly instruct the lead to wait.

**Files:**
- Modify: `src/mcp.rs:1030-1033` (submit_to_queue success response)

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn submit_to_queue_response_instructs_wait() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut task = make_task("task-wait", None, TaskStatus::Active);
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SubmitToQueueParams {
        task_id: "task-wait".into(),
        branch: "hive/test/lead-1".into(),
    });
    let result = mcp.hive_submit_to_queue(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    // Response should tell the lead to wait
    assert!(
        text.contains("hive_wait_for_activity"),
        "response should instruct lead to call hive_wait_for_activity, got: {text}"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test submit_to_queue_response_instructs_wait -- --nocapture`
Expected: FAIL — current response doesn't mention hive_wait_for_activity

**Step 3: Update the success response in `hive_submit_to_queue`**

Replace the success response at line 1030-1033 with:

```rust
Ok(_) => {
    // Set task_id on the reviewer agent
    if let Ok(mut reviewer_agent) = state.load_agent(&self.run_id, &reviewer_id) {
        reviewer_agent.task_id = Some(p.task_id.clone());
        let _ = state.save_agent(&self.run_id, &reviewer_agent);
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "Spawned reviewer '{}' for task '{}'. \
        IMPORTANT: Do NOT exit. Call hive_wait_for_activity to wait for the review result. \
        The reviewer will either auto-merge (approve), send you feedback (request-changes), \
        or mark the task failed (reject). \
        If you receive feedback, address it and call hive_submit_to_queue again.",
        reviewer_id, p.task_id
    ))]))
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test submit_to_queue_response_instructs_wait -- --nocapture`
Expected: PASS

**Step 5: Run all tests**

Run: `cargo test --all-targets`
Expected: All pass

**Step 6: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: submit_to_queue response instructs lead to wait for review"
```

---

### Task 3: Update lead prompt with post-submission protocol

The lead prompt needs explicit instructions about what to do after submitting to the queue.

**Files:**
- Modify: `src/agent.rs:453-457` (Code Review Protocol section in lead prompt)

**Step 1: Write the failing test**

```rust
#[test]
fn lead_prompt_includes_post_submission_protocol() {
    let prompt = lead_prompt("lead-1", Some("coordinator"), "test task");
    assert!(
        prompt.contains("Post-Submission Protocol"),
        "lead prompt should include Post-Submission Protocol section"
    );
    assert!(
        prompt.contains("hive_wait_for_activity"),
        "lead prompt should mention hive_wait_for_activity after submission"
    );
}
```

Note: you'll need to check how `lead_prompt` is exposed for testing. Looking at the existing test at line 920, the prompt is generated inline via the `AgentRole::Lead` match arm. The test calls `AgentSpawner::generate_prompt` or similar. Check the existing test pattern:

```rust
// From existing tests — look at how the lead prompt tests work around line 920
```

If there isn't a standalone function, write the test using the same pattern as the existing lead prompt tests.

**Step 2: Run test to verify it fails**

Run: `cargo test lead_prompt_includes_post_submission_protocol -- --nocapture`
Expected: FAIL

**Step 3: Update the lead prompt**

Replace the `## Code Review Protocol` section (lines 453-457) with:

```
## Code Review Protocol
- Use hive_review_agent to see commits and diff stat.
- Verify: tests pass (check worker's output), no unrelated changes, matches the task description.
- If changes needed, send a message to the worker explaining what to fix. They will be auto-resumed.
- Only submit to merge queue after review passes.

## Post-Submission Protocol
- After calling hive_submit_to_queue, a reviewer agent is spawned automatically.
- Do NOT exit after submitting. Call hive_wait_for_activity to wait for the review result.
- If the reviewer approves, it will auto-merge your branch. You will see the task status change to "merged".
- If the reviewer requests changes, you will receive a message with feedback. Address the feedback, then submit again.
- If the reviewer rejects, the task is marked "failed". Notify the coordinator.
- Only exit after your task reaches a terminal status (merged, failed, cancelled, absorbed).
```

**Step 4: Run test to verify it passes**

Run: `cargo test lead_prompt_includes_post_submission_protocol -- --nocapture`
Expected: PASS

**Step 5: Run all tests and lint**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass

**Step 6: Commit**

```bash
git add src/agent.rs
git commit -m "feat: lead prompt includes post-submission protocol"
```

---

### Task 4: Notify lead (not just coordinator) on merge outcome

When auto-merge succeeds or fails, the lead should know so it can exit or retry. Currently `notify_submitter` in `process_next_merge` notifies the coordinator. We need to also notify the lead.

**Files:**
- Modify: `src/mcp.rs` (the approve arm in `hive_review_verdict`)

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn review_verdict_approve_notifies_lead() {
    // Same git setup as review_verdict_approve_auto_merges test
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task5678", AgentRole::Reviewer);
    let state = mcp.state();
    let root = state.repo_root().to_path_buf();

    // Init git repo with main branch and a feature branch
    std::process::Command::new("git").args(["init"]).current_dir(&root).output().unwrap();
    std::process::Command::new("git").args(["checkout", "-b", "main"]).current_dir(&root).output().unwrap();
    std::fs::write(root.join("README.md"), "# Test").unwrap();
    std::process::Command::new("git").args(["add", "."]).current_dir(&root).output().unwrap();
    std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "init"])
        .current_dir(&root).output().unwrap();
    std::process::Command::new("git").args(["checkout", "-b", "hive/test/lead-1"]).current_dir(&root).output().unwrap();
    std::fs::write(root.join("feature.txt"), "new feature").unwrap();
    std::process::Command::new("git").args(["add", "."]).current_dir(&root).output().unwrap();
    std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "feat"])
        .current_dir(&root).output().unwrap();
    std::process::Command::new("git").args(["checkout", "main"]).current_dir(&root).output().unwrap();

    let mut task = make_task("task-5678", None, TaskStatus::Review);
    task.branch = Some("hive/test/lead-1".into());
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None, worktree: None, heartbeat: None,
        task_id: Some("task-5678".into()),
        session_id: None, last_completed_at: None, messages_read_at: None, retry_count: 0,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-5678".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    mcp.hive_review_verdict(params).await.unwrap();

    // Check that lead-1 received a notification about the merge
    let messages = state.list_messages("test-run").unwrap_or_default();
    let lead_msgs: Vec<_> = messages.iter().filter(|m| m.to == "lead-1").collect();
    assert!(
        !lead_msgs.is_empty(),
        "lead should receive a notification about merge result"
    );
    assert!(
        lead_msgs.iter().any(|m| m.body.contains("merged") || m.body.contains("Merged")),
        "lead notification should mention merge: {:?}",
        lead_msgs.iter().map(|m| &m.body).collect::<Vec<_>>()
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test review_verdict_approve_notifies_lead -- --nocapture`
Expected: FAIL — no message sent to the lead

**Step 3: Add lead notification in the approve path**

In the approve arm of `hive_review_verdict`, after the `process_next_merge` call and coordinator notification, add a notification to the lead:

```rust
// Notify the lead about the merge outcome
if let Some(ref assigned) = task.assigned_to {
    let _ = state.save_message(
        &self.run_id,
        &Message {
            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
            from: self.agent_id.clone(),
            to: assigned.clone(),
            timestamp: Utc::now(),
            message_type: MessageType::Status,
            body: result_text.clone(),
            refs: vec![p.task_id.clone()],
        },
    );
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test review_verdict_approve_notifies_lead -- --nocapture`
Expected: PASS

**Step 5: Run all tests**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass

**Step 6: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: notify lead of merge outcome after review approve"
```

---

### Task 5: Final integration verification

**Step 1: Run the full test suite**

Run: `cargo test --all-targets`
Expected: All pass

**Step 2: Run clippy and format check**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean

**Step 3: Verify the flow end-to-end by reading the code**

Trace the flow:
1. Lead calls `hive_submit_to_queue` → gets response telling it to wait → reviewer spawned
2. Reviewer calls `hive_review_verdict("approve")` → task Queued → added to merge queue → `process_next_merge` runs → task Merged → lead notified → coordinator notified
3. Reviewer calls `hive_review_verdict("request-changes")` → task Active → message sent to lead → lead (still alive, waiting) receives feedback → addresses it → resubmits
4. Lead sees task status change via `hive_wait_for_activity` and exits

**Step 4: Commit**

```bash
git add -A
git commit -m "chore: final cleanup for review-merge automation"
```
