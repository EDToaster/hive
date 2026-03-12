# Agent Reliability & Quality-of-Life Improvements

8 targeted fixes addressing the highest-impact issues from dogfooding. All changes are backward-compatible, most are small (20-60 LOC production code each).

## Task 1: Worktree Parent Branching

**Problem:** `Git::worktree_add` always branches from HEAD (main). Workers don't see their lead's committed work, causing merge conflicts.

**Changes:**
- `src/git.rs`: Add `start_point: Option<&str>` param to `worktree_add`. When provided, appends it to `git worktree add <path> -b <branch> <start_point>`.
- `src/agent.rs`: In `spawn()`, resolve the parent agent's branch (`hive/{run_id}/{parent_id}`) when parent has a worktree. Pass as `start_point`. Coordinator (no worktree) falls back to `None` (branches from main, same as today).
- Update existing tests calling `worktree_add` to pass `None`. Add new test verifying child worktree contains parent's commits.

**Edge cases:** Coordinator spawning leads = no change. Workers branch off lead. Reviewers branch off lead (can see work to review). Uncommitted parent work not included (by design — agents commit frequently). Merge queue unaffected — auto-rebase handles divergence.

~20 LOC production, ~30 LOC test.

## Task 2: wait_for_activity Cross-Team Filtering

**Problem:** `wait_for_activity` wakes agents on ALL events in the run directory. Leads get noise from other teams, wasting context and API calls.

**Changes:**
- `src/wait.rs`: Add `AgentContext` struct that computes a "team" set (self + parent + direct children) and relevant task set from the agent hierarchy. Add `agent_id: Option<&str>` param to `wait_for_activity`. When `Some`, build `AgentContext` before watching and filter in `describe_event`:
  - Messages: only if `msg.to == agent_id` or `msg.from` is in team
  - Agent changes: only if changed agent is in team
  - Task changes: only if task is in relevant_tasks set
  - Merge queue: always pass for coordinators, filter by `submitted_by` for leads
- `src/mcp.rs`: Pass `Some(&self.agent_id)` to `wait_for_activity`. CLI passes `None` for backward compat.
- Handle edge cases: new agents spawned mid-watch (check parent on agent creation events), task reassignment (check new `assigned_to`).

~200 LOC including tests.

## Task 3: Agent Status Digest in wait_for_activity

**Problem:** Leads must explicitly call `hive_check_agents` to learn worker status. They frequently forget after `wait_for_activity` returns, missing completions.

**Changes (two-part):**
- `src/wait.rs`: Extract `agent_status_digest(repo_root, run_id) -> String` that produces per-agent one-liners (status, task, timing). Append this digest to every `wait_for_activity` return (both event and timeout paths):
  ```
  agent worker-1 status changed to idle

  --- Agent Status ---
  worker-1: idle (task-3, idle 45s)
  worker-2: running (task-4, heartbeat 12s ago)
  ```
- `src/mcp.rs`: In `hive_check_agents`, when detecting Running->Idle/Failed/Stalled transition, auto-send a notification message to the agent's parent and call `try_wake_agent` to resume idle parents immediately.

~70 LOC total.

## Task 4: Early Session ID Capture for Crash Recovery

**Problem:** Session ID only captured at agent exit. If agent crashes, no session ID = no resume = must destroy worktree and start fresh, losing all conversation context.

**Changes:**
- `src/mcp.rs`: Add `parse_early_session_id(output_path)` that reads only the first line of `output.jsonl` (the `{"type":"system","subtype":"init","session_id":"..."}` message written immediately on Claude Code start). In `hive_check_agents`, for running agents with `session_id == None`, attempt early capture and save to `agent.json`.
- `src/mcp.rs`: Add `resume_agent()` helper that spawns `claude --resume <session_id>` in the existing worktree (preserving conversation context). Modify `hive_retry_agent` to prefer resume when `session_id` exists, fall through to existing destroy-and-respawn when absent.
- Session ID cleared on resume (will be re-captured from new output). If `--resume` fails, existing respawn logic catches it on next `check_agents` cycle.

~80 LOC production.

## Task 5: Git Commits in Agent Status

**Problem:** `hive_check_agents` shows unstaged changes but not commits. Leads have no visibility into worker progress.

**Changes:**
- `src/mcp.rs`: In `hive_check_agents`, after the existing `uncommitted_changes` field, add `recent_commits` (last 10 commits via existing `Git::log_oneline_since`) and `commit_count` to the per-agent JSON response. Truncate with "... and N more" when >10 commits.
- No changes to `git.rs` — `Git::log_oneline_since` already exists and is tested.

~15 LOC production.

## Task 6: Explorer/Evaluator Bug Fixes

**Problem:** Three bugs block the EXPLORE mode lifecycle: (1) coordinator can't message explorers, (2) explorers can't update their own tasks, (3) explorer tasks get stuck in "review".

**Changes:**
- `src/mcp.rs` (~line 714): Expand coordinator `hive_send_message` validation to allow `Explorer` and `Evaluator` targets alongside `Lead`.
- `src/mcp.rs` (~line 503): Add `AgentRole::Explorer | AgentRole::Evaluator` cases to `ownership_ok` in `hive_update_task` (same logic as Worker — can update own assigned task).
- `src/agent.rs`: Update coordinator EXPLORE prompt Phase 3 to include guidance on closing explorer/evaluator tasks (set winning explorer to "approved", others to "absorbed").
- `src/mcp.rs` (~line 728): Fix misleading error message from "Reviewers can only..." to "This role can only send messages to the coordinator."
- `src/agent.rs`: Add evaluator prompt guidance on reading other branches (`git diff main...<branch>`, `git show <branch>:<file>`).

~30 LOC production.

## Task 7: Worker Completion Race Condition

**Problem:** Worker finishes and exits, but agent.json still says "running" until `hive_check_agents` detects the dead process. Leads see stale status.

**Changes:**
- `src/cli.rs` + `src/main.rs`: Add `hive agent-exit --run <id> --agent <id>` CLI command that transitions Running->Idle, sets `last_completed_at`, clears `pid`, captures session ID from output.
- `src/agent.rs`: Update Stop hook to chain: `hive read-messages ... --stop-hook && hive agent-exit ...`. Using `&&` ensures `agent-exit` only runs when `read-messages` exits 0 (no unread messages = agent truly exiting, not being resumed).
- Extract `parse_session_id_from_output` to a shared location (used by both MCP and CLI).
- Existing `hive_check_agents` fallback preserved for crashes/kills where Stop hook doesn't fire.

Ordering guarantee: task update -> message -> agent status -> process exit.

~55 LOC production.

## Task 8: File Overwrite Fixes

**Problem:** (1) `output.jsonl` truncated on respawn/wake, losing session history. (2) `.mcp.json` overwritten by hive, destroying user MCP config.

**Changes:**
- `src/agent.rs` + `src/mcp.rs`: Replace `File::create` with `OpenOptions::new().create(true).append(true).open(...)` for `output.jsonl` and `stderr.log` in spawn and `try_wake_agent`. Write a JSON session boundary marker (`{"type":"session_boundary","timestamp":"...","reason":"spawn|wake|retry"}`) before each new session.
- `src/agent.rs`: Add `write_mcp_config(path, run_id, agent_id)` helper that reads existing `.mcp.json`, adds/updates only the `"hive"` key under `"mcpServers"`, preserves everything else. Apply to `spawn()`.
- `src/main.rs`: Apply `write_mcp_config` to `cmd_start` and `cmd_explore`. Replace `remove_file(.mcp.json)` in `cmd_stop` with `remove_hive_mcp_entry` that only removes the `"hive"` key (deletes file only if it was hive-only).
- `src/agent.rs`: Add `.mcp.json` to `.gitignore` alongside `.hive` during spawn.

~60 LOC production.

## Implementation Order (Suggested)

Tasks are independent and can be parallelized. Suggested lead decomposition:

**Lead 1 — Git & Worktree** (Tasks 1, 8): Worktree parent branching + file overwrite fixes. Both touch `git.rs` and `agent.rs` spawn sequence.

**Lead 2 — Event System** (Tasks 2, 3): wait_for_activity filtering + status digest. Both modify `wait.rs` and the wait/notification flow.

**Lead 3 — Agent Lifecycle** (Tasks 4, 7): Early session capture + completion race. Both touch agent status transitions, session ID parsing, and the Stop hook. Task 7's `parse_session_id_from_output` extraction is a prerequisite for Task 4.

**Lead 4 — Role Fixes & Status** (Tasks 5, 6): Explorer bug fixes + commits in status. Both modify `mcp.rs` permission/status code. Small and low-risk.
