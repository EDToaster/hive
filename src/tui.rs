use crate::state::HiveState;
use crate::types::*;
use chrono::{DateTime, Utc};
use crossterm::ExecutableCommand;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use rusqlite::Connection;
use std::collections::HashSet;
use std::io::stdout;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Swarm,
    Tasks,
    Activity,
}

#[derive(Clone)]
enum Overlay {
    Agent(String),
    Task(String),
    AgentOutput(String),
}

struct TuiState {
    focused_pane: Pane,
    swarm_selected: Option<usize>,
    tasks_selected: Option<usize>,
    activity_scroll: usize,
    activity_auto_scroll: bool,
    output_scroll: usize,
    output_auto_scroll: bool,
    overlay: Option<Overlay>,
    selected_agent_filter: Option<String>,
    collapsed_tasks: HashSet<String>,
    collapsed_agents: HashSet<String>,
    spec_scroll: usize,
    mouse_enabled: bool,
    /// Cached pane areas for mouse hit-testing (updated each frame)
    swarm_area: Rect,
    tasks_area: Rect,
    activity_area: Rect,
    overlay_area: Rect,
    spec_area: Rect,
    /// Double-click detection
    last_click: Option<(u16, u16, Instant)>,
    /// Whether running inside a terminal multiplexer
    inside_multiplexer: Option<&'static str>,
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
        #[allow(dead_code)]
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
// Flattened tree node (for swarm pane)
// ---------------------------------------------------------------------------

struct TreeNode {
    agent_id: String,
    prefix: String,
    status: AgentStatus,
    task_id: Option<String>,
    heartbeat: Option<DateTime<Utc>>,
    role: AgentRole,
    has_children: bool,
    indicator: String,
}

// ---------------------------------------------------------------------------
// Flattened tree node (for tasks pane)
// ---------------------------------------------------------------------------

struct TaskTreeNode {
    task_id: String,
    prefix: String,    // box-drawing connector prefix
    indicator: String, // "▼ " / "▶ " / "  " (collapse indicator)
    title: String,     // task title, with aggregate suffix when collapsed
    status: TaskStatus,
    assigned_to: Option<String>,
    #[allow(dead_code)] // read in tests; will be used by Space-toggle (Task 4)
    review_count: u32,
    has_children: bool,
}

fn build_task_tree(tasks: &[Task], collapsed: &HashSet<String>) -> Vec<TaskTreeNode> {
    let mut nodes = Vec::new();

    // Partition into roots and children
    let roots: Vec<&Task> = tasks.iter().filter(|t| t.parent_task.is_none()).collect();
    let mut children_map: std::collections::HashMap<&str, Vec<&Task>> =
        std::collections::HashMap::new();
    for task in tasks {
        if let Some(ref parent_id) = task.parent_task {
            children_map
                .entry(parent_id.as_str())
                .or_default()
                .push(task);
        }
    }

    // Sort children by created_at within each group
    for children in children_map.values_mut() {
        children.sort_by_key(|t| t.created_at);
    }

    // Roots are already sorted by created_at from state.list_tasks
    for root in &roots {
        let children = children_map.get(root.id.as_str());
        let has_children = children.is_some_and(|c| !c.is_empty());
        let is_collapsed = collapsed.contains(&root.id);

        let indicator = if !has_children {
            "  ".to_string()
        } else if is_collapsed {
            "\u{25B6} ".to_string() // ▶
        } else {
            "\u{25BC} ".to_string() // ▼
        };

        let title = if is_collapsed && has_children {
            let aggregate = aggregate_child_status(children.unwrap());
            format!("{} [{}]", root.title, aggregate)
        } else if root.review_count > 0 {
            format!("{} (review cycle {})", root.title, root.review_count)
        } else {
            root.title.clone()
        };

        nodes.push(TaskTreeNode {
            task_id: root.id.clone(),
            prefix: String::new(),
            indicator,
            title,
            status: root.status,
            assigned_to: root.assigned_to.clone(),
            review_count: root.review_count,
            has_children,
        });

        // Add children if expanded
        if has_children && !is_collapsed {
            let kids = children.unwrap();
            for (i, child) in kids.iter().enumerate() {
                let is_last = i == kids.len() - 1;
                let connector = if is_last {
                    "  \u{2514}\u{2500} " // └─
                } else {
                    "  \u{251C}\u{2500} " // ├─
                };

                let child_title = if child.review_count > 0 {
                    format!("{} (review cycle {})", child.title, child.review_count)
                } else {
                    child.title.clone()
                };

                nodes.push(TaskTreeNode {
                    task_id: child.id.clone(),
                    prefix: connector.to_string(),
                    indicator: String::new(),
                    title: child_title,
                    status: child.status,
                    assigned_to: child.assigned_to.clone(),
                    review_count: child.review_count,
                    has_children: false,
                });
            }
        }
    }

    nodes
}

fn aggregate_child_status(children: &[&Task]) -> String {
    let counts: Vec<(TaskStatus, &str)> = vec![
        (TaskStatus::Active, "active"),
        (TaskStatus::Review, "review"),
        (TaskStatus::Queued, "queued"),
        (TaskStatus::Approved, "approved"),
        (TaskStatus::Merged, "merged"),
        (TaskStatus::Absorbed, "absorbed"),
        (TaskStatus::Pending, "pending"),
        (TaskStatus::Blocked, "blocked"),
        (TaskStatus::Failed, "failed"),
        (TaskStatus::Cancelled, "cancelled"),
    ];

    let mut parts = Vec::new();
    for (status, label) in &counts {
        let n = children.iter().filter(|t| t.status == *status).count();
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    parts.join(", ")
}

fn build_tree(agents: &[Agent], collapsed: &HashSet<String>) -> Vec<TreeNode> {
    let mut nodes = Vec::new();
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
        add_agent_node(&mut nodes, agents, root, "", "", collapsed);
    }
    nodes
}

fn agent_children<'a>(agents: &'a [Agent], agent_id: &str) -> Vec<&'a Agent> {
    let mut children: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(agent_id))
        .collect();
    children.sort_by(|a, b| a.id.cmp(&b.id));
    children
}

fn aggregate_agent_status(agents: &[Agent], agent_id: &str) -> String {
    // Recursively collect all descendants
    let mut descendants = Vec::new();
    let mut frontier = vec![agent_id];
    while let Some(id) = frontier.pop() {
        for a in agents {
            if a.parent.as_deref() == Some(id) {
                descendants.push(a);
                frontier.push(&a.id);
            }
        }
    }
    let statuses = [
        (AgentStatus::Running, "run"),
        (AgentStatus::Idle, "idle"),
        (AgentStatus::Done, "done"),
        (AgentStatus::Failed, "fail"),
        (AgentStatus::Stalled, "stal"),
    ];
    let mut parts = Vec::new();
    for (status, label) in &statuses {
        let n = descendants.iter().filter(|a| a.status == *status).count();
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    parts.join(", ")
}

fn add_agent_node(
    nodes: &mut Vec<TreeNode>,
    agents: &[Agent],
    agent: &Agent,
    this_prefix: &str,
    child_prefix: &str,
    collapsed: &HashSet<String>,
) {
    let children = agent_children(agents, &agent.id);
    let has_children = !children.is_empty();
    let is_collapsed = collapsed.contains(&agent.id);

    let indicator = if !has_children {
        String::new()
    } else if is_collapsed {
        "\u{25B6} ".to_string() // ▶
    } else {
        "\u{25BC} ".to_string() // ▼
    };

    nodes.push(TreeNode {
        agent_id: agent.id.clone(),
        prefix: this_prefix.to_string(),
        status: agent.status,
        task_id: agent.task_id.clone(),
        heartbeat: agent.heartbeat,
        role: agent.role,
        has_children,
        indicator,
    });

    if !is_collapsed {
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
            add_agent_node(nodes, agents, child, &this_pref, &next_pref, collapsed);
        }
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
        TaskStatus::Absorbed => Color::Cyan,
        TaskStatus::Cancelled => Color::DarkGray,
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
        TaskStatus::Absorbed => "\u{25C8} absorbed",
        TaskStatus::Cancelled => "\u{2205} cancelled",
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

fn truncate_spans(spans: Vec<Span<'_>>, max_width: usize) -> Vec<Span<'_>> {
    if max_width == 0 {
        return vec![];
    }
    let total: usize = spans.iter().map(|s| s.content.len()).sum();
    if total <= max_width {
        return spans;
    }

    let mut result = Vec::new();
    let target = max_width.saturating_sub(1); // reserve 1 char for ellipsis
    let mut remaining = target;
    for span in spans {
        let len = span.content.len();
        if remaining == 0 {
            break;
        }
        if len <= remaining {
            remaining -= len;
            result.push(span);
        } else {
            let truncated: String = span.content.chars().take(remaining).collect();
            result.push(Span::styled(truncated, span.style));
            remaining = 0;
        }
    }
    result.push(Span::styled(
        "\u{2026}",
        Style::default().fg(Color::DarkGray),
    )); // …
    result
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

// ---------------------------------------------------------------------------
// Mouse handling
// ---------------------------------------------------------------------------

fn handle_mouse(
    ui: &mut TuiState,
    mouse: MouseEvent,
    tree_nodes: &[TreeNode],
    task_tree_nodes: &[TaskTreeNode],
) {
    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        // --- Scroll wheel: target pane under cursor ---
        MouseEventKind::ScrollDown => {
            if ui.overlay.is_some() && ui.overlay_area.contains((col, row).into()) {
                // Scroll down in overlay (toward bottom)
                if ui.output_scroll > 0 {
                    ui.output_auto_scroll = false;
                    ui.output_scroll = ui.output_scroll.saturating_sub(3);
                    if ui.output_scroll == 0 {
                        ui.output_auto_scroll = true;
                    }
                }
            } else if ui.swarm_area.contains((col, row).into()) {
                let max = tree_nodes.len().saturating_sub(1);
                let next = ui.swarm_selected.map_or(0, |i| (i + 1).min(max));
                ui.swarm_selected = Some(next);
                ui.selected_agent_filter = tree_nodes.get(next).map(|n| n.agent_id.clone());
            } else if ui.spec_area.width > 0 && ui.spec_area.contains((col, row).into()) {
                ui.spec_scroll = ui.spec_scroll.saturating_add(3);
            } else if ui.tasks_area.contains((col, row).into()) {
                let max = task_tree_nodes.len().saturating_sub(1);
                let next = ui.tasks_selected.map_or(0, |i| (i + 1).min(max));
                ui.tasks_selected = Some(next);
            } else if ui.activity_area.contains((col, row).into()) {
                ui.activity_auto_scroll = false;
                ui.activity_scroll = ui.activity_scroll.saturating_add(3);
            }
        }
        MouseEventKind::ScrollUp => {
            if ui.overlay.is_some() && ui.overlay_area.contains((col, row).into()) {
                ui.output_auto_scroll = false;
                ui.output_scroll = ui.output_scroll.saturating_add(3);
            } else if ui.swarm_area.contains((col, row).into()) {
                if let Some(i) = ui.swarm_selected {
                    let next = i.saturating_sub(1);
                    ui.swarm_selected = Some(next);
                    ui.selected_agent_filter = tree_nodes.get(next).map(|n| n.agent_id.clone());
                }
            } else if ui.spec_area.width > 0 && ui.spec_area.contains((col, row).into()) {
                ui.spec_scroll = ui.spec_scroll.saturating_sub(3);
            } else if ui.tasks_area.contains((col, row).into()) {
                if let Some(i) = ui.tasks_selected {
                    ui.tasks_selected = Some(i.saturating_sub(1));
                }
            } else if ui.activity_area.contains((col, row).into()) {
                ui.activity_auto_scroll = false;
                ui.activity_scroll = ui.activity_scroll.saturating_sub(3);
            }
        }

        // --- Left click ---
        MouseEventKind::Down(MouseButton::Left) => {
            let now = Instant::now();

            // Click outside overlay dismisses it
            if ui.overlay.is_some() {
                if !ui.overlay_area.contains((col, row).into()) {
                    ui.overlay = None;
                    ui.last_click = None;
                    return;
                }
                // Clicks inside overlay are consumed
                return;
            }

            // Detect double-click
            let is_double = ui.last_click.is_some_and(|(lc, lr, lt)| {
                lc == col && lr == row && now.duration_since(lt).as_millis() < DOUBLE_CLICK_MS
            });

            // Click in Swarm pane
            if ui.swarm_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Swarm;
                if let Some(idx) = pane_row_index(ui.swarm_area, row)
                    && idx < tree_nodes.len()
                {
                    ui.swarm_selected = Some(idx);
                    ui.selected_agent_filter = tree_nodes.get(idx).map(|n| n.agent_id.clone());

                    let node = &tree_nodes[idx];

                    // Check if click is on the collapse/expand toggle indicator
                    if node.has_children {
                        let toggle_col_start = ui.swarm_area.x + 1 + node.prefix.len() as u16;
                        let toggle_col_end = toggle_col_start + 2;
                        if col >= toggle_col_start && col < toggle_col_end {
                            if ui.collapsed_agents.contains(&node.agent_id) {
                                ui.collapsed_agents.remove(&node.agent_id);
                            } else {
                                ui.collapsed_agents.insert(node.agent_id.clone());
                            }
                        }
                    }

                    // Double-click opens detail overlay
                    if is_double {
                        ui.overlay = Some(Overlay::Agent(node.agent_id.clone()));
                    }
                }
            }
            // Click in Tasks pane (extra -1 for table header row)
            else if ui.tasks_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Tasks;
                if let Some(raw) = pane_row_index(ui.tasks_area, row)
                    && raw > 0
                    && (raw - 1) < task_tree_nodes.len()
                {
                    let idx = raw - 1;
                    ui.tasks_selected = Some(idx);

                    let node = &task_tree_nodes[idx];

                    // Check if click is on the collapse/expand toggle indicator
                    if node.has_children {
                        let toggle_col_start = ui.tasks_area.x + 1 + node.prefix.len() as u16;
                        let toggle_col_end = toggle_col_start + 2;
                        if col >= toggle_col_start && col < toggle_col_end {
                            if ui.collapsed_tasks.contains(&node.task_id) {
                                ui.collapsed_tasks.remove(&node.task_id);
                            } else {
                                ui.collapsed_tasks.insert(node.task_id.clone());
                            }
                        }
                    }

                    // Double-click opens task detail overlay
                    if is_double {
                        ui.overlay = Some(Overlay::Task(node.task_id.clone()));
                    }
                }
            }
            // Click in Activity pane
            else if ui.activity_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Activity;
            }

            // Record click for double-click detection
            ui.last_click = Some((col, row, now));
        }

        // Ignore move, drag, and other mouse events for performance
        _ => {}
    }
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

    let left_text = " \u{2B21} HIVE";
    let total_width = area.width as usize;
    let content_width = left_text.len() + right.len();
    let gap = total_width.saturating_sub(content_width);

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

fn render_stats_bar(
    frame: &mut Frame,
    area: Rect,
    agents: &[Agent],
    tasks: &[Task],
    state: &HiveState,
    ui: &TuiState,
) {
    let ops_count = state.load_operations().len();
    let conventions_count = state
        .load_conventions()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    let failures_count = state.load_failures().len();

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
        TaskStatus::Absorbed,
        TaskStatus::Queued,
        TaskStatus::Review,
        TaskStatus::Pending,
        TaskStatus::Blocked,
        TaskStatus::Approved,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
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

    spans.push(Span::raw("    Memory: "));
    spans.push(Span::styled(
        format!("{ops_count} ops | {conventions_count} conventions | {failures_count} failures"),
        Style::default().fg(Color::Magenta),
    ));

    // Mouse indicator with subtle multiplexer warning
    if ui.mouse_enabled {
        let label = if let Some(mux) = ui.inside_multiplexer {
            format!("    [m] Mouse mode ON ({mux})")
        } else {
            "    [m] Mouse mode ON".to_string()
        };
        spans.push(Span::styled(label, Style::default().fg(Color::Green)));
    } else {
        spans.push(Span::styled(
            "    [m] Mouse mode OFF",
            Style::default().fg(Color::Gray),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Render: Swarm pane
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render_swarm_pane(
    frame: &mut Frame,
    area: Rect,
    tree_nodes: &[TreeNode],
    agents: &[Agent],
    queue: &MergeQueue,
    ui: &TuiState,
    stall_timeout: i64,
    latest_actions: &std::collections::HashMap<String, String>,
) {
    let now = Utc::now();
    let inner_width = area.width.saturating_sub(2) as usize; // subtract borders
    let mut items: Vec<ListItem> = tree_nodes
        .iter()
        .map(|node| {
            let dimmed = matches!(node.status, AgentStatus::Done | AgentStatus::Failed);
            let base_color = if dimmed {
                Color::Gray
            } else {
                agent_status_color(node.status)
            };
            let name_color = match node.role {
                AgentRole::Planner => Color::Cyan,
                AgentRole::Postmortem => Color::DarkGray,
                _ => base_color,
            };

            let mut spans = vec![
                Span::raw(&node.prefix),
                Span::raw(&node.indicator),
                Span::styled(&node.agent_id, Style::default().fg(name_color)),
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

            if node.has_children && ui.collapsed_agents.contains(&node.agent_id) {
                let agg = aggregate_agent_status(agents, &node.agent_id);
                spans.push(Span::styled(
                    format!(" [{agg}]"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            if node.role != AgentRole::Coordinator
                && let Some(hb) = node.heartbeat
            {
                let age = (now - hb).num_seconds().max(0);
                let hb_color = if dimmed {
                    Color::Gray
                } else {
                    heartbeat_color(age, stall_timeout)
                };
                // Show current action for running agents
                if let Some(action) = latest_actions.get(&node.agent_id).filter(|_| !dimmed) {
                    spans.push(Span::styled(
                        format!(" {action}"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                spans.push(Span::styled(
                    format!(" {}", format_duration_short(age)),
                    Style::default().fg(hb_color),
                ));
            }

            ListItem::new(Line::from(truncate_spans(spans, inner_width)))
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
        .title_bottom(Line::from(" [Enter] detail  [o] output ").right_aligned())
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

fn render_tasks_pane(
    frame: &mut Frame,
    area: Rect,
    tree_nodes: &[TaskTreeNode],
    tasks: &[Task],
    ui: &TuiState,
) {
    // Build set of highlighted task IDs: tasks assigned to the selected agent + their subtasks
    let highlighted_tasks: HashSet<&str> = if let Some(ref filter) = ui.selected_agent_filter {
        let direct: HashSet<&str> = tasks
            .iter()
            .filter(|t| t.assigned_to.as_deref() == Some(filter.as_str()))
            .map(|t| t.id.as_str())
            .collect();
        let mut all = direct.clone();
        // Add children of directly matched tasks (recursively)
        let mut frontier: Vec<&str> = direct.into_iter().collect();
        while let Some(parent_id) = frontier.pop() {
            for t in tasks {
                if t.parent_task.as_deref() == Some(parent_id) && all.insert(t.id.as_str()) {
                    frontier.push(t.id.as_str());
                }
            }
        }
        all
    } else {
        HashSet::new()
    };

    let rows: Vec<Row> = tree_nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let is_dimmed = ui.selected_agent_filter.is_some()
                && !highlighted_tasks.contains(node.task_id.as_str());

            let stripe = if is_dimmed {
                Style::default().fg(Color::Rgb(110, 110, 120))
            } else if i % 2 == 0 {
                Style::default().bg(Color::Rgb(45, 45, 55))
            } else {
                Style::default()
            };

            let id_cell = format!("{}{}{}", node.indicator, node.prefix, node.task_id);
            let assigned = node.assigned_to.as_deref().unwrap_or("--");

            let status_style = if is_dimmed {
                Style::default().fg(Color::Rgb(110, 110, 120))
            } else {
                Style::default().fg(task_status_color(node.status))
            };

            Row::new(vec![
                Cell::from(id_cell),
                Cell::from(Span::styled(task_status_bullet(node.status), status_style)),
                Cell::from(assigned.to_string()),
                Cell::from(node.title.clone()),
            ])
            .style(stripe)
        })
        .collect();

    let widths = [
        Constraint::Length(20), // indicator + prefix + ID
        Constraint::Length(12), // status bullet
        Constraint::Length(14), // assigned agent
        Constraint::Fill(1),    // title gets remaining space
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
// Tool call formatting
// ---------------------------------------------------------------------------

/// Extract a value for `key` from a comma-separated "key=value, key2=value2" string.
/// Return a display string and color for a tool call.
/// Hive tools get `Color::Yellow`, standard Claude tools get `Color::Gray`.
fn format_tool_display(tool_name: &str, _args_summary: Option<&str>) -> (String, Color) {
    // Strip MCP prefix: "mcp__<server>__<tool>" → "<tool>"
    let tool_name = if let Some(rest) = tool_name.strip_prefix("mcp__") {
        rest.find("__").map_or(tool_name, |i| &rest[i + 2..])
    } else {
        tool_name
    };

    match tool_name {
        // --- Hive MCP tools ---
        "hive_wait_for_activity" => ("WaitForActivity".to_string(), Color::Yellow),
        "hive_spawn_agent" => ("SpawnAgent".to_string(), Color::Yellow),
        "hive_check_agents" => ("CheckAgents".to_string(), Color::Yellow),
        "hive_send_message" => ("SendMessage".to_string(), Color::Yellow),
        "hive_create_task" => ("CreateTask".to_string(), Color::Yellow),
        "hive_update_task" => ("UpdateTask".to_string(), Color::Yellow),
        "hive_submit_to_queue" => ("SubmitToQueue".to_string(), Color::Yellow),
        "hive_merge_next" => ("MergeNext".to_string(), Color::Yellow),
        "hive_review_verdict" => ("ReviewVerdict".to_string(), Color::Yellow),
        name if name.starts_with("hive_") => {
            let short = name.strip_prefix("hive_").unwrap_or(name);
            (short.to_string(), Color::Yellow)
        }
        // --- Common Claude tools ---
        "Read" | "Write" | "Edit" | "Bash" | "Grep" | "Glob" => {
            (tool_name.to_string(), Color::Gray)
        }
        // --- Fallback ---
        _ => (tool_name.to_string(), Color::Gray),
    }
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
                .is_some_and(|f| match entry {
                    ActivityEntry::Message { from, to, .. } => f != from && f != to,
                    ActivityEntry::ToolCall { agent_id, .. } => f != agent_id,
                });

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
                    status,
                    duration_ms,
                    ..
                } => {
                    let ts = timestamp.format("%H:%M:%S");
                    let (display, tool_color) = format_tool_display(tool_name, None);
                    let (icon, color) = if status == "success" {
                        if is_dimmed {
                            ("\u{2713}", Color::Rgb(110, 110, 120))
                        } else {
                            ("\u{2713}", tool_color)
                        }
                    } else if is_dimmed {
                        ("\u{2717}", Color::Rgb(110, 110, 120))
                    } else {
                        ("\u{2717}", Color::Red)
                    };
                    let dur = duration_ms.map(|d| format!(" {d}ms")).unwrap_or_default();
                    let text = format!("{ts} {icon}  {agent_id}  {display}{dur}");
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

    // Scrollbar
    let scroll_position = if ui.activity_auto_scroll {
        activity.len().saturating_sub(1)
    } else {
        ui.activity_scroll
    };
    let mut scrollbar_state = ScrollbarState::new(activity.len()).position(scroll_position);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .track_style(Style::default().fg(Color::DarkGray))
        .thumb_style(Style::default().fg(Color::Gray));
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

// ---------------------------------------------------------------------------
// Render: Planning view
// ---------------------------------------------------------------------------

fn render_planning_view(frame: &mut Frame, area: Rect, planner: &Agent) {
    let elapsed = planner
        .heartbeat
        .map(|hb| {
            let age = (Utc::now() - hb).num_seconds().max(0);
            format_duration_short(age)
        })
        .unwrap_or_else(|| "??".to_string());

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "\u{27C1} Planning...",
            Style::default().fg(Color::Cyan).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Planner agent is analyzing the codebase and writing a spec",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            format!("Elapsed: {elapsed}"),
            Style::default().fg(Color::White),
        )),
    ];

    let block = Block::default()
        .title(" Planning Phase ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Render: Spec viewer
// ---------------------------------------------------------------------------

fn render_spec_viewer(frame: &mut Frame, area: Rect, spec: &str, scroll: usize) {
    let lines: Vec<Line> = spec.lines().map(|l| Line::from(l.to_string())).collect();
    let block = Block::default()
        .title(" Spec ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Render: Detail overlay
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render_overlay(
    frame: &mut Frame,
    overlay: &Overlay,
    agents: &[Agent],
    tasks: &[Task],
    state: &HiveState,
    run_id: &str,
    output_scroll: usize,
    output_auto_scroll: bool,
) {
    match overlay {
        Overlay::Agent(agent_id) => {
            let area = centered_rect(60, 80, frame.area());
            frame.render_widget(Clear, area);
            if let Some(agent) = agents.iter().find(|a| &a.id == agent_id) {
                render_agent_overlay(frame, area, agent);
            }
        }
        Overlay::Task(task_id) => {
            let area = centered_rect(60, 80, frame.area());
            frame.render_widget(Clear, area);
            if let Some(task) = tasks.iter().find(|t| &t.id == task_id) {
                render_task_overlay(frame, area, task);
            }
        }
        Overlay::AgentOutput(agent_id) => {
            let area = centered_rect(90, 90, frame.area());
            frame.render_widget(Clear, area);
            let path = state.agents_dir(run_id).join(agent_id).join("output.jsonl");
            render_agent_output_overlay(
                frame,
                area,
                agent_id,
                &path,
                output_scroll,
                output_auto_scroll,
            );
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
    ];

    if task.review_count > 0 {
        lines.push(Line::from(format!(" Reviews:     {}", task.review_count)));
    }

    lines.push(Line::from(""));

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

fn render_agent_output_overlay(
    frame: &mut Frame,
    area: Rect,
    agent_id: &str,
    path: &std::path::Path,
    scroll: usize,
    auto_scroll: bool,
) {
    use crate::output::{OutputEntry, load_output_file, parse_output_lines};

    let raw_lines = load_output_file(path);
    let entries = parse_output_lines(&raw_lines);

    let mut lines: Vec<Line> = Vec::new();

    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for entry in &entries {
            match entry {
                OutputEntry::AssistantText(text) => {
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(
                            format!(" {l}"),
                            Style::default().fg(Color::White),
                        )));
                    }
                }
                OutputEntry::ToolUse {
                    name,
                    input_summary,
                } => {
                    lines.push(Line::from(vec![
                        Span::styled("\u{25b6} ", Style::default().fg(Color::Yellow)),
                        Span::styled(
                            name.as_str(),
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" {input_summary}"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                OutputEntry::ToolResult { content } => {
                    let result_lines: Vec<&str> = content.lines().collect();
                    let show = result_lines.len().min(5);
                    for l in &result_lines[..show] {
                        lines.push(Line::from(Span::styled(
                            format!("   {l}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                    if result_lines.len() > 5 {
                        lines.push(Line::from(Span::styled(
                            format!("   ... ({} more lines)", result_lines.len() - 5),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                OutputEntry::Result {
                    duration_ms,
                    cost_usd,
                    num_turns,
                    ..
                } => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "\u{2500}\u{2500} Session Complete \u{2500}\u{2500}",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )));
                    let secs = duration_ms / 1000;
                    lines.push(Line::from(Span::styled(
                        format!(
                            " Duration: {}m{}s  Cost: ${:.4}  Turns: {}",
                            secs / 60,
                            secs % 60,
                            cost_usd,
                            num_turns
                        ),
                        Style::default().fg(Color::Green),
                    )));
                }
            }
        }
    }

    // Overscroll: breathing room at the bottom
    for _ in 0..5 {
        lines.push(Line::from(""));
    }

    let logical_lines = lines.len();
    // Visible height inside the block (subtract 2 for top/bottom border)
    let visible = area.height.saturating_sub(2) as usize;
    // Extra visual rows from wrapping: Paragraph::scroll operates on logical lines
    // but wrapped lines consume extra visual rows, so we need extra scroll headroom
    let wrap_width = area.width.saturating_sub(2) as usize;
    let wrap_extra: usize = lines
        .iter()
        .map(|line| {
            let w: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if wrap_width == 0 {
                0
            } else {
                w.saturating_sub(1) / wrap_width
            } // extra rows beyond first
        })
        .sum();
    let max_scroll = (logical_lines + wrap_extra).saturating_sub(visible);
    // scroll is offset from bottom: 0 = bottom, N = N lines up
    let effective_scroll = if auto_scroll {
        max_scroll
    } else {
        max_scroll.saturating_sub(scroll)
    };

    let block = Block::default()
        .title(format!(" Output: {} ", agent_id))
        .title_bottom(Line::from(" [j/k] scroll  [G] follow  [Esc] close ").right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll as u16, 0));
    frame.render_widget(paragraph, area);

    // Scrollbar
    let mut scrollbar_state =
        ScrollbarState::new(logical_lines + wrap_extra).position(effective_scroll);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .track_style(Style::default().fg(Color::DarkGray))
        .thumb_style(Style::default().fg(Color::Gray));
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashSet;

    fn make_task(id: &str, title: &str, parent: Option<&str>, status: TaskStatus) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            status,
            urgency: Urgency::Normal,
            blocking: vec![],
            blocked_by: vec![],
            assigned_to: None,
            created_by: "test".into(),
            parent_task: parent.map(|s| s.into()),
            branch: None,
            domain: None,
            review_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn task_tree_roots_sorted_by_creation() {
        let tasks = vec![
            make_task("t-2", "Second", None, TaskStatus::Pending),
            make_task("t-1", "First", None, TaskStatus::Active),
        ];
        let nodes = build_task_tree(&tasks, &HashSet::new());
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].task_id, "t-2");
        assert_eq!(nodes[1].task_id, "t-1");
    }

    #[test]
    fn task_tree_children_nested_under_parent() {
        let tasks = vec![
            make_task("parent", "Parent Task", None, TaskStatus::Merged),
            make_task("child-1", "First Child", Some("parent"), TaskStatus::Merged),
            make_task(
                "child-2",
                "Second Child",
                Some("parent"),
                TaskStatus::Pending,
            ),
        ];
        let nodes = build_task_tree(&tasks, &HashSet::new());
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].task_id, "parent");
        assert!(nodes[0].has_children);
        assert!(nodes[0].indicator.contains('\u{25BC}')); // ▼
        assert_eq!(nodes[1].task_id, "child-1");
        assert!(nodes[1].prefix.contains('\u{251C}')); // ├
        assert_eq!(nodes[2].task_id, "child-2");
        assert!(nodes[2].prefix.contains('\u{2514}')); // └
    }

    #[test]
    fn task_tree_collapsed_hides_children() {
        let tasks = vec![
            make_task("parent", "Parent Task", None, TaskStatus::Merged),
            make_task("child-1", "First Child", Some("parent"), TaskStatus::Merged),
            make_task(
                "child-2",
                "Second Child",
                Some("parent"),
                TaskStatus::Pending,
            ),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert("parent".to_string());
        let nodes = build_task_tree(&tasks, &collapsed);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].task_id, "parent");
        assert!(nodes[0].indicator.contains('\u{25B6}')); // ▶
        assert!(nodes[0].title.contains("[1 merged, 1 pending]"));
    }

    #[test]
    fn task_tree_childless_root_has_no_indicator() {
        let tasks = vec![make_task("lone", "Lone Task", None, TaskStatus::Active)];
        let nodes = build_task_tree(&tasks, &HashSet::new());
        assert_eq!(nodes.len(), 1);
        assert!(!nodes[0].has_children);
        assert_eq!(nodes[0].indicator, "  ");
    }

    #[test]
    fn task_tree_aggregate_status_only_nonzero() {
        let tasks = vec![
            make_task("p", "Parent", None, TaskStatus::Merged),
            make_task("c1", "C1", Some("p"), TaskStatus::Merged),
            make_task("c2", "C2", Some("p"), TaskStatus::Merged),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert("p".to_string());
        let nodes = build_task_tree(&tasks, &collapsed);
        assert_eq!(nodes[0].title, "Parent [2 merged]");
    }

    #[test]
    fn task_tree_real_world_run_af5af78f() {
        // Reproduce the actual task hierarchy from run af5af78f
        let tasks = vec![
            make_task("task-77cc4261", "D1: Types+State", None, TaskStatus::Merged),
            make_task(
                "task-d345f9c0",
                "Add types to types.rs",
                Some("task-77cc4261"),
                TaskStatus::Pending,
            ),
            make_task(
                "task-c2095213",
                "Add state operations",
                Some("task-77cc4261"),
                TaskStatus::Pending,
            ),
            make_task(
                "task-f55567b3",
                "Combined impl",
                Some("task-77cc4261"),
                TaskStatus::Merged,
            ),
            make_task("task-59327398", "D2: MCP Tools", None, TaskStatus::Merged),
            make_task(
                "task-d1d70ac5",
                "Add Hive Mind MCP tools",
                Some("task-59327398"),
                TaskStatus::Merged,
            ),
            make_task("task-d7b90b26", "D3: Prompts", None, TaskStatus::Merged),
            make_task(
                "task-bf3a761c",
                "Implement prompts",
                Some("task-d7b90b26"),
                TaskStatus::Merged,
            ),
            make_task(
                "task-0290d750",
                "D4: CLI Commands",
                None,
                TaskStatus::Merged,
            ),
            make_task(
                "task-104b2e65",
                "Add explore + mind CLI",
                Some("task-0290d750"),
                TaskStatus::Merged,
            ),
        ];

        // Expanded: 10 nodes total, 4 roots + 6 children
        let nodes = build_task_tree(&tasks, &HashSet::new());
        assert_eq!(nodes.len(), 10);

        // First root is D1 with ▼ indicator
        assert_eq!(nodes[0].task_id, "task-77cc4261");
        assert!(nodes[0].has_children);
        assert!(nodes[0].indicator.contains('\u{25BC}'));

        // Its 3 children follow
        assert_eq!(nodes[1].task_id, "task-d345f9c0");
        assert_eq!(nodes[2].task_id, "task-c2095213");
        assert_eq!(nodes[3].task_id, "task-f55567b3");

        // D2 is next root
        assert_eq!(nodes[4].task_id, "task-59327398");

        // Collapse D1: only 7 nodes visible
        let mut collapsed = HashSet::new();
        collapsed.insert("task-77cc4261".to_string());
        let nodes = build_task_tree(&tasks, &collapsed);
        assert_eq!(nodes.len(), 7);
        assert!(nodes[0].title.contains("[1 merged, 2 pending]"));
        assert_eq!(nodes[1].task_id, "task-59327398");
    }

    #[test]
    fn format_action_summary_strips_hive_prefix() {
        assert_eq!(
            format_action_summary("hive_wait_for_activity", None),
            "wait_for_activity"
        );
        assert_eq!(
            format_action_summary("hive_send_message", Some("to=coordinator")),
            "send_message to=coordinator"
        );
    }

    #[test]
    fn format_action_summary_strips_mcp_prefix() {
        assert_eq!(
            format_action_summary("mcp__hive__hive_spawn_agent", Some("role=worker")),
            "spawn_agent role=worker"
        );
    }

    #[test]
    fn format_action_summary_truncates_long_args() {
        let long_args = "a]".repeat(20); // 40 chars
        let result = format_action_summary("Read", Some(&long_args));
        assert!(result.len() < 40, "should truncate: {result}");
        assert!(result.ends_with('…'));
    }

    #[test]
    fn format_action_summary_no_prefix_passthrough() {
        assert_eq!(
            format_action_summary("Read", Some("src/main.rs")),
            "Read src/main.rs"
        );
        assert_eq!(format_action_summary("Write", None), "Write");
    }

    #[test]
    fn load_latest_actions_with_no_db() {
        let result = load_latest_actions(&None, "test-run");
        assert!(result.is_empty());
    }

    #[test]
    fn load_latest_actions_from_in_memory_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tool_calls (
                id INTEGER PRIMARY KEY,
                run_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                args_summary TEXT,
                status TEXT NOT NULL DEFAULT 'ok',
                duration_ms INTEGER,
                timestamp TEXT NOT NULL
            );
            INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
            VALUES ('run1', 'worker-1', 'Read', 'src/main.rs', 'ok', '2025-01-01T00:00:01Z');
            INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
            VALUES ('run1', 'worker-1', 'Edit', 'src/tui.rs', 'ok', '2025-01-01T00:00:02Z');
            INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
            VALUES ('run1', 'worker-2', 'hive_send_message', 'to=lead', 'ok', '2025-01-01T00:00:03Z');
            INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
            VALUES ('run2', 'worker-3', 'Read', 'other.rs', 'ok', '2025-01-01T00:00:04Z');",
        )
        .unwrap();

        let db = Some(conn);
        let actions = load_latest_actions(&db, "run1");
        assert_eq!(actions.len(), 2);
        assert_eq!(actions.get("worker-1").unwrap(), "Edit src/tui.rs");
        assert_eq!(actions.get("worker-2").unwrap(), "send_message to=lead");
        // worker-3 belongs to run2, should not appear
        assert!(!actions.contains_key("worker-3"));
    }

    #[test]
    fn task_tree_navigation_skips_collapsed_children() {
        let tasks = vec![
            make_task("p1", "Parent 1", None, TaskStatus::Active),
            make_task("c1", "Child 1", Some("p1"), TaskStatus::Pending),
            make_task("c2", "Child 2", Some("p1"), TaskStatus::Pending),
            make_task("p2", "Parent 2", None, TaskStatus::Active),
        ];

        // Expanded: 4 visible nodes
        let expanded = build_task_tree(&tasks, &HashSet::new());
        assert_eq!(expanded.len(), 4);

        // Collapsed p1: 2 visible nodes (p1, p2)
        let mut collapsed = HashSet::new();
        collapsed.insert("p1".to_string());
        let collapsed_nodes = build_task_tree(&tasks, &collapsed);
        assert_eq!(collapsed_nodes.len(), 2);
        assert_eq!(collapsed_nodes[0].task_id, "p1");
        assert_eq!(collapsed_nodes[1].task_id, "p2");
    }

    // -----------------------------------------------------------------------
    // format_tool_display tests
    // -----------------------------------------------------------------------

    #[test]
    fn format_hive_tools_show_name_only() {
        let cases = vec![
            ("hive_wait_for_activity", "WaitForActivity"),
            ("hive_spawn_agent", "SpawnAgent"),
            ("hive_check_agents", "CheckAgents"),
            ("hive_send_message", "SendMessage"),
            ("hive_create_task", "CreateTask"),
            ("hive_update_task", "UpdateTask"),
            ("hive_submit_to_queue", "SubmitToQueue"),
            ("hive_merge_next", "MergeNext"),
            ("hive_review_verdict", "ReviewVerdict"),
        ];
        for (tool, expected) in cases {
            let (display, color) = format_tool_display(tool, None);
            assert_eq!(display, expected, "tool: {tool}");
            assert_eq!(color, Color::Yellow);
        }
    }

    #[test]
    fn format_hive_unknown_tool() {
        let (display, color) = format_tool_display("hive_something_new", None);
        assert_eq!(display, "something_new");
        assert_eq!(color, Color::Yellow);
    }

    #[test]
    fn format_claude_tools_show_name_only() {
        for tool in &["Read", "Write", "Edit", "Bash", "Grep", "Glob"] {
            let (display, color) = format_tool_display(tool, None);
            assert_eq!(display, *tool);
            assert_eq!(color, Color::Gray);
        }
    }

    #[test]
    fn format_unknown_tool() {
        let (display, color) = format_tool_display("SomeNewTool", None);
        assert_eq!(display, "SomeNewTool");
        assert_eq!(color, Color::Gray);
    }

    // -----------------------------------------------------------------------
    // Mouse support tests
    // -----------------------------------------------------------------------

    fn make_tree_node(id: &str, has_children: bool) -> TreeNode {
        TreeNode {
            agent_id: id.into(),
            prefix: String::new(),
            status: AgentStatus::Running,
            task_id: None,
            heartbeat: None,
            role: AgentRole::Worker,
            has_children,
            indicator: if has_children {
                "\u{25BC} ".into()
            } else {
                "  ".into()
            },
        }
    }

    fn make_task_tree_node(id: &str, has_children: bool) -> TaskTreeNode {
        TaskTreeNode {
            task_id: id.into(),
            prefix: String::new(),
            indicator: if has_children {
                "\u{25BC} ".into()
            } else {
                "  ".into()
            },
            title: format!("Task {id}"),
            status: TaskStatus::Active,
            assigned_to: None,
            review_count: 0,
            has_children,
        }
    }

    fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_default_on() {
        let ui = TuiState::default();
        assert!(ui.mouse_enabled, "mouse should be on by default");
    }

    #[test]
    fn pane_row_index_inside_border() {
        let area = Rect::new(0, 5, 40, 10); // y=5..15, inner rows y=6..13
        assert_eq!(pane_row_index(area, 5), None); // top border
        assert_eq!(pane_row_index(area, 6), Some(0)); // first inner row
        assert_eq!(pane_row_index(area, 7), Some(1));
        assert_eq!(pane_row_index(area, 14), None); // bottom border
    }

    #[test]
    fn mouse_click_selects_swarm_item() {
        let mut ui = TuiState {
            swarm_area: Rect::new(0, 2, 40, 10),
            ..Default::default()
        };
        let nodes = vec![
            make_tree_node("agent-0", false),
            make_tree_node("agent-1", false),
        ];
        let task_nodes: Vec<TaskTreeNode> = vec![];

        // Click on row 4 → inner index 1 → agent-1
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 4),
            &nodes,
            &task_nodes,
        );
        assert_eq!(ui.swarm_selected, Some(1));
        assert_eq!(ui.focused_pane, Pane::Swarm);
        assert_eq!(ui.selected_agent_filter.as_deref(), Some("agent-1"));
    }

    #[test]
    fn mouse_click_selects_task_item() {
        let mut ui = TuiState {
            tasks_area: Rect::new(40, 2, 60, 10),
            ..Default::default()
        };
        let tree_nodes: Vec<TreeNode> = vec![];
        let task_nodes = vec![
            make_task_tree_node("task-0", false),
            make_task_tree_node("task-1", false),
        ];

        // Click on header row (y=3) → no selection (header)
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 3),
            &tree_nodes,
            &task_nodes,
        );
        assert_eq!(ui.tasks_selected, None);

        // Click on first data row (y=4) → index 0
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
            &tree_nodes,
            &task_nodes,
        );
        assert_eq!(ui.tasks_selected, Some(0));
        assert_eq!(ui.focused_pane, Pane::Tasks);
    }

    #[test]
    fn mouse_click_focuses_activity_pane() {
        let mut ui = TuiState {
            activity_area: Rect::new(0, 15, 100, 10),
            ..Default::default()
        };

        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 10, 18),
            &[],
            &[],
        );
        assert_eq!(ui.focused_pane, Pane::Activity);
    }

    #[test]
    fn mouse_scroll_down_in_swarm_advances_by_1() {
        let mut ui = TuiState {
            swarm_area: Rect::new(0, 2, 40, 10),
            swarm_selected: Some(0),
            ..Default::default()
        };
        let nodes = vec![
            make_tree_node("a-0", false),
            make_tree_node("a-1", false),
            make_tree_node("a-2", false),
            make_tree_node("a-3", false),
            make_tree_node("a-4", false),
        ];

        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::ScrollDown, 5, 5),
            &nodes,
            &[],
        );
        assert_eq!(ui.swarm_selected, Some(1)); // 0 + 1
    }

    #[test]
    fn mouse_scroll_in_activity_increases_by_3() {
        let mut ui = TuiState {
            activity_area: Rect::new(0, 15, 100, 10),
            activity_scroll: 5,
            activity_auto_scroll: false,
            ..Default::default()
        };

        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::ScrollDown, 10, 18),
            &[],
            &[],
        );
        assert_eq!(ui.activity_scroll, 8); // 5 + 3
    }

    #[test]
    fn mouse_double_click_opens_agent_overlay() {
        let mut ui = TuiState {
            swarm_area: Rect::new(0, 2, 40, 10),
            ..Default::default()
        };
        let nodes = vec![make_tree_node("agent-x", false)];

        // First click
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
            &nodes,
            &[],
        );
        assert!(ui.overlay.is_none());

        // Second click at same position (double-click)
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
            &nodes,
            &[],
        );
        assert!(matches!(ui.overlay, Some(Overlay::Agent(ref id)) if id == "agent-x"));
    }

    #[test]
    fn mouse_double_click_opens_task_overlay() {
        let mut ui = TuiState {
            tasks_area: Rect::new(40, 2, 60, 10),
            ..Default::default()
        };
        let task_nodes = vec![make_task_tree_node("task-y", false)];

        // First click (y=4: first data row, past header at y=3)
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
            &[],
            &task_nodes,
        );
        // Second click (double-click)
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
            &[],
            &task_nodes,
        );
        assert!(matches!(ui.overlay, Some(Overlay::Task(ref id)) if id == "task-y"));
    }

    #[test]
    fn mouse_click_outside_overlay_dismisses() {
        let mut ui = TuiState {
            overlay: Some(Overlay::Agent("agent-z".into())),
            overlay_area: Rect::new(20, 5, 60, 30),
            ..Default::default()
        };

        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
            &[],
            &[],
        );
        assert!(ui.overlay.is_none());
    }

    #[test]
    fn mouse_click_inside_overlay_does_not_dismiss() {
        let mut ui = TuiState {
            overlay: Some(Overlay::Agent("agent-z".into())),
            overlay_area: Rect::new(20, 5, 60, 30),
            ..Default::default()
        };

        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 40, 15),
            &[],
            &[],
        );
        assert!(ui.overlay.is_some());
    }

    #[test]
    fn mouse_scroll_in_overlay_adjusts_output_scroll() {
        let mut ui = TuiState {
            overlay: Some(Overlay::AgentOutput("agent-o".into())),
            overlay_area: Rect::new(5, 3, 90, 40),
            ..Default::default()
        };

        // Scroll up inside overlay
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::ScrollUp, 50, 20),
            &[],
            &[],
        );
        assert_eq!(ui.output_scroll, 3);
        assert!(!ui.output_auto_scroll);
    }

    #[test]
    fn mouse_click_on_collapse_toggle_toggles_agent() {
        let mut ui = TuiState {
            swarm_area: Rect::new(0, 2, 40, 10),
            ..Default::default()
        };
        let nodes = vec![make_tree_node("lead-1", true)]; // has_children=true, prefix=""

        // Toggle indicator is at col = swarm_area.x + 1 + prefix.len() = 0 + 1 + 0 = 1
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
            &nodes,
            &[],
        );
        assert!(ui.collapsed_agents.contains("lead-1"));

        // Click again to expand
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
            &nodes,
            &[],
        );
        assert!(!ui.collapsed_agents.contains("lead-1"));
    }

    #[test]
    fn mouse_click_on_collapse_toggle_toggles_task() {
        let mut ui = TuiState {
            tasks_area: Rect::new(40, 2, 60, 10),
            ..Default::default()
        };
        let task_nodes = vec![make_task_tree_node("task-p", true)]; // has_children=true

        // Toggle indicator at col = tasks_area.x + 1 + prefix.len() = 40 + 1 + 0 = 41
        // y=4: first data row (y=3 is header)
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 41, 4),
            &[],
            &task_nodes,
        );
        assert!(ui.collapsed_tasks.contains("task-p"));

        // Click again to expand
        handle_mouse(
            &mut ui,
            make_mouse(MouseEventKind::Down(MouseButton::Left), 41, 4),
            &[],
            &task_nodes,
        );
        assert!(!ui.collapsed_tasks.contains("task-p"));
    }

    #[test]
    fn detect_multiplexer_returns_valid_value() {
        let result = detect_multiplexer();
        if let Some(name) = result {
            assert!(
                name == "Zellij" || name == "tmux" || name == "screen",
                "unexpected multiplexer: {name}"
            );
        }
    }
}
