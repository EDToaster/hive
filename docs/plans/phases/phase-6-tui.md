# Phase 6: TUI Dashboard

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a read-only TUI monitoring dashboard with panels for agents, tasks, merge queue, and messages.

**Prerequisite:** Phase 5 complete (MCP server working).

**Spec:** See `docs/plans/2026-03-08-hive-spec.md` for the full design.

---

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

## Next Phase

Proceed to Phase 7: `docs/plans/phases/phase-7-integration.md`
