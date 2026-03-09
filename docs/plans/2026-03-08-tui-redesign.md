# Hive TUI Redesign

**Goal:** Transform the TUI from a basic table viewer into an information-dense, visually distinctive dashboard for live monitoring and post-run review.

**Approach:** Single-screen layout with three focused zones (Swarm tree, Tasks table, Activity stream), scrollable panes, detail overlays, and a branded visual identity using unicode symbols and color.

---

## Layout

```
+-- HIVE ---------------------------------- Run: <id> (12m34s) -- 12:34:05 -+
| Agents: 3 running . 2 idle . 1 done   Tasks: 4 active . 2 merged         |
+---------------------------+-----------------------------------------------+
|  Swarm                    |  Tasks                                        |
|                           |                                               |
|  coordinator         3m   |  task-1  * active  lead-a   Add messages      |
|  +- lead-a [run] t-1 12s  |  task-2  * active  w-1     CLI read-msg       |
|  |  +- w-1 [run] t-2  8s  |  task-3  o merged  --      Update prompts     |
|  |  +- w-2 [idle]     1m  |  task-4  o queued  lead-b  Stop hook          |
|  +- lead-b [run] t-4  5s  |  task-5  o pending --      Integration        |
|     +- w-3 [run] t-5  3s  |                                               |
|                           |                                               |
|  Queue ----------------   |                                               |
|  1. lead-a/task-1         |                                               |
|  2. lead-b/task-4         |                                               |
+---------------------------+-----------------------------------------------+
|  Activity                                                                 |
|  12:34:02 > lead-a -> coordinator: Tasks 1+2 complete, submitting         |
|  12:34:05   worker-1  hive_update_task  task_id=task-1       ok 150ms     |
|  12:34:05   worker-2  hive_heartbeat                         ok  12ms     |
|  12:34:08 > coordinator -> lead-b: Proceed with task 4                    |
|  12:34:10   lead-b    hive_spawn_agent  role=worker          ok  89ms     |
+--------------------------------------------------------------------------- +
```

---

## Shared Chrome

### Title Bar

- Left: hex icon + "HIVE" branding
- Right: run ID, uptime (elapsed since `RunMetadata.created_at`), wall clock
- Uptime computed as `now - run.created_at`

### Stats Bar

Below the title, always visible:

- Agent counts grouped by status, color-coded (e.g. "3 running" in green, "1 failed" in red)
- Task counts grouped by status, color-coded

### Pane Focus

- Active pane: bright cyan border
- Inactive panes: dim gray border
- `Tab` cycles focus: Swarm -> Tasks -> Activity

### Navigation

| Key     | Action                                          |
|---------|-------------------------------------------------|
| `j`/`k` | Scroll within focused pane                      |
| `Tab`   | Cycle pane focus                                |
| `Enter` | Open detail overlay for selected agent/task     |
| `Esc`   | Close overlay / deselect agent                  |
| `G`     | Jump to bottom of activity stream (resume auto-scroll) |
| `q`     | Quit                                            |

---

## Swarm Pane (upper left)

Agent hierarchy as a tree, built from each agent's `parent` field.

### Tree Structure

```
 coordinator          3m
 +- lead-a [run] t-1 12s
 |  +- w-1 [run] t-2  8s
 |  +- w-2 [idle]     1m
 +- lead-b [run] t-4  5s
    +- w-3 [run] t-5  3s
```

### Data Shown Per Agent

- **Agent ID** (abbreviated if needed)
- **Status** in brackets, abbreviated: `[run]`, `[idle]`, `[done]`, `[fail]`, `[stal]`
- **Task ID** inline for agents with an active `task_id`
- **Heartbeat age** right-aligned: time since `agent.heartbeat`

### Visual Treatment

- Status color-coded: green=running, cyan=idle, blue=done, red=failed, yellow=stalled
- Heartbeat age: normal if < 2min, yellow if > 2min, red if > `stall_timeout_seconds` from config
- Done/failed agents: dimmed foreground (lower contrast)
- Tree lines use unicode box-drawing characters

### Agent Selection Behavior

Selecting an agent in the swarm pane dims non-matching entries in the activity stream (entries remain visible for temporal context, but the selected agent's entries are highlighted). Pressing `Esc` deselects and restores full brightness.

### Inline Merge Queue

When merge queue has entries, rendered below the tree:

```
 Queue ----------------
 1. lead-a/task-1
 2. lead-b/task-4
```

---

## Tasks Pane (upper right)

### Table Columns

| Column   | Width     | Content                          |
|----------|-----------|----------------------------------|
| ID       | fixed     | Task ID                          |
| Status   | fixed     | Colored bullet + status name     |
| Assigned | fixed     | Agent ID or `--`                 |
| Title    | remaining | Full task title, no truncation   |

### Status Bullets

- `*` green = active
- `*` blue = merged
- `o` yellow = queued, review
- `o` gray = pending
- `o` orange = blocked
- `x` red = failed

### Visual Treatment

- Row striping: alternating subtle background tint for readability
- Title column takes all remaining horizontal space (dynamically sized)
- Scrollable with `j`/`k` when focused

---

## Activity Stream (bottom)

Unified chronological feed interleaving messages and tool calls.

### Message Entries

Source: `.hive/runs/<run>/messages/` JSON files.

```
 12:34:02 > lead-a -> coordinator: Tasks 1+2 complete, submitting
```

- Prefix: `>` marker
- Color: cyan, brighter weight
- Format: `HH:MM:SS > <from> -> <to>: <body>`

### Tool Call Entries

Source: `log.db` via new `recent_tool_calls` query.

```
 12:34:05   worker-1  hive_update_task  task_id=task-1       ok 150ms
```

- No prefix (indented to align with message body)
- Color: dim gray
- Failed calls: red foreground, `x` instead of checkmark
- Format: `HH:MM:SS   <agent>  <tool_name>  <args_summary>  <status> <duration>`

### Scroll Behavior

- Auto-scrolls to latest entry by default
- `j`/`k` scrolling pauses auto-scroll
- `G` resumes auto-scroll (jumps to bottom)

### Filtering

When an agent is selected in the swarm pane, activity entries from other agents are dimmed but not hidden. This preserves temporal context while highlighting the selected agent's activity.

---

## Detail Overlay

Pressing `Enter` on a selected agent or task opens a centered modal overlay.

### Agent Detail

```
+- Agent: worker-1 ----------------------------+
| Role:        worker                           |
| Status:      running                          |
| Parent:      lead-a                           |
| Task:        task-2                           |
| Worktree:    .hive/runs/.../worktrees/w-1     |
| PID:         48291                            |
| Heartbeat:   12:34:05 (8s ago)                |
| Session:     abc123                           |
|                                  [Esc] close  |
+-----------------------------------------------+
```

### Task Detail

```
+- Task: task-2 --------------------------------+
| Title:       CLI read-messages command         |
| Status:      active                            |
| Urgency:     normal                            |
| Assigned:    worker-1                          |
| Created by:  lead-a                            |
| Branch:      hive/run1/lead-a                  |
| Domain:      cli                               |
| Blocked by:  (none)                            |
| Blocking:    task-4                            |
| Created:     12:20:00                          |
| Updated:     12:34:02                          |
|                                                |
| Description:                                   |
| Add hive read-messages CLI command with        |
| --agent, --run, and --unread flags...          |
|                                   [Esc] close  |
+------------------------------------------------+
```

`Esc` dismisses the overlay.

---

## Implementation Guide

### Lead Decomposition

Two leads with clean file boundaries to avoid merge conflicts:

**Lead 1: Backend Data Layer** (domain: `backend`, files: `src/logging.rs`)
- Add `ToolCallRow` struct and `recent_tool_calls` method to `LogDb`
- Small scope, should merge first so Lead 2 can use it
- See "Backend Addition" section below for exact spec

**Lead 2: TUI Rewrite** (domain: `tui`, files: `src/tui.rs`)
- Full rewrite of the TUI rendering and interaction
- Can assume `LogDb::recent_tool_calls` exists (Lead 1 merges first)
- See "TUI Implementation" section below for details

Leads should read the relevant source files, write a detailed implementation plan with exact code changes, then spawn workers to execute.

---

### Backend Addition (Lead 1)

**File:** `src/logging.rs`

Add a new struct:

```rust
#[derive(Debug, Clone)]
pub struct ToolCallRow {
    pub timestamp: String,
    pub agent_id: String,
    pub tool_name: String,
    pub args_summary: Option<String>,
    pub status: String,
    pub duration_ms: Option<i64>,
}
```

Add a new method on `LogDb`:

```rust
pub fn recent_tool_calls(
    &self,
    run_id: &str,
    limit: usize,
    agent_id: Option<&str>,
) -> Result<Vec<ToolCallRow>, String>
```

Behavior:
- Query `tool_calls` table filtered by `run_id`
- When `agent_id` is `Some`, add `WHERE agent_id = ?` filter
- Order by `timestamp DESC`, `LIMIT` to `limit`
- Reverse results before returning (so caller gets chronological order)
- Map rows to `ToolCallRow` structs

Tests to add:
- Empty DB returns empty vec
- Respects limit parameter
- Filters by agent_id when provided
- Returns results in chronological order
- Ignores rows from other run_ids

---

### TUI Implementation (Lead 2)

**File:** `src/tui.rs`

This is a full rewrite. The current file is ~187 lines of basic table rendering. The new implementation needs:

#### 1. State Management

Add a `TuiState` struct to track:
- `focused_pane: Pane` enum (Swarm, Tasks, Activity)
- `swarm_scroll: usize` and `swarm_selected: Option<usize>` for the tree
- `tasks_scroll: usize` and `tasks_selected: Option<usize>` for the task list
- `activity_scroll: usize` and `activity_auto_scroll: bool` for the stream
- `overlay: Option<Overlay>` enum (AgentDetail(agent_id), TaskDetail(task_id))
- `selected_agent_filter: Option<String>` for activity stream dimming

#### 2. Input Handling

Replace the current single `q`/`Esc` handler with:
- `j`/`k`: scroll within focused pane, update selected index
- `Tab`: cycle `focused_pane` through Swarm -> Tasks -> Activity
- `Enter`: open detail overlay for selected agent/task (based on focused pane)
- `Esc`: close overlay if open, else deselect agent filter, else do nothing
- `G`: jump to bottom of activity stream, re-enable auto-scroll
- `q`: quit (only when no overlay is open)

#### 3. Layout

Replace the current 3-chunk vertical layout with:

```
Vertical [
    Length(1),   // Title bar
    Length(1),   // Stats bar
    Min(8),      // Main content (horizontal split: Swarm 35% | Tasks 65%)
    Length(10),  // Activity stream
]
```

The main content area splits horizontally:
```
Horizontal [
    Percentage(35),  // Swarm pane
    Percentage(65),  // Tasks pane
]
```

#### 4. Title Bar Rendering

- Left: `" ⬡ HIVE"` in cyan bold
- Right: `"Run: {run_id} ({uptime}) -- {HH:MM:SS}"` in dim white
- Uptime: load `RunMetadata` from `run.json`, compute `Utc::now() - created_at`, format as `Xm Ys`
- Note: `state.rs` has `run_dir(run_id).join("run.json")` but no `load_run_metadata` method. Either add one to `state.rs` or read the file directly in the TUI. Adding a method is cleaner:
  ```rust
  // Add to state.rs
  pub fn load_run_metadata(&self, run_id: &str) -> Result<RunMetadata, String>
  ```

#### 5. Stats Bar Rendering

Single line, e.g.: `" Agents: 3 running · 2 idle · 1 done    Tasks: 4 active · 2 merged · 1 blocked"`

- Count agents/tasks by status from the already-loaded vecs
- Color each count to match its status color
- Use `·` as separator

#### 6. Swarm Pane

Build a tree from agents:
1. Sort agents: coordinator first, then group by parent
2. Recursively render: indent with box-drawing chars (`├─`, `└─`, `│`)
3. Per line: `{indent} {agent_id} [{status_abbrev}] {task_id?} {heartbeat_age}`

Heartbeat age formatting:
- Compute `Utc::now() - agent.heartbeat`
- Format: `<60s` -> `Xs`, `<60m` -> `Xm`, else `Xh`
- Color: default if < 120s, yellow if < `stall_timeout`, red if >= `stall_timeout`
- `stall_timeout` from `state.load_config().stall_timeout_seconds`

Done/failed agents: render with `Style::default().fg(Color::DarkGray)`

Merge queue: if non-empty, render below tree with a separator line

Active pane border: bright cyan when `focused_pane == Swarm`, dim gray otherwise

When an agent is selected (`swarm_selected` is Some), set `selected_agent_filter` to that agent's ID for activity stream dimming.

#### 7. Tasks Pane

Render as a `Table` widget with dynamic column widths:
- ID: `Constraint::Min(12)`
- Status: `Constraint::Min(10)` — colored bullet + status name
- Assigned: `Constraint::Min(10)`
- Title: `Constraint::Percentage(remaining)` or `Constraint::Min(20)` — takes all remaining space

Status bullets (use unicode):
- Active: `●` green
- Merged: `●` blue
- Queued/Review: `◉` yellow
- Pending: `○` gray
- Blocked: `○` orange (Color::Rgb(255, 165, 0) or Color::Yellow as fallback)
- Failed: `✗` red

Row striping: even rows get `Style::default().bg(Color::Rgb(30, 30, 30))` (or similar dark tint)

#### 8. Activity Stream

This pane interleaves two data sources into one chronological list:

**Data loading (each tick):**
1. Messages: already loaded via `state.list_messages(run_id)` — has timestamp, from, to, body
2. Tool calls: load via `LogDb::recent_tool_calls(run_id, 200, None)` — has timestamp, agent_id, tool_name, args_summary, status, duration_ms

**Interleaving:**
- Convert both to a common `ActivityEntry` enum or struct with a timestamp
- Sort by timestamp ascending
- Render as a scrollable list

**Message rendering:**
```
 {HH:MM:SS} ▸ {from} → {to}: {body}
```
- Style: cyan foreground

**Tool call rendering:**
```
 {HH:MM:SS}   {agent_id}  {tool_name}  {args_summary}  {✓|✗} {duration}ms
```
- Style: `Color::DarkGray` for success, `Color::Red` for failed (status != "success")

**Agent filter dimming:**
When `selected_agent_filter` is Some, entries not matching that agent get `Style::default().fg(Color::Rgb(60, 60, 60))` (very dim).

**Auto-scroll:**
- If `activity_auto_scroll` is true, set scroll offset to bottom each frame
- `j`/`k` sets `activity_auto_scroll = false`
- `G` sets `activity_auto_scroll = true`

#### 9. Detail Overlay

When `overlay` is Some, render a centered `Clear` + `Block` on top of everything:

- Width: 60% of screen width, max 60 chars
- Height: sized to content
- Border: `Borders::ALL`, title from the agent/task ID
- Content: key-value pairs as `Paragraph` lines
- Bottom-right: `[Esc] close` hint

Agent overlay fields: role, status, parent, task, worktree, pid, heartbeat (with age), session
Task overlay fields: title, status, urgency, assigned, created_by, branch, domain, blocked_by, blocking, created, updated, description (wrapped)

#### 10. LogDb Integration

The TUI currently doesn't open `log.db`. In `run_tui()` or `run_tui_loop()`:
- Open `LogDb` from `.hive/log.db` path
- Pass it alongside `HiveState` into the render loop
- If `log.db` doesn't exist or can't open, gracefully degrade (empty activity stream for tool calls, messages still shown)
