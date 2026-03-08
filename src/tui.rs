use crate::state::HiveState;
use crate::types::*;
use chrono::{DateTime, Utc};
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use rusqlite::Connection;
use std::io::stdout;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Swarm,
    Tasks,
    Activity,
}

#[derive(Clone)]
enum Overlay {
    Agent(String),
    Task(String),
}

struct TuiState {
    focused_pane: Pane,
    swarm_selected: Option<usize>,
    tasks_selected: Option<usize>,
    activity_scroll: usize,
    activity_auto_scroll: bool,
    overlay: Option<Overlay>,
    selected_agent_filter: Option<String>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            focused_pane: Pane::Swarm,
            swarm_selected: None,
            tasks_selected: None,
            activity_scroll: 0,
            activity_auto_scroll: true,
            overlay: None,
            selected_agent_filter: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Activity entries (interleaved messages + tool calls)
// ---------------------------------------------------------------------------

enum ActivityEntry {
    Message {
        timestamp: DateTime<Utc>,
        from: String,
        to: String,
        body: String,
    },
    ToolCall {
        timestamp: DateTime<Utc>,
        agent_id: String,
        tool_name: String,
        args_summary: Option<String>,
        status: String,
        duration_ms: Option<i64>,
    },
}

impl ActivityEntry {
    fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::Message { timestamp, .. } | Self::ToolCall { timestamp, .. } => *timestamp,
        }
    }

    fn agent_id(&self) -> &str {
        match self {
            Self::Message { from, .. } => from,
            Self::ToolCall { agent_id, .. } => agent_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Flattened tree node (for swarm pane)
// ---------------------------------------------------------------------------

struct TreeNode {
    agent_id: String,
    prefix: String,
    status: AgentStatus,
    task_id: Option<String>,
    heartbeat: Option<DateTime<Utc>>,
    _role: AgentRole,
}

fn build_tree(agents: &[Agent]) -> Vec<TreeNode> {
    let mut nodes = Vec::new();
    // Find roots (no parent)
    let mut roots: Vec<&Agent> = agents.iter().filter(|a| a.parent.is_none()).collect();
    roots.sort_by(|a, b| {
        let ord_a = if a.role == AgentRole::Coordinator {
            0
        } else {
            1
        };
        let ord_b = if b.role == AgentRole::Coordinator {
            0
        } else {
            1
        };
        ord_a.cmp(&ord_b).then(a.id.cmp(&b.id))
    });
    for root in &roots {
        add_children(&mut nodes, agents, root, "");
    }
    nodes
}

fn add_children(nodes: &mut Vec<TreeNode>, agents: &[Agent], agent: &Agent, prefix: &str) {
    nodes.push(TreeNode {
        agent_id: agent.id.clone(),
        prefix: prefix.to_string(),
        status: agent.status,
        task_id: agent.task_id.clone(),
        heartbeat: agent.heartbeat,
        _role: agent.role,
    });

    let mut children: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(&agent.id))
        .collect();
    children.sort_by(|a, b| a.id.cmp(&b.id));

    for (i, child) in children.iter().enumerate() {
        let is_last = i == children.len() - 1;
        let connector = if is_last {
            "\u{2514}\u{2500} "
        } else {
            "\u{251C}\u{2500} "
        };
        let child_prefix = format!("{prefix}{connector}");
        let continuation = if is_last { "   " } else { "\u{2502}  " };
        let next_prefix = format!("{prefix}{continuation}");
        add_subtree(nodes, agents, child, &child_prefix, &next_prefix);
    }
}

fn add_subtree(
    nodes: &mut Vec<TreeNode>,
    agents: &[Agent],
    agent: &Agent,
    this_prefix: &str,
    child_prefix: &str,
) {
    nodes.push(TreeNode {
        agent_id: agent.id.clone(),
        prefix: this_prefix.to_string(),
        status: agent.status,
        task_id: agent.task_id.clone(),
        heartbeat: agent.heartbeat,
        _role: agent.role,
    });

    let mut children: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(&agent.id))
        .collect();
    children.sort_by(|a, b| a.id.cmp(&b.id));

    for (i, child) in children.iter().enumerate() {
        let is_last = i == children.len() - 1;
        let connector = if is_last {
            "\u{2514}\u{2500} "
        } else {
            "\u{251C}\u{2500} "
        };
        let this_pref = format!("{child_prefix}{connector}");
        let continuation = if is_last { "   " } else { "\u{2502}  " };
        let next_pref = format!("{child_prefix}{continuation}");
        add_subtree(nodes, agents, child, &this_pref, &next_pref);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn status_abbrev(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Running => "run",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Failed => "fail",
        AgentStatus::Stalled => "stal",
    }
}

fn agent_status_color(s: AgentStatus) -> Color {
    match s {
        AgentStatus::Running => Color::Green,
        AgentStatus::Idle => Color::Cyan,
        AgentStatus::Done => Color::LightBlue,
        AgentStatus::Failed => Color::Red,
        AgentStatus::Stalled => Color::Yellow,
    }
}

fn task_status_color(s: TaskStatus) -> Color {
    match s {
        TaskStatus::Active | TaskStatus::Approved => Color::Green,
        TaskStatus::Merged => Color::LightBlue,
        TaskStatus::Queued | TaskStatus::Review | TaskStatus::Blocked => Color::Yellow,
        TaskStatus::Pending => Color::Gray,
        TaskStatus::Failed => Color::Red,
    }
}

fn task_status_bullet(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Active => "\u{25CF} active",
        TaskStatus::Merged => "\u{25CF} merged",
        TaskStatus::Queued => "\u{25C9} queued",
        TaskStatus::Review => "\u{25C9} review",
        TaskStatus::Pending => "\u{25CB} pending",
        TaskStatus::Blocked => "\u{25CB} blocked",
        TaskStatus::Approved => "\u{25CF} approved",
        TaskStatus::Failed => "\u{2717} failed",
    }
}

fn format_duration_short(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn heartbeat_color(age_secs: i64, stall_timeout: i64) -> Color {
    if age_secs < 120 {
        Color::White
    } else if age_secs < stall_timeout {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn border_color(focused: Pane, this: Pane) -> Color {
    if focused == this {
        Color::Cyan
    } else {
        Color::Gray
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn load_run_metadata(state: &HiveState, run_id: &str) -> Option<RunMetadata> {
    let path = state.run_dir(run_id).join("run.json");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn load_tool_calls(log_db: &Option<Connection>, run_id: &str) -> Vec<ActivityEntry> {
    let conn = match log_db {
        Some(c) => c,
        None => return vec![],
    };
    let mut stmt = match conn.prepare(
        "SELECT timestamp, agent_id, tool_name, args_summary, status, duration_ms \
         FROM tool_calls WHERE run_id = ?1 ORDER BY timestamp DESC LIMIT 200",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(rusqlite::params![run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut entries: Vec<ActivityEntry> = rows
        .filter_map(|r| r.ok())
        .filter_map(
            |(ts_str, agent_id, tool_name, args_summary, status, duration_ms)| {
                let ts = ts_str.parse::<DateTime<Utc>>().ok()?;
                Some(ActivityEntry::ToolCall {
                    timestamp: ts,
                    agent_id,
                    tool_name,
                    args_summary,
                    status,
                    duration_ms,
                })
            },
        )
        .collect();
    entries.reverse();
    entries
}

// ---------------------------------------------------------------------------
// Terminal guard (RAII)
// ---------------------------------------------------------------------------

/// RAII guard that restores terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run_tui() -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    // Try to open log.db for tool call activity (direct connection, no LogDb wrapper needed)
    let log_db = {
        let run_log_path = state.run_dir(&run_id).join("log.db");
        let hive_log_path = state.hive_dir().join("log.db");
        Connection::open(&run_log_path)
            .ok()
            .or_else(|| Connection::open(&hive_log_path).ok())
    };

    enable_raw_mode().map_err(|e| e.to_string())?;
    let _guard = TerminalGuard;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;

    run_tui_loop(&state, &run_id, &log_db)
}

fn run_tui_loop(
    state: &HiveState,
    run_id: &str,
    log_db: &Option<Connection>,
) -> Result<(), String> {
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;

    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();
    let mut ui = TuiState::default();
    let config = state.load_config();
    let stall_timeout = config.stall_timeout_seconds;

    loop {
        // ---- Load data ----
        let agents = state.list_agents(run_id).unwrap_or_default();
        let tasks = state.list_tasks(run_id).unwrap_or_default();
        let queue = state
            .load_merge_queue(run_id)
            .unwrap_or(MergeQueue { entries: vec![] });
        let messages = state.list_messages(run_id).unwrap_or_default();
        let run_meta = load_run_metadata(state, run_id);
        let tree_nodes = build_tree(&agents);

        // Build activity entries
        let mut activity: Vec<ActivityEntry> = messages
            .iter()
            .map(|m| ActivityEntry::Message {
                timestamp: m.timestamp,
                from: m.from.clone(),
                to: m.to.clone(),
                body: m.body.clone(),
            })
            .collect();
        activity.extend(load_tool_calls(log_db, run_id));
        activity.sort_by_key(|e| e.timestamp());

        let activity_len = activity.len();

        // ---- Draw ----
        terminal
            .draw(|frame| {
                let outer = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),  // Title bar
                        Constraint::Length(1),  // Stats bar
                        Constraint::Min(8),     // Main content
                        Constraint::Length(10), // Activity stream
                    ])
                    .split(frame.area());

                let main_content = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(35), // Swarm
                        Constraint::Percentage(65), // Tasks
                    ])
                    .split(outer[2]);

                // -- Title bar --
                render_title_bar(frame, outer[0], run_id, &run_meta);

                // -- Stats bar --
                render_stats_bar(frame, outer[1], &agents, &tasks);

                // -- Swarm pane --
                render_swarm_pane(
                    frame,
                    main_content[0],
                    &tree_nodes,
                    &queue,
                    &ui,
                    stall_timeout,
                );

                // -- Tasks pane --
                render_tasks_pane(frame, main_content[1], &tasks, &ui);

                // -- Activity stream --
                render_activity_stream(frame, outer[3], &activity, &ui);

                // -- Overlay --
                if let Some(ref overlay) = ui.overlay {
                    render_overlay(frame, overlay, &agents, &tasks);
                }
            })
            .map_err(|e| e.to_string())?;

        // ---- Handle input ----
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| e.to_string())?
            && let Event::Key(key) = event::read().map_err(|e| e.to_string())?
        {
            match key.code {
                KeyCode::Char('q') => {
                    if ui.overlay.is_none() {
                        break;
                    }
                }
                KeyCode::Esc => {
                    if ui.overlay.is_some() {
                        ui.overlay = None;
                    } else if ui.selected_agent_filter.is_some() {
                        ui.selected_agent_filter = None;
                        ui.swarm_selected = None;
                    }
                }
                KeyCode::Tab => {
                    ui.focused_pane = match ui.focused_pane {
                        Pane::Swarm => Pane::Tasks,
                        Pane::Tasks => Pane::Activity,
                        Pane::Activity => Pane::Swarm,
                    };
                }
                KeyCode::Char('j') | KeyCode::Down => match ui.focused_pane {
                    Pane::Swarm => {
                        let max = tree_nodes.len().saturating_sub(1);
                        let next = ui.swarm_selected.map_or(0, |i| (i + 1).min(max));
                        ui.swarm_selected = Some(next);
                        ui.selected_agent_filter = tree_nodes.get(next).map(|n| n.agent_id.clone());
                    }
                    Pane::Tasks => {
                        let max = tasks.len().saturating_sub(1);
                        let next = ui.tasks_selected.map_or(0, |i| (i + 1).min(max));
                        ui.tasks_selected = Some(next);
                    }
                    Pane::Activity => {
                        ui.activity_auto_scroll = false;
                        ui.activity_scroll = ui.activity_scroll.saturating_add(1);
                    }
                },
                KeyCode::Char('k') | KeyCode::Up => match ui.focused_pane {
                    Pane::Swarm => {
                        if let Some(i) = ui.swarm_selected {
                            let next = i.saturating_sub(1);
                            ui.swarm_selected = Some(next);
                            ui.selected_agent_filter =
                                tree_nodes.get(next).map(|n| n.agent_id.clone());
                        }
                    }
                    Pane::Tasks => {
                        if let Some(i) = ui.tasks_selected {
                            ui.tasks_selected = Some(i.saturating_sub(1));
                        }
                    }
                    Pane::Activity => {
                        ui.activity_auto_scroll = false;
                        ui.activity_scroll = ui.activity_scroll.saturating_sub(1);
                    }
                },
                KeyCode::Char('G') => {
                    ui.activity_auto_scroll = true;
                }
                KeyCode::Enter => match ui.focused_pane {
                    Pane::Swarm => {
                        if let Some(i) = ui.swarm_selected
                            && let Some(node) = tree_nodes.get(i)
                        {
                            ui.overlay = Some(Overlay::Agent(node.agent_id.clone()));
                        }
                    }
                    Pane::Tasks => {
                        if let Some(i) = ui.tasks_selected
                            && let Some(task) = tasks.get(i)
                        {
                            ui.overlay = Some(Overlay::Task(task.id.clone()));
                        }
                    }
                    Pane::Activity => {}
                },
                _ => {}
            }
        }

        // Auto-scroll activity to bottom
        if ui.activity_auto_scroll {
            ui.activity_scroll = activity_len.saturating_sub(1);
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Render: Title bar
// ---------------------------------------------------------------------------

fn render_title_bar(frame: &mut Frame, area: Rect, run_id: &str, run_meta: &Option<RunMetadata>) {
    let uptime = run_meta
        .as_ref()
        .map(|m| {
            let elapsed = Utc::now() - m.created_at;
            let total_secs = elapsed.num_seconds().max(0);
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            format!("{mins}m {secs}s")
        })
        .unwrap_or_else(|| "??".to_string());
    let clock = chrono::Local::now().format("%H:%M:%S");
    let right = format!("Run: {run_id} ({uptime}) \u{2500}\u{2500} {clock}");

    let padding = area.width as usize;
    let left_text = " \u{2B21} HIVE";
    let gap = padding
        .saturating_sub(left_text.len())
        .saturating_sub(right.len())
        .saturating_sub(1);

    let line = Line::from(vec![
        Span::styled(left_text, Style::default().fg(Color::Cyan).bold()),
        Span::raw(" ".repeat(gap)),
        Span::styled(right, Style::default().fg(Color::Gray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Render: Stats bar
// ---------------------------------------------------------------------------

fn render_stats_bar(frame: &mut Frame, area: Rect, agents: &[Agent], tasks: &[Task]) {
    let mut spans: Vec<Span> = vec![Span::raw(" Agents: ")];

    let agent_statuses = [
        AgentStatus::Running,
        AgentStatus::Idle,
        AgentStatus::Done,
        AgentStatus::Failed,
        AgentStatus::Stalled,
    ];
    let mut first = true;
    for &status in &agent_statuses {
        let count = agents.iter().filter(|a| a.status == status).count();
        if count > 0 {
            if !first {
                spans.push(Span::styled(" \u{00B7} ", Style::default().fg(Color::Gray)));
            }
            spans.push(Span::styled(
                format!("{count} {}", format!("{status:?}").to_lowercase()),
                Style::default().fg(agent_status_color(status)),
            ));
            first = false;
        }
    }

    spans.push(Span::raw("    Tasks: "));

    let task_statuses = [
        TaskStatus::Active,
        TaskStatus::Merged,
        TaskStatus::Queued,
        TaskStatus::Review,
        TaskStatus::Pending,
        TaskStatus::Blocked,
        TaskStatus::Approved,
        TaskStatus::Failed,
    ];
    first = true;
    for &status in &task_statuses {
        let count = tasks.iter().filter(|t| t.status == status).count();
        if count > 0 {
            if !first {
                spans.push(Span::styled(" \u{00B7} ", Style::default().fg(Color::Gray)));
            }
            spans.push(Span::styled(
                format!("{count} {}", format!("{status:?}").to_lowercase()),
                Style::default().fg(task_status_color(status)),
            ));
            first = false;
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Render: Swarm pane
// ---------------------------------------------------------------------------

fn render_swarm_pane(
    frame: &mut Frame,
    area: Rect,
    tree_nodes: &[TreeNode],
    queue: &MergeQueue,
    ui: &TuiState,
    stall_timeout: i64,
) {
    let now = Utc::now();
    let mut items: Vec<ListItem> = tree_nodes
        .iter()
        .map(|node| {
            let dimmed = matches!(node.status, AgentStatus::Done | AgentStatus::Failed);
            let base_color = if dimmed {
                Color::Gray
            } else {
                agent_status_color(node.status)
            };

            let mut spans = vec![
                Span::raw(&node.prefix),
                Span::styled(&node.agent_id, Style::default().fg(base_color)),
                Span::styled(
                    format!(" [{}]", status_abbrev(node.status)),
                    Style::default().fg(base_color),
                ),
            ];

            if let Some(ref tid) = node.task_id {
                spans.push(Span::styled(
                    format!(" {tid}"),
                    Style::default().fg(if dimmed { Color::Gray } else { Color::White }),
                ));
            }

            if let Some(hb) = node.heartbeat {
                let age = (now - hb).num_seconds().max(0);
                let hb_color = if dimmed {
                    Color::Gray
                } else {
                    heartbeat_color(age, stall_timeout)
                };
                spans.push(Span::styled(
                    format!(" {}", format_duration_short(age)),
                    Style::default().fg(hb_color),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // Merge queue below tree
    if !queue.entries.is_empty() {
        items.push(ListItem::new(""));
        items.push(ListItem::new(Span::styled(
            "Queue \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            Style::default().fg(Color::Gray),
        )));
        for (i, entry) in queue.entries.iter().enumerate() {
            items.push(ListItem::new(Span::styled(
                format!("{}. {}/{}", i + 1, entry.submitted_by, entry.task_id),
                Style::default().fg(Color::White),
            )));
        }
    }

    let bc = border_color(ui.focused_pane, Pane::Swarm);
    let block = Block::default()
        .title(" Swarm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(bc));

    let mut list_state = ListState::default();
    list_state.select(ui.swarm_selected);

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(55, 55, 90)));

    frame.render_stateful_widget(list, area, &mut list_state);
}

// ---------------------------------------------------------------------------
// Render: Tasks pane
// ---------------------------------------------------------------------------

fn render_tasks_pane(frame: &mut Frame, area: Rect, tasks: &[Task], ui: &TuiState) {
    let rows: Vec<Row> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let stripe = if i % 2 == 0 {
                Style::default().bg(Color::Rgb(45, 45, 55))
            } else {
                Style::default()
            };
            let assigned = t.assigned_to.as_deref().unwrap_or("--");
            Row::new(vec![
                Cell::from(t.id.clone()),
                Cell::from(Span::styled(
                    task_status_bullet(t.status),
                    Style::default().fg(task_status_color(t.status)),
                )),
                Cell::from(assigned.to_string()),
                Cell::from(t.title.clone()),
            ])
            .style(stripe)
        })
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Min(12),
        Constraint::Min(10),
        Constraint::Min(20),
    ];

    let bc = border_color(ui.focused_pane, Pane::Tasks);
    let block = Block::default()
        .title(" Tasks ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(bc));

    let mut table_state = TableState::default();
    table_state.select(ui.tasks_selected);

    let table = Table::new(rows, widths)
        .block(block)
        .header(
            Row::new(vec!["ID", "Status", "Assigned", "Title"])
                .bold()
                .style(Style::default().fg(Color::White)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(55, 55, 90)));

    frame.render_stateful_widget(table, area, &mut table_state);
}

// ---------------------------------------------------------------------------
// Render: Activity stream
// ---------------------------------------------------------------------------

fn render_activity_stream(
    frame: &mut Frame,
    area: Rect,
    activity: &[ActivityEntry],
    ui: &TuiState,
) {
    let items: Vec<ListItem> = activity
        .iter()
        .map(|entry| {
            let is_dimmed = ui
                .selected_agent_filter
                .as_ref()
                .is_some_and(|f| f != entry.agent_id());

            match entry {
                ActivityEntry::Message {
                    timestamp,
                    from,
                    to,
                    body,
                } => {
                    let ts = timestamp.format("%H:%M:%S");
                    let text = format!("{ts} \u{25B8} {from} \u{2192} {to}: {body}");
                    let color = if is_dimmed {
                        Color::Rgb(110, 110, 120)
                    } else {
                        Color::Cyan
                    };
                    ListItem::new(Span::styled(text, Style::default().fg(color)))
                }
                ActivityEntry::ToolCall {
                    timestamp,
                    agent_id,
                    tool_name,
                    args_summary,
                    status,
                    duration_ms,
                } => {
                    let ts = timestamp.format("%H:%M:%S");
                    let args = args_summary.as_deref().unwrap_or("");
                    let (icon, color) = if status == "success" {
                        if is_dimmed {
                            ("\u{2713}", Color::Rgb(110, 110, 120))
                        } else {
                            ("\u{2713}", Color::Gray)
                        }
                    } else if is_dimmed {
                        ("\u{2717}", Color::Rgb(110, 110, 120))
                    } else {
                        ("\u{2717}", Color::Red)
                    };
                    let dur = duration_ms.map(|d| format!(" {d}ms")).unwrap_or_default();
                    let text = format!("{ts}   {agent_id}  {tool_name}  {args}  {icon}{dur}");
                    ListItem::new(Span::styled(text, Style::default().fg(color)))
                }
            }
        })
        .collect();

    let bc = border_color(ui.focused_pane, Pane::Activity);
    let block = Block::default()
        .title(" Activity ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(bc));

    // Compute visible height (area height minus borders)
    let visible_height = area.height.saturating_sub(2) as usize;
    let mut list_state = ListState::default();
    if ui.activity_auto_scroll {
        // Select last visible item
        if activity.len() > visible_height {
            list_state.select(Some(activity.len().saturating_sub(1)));
        }
    } else {
        let sel = ui.activity_scroll.min(activity.len().saturating_sub(1));
        list_state.select(Some(sel));
    }

    let list = List::new(items).block(block);

    frame.render_stateful_widget(list, area, &mut list_state);
}

// ---------------------------------------------------------------------------
// Render: Detail overlay
// ---------------------------------------------------------------------------

fn render_overlay(frame: &mut Frame, overlay: &Overlay, agents: &[Agent], tasks: &[Task]) {
    let area = centered_rect(60, 80, frame.area());
    frame.render_widget(Clear, area);

    match overlay {
        Overlay::Agent(agent_id) => {
            if let Some(agent) = agents.iter().find(|a| &a.id == agent_id) {
                render_agent_overlay(frame, area, agent);
            }
        }
        Overlay::Task(task_id) => {
            if let Some(task) = tasks.iter().find(|t| &t.id == task_id) {
                render_task_overlay(frame, area, task);
            }
        }
    }
}

fn render_agent_overlay(frame: &mut Frame, area: Rect, agent: &Agent) {
    let now = Utc::now();
    let heartbeat_str = agent
        .heartbeat
        .map(|hb| {
            let age = (now - hb).num_seconds().max(0);
            format!(
                "{} ({} ago)",
                hb.format("%H:%M:%S"),
                format_duration_short(age)
            )
        })
        .unwrap_or_else(|| "--".to_string());

    let lines = vec![
        Line::from(format!(" Role:        {:?}", agent.role)),
        Line::from(format!(" Status:      {:?}", agent.status)),
        Line::from(format!(
            " Parent:      {}",
            agent.parent.as_deref().unwrap_or("--")
        )),
        Line::from(format!(
            " Task:        {}",
            agent.task_id.as_deref().unwrap_or("--")
        )),
        Line::from(format!(
            " Worktree:    {}",
            agent.worktree.as_deref().unwrap_or("--")
        )),
        Line::from(format!(
            " PID:         {}",
            agent
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "--".to_string())
        )),
        Line::from(format!(" Heartbeat:   {heartbeat_str}")),
        Line::from(format!(
            " Session:     {}",
            agent.session_id.as_deref().unwrap_or("--")
        )),
        Line::from(""),
        Line::from(Span::styled(
            "                               [Esc] close",
            Style::default().fg(Color::Gray),
        )),
    ];

    let block = Block::default()
        .title(format!(" Agent: {} ", agent.id))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_task_overlay(frame: &mut Frame, area: Rect, task: &Task) {
    let blocked_by = if task.blocked_by.is_empty() {
        "(none)".to_string()
    } else {
        task.blocked_by.join(", ")
    };
    let blocking = if task.blocking.is_empty() {
        "(none)".to_string()
    } else {
        task.blocking.join(", ")
    };

    let mut lines = vec![
        Line::from(format!(" Title:       {}", task.title)),
        Line::from(format!(" Status:      {:?}", task.status)),
        Line::from(format!(" Urgency:     {:?}", task.urgency)),
        Line::from(format!(
            " Assigned:    {}",
            task.assigned_to.as_deref().unwrap_or("--")
        )),
        Line::from(format!(" Created by:  {}", task.created_by)),
        Line::from(format!(
            " Branch:      {}",
            task.branch.as_deref().unwrap_or("--")
        )),
        Line::from(format!(
            " Domain:      {}",
            task.domain.as_deref().unwrap_or("--")
        )),
        Line::from(format!(" Blocked by:  {blocked_by}")),
        Line::from(format!(" Blocking:    {blocking}")),
        Line::from(format!(
            " Created:     {}",
            task.created_at.format("%H:%M:%S")
        )),
        Line::from(format!(
            " Updated:     {}",
            task.updated_at.format("%H:%M:%S")
        )),
        Line::from(""),
    ];

    if !task.description.is_empty() {
        lines.push(Line::from(" Description:"));
        for l in task.description.lines() {
            lines.push(Line::from(format!(" {l}")));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "                                [Esc] close",
        Style::default().fg(Color::Gray),
    )));

    let block = Block::default()
        .title(format!(" Task: {} ", task.id))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}
