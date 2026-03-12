mod gantt;
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
use std::collections::{HashMap, HashSet};
use std::io::stdout;
use std::time::{Duration, Instant};

use gantt::render_gantt_view;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewMode {
    Normal,
    Gantt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum FilterMode {
    #[default]
    All,
    RunningOnly,
    FailedOnly,
}

impl FilterMode {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::RunningOnly,
            Self::RunningOnly => Self::FailedOnly,
            Self::FailedOnly => Self::All,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::RunningOnly => "running",
            Self::FailedOnly => "failed",
        }
    }
}

#[derive(Clone)]
pub(crate) enum Overlay {
    Agent(String),
    Task(String),
    AgentOutput(String),
    Help,
}

pub(crate) struct TuiState {
    pub focused_pane: Pane,
    pub view_mode: ViewMode,
    pub swarm_selected: Option<usize>,
    pub tasks_selected: Option<usize>,
    pub activity_scroll: usize,
    pub activity_auto_scroll: bool,
    pub output_scroll: usize,
    pub output_auto_scroll: bool,
    /// Set of output entry indices that are expanded in the AgentOutput overlay
    pub output_expanded_entries: HashSet<usize>,
    pub overlay: Option<Overlay>,
    pub selected_agent_filter: Option<String>,
    pub collapsed_tasks: HashSet<String>,
    pub collapsed_agents: HashSet<String>,
    pub spec_scroll: usize,
    pub gantt_scroll: usize,
    pub mouse_enabled: bool,
    /// Incremental search/filter for the activity pane (vim-style /)
    pub search_query: String,
    pub search_active: bool,
    /// Agent status filter preset
    pub filter_mode: FilterMode,
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
    /// Animation tick counter — incremented once per tick_rate interval
    pub tick: u64,
    /// Most recent known status per agent (for detecting changes)
    pub last_agent_statuses: HashMap<String, AgentStatus>,
    /// Tick at which each agent last changed status (for border flash)
    pub status_changed_tick: HashMap<String, u64>,
    /// Most recent tick where any swarm agent status changed (drives border flash)
    pub swarm_last_change_tick: Option<u64>,
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
            view_mode: ViewMode::Normal,
            swarm_selected: None,
            tasks_selected: None,
            activity_scroll: 0,
            activity_auto_scroll: true,
            output_scroll: 0,
            output_auto_scroll: true,
            output_expanded_entries: HashSet::new(),
            overlay: None,
            selected_agent_filter: None,
            collapsed_tasks: HashSet::new(),
            collapsed_agents: HashSet::new(),
            mouse_enabled: true,
            search_query: String::new(),
            search_active: false,
            filter_mode: FilterMode::All,
            swarm_area: Rect::default(),
            tasks_area: Rect::default(),
            activity_area: Rect::default(),
            overlay_area: Rect::default(),
            spec_area: Rect::default(),
            spec_scroll: 0,
            gantt_scroll: 0,
            last_click: None,
            inside_multiplexer: detect_multiplexer(),
            tick: 0,
            last_agent_statuses: HashMap::new(),
            status_changed_tick: HashMap::new(),
            swarm_last_change_tick: None,
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
// Metrics
// ---------------------------------------------------------------------------

pub(crate) struct MetricsData {
    pub tasks_done: usize,
    pub tasks_total: usize,
    pub total_cost_usd: f64,
    /// Tool calls in the last 60 seconds
    pub throughput_per_min: f64,
    /// (agent_id, sparkline_buckets) for active agents — 20 x 30s buckets = 10min
    pub sparklines: Vec<(String, Vec<u64>)>,
}

fn compute_metrics(
    agents: &[crate::types::Agent],
    tasks: &[crate::types::Task],
    activity: &[ActivityEntry],
    state: &HiveState,
    run_id: &str,
) -> MetricsData {
    use crate::types::AgentStatus;

    let tasks_total = tasks.len();
    let tasks_done = tasks.iter().filter(|t| t.status.is_resolved()).count();

    let total_cost_usd: f64 = agents
        .iter()
        .filter_map(|a| state.load_agent_cost(run_id, &a.id))
        .map(|c| c.cost_usd)
        .sum();

    let now = Utc::now();
    let one_min_ago = now - chrono::Duration::seconds(60);
    let throughput_per_min = activity
        .iter()
        .filter(
            |e| matches!(e, ActivityEntry::ToolCall { timestamp, .. } if *timestamp >= one_min_ago),
        )
        .count() as f64;

    const NUM_BUCKETS: usize = 20;
    const BUCKET_SECS: i64 = 30;
    let sparklines = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Running)
        .map(|agent| {
            let mut buckets = vec![0u64; NUM_BUCKETS];
            for entry in activity {
                if let ActivityEntry::ToolCall {
                    agent_id,
                    timestamp,
                    ..
                } = entry
                {
                    if agent_id != &agent.id {
                        continue;
                    }
                    let age = (now - *timestamp).num_seconds();
                    if age >= 0 && age < (NUM_BUCKETS as i64 * BUCKET_SECS) {
                        let bucket = (age / BUCKET_SECS) as usize;
                        let bucket_idx = NUM_BUCKETS.saturating_sub(1 + bucket);
                        if bucket_idx < NUM_BUCKETS {
                            buckets[bucket_idx] += 1;
                        }
                    }
                }
            }
            (agent.id.clone(), buckets)
        })
        .collect();

    MetricsData {
        tasks_done,
        tasks_total,
        total_cost_usd,
        throughput_per_min,
        sparklines,
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

/// Load the first tool call timestamp per agent from log.db as approximate spawn time.
fn load_agent_spawn_times(
    log_db: &Option<Connection>,
    run_id: &str,
) -> std::collections::HashMap<String, DateTime<Utc>> {
    let conn = match log_db {
        Some(c) => c,
        None => return std::collections::HashMap::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT agent_id, MIN(timestamp) FROM tool_calls WHERE run_id = ?1 GROUP BY agent_id",
    ) {
        Ok(s) => s,
        Err(_) => return std::collections::HashMap::new(),
    };
    let rows = match stmt.query_map(rusqlite::params![run_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(r) => r,
        Err(_) => return std::collections::HashMap::new(),
    };
    rows.filter_map(|r| r.ok())
        .filter_map(|(agent_id, ts_str)| {
            ts_str
                .parse::<DateTime<Utc>>()
                .ok()
                .map(|ts| (agent_id, ts))
        })
        .collect()
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

        // Track agent status changes for border flash animation
        for agent in &agents {
            let prev = ui.last_agent_statuses.get(&agent.id).copied();
            if prev != Some(agent.status) && prev.is_some() {
                ui.status_changed_tick.insert(agent.id.clone(), ui.tick);
                ui.swarm_last_change_tick = Some(ui.tick);
            }
            ui.last_agent_statuses
                .insert(agent.id.clone(), agent.status);
        }

        // Load latest action per agent for swarm pane display
        let latest_actions = load_latest_actions(log_db, run_id);

        // Load agent spawn times from log.db for gantt view
        let spawn_times = load_agent_spawn_times(log_db, run_id);

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
                Constraint::Length(1), // [0] Title bar
                Constraint::Length(1), // [1] Stats bar
                Constraint::Length(3), // [2] Metrics bar
                Constraint::Fill(3),   // [3] Main content
                Constraint::Fill(1),   // [4] Activity stream
            ])
            .split(term_area);

        let metrics = compute_metrics(&agents, &tasks, &activity, state, run_id);

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
                .split(outer[3]);
            (main_content[0], main_content[1])
        };

        ui.swarm_area = swarm_area;
        ui.tasks_area = tasks_area;
        ui.activity_area = outer[4];
        ui.overlay_area = if ui.overlay.is_some() {
            let pct = if matches!(ui.overlay, Some(Overlay::AgentOutput(_))) {
                (90, 90)
            } else if matches!(ui.overlay, Some(Overlay::Help)) {
                (70, 85)
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
                render_title_bar(frame, outer[0], run_id, &run_meta, ui.tick);

                // -- Stats bar --
                render_stats_bar(frame, outer[1], &agents, &tasks, state, &ui);

                // -- Metrics bar --
                render_metrics_bar(frame, outer[2], &metrics, &agents);

                // -- Main content: gantt, planning view, or normal swarm+tasks --
                if ui.view_mode == ViewMode::Gantt {
                    render_gantt_view(
                        frame,
                        outer[3],
                        &agents,
                        &queue,
                        &run_meta,
                        &spawn_times,
                        ui.gantt_scroll,
                    );
                } else if let Some(planner) = planner_agent {
                    render_planning_view(frame, outer[3], planner, ui.tick);
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
                render_activity_stream(frame, outer[4], &activity, &agents, &ui);

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
                        &ui.output_expanded_entries,
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
                if let Some(Overlay::AgentOutput(ref agent_id)) = ui.overlay.clone() {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') => {
                            ui.overlay = None;
                            ui.output_expanded_entries.clear();
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
                        KeyCode::Enter => {
                            // Toggle expand/collapse all ToolResult entries
                            use crate::output::{
                                OutputEntry, load_output_file, parse_output_lines,
                            };
                            let path = state.agents_dir(run_id).join(agent_id).join("output.jsonl");
                            let raw = load_output_file(&path);
                            let entries = parse_output_lines(&raw);
                            let result_indices: Vec<usize> = entries
                                .iter()
                                .enumerate()
                                .filter(|(_, e)| matches!(e, OutputEntry::ToolResult { .. }))
                                .map(|(i, _)| i)
                                .collect();
                            let all_expanded = result_indices
                                .iter()
                                .all(|i| ui.output_expanded_entries.contains(i));
                            if all_expanded {
                                for i in &result_indices {
                                    ui.output_expanded_entries.remove(i);
                                }
                            } else {
                                for i in result_indices {
                                    ui.output_expanded_entries.insert(i);
                                }
                            }
                        }
                        _ => {}
                    }
                    // Skip normal key dispatch
                    if last_tick.elapsed() >= tick_rate {
                        last_tick = Instant::now();
                    }
                    continue;
                }

                // 2. Help overlay: any meaningful key closes it
                if matches!(ui.overlay, Some(Overlay::Help)) {
                    ui.overlay = None;
                    if last_tick.elapsed() >= tick_rate {
                        last_tick = Instant::now();
                    }
                    continue;
                }

                // 3. Search mode: capture text input
                if ui.search_active {
                    match key.code {
                        KeyCode::Esc => {
                            ui.search_active = false;
                            ui.search_query.clear();
                        }
                        KeyCode::Enter => {
                            // Commit search — keep filter active, exit typing mode
                            ui.search_active = false;
                        }
                        KeyCode::Backspace => {
                            ui.search_query.pop();
                        }
                        KeyCode::Char(c) => {
                            ui.search_query.push(c);
                        }
                        _ => {}
                    }
                    if last_tick.elapsed() >= tick_rate {
                        last_tick = Instant::now();
                    }
                    continue;
                }

                // 4. Normal key dispatch
                match key.code {
                    KeyCode::Char('q') => {
                        if ui.overlay.is_none() {
                            break;
                        }
                    }
                    KeyCode::Esc => {
                        if ui.overlay.is_some() {
                            ui.overlay = None;
                        } else if !ui.search_query.is_empty() {
                            ui.search_query.clear();
                        } else if ui.selected_agent_filter.is_some() {
                            ui.selected_agent_filter = None;
                            ui.swarm_selected = None;
                        }
                    }
                    KeyCode::Char('?') => {
                        if ui.overlay.is_none() {
                            ui.overlay = Some(Overlay::Help);
                        }
                    }
                    KeyCode::Char('/') => {
                        ui.search_active = true;
                        ui.focused_pane = Pane::Activity;
                    }
                    KeyCode::Char('f') => {
                        ui.filter_mode = ui.filter_mode.next();
                    }
                    KeyCode::Char('1') => {
                        ui.focused_pane = Pane::Swarm;
                    }
                    KeyCode::Char('2') => {
                        ui.focused_pane = Pane::Tasks;
                    }
                    KeyCode::Char('3') => {
                        ui.focused_pane = Pane::Activity;
                    }
                    KeyCode::Tab => {
                        ui.focused_pane = match ui.focused_pane {
                            Pane::Swarm => Pane::Tasks,
                            Pane::Tasks => Pane::Activity,
                            Pane::Activity => Pane::Swarm,
                        };
                    }
                    KeyCode::Char('t') => {
                        ui.view_mode = match ui.view_mode {
                            ViewMode::Normal => ViewMode::Gantt,
                            ViewMode::Gantt => ViewMode::Normal,
                        };
                        ui.gantt_scroll = 0;
                    }
                    KeyCode::Char('m') => {
                        ui.mouse_enabled = !ui.mouse_enabled;
                        if ui.mouse_enabled {
                            let _ = stdout().execute(EnableMouseCapture);
                        } else {
                            let _ = stdout().execute(DisableMouseCapture);
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if ui.view_mode == ViewMode::Gantt {
                            ui.gantt_scroll = ui.gantt_scroll.saturating_add(1);
                        } else {
                            match ui.focused_pane {
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
                            }
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if ui.view_mode == ViewMode::Gantt {
                            ui.gantt_scroll = ui.gantt_scroll.saturating_sub(1);
                        } else {
                            match ui.focused_pane {
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
                            }
                        }
                    }
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
                            ui.output_expanded_entries.clear();
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
            ui.tick = ui.tick.wrapping_add(1);
        }
    }

    Ok(())
}
