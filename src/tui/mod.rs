mod helpers;
mod input;
mod overlay;
mod render;
pub(crate) mod tree;

#[cfg(test)]
mod tests;

use crate::state::HiveState;
use crate::types::*;
use chrono::{DateTime, Utc};
use crossterm::ExecutableCommand;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use rusqlite::Connection;
use std::collections::HashSet;
use std::io::stdout;
use std::time::{Duration, Instant};

use helpers::centered_rect;
use input::handle_mouse;
use overlay::render_overlay;
use render::*;
use tree::{build_task_tree, build_tree};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    Swarm,
    Tasks,
    Activity,
}

#[derive(Clone)]
pub(crate) enum Overlay {
    Agent(String),
    Task(String),
    AgentOutput(String),
}

pub(crate) struct TuiState {
    pub focused_pane: Pane,
    pub swarm_selected: Option<usize>,
    pub tasks_selected: Option<usize>,
    pub activity_scroll: usize,
    pub activity_auto_scroll: bool,
    pub output_scroll: usize,
    pub output_auto_scroll: bool,
    pub overlay: Option<Overlay>,
    pub selected_agent_filter: Option<String>,
    pub collapsed_tasks: HashSet<String>,
    pub collapsed_agents: HashSet<String>,
    pub spec_scroll: usize,
    pub mouse_enabled: bool,
    /// Cached pane areas for mouse hit-testing (updated each frame)
    pub swarm_area: Rect,
    pub tasks_area: Rect,
    pub activity_area: Rect,
    pub overlay_area: Rect,
    pub spec_area: Rect,
    /// Double-click detection
    pub last_click: Option<(u16, u16, Instant)>,
    /// Whether running inside a terminal multiplexer
    pub inside_multiplexer: Option<&'static str>,
}

/// Detect if running inside a known terminal multiplexer.
fn detect_multiplexer() -> Option<&'static str> {
    if std::env::var("ZELLIJ_SESSION_NAME").is_ok() || std::env::var("ZELLIJ").is_ok() {
        Some("Zellij")
    } else if std::env::var("TMUX").is_ok() {
        Some("tmux")
    } else if std::env::var("STY").is_ok() {
        Some("screen")
    } else {
        None
    }
}

const DOUBLE_CLICK_MS: u128 = 400;

/// Convert a mouse row within a bordered pane to a list item index.
/// Returns None if the click is on a border.
fn pane_row_index(area: Rect, row: u16) -> Option<usize> {
    if row <= area.y || row >= area.y + area.height.saturating_sub(1) {
        return None;
    }
    Some((row - area.y - 1) as usize)
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            focused_pane: Pane::Swarm,
            swarm_selected: None,
            tasks_selected: None,
            activity_scroll: 0,
            activity_auto_scroll: true,
            output_scroll: 0,
            output_auto_scroll: true,
            overlay: None,
            selected_agent_filter: None,
            collapsed_tasks: HashSet::new(),
            collapsed_agents: HashSet::new(),
            mouse_enabled: true,
            swarm_area: Rect::default(),
            tasks_area: Rect::default(),
            activity_area: Rect::default(),
            overlay_area: Rect::default(),
            spec_area: Rect::default(),
            spec_scroll: 0,
            last_click: None,
            inside_multiplexer: detect_multiplexer(),
        }
    }
}

// ---------------------------------------------------------------------------
// Activity entries (interleaved messages + tool calls)
// ---------------------------------------------------------------------------

pub(crate) enum ActivityEntry {
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
}

// ---------------------------------------------------------------------------
// Data loading helpers
// ---------------------------------------------------------------------------

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

fn load_latest_actions(
    log_db: &Option<Connection>,
    run_id: &str,
) -> std::collections::HashMap<String, String> {
    let conn = match log_db {
        Some(c) => c,
        None => return std::collections::HashMap::new(),
    };
    // Get the most recent tool call per agent using a window function
    let mut stmt = match conn.prepare(
        "SELECT agent_id, tool_name, args_summary FROM tool_calls \
         WHERE run_id = ?1 AND rowid IN ( \
           SELECT rowid FROM ( \
             SELECT rowid, ROW_NUMBER() OVER (PARTITION BY agent_id ORDER BY timestamp DESC) as rn \
             FROM tool_calls WHERE run_id = ?1 \
           ) WHERE rn = 1 \
         )",
    ) {
        Ok(s) => s,
        Err(_) => return std::collections::HashMap::new(),
    };
    let rows = match stmt.query_map(rusqlite::params![run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return std::collections::HashMap::new(),
    };
    rows.filter_map(|r| r.ok())
        .map(|(agent_id, tool_name, args_summary)| {
            let action = format_action_summary(&tool_name, args_summary.as_deref());
            (agent_id, action)
        })
        .collect()
}

fn format_action_summary(tool_name: &str, args_summary: Option<&str>) -> String {
    // Strip common prefixes for readability
    let short_name = tool_name
        .strip_prefix("hive_")
        .or_else(|| tool_name.strip_prefix("mcp__hive__hive_"))
        .unwrap_or(tool_name);

    match args_summary {
        Some(summary) if !summary.is_empty() => {
            // Truncate the summary to keep it compact
            let truncated = if summary.len() > 30 {
                format!("{}…", &summary[..29])
            } else {
                summary.to_string()
            };
            format!("{short_name} {truncated}")
        }
        _ => short_name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Terminal guard (RAII)
// ---------------------------------------------------------------------------

/// RAII guard that restores terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = stdout().execute(DisableMouseCapture);
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
    stdout()
        .execute(EnableMouseCapture)
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
        let tree_nodes = build_tree(&agents, &ui.collapsed_agents);
        let task_tree_nodes = build_task_tree(&tasks, &ui.collapsed_tasks);
        let task_tree_len = task_tree_nodes.len();

        // Load latest action per agent for swarm pane display
        let latest_actions = load_latest_actions(log_db, run_id);

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

        // ---- Compute layout areas for mouse hit-testing ----
        let term_area = terminal.get_frame().area();
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Length(1), // Stats bar
                Constraint::Fill(3),   // Main content
                Constraint::Fill(1),   // Activity stream
            ])
            .split(term_area);

        let planner_agent = agents
            .iter()
            .find(|a| a.role == AgentRole::Planner && a.status == AgentStatus::Running);

        let (swarm_area, tasks_area) = if planner_agent.is_some() {
            (Rect::default(), Rect::default())
        } else {
            let main_content = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Fill(2), // Swarm: ~40%
                    Constraint::Fill(3), // Tasks: ~60%
                ])
                .split(outer[2]);
            (main_content[0], main_content[1])
        };

        ui.swarm_area = swarm_area;
        ui.tasks_area = tasks_area;
        ui.activity_area = outer[3];
        ui.overlay_area = if ui.overlay.is_some() {
            let pct = if matches!(ui.overlay, Some(Overlay::AgentOutput(_))) {
                (90, 90)
            } else {
                (60, 80)
            };
            centered_rect(pct.0, pct.1, term_area)
        } else {
            Rect::default()
        };

        // ---- Draw ----
        terminal
            .draw(|frame| {
                // -- Title bar --
                render_title_bar(frame, outer[0], run_id, &run_meta);

                // -- Stats bar --
                render_stats_bar(frame, outer[1], &agents, &tasks, state, &ui);

                // -- Main content: planning view or normal swarm+tasks --
                if let Some(planner) = planner_agent {
                    render_planning_view(frame, outer[2], planner);
                } else {
                    // -- Swarm pane --
                    render_swarm_pane(
                        frame,
                        swarm_area,
                        &tree_nodes,
                        &agents,
                        &queue,
                        &ui,
                        stall_timeout,
                        &latest_actions,
                    );

                    // -- Tasks pane with optional spec viewer --
                    let spec_content = state.load_planner_spec(run_id);
                    if let Some(ref spec) = spec_content {
                        let tasks_and_spec = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                            .split(tasks_area);
                        render_tasks_pane(frame, tasks_and_spec[0], &task_tree_nodes, &tasks, &ui);
                        ui.spec_area = tasks_and_spec[1];
                        render_spec_viewer(frame, tasks_and_spec[1], spec, ui.spec_scroll);
                    } else {
                        ui.spec_area = Rect::default();
                        render_tasks_pane(frame, tasks_area, &task_tree_nodes, &tasks, &ui);
                    }
                }

                // -- Activity stream --
                render_activity_stream(frame, outer[3], &activity, &ui);

                // -- Overlay --
                if let Some(ref overlay) = ui.overlay {
                    render_overlay(
                        frame,
                        overlay,
                        &agents,
                        &tasks,
                        state,
                        run_id,
                        ui.output_scroll,
                        ui.output_auto_scroll,
                    );
                }
            })
            .map_err(|e| e.to_string())?;

        // ---- Handle input ----
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| e.to_string())? {
            let ev = event::read().map_err(|e| e.to_string())?;

            // --- Mouse events ---
            if let Event::Mouse(mouse) = ev
                && ui.mouse_enabled
            {
                handle_mouse(&mut ui, mouse, &tree_nodes, &task_tree_nodes);
            }

            // --- Keyboard events ---
            if let Event::Key(key) = ev {
                // Intercept keys when AgentOutput overlay is open
                if matches!(ui.overlay, Some(Overlay::AgentOutput(_))) {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') => {
                            ui.overlay = None;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            // output_scroll is offset from bottom: 0 = bottom
                            if ui.output_scroll > 0 {
                                ui.output_auto_scroll = false;
                                ui.output_scroll = ui.output_scroll.saturating_sub(1);
                                if ui.output_scroll == 0 {
                                    ui.output_auto_scroll = true;
                                }
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            // output_scroll is offset from bottom: increase to scroll up
                            ui.output_auto_scroll = false;
                            ui.output_scroll = ui.output_scroll.saturating_add(1);
                        }
                        KeyCode::Char('G') => {
                            ui.output_auto_scroll = true;
                            ui.output_scroll = 0;
                        }
                        _ => {}
                    }
                    // Skip normal key dispatch
                    if last_tick.elapsed() >= tick_rate {
                        last_tick = Instant::now();
                    }
                    continue;
                }

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
                    KeyCode::Char('m') => {
                        ui.mouse_enabled = !ui.mouse_enabled;
                        if ui.mouse_enabled {
                            let _ = stdout().execute(EnableMouseCapture);
                        } else {
                            let _ = stdout().execute(DisableMouseCapture);
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => match ui.focused_pane {
                        Pane::Swarm => {
                            let max = tree_nodes.len().saturating_sub(1);
                            let next = ui.swarm_selected.map_or(0, |i| (i + 1).min(max));
                            ui.swarm_selected = Some(next);
                            ui.selected_agent_filter =
                                tree_nodes.get(next).map(|n| n.agent_id.clone());
                        }
                        Pane::Tasks => {
                            let max = task_tree_len.saturating_sub(1);
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
                    KeyCode::Char(' ') => match ui.focused_pane {
                        Pane::Swarm => {
                            if let Some(i) = ui.swarm_selected
                                && let Some(node) = tree_nodes.get(i)
                                && node.has_children
                            {
                                if ui.collapsed_agents.contains(&node.agent_id) {
                                    ui.collapsed_agents.remove(&node.agent_id);
                                } else {
                                    ui.collapsed_agents.insert(node.agent_id.clone());
                                }
                            }
                        }
                        Pane::Tasks => {
                            if let Some(i) = ui.tasks_selected
                                && let Some(node) = task_tree_nodes.get(i)
                                && node.has_children
                            {
                                if ui.collapsed_tasks.contains(&node.task_id) {
                                    ui.collapsed_tasks.remove(&node.task_id);
                                } else {
                                    ui.collapsed_tasks.insert(node.task_id.clone());
                                }
                            }
                        }
                        Pane::Activity => {}
                    },
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
                                && let Some(node) = task_tree_nodes.get(i)
                            {
                                ui.overlay = Some(Overlay::Task(node.task_id.clone()));
                            }
                        }
                        Pane::Activity => {}
                    },
                    KeyCode::Char('o') => {
                        if ui.focused_pane == Pane::Swarm
                            && ui.overlay.is_none()
                            && let Some(i) = ui.swarm_selected
                            && let Some(node) = tree_nodes.get(i)
                        {
                            ui.output_scroll = 0;
                            ui.output_auto_scroll = true;
                            ui.overlay = Some(Overlay::AgentOutput(node.agent_id.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Auto-scroll activity to bottom, or clamp manual scroll to valid range
        if ui.activity_auto_scroll {
            ui.activity_scroll = activity_len.saturating_sub(1);
        } else {
            ui.activity_scroll = ui.activity_scroll.min(activity_len.saturating_sub(1));
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    Ok(())
}
