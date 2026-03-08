# Unified Message Delivery

**Goal:** Ensure agents reliably receive messages regardless of whether they're running or idle, without requiring prompt compliance or manual polling.

**Problem:** Currently, messages to idle agents work (auto-wake via `--resume`), but messages to running agents are silently dropped into JSON files that nobody reads. There's no `hive_read_messages` tool, and no mechanism to surface pending messages before an agent finishes.

**Architecture:** Three changes: (1) a `hive_read_messages` MCP tool + CLI command, (2) a `Stop` hook that blocks agent exit when unread messages exist, (3) a `messages_read_at` cursor on agent state for tracking what's been seen.

**Tech Stack:** Rust, Claude Code hooks (command type), MCP via `rmcp` crate.

---

## Design

### Message lifecycle

```
Sender calls hive_send_message
  |
  +--> Message JSON saved to .hive/runs/<run>/messages/
  |
  +--> Is target idle with session_id?
         YES --> auto-wake via --resume (existing, unchanged)
         NO  --> message sits on disk
                   |
                   +--> Running agent finishes current work
                   +--> Stop hook fires, calls `hive read-messages --unread`
                   +--> Unread messages exist?
                          YES --> hook exits 2, stderr contains messages
                                  --> Claude sees error, processes messages
                                  --> agent calls hive_read_messages (updates cursor)
                                  --> agent continues working
                                  --> eventually tries to stop again
                          NO  --> hook exits 0, agent stops normally
                                  --> agent goes idle
```

### Unread tracking

Add `messages_read_at` (ISO 8601 timestamp) to the Agent struct. This serves as a cursor:
- Set to `now()` whenever `hive_read_messages` is called
- The Stop hook queries messages where `timestamp > messages_read_at` AND `timestamp > last_completed_at`
- On first run (both fields null), all messages to this agent are "unread"

Using a timestamp cursor rather than per-message read flags keeps the design stateless and avoids modifying message files.

---

## Task 1: Add `messages_read_at` field to Agent struct

**Files:**
- Modify: `src/types.rs` — add `messages_read_at: Option<DateTime<Utc>>` to `Agent` struct
- Modify: `src/agent.rs` — set `messages_read_at: None` in `AgentSpawner::spawn()`
- Modify: `src/mcp.rs` — set `messages_read_at: None` in any place that constructs Agent structs

**Details:**
- Add the field after `last_completed_at`
- It's `Option<DateTime<Utc>>`, serde-serialized, defaults to None
- Update any test that constructs Agent structs to include the new field

---

## Task 2: Add `hive_read_messages` MCP tool

**Files:**
- Modify: `src/state.rs` — add `load_messages_for_agent(run_id, agent_id, since: Option<DateTime<Utc>>) -> Vec<Message>` method
- Modify: `src/mcp.rs` — add `ReadMessagesParams` struct + `hive_read_messages` tool method

**Details:**

`load_messages_for_agent` in state.rs:
- Read all message JSON files in `.hive/runs/<run>/messages/`
- Filter to messages where `to == agent_id`
- If `since` is Some, filter to messages where `timestamp > since`
- Sort by timestamp ascending
- Return the filtered list

`ReadMessagesParams`:
```rust
#[derive(Deserialize, JsonSchema)]
pub struct ReadMessagesParams {
    /// Only return messages newer than this timestamp. If omitted, returns unread messages (since last read or last idle).
    pub since: Option<String>,
}
```

`hive_read_messages` tool:
- Any role can call it (no role restriction)
- Determine the "since" cutoff: use `params.since` if provided, otherwise use `max(agent.messages_read_at, agent.last_completed_at)`, falling back to None (all messages)
- Call `load_messages_for_agent` with the cutoff
- Update `agent.messages_read_at = Some(Utc::now())` and save
- Return JSON array of messages (id, from, timestamp, message_type, body)
- If no messages, return `{"messages": [], "count": 0}`

---

## Task 3: Add `hive read-messages` CLI command

**Files:**
- Modify: `src/cli.rs` — add `ReadMessages` command variant
- Modify: `src/main.rs` — add dispatch + handler

**Details:**

CLI command:
```
hive read-messages --agent <agent-id> [--run <run-id>] [--unread]
```

- `--agent` (required): which agent's messages to read
- `--run` (optional): defaults to active run
- `--unread`: if set, only show messages since `max(messages_read_at, last_completed_at)`

Behavior:
- Load agent to get timestamps
- Call `state.load_messages_for_agent()` with appropriate `since`
- Print messages as JSON array to stdout
- Do NOT update `messages_read_at` (CLI is read-only, only the MCP tool updates the cursor)
- Exit 0 if no messages, exit 0 with output if messages exist

This CLI command is what the Stop hook will call.

---

## Task 4: Add Stop hook to agent spawn config

**Files:**
- Modify: `src/agent.rs` — update the settings.local.json generation in `AgentSpawner::spawn()`

**Details:**

Add a `Stop` hook alongside the existing `PostToolUse` hook:

```json
{
  "hooks": {
    "PostToolUse": [... existing ...],
    "Stop": [{
      "matcher": "*",
      "hooks": [{
        "type": "command",
        "command": "hive read-messages --agent <agent_id> --run <run_id> --unread --stop-hook"
      }]
    }]
  }
}
```

Add a `--stop-hook` flag to `hive read-messages` that changes the exit behavior:
- If unread messages exist: print them to **stderr** and exit **2** (Claude sees the error and continues)
- If no unread messages: exit **0** (agent stops normally)

Without `--stop-hook`, the command always exits 0 and prints to stdout (normal CLI usage).

---

## Task 5: Update agent prompts

**Files:**
- Modify: `src/agent.rs` — update `generate_prompt` for lead and worker roles

**Details:**

Add to lead prompt Responsibilities:
```
- When you receive messages via the Stop hook, process them before finishing.
- Use hive_read_messages to acknowledge messages and check for more.
```

Add to worker prompt Responsibilities:
```
- When you receive messages via the Stop hook, process them before finishing.
- Use hive_read_messages to acknowledge messages and check for more.
```

Remove or keep the existing "check messages before finishing" lines if present — the Stop hook now enforces this automatically, but the prompt guidance helps the agent understand what's happening when the hook fires.

---

## Implementation Order

Tasks 1-3 can be done in parallel (data model, MCP tool, CLI are independent once the types are agreed).
Task 4 depends on Task 3 (Stop hook calls the CLI).
Task 5 is independent.

Suggested grouping:
- **Lead A**: Tasks 1 + 2 (types + MCP tool) — touches types.rs, state.rs, mcp.rs
- **Lead B**: Tasks 3 + 4 (CLI + Stop hook) — touches cli.rs, main.rs, agent.rs
- **Lead C**: Task 5 (prompts) — touches agent.rs only, minimal diff

Note: Lead B depends on Lead A for `load_messages_for_agent` in state.rs. Lead B should implement Task 3 assuming that function exists and will be available after merge.

---

## Workflow

Each lead should follow this process:

1. **Read the relevant source files** for their assigned tasks. Understand the current code structure, existing patterns, and exact line numbers.
2. **Write a detailed implementation plan** with exact code changes — specific line numbers, function signatures, struct modifications, test cases. The plan should be precise enough that a worker can execute it mechanically.
3. **Spawn workers** with the detailed plan as their assignment. Each worker gets a self-contained task with all the context needed to implement it.
4. **Review worker output** when they complete. Send feedback if changes are needed. Submit to merge queue when approved.
