use crate::state::HiveState;
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::io::stdout;
use std::time::{Duration, Instant};

/// RAII guard that restores terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}

pub fn run_tui() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    enable_raw_mode().map_err(|e| e.to_string())?;
    let _guard = TerminalGuard;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;

    run_tui_loop(&state, &run_id)
    // _guard drops here, restoring terminal even on panic
}

fn run_tui_loop(state: &HiveState, run_id: &str) -> Result<(), String> {
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;

    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();

    loop {
        // Read state
        let agents = state.list_agents(run_id).unwrap_or_default();
        let tasks = state.list_tasks(run_id).unwrap_or_default();
        let queue = state
            .load_merge_queue(run_id)
            .unwrap_or(crate::types::MergeQueue { entries: vec![] });
        let messages = state.list_messages(run_id).unwrap_or_default();

        terminal
            .draw(|frame| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Title
                        Constraint::Min(6),    // Main content
                        Constraint::Length(8), // Messages
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
                            crate::types::AgentStatus::Idle => Style::default().fg(Color::Cyan),
                            crate::types::AgentStatus::Stalled => {
                                Style::default().fg(Color::Yellow)
                            }
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
                    .map(|(i, e)| ListItem::new(format!("{}. {} ({})", i + 1, e.branch, e.task_id)))
                    .collect();
                let queue_list = List::new(queue_items).block(
                    Block::default()
                        .title(" Merge Queue ")
                        .borders(Borders::ALL),
                );
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
                let msg_list = List::new(recent).block(
                    Block::default()
                        .title(" Messages (recent) ")
                        .borders(Borders::ALL),
                );
                frame.render_widget(msg_list, chunks[2]);
            })
            .map_err(|e| e.to_string())?;

        // Handle input
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| e.to_string())?
            && let Event::Key(key) = event::read().map_err(|e| e.to_string())?
            && (key.code == KeyCode::Char('q') || key.code == KeyCode::Esc)
        {
            break;
        }
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    Ok(())
}
