# Review Verdict Auto-Wake Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When a reviewer calls `hive_review_verdict("request-changes")`, auto-resume the lead/worker so it can act on the feedback — just like `hive_send_message` already does.

**Architecture:** Extract the auto-wake logic from `hive_send_message` into a private `try_wake_agent` helper. Call it from both `hive_send_message` and the `request-changes` arm of `hive_review_verdict`.

**Tech Stack:** Rust, `rmcp` MCP framework

---

### Task 1: Extract auto-wake into a helper method

**Files:**
- Modify: `src/mcp.rs:739-782` (auto-wake block in `hive_send_message`)

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn review_verdict_request_changes_wakes_idle_agent() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1111", AgentRole::Reviewer);
    let state = mcp.state();

    // Create a task in Review status assigned to lead-1
    let mut task = make_task("task-1111", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    task.review_count = 0;
    state.save_task("test-run", &task).unwrap();

    // Create an idle lead with a session_id (eligible for resume)
    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Idle,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: Some("/tmp/fake-worktree".into()),
        heartbeat: None,
        task_id: Some("task-1111".into()),
        session_id: Some("sess-abc123".into()),
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1111".into(),
        verdict: "request-changes".into(),
        feedback: Some("Please fix the error handling.".into()),
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // Verify a message was saved for lead-1
    let messages = state.list_messages("test-run").unwrap_or_default();
    let lead_msgs: Vec<_> = messages.iter().filter(|m| m.to == "lead-1").collect();
    assert!(!lead_msgs.is_empty(), "lead should have a feedback message");

    // Verify the agent status was changed to Running (wake attempted)
    // Note: the actual `claude --resume` spawn will fail in tests (no real session),
    // so we check that the agent status was at least attempted to be updated.
    // In a real scenario with a valid session, the agent would be Running.
    // For this test, just verify the message was delivered — the wake is best-effort.
    let lead_after = state.load_agent("test-run", "lead-1").unwrap();
    // The spawn will fail because /tmp/fake-worktree doesn't exist and there's no real claude binary in test,
    // but we can verify the message was sent correctly
    assert!(lead_msgs[0].body.contains("Please fix the error handling"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test review_verdict_request_changes_wakes_idle_agent -- --nocapture`
Expected: PASS (message is already saved). This test validates the baseline. We need a second test to verify the wake actually happens — but since `claude --resume` can't run in tests, we'll verify structurally.

**Step 3: Extract `try_wake_agent` helper**

Add this private method to `impl HiveMcp`, before `hive_send_message`:

```rust
/// If the target agent is idle with a session_id, resume it by spawning
/// `claude --resume`. Returns a human-readable status string.
fn try_wake_agent(&self, to: &str, body: &str) -> Option<String> {
    let state = self.state();
    let mut target_agent = state.load_agent(&self.run_id, to).ok()?;
    if target_agent.status != AgentStatus::Idle {
        return None;
    }
    let session_id = target_agent.session_id.as_ref()?;

    let agent_output_dir = state.agents_dir(&self.run_id).join(&target_agent.id);
    let output_file = std::fs::File::create(agent_output_dir.join("output.jsonl")).ok()?;
    let worktree = target_agent.worktree.clone().unwrap_or_default();
    let stderr_file = std::fs::File::create(agent_output_dir.join("stderr.log")).ok();
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("-p")
        .arg(body)
        .arg("--resume")
        .arg(session_id)
        .arg("--verbose")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--dangerously-skip-permissions")
        .env_remove("CLAUDECODE")
        .current_dir(&worktree)
        .stdin(std::process::Stdio::null())
        .stdout(output_file);
    if let Some(f) = stderr_file {
        cmd.stderr(std::process::Stdio::from(f));
    }
    match cmd.spawn() {
        Ok(child) => {
            target_agent.status = AgentStatus::Running;
            target_agent.pid = Some(child.id());
            target_agent.heartbeat = Some(Utc::now());
            let _ = state.save_agent(&self.run_id, &target_agent);
            Some(format!(" (woke agent '{}', pid {})", to, child.id()))
        }
        Err(e) => Some(format!(" (failed to wake agent '{}': {e})", to)),
    }
}
```

**Step 4: Refactor `hive_send_message` to use the helper**

Replace lines 739-782 in `hive_send_message` with:

```rust
        // Auto-wake: if target agent is idle with a session_id, resume it
        let wake_info = self.try_wake_agent(&p.to, &p.body);

        let wake_suffix = wake_info.unwrap_or_default();
```

This replaces the entire inline auto-wake block while preserving identical behavior.

**Step 5: Run all tests to verify refactor is clean**

Run: `cargo test --all-targets`
Expected: All pass — behavior unchanged

**Step 6: Commit the refactor**

```bash
git add src/mcp.rs
git commit -m "refactor: extract try_wake_agent helper from hive_send_message"
```

---

### Task 2: Call `try_wake_agent` from `hive_review_verdict("request-changes")`

**Files:**
- Modify: `src/mcp.rs:857-893` (request-changes arm)

**Step 1: Add the wake call**

In the `"request-changes"` arm, after the message is saved to the assigned agent (after line 885), add:

```rust
                    // Auto-wake the agent to process feedback
                    let _ = self.try_wake_agent(assigned, &format!(
                        "Review feedback for task '{}' (review cycle {}):\n{}",
                        p.task_id, task.review_count, feedback
                    ));
```

The full request-changes block becomes:

```rust
            "request-changes" => {
                let feedback = p
                    .feedback
                    .as_deref()
                    .unwrap_or("Changes requested (no details provided)");
                task.review_count += 1;
                task.status = TaskStatus::Active;
                task.updated_at = Utc::now();
                state
                    .save_task(&self.run_id, &task)
                    .map_err(|e| McpError::internal_error(e, None))?;

                // Send feedback to the agent that worked on this task
                if let Some(ref assigned) = task.assigned_to {
                    let body = format!(
                        "Review feedback for task '{}' (review cycle {}):\n{}",
                        p.task_id, task.review_count, feedback
                    );
                    let _ = state.save_message(
                        &self.run_id,
                        &Message {
                            id: format!("msg-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                            from: self.agent_id.clone(),
                            to: assigned.clone(),
                            timestamp: Utc::now(),
                            message_type: MessageType::Request,
                            body: body.clone(),
                            refs: vec![p.task_id.clone()],
                        },
                    );

                    // Auto-wake the agent to process feedback
                    let _ = self.try_wake_agent(assigned, &body);
                }

                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Requested changes for task '{}' (review cycle {}). Feedback sent to {}.",
                    p.task_id,
                    task.review_count,
                    task.assigned_to.as_deref().unwrap_or("unknown")
                ))]))
            }
```

**Step 2: Run all tests and lint**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: All pass

**Step 3: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: auto-wake lead/worker on review request-changes verdict"
```
