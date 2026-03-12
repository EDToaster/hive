use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::*;

use super::helpers::*;
use super::tree::{TaskTreeNode, TreeNode};
use super::{ActivityEntry, Pane, TuiState};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Render: Title bar
// ---------------------------------------------------------------------------

pub(super) fn render_title_bar(
    frame: &mut Frame,
    area: Rect,
    run_id: &str,
    run_meta: &Option<RunMetadata>,
) {
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

pub(super) fn render_stats_bar(
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
pub(super) fn render_swarm_pane(
    frame: &mut Frame,
    area: Rect,
    tree_nodes: &[TreeNode],
    agents: &[Agent],
    queue: &MergeQueue,
    ui: &TuiState,
    stall_timeout: i64,
    latest_actions: &std::collections::HashMap<String, String>,
) {
    use super::tree::aggregate_agent_status;

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

pub(super) fn render_tasks_pane(
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
pub(super) fn extract_arg<'a>(args: &'a str, key: &str) -> Option<&'a str> {
    for part in args.split(',') {
        let part = part.trim();
        if let Some(val) = part
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(val.trim());
        }
    }
    None
}

/// Return a rich display string and color for a tool call.
///
/// Hive tools get `Color::Yellow`, standard Claude tools get `Color::Gray`.
/// Returns (tool_display, args_display, color) for a tool call.
pub(super) fn format_tool_display(
    tool_name: &str,
    args_summary: Option<&str>,
) -> (String, String, Color) {
    let args = args_summary.unwrap_or("");
    // Strip MCP prefix: "mcp__<server>__<tool>" → "<tool>"
    let tool_name = if let Some(rest) = tool_name.strip_prefix("mcp__") {
        rest.find("__").map_or(tool_name, |i| &rest[i + 2..])
    } else {
        tool_name
    };

    match tool_name {
        // --- Hive MCP tools ---
        "hive_wait_for_activity" => {
            let timeout = extract_arg(args, "timeout_secs").unwrap_or("?");
            (
                "WaitForActivity".into(),
                format!("timeout: {timeout}s"),
                Color::Yellow,
            )
        }
        "hive_spawn_agent" => {
            let agent = extract_arg(args, "agent_id").unwrap_or("?");
            let role = extract_arg(args, "role").unwrap_or("?");
            (
                "SpawnAgent".into(),
                format!("{agent} {role}"),
                Color::Yellow,
            )
        }
        "hive_check_agents" => ("CheckAgents".into(), String::new(), Color::Yellow),
        "hive_send_message" => {
            let target = extract_arg(args, "to").unwrap_or("?");
            (
                "SendMessage".into(),
                format!("\u{2192} {target}"),
                Color::Yellow,
            )
        }
        "hive_create_task" => {
            let title = extract_arg(args, "title").unwrap_or("?");
            ("CreateTask".into(), title.to_string(), Color::Yellow)
        }
        "hive_update_task" => {
            let task = extract_arg(args, "task_id").unwrap_or("?");
            let status = extract_arg(args, "status").unwrap_or("?");
            (
                "UpdateTask".into(),
                format!("{task} \u{2192} {status}"),
                Color::Yellow,
            )
        }
        "hive_submit_to_queue" => {
            let task = extract_arg(args, "task_id").unwrap_or("?");
            ("SubmitToQueue".into(), task.to_string(), Color::Yellow)
        }
        "hive_merge_next" => ("MergeNext".into(), String::new(), Color::Yellow),
        "hive_review_verdict" => {
            let task = extract_arg(args, "task_id").unwrap_or("?");
            let verdict = extract_arg(args, "verdict").unwrap_or("?");
            (
                "ReviewVerdict".into(),
                format!("{task}: {verdict}"),
                Color::Yellow,
            )
        }
        "hive_list_agents" => ("ListAgents".into(), String::new(), Color::Yellow),
        "hive_list_tasks" => {
            let status = extract_arg(args, "status").unwrap_or("");
            ("ListTasks".into(), status.to_string(), Color::Yellow)
        }
        "hive_heartbeat" => ("Heartbeat".into(), String::new(), Color::Yellow),
        "hive_read_messages" => ("ReadMessages".into(), String::new(), Color::Yellow),
        "hive_run_cost" => ("RunCost".into(), String::new(), Color::Yellow),
        "hive_log_tool" => {
            let tool = extract_arg(args, "tool").unwrap_or("?");
            ("LogTool".into(), tool.to_string(), Color::Yellow)
        }
        "hive_query_mind" => {
            let query = extract_arg(args, "query").unwrap_or("?");
            ("QueryMind".into(), query.to_string(), Color::Yellow)
        }
        "hive_discover" => ("Discover".into(), String::new(), Color::Yellow),
        "hive_synthesize" => ("Synthesize".into(), String::new(), Color::Yellow),
        "hive_establish_convention" => ("EstablishConvention".into(), String::new(), Color::Yellow),
        "hive_save_spec" => ("SaveSpec".into(), String::new(), Color::Yellow),
        "hive_save_memory" => {
            let mt = extract_arg(args, "memory_type").unwrap_or("?");
            ("SaveMemory".into(), mt.to_string(), Color::Yellow)
        }
        "hive_review_agent" => {
            let agent = extract_arg(args, "agent_id").unwrap_or("?");
            ("ReviewAgent".into(), agent.to_string(), Color::Yellow)
        }
        "hive_retry_agent" => {
            let agent = extract_arg(args, "agent_id").unwrap_or("?");
            ("RetryAgent".into(), agent.to_string(), Color::Yellow)
        }
        // Catch-all for other hive_ tools — convert snake_case to PascalCase
        name if name.starts_with("hive_") => {
            let short = name.strip_prefix("hive_").unwrap_or(name);
            let pascal: String = short
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().to_string() + c.as_str(),
                    }
                })
                .collect();
            let detail = if args.is_empty() {
                String::new()
            } else {
                let truncated = if args.len() > 40 { &args[..40] } else { args };
                truncated.to_string()
            };
            (pascal, detail, Color::Yellow)
        }

        // --- Common Claude tools ---
        "Read" => {
            let path = extract_arg(args, "file_path").unwrap_or("?");
            ("Read".into(), path.to_string(), Color::Gray)
        }
        "Write" => {
            let path = extract_arg(args, "file_path").unwrap_or("?");
            ("Write".into(), path.to_string(), Color::Gray)
        }
        "Edit" => {
            let path = extract_arg(args, "file_path").unwrap_or("?");
            ("Edit".into(), path.to_string(), Color::Gray)
        }
        "Bash" => {
            let cmd = extract_arg(args, "command").unwrap_or("?");
            let preview = if cmd.len() > 50 { &cmd[..50] } else { cmd };
            ("$".into(), preview.to_string(), Color::Blue)
        }
        "Grep" => {
            let pattern = extract_arg(args, "pattern").unwrap_or("?");
            ("Grep".into(), pattern.to_string(), Color::Gray)
        }
        "Glob" => {
            let pattern = extract_arg(args, "pattern").unwrap_or("?");
            ("Glob".into(), pattern.to_string(), Color::Gray)
        }

        // --- Fallback ---
        _ => {
            let detail = if args.is_empty() {
                String::new()
            } else {
                let truncated = if args.len() > 40 { &args[..40] } else { args };
                truncated.to_string()
            };
            (tool_name.to_string(), detail, Color::Gray)
        }
    }
}

// ---------------------------------------------------------------------------
// Activity collapsing: group consecutive same-tool calls
// ---------------------------------------------------------------------------

pub(super) struct CollapsedEntry<'a> {
    pub entry: &'a ActivityEntry,
    pub collapsed_count: usize, // 0 means no collapse, N means "+N more"
}

/// Collapse consecutive ToolCall entries with the same (agent_id, tool_name)
/// into a single entry (the most recent / last in the run) with a count.
/// Messages and different tool calls break the sequence.
pub(super) fn collapse_activity(activity: &[ActivityEntry]) -> Vec<CollapsedEntry<'_>> {
    let mut result: Vec<CollapsedEntry<'_>> = Vec::with_capacity(activity.len());
    let mut i = 0;
    while i < activity.len() {
        if let ActivityEntry::ToolCall {
            agent_id,
            tool_name,
            ..
        } = &activity[i]
        {
            // Count consecutive entries with same agent_id + tool_name
            let mut j = i + 1;
            while j < activity.len() {
                if let ActivityEntry::ToolCall {
                    agent_id: aid,
                    tool_name: tn,
                    ..
                } = &activity[j]
                    && aid == agent_id
                    && tn == tool_name
                {
                    j += 1;
                    continue;
                }
                break;
            }
            let count = j - i;
            // Show the last entry in the group (most recent)
            result.push(CollapsedEntry {
                entry: &activity[j - 1],
                collapsed_count: count - 1,
            });
            i = j;
        } else {
            result.push(CollapsedEntry {
                entry: &activity[i],
                collapsed_count: 0,
            });
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Render: Activity stream
// ---------------------------------------------------------------------------

pub(super) fn render_activity_stream(
    frame: &mut Frame,
    area: Rect,
    activity: &[ActivityEntry],
    ui: &TuiState,
) {
    let collapsed = collapse_activity(activity);
    let items: Vec<ListItem> = collapsed
        .iter()
        .map(|collapsed_entry| {
            let entry = collapsed_entry.entry;
            let extra_count = collapsed_entry.collapsed_count;
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
                    args_summary,
                    status,
                    duration_ms,
                } => {
                    let ts = timestamp.format("%H:%M:%S");
                    let (tool_display, args_display, tool_color) =
                        format_tool_display(tool_name, args_summary.as_deref());
                    let (icon, icon_color) = if status == "success" {
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
                    let mut spans = vec![
                        Span::styled(format!("{ts} {icon}  "), Style::default().fg(icon_color)),
                        Span::styled(
                            format!("{agent_id}  "),
                            Style::default().fg(if is_dimmed {
                                Color::Rgb(110, 110, 120)
                            } else {
                                Color::DarkGray
                            }),
                        ),
                        Span::styled(
                            tool_display.to_string(),
                            Style::default().fg(if is_dimmed {
                                Color::Rgb(110, 110, 120)
                            } else {
                                tool_color
                            }),
                        ),
                    ];
                    if !args_display.is_empty() {
                        spans.push(Span::styled(
                            format!(" {args_display}"),
                            Style::default().fg(if is_dimmed {
                                Color::Rgb(110, 110, 120)
                            } else {
                                Color::Gray
                            }),
                        ));
                    }
                    if extra_count > 0 {
                        spans.push(Span::styled(
                            format!(" +{extra_count}"),
                            Style::default().fg(if is_dimmed {
                                Color::Rgb(110, 110, 120)
                            } else {
                                Color::DarkGray
                            }),
                        ));
                    }
                    if !dur.is_empty() {
                        spans.push(Span::styled(dur, Style::default().fg(Color::DarkGray)));
                    }
                    ListItem::new(Line::from(spans))
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
    let num_items = collapsed.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let mut list_state = ListState::default();
    if ui.activity_auto_scroll {
        // Select last visible item
        if num_items > visible_height {
            list_state.select(Some(num_items.saturating_sub(1)));
        }
    } else {
        let sel = ui.activity_scroll.min(num_items.saturating_sub(1));
        list_state.select(Some(sel));
    }

    let list = List::new(items).block(block);

    frame.render_stateful_widget(list, area, &mut list_state);

    // Scrollbar
    let scroll_position = if ui.activity_auto_scroll {
        num_items.saturating_sub(1)
    } else {
        ui.activity_scroll
    };
    let mut scrollbar_state = ScrollbarState::new(num_items).position(scroll_position);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .track_style(Style::default().fg(Color::DarkGray))
        .thumb_style(Style::default().fg(Color::Gray));
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

// ---------------------------------------------------------------------------
// Render: Planning view
// ---------------------------------------------------------------------------

pub(super) fn render_planning_view(frame: &mut Frame, area: Rect, planner: &Agent) {
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

pub(super) fn render_spec_viewer(frame: &mut Frame, area: Rect, spec: &str, scroll: usize) {
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
