use crate::types::*;
use chrono::{DateTime, Utc};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashMap;

use super::AgentToolCall;

// ---------------------------------------------------------------------------
// Tool call phase classification
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Explore,   // Read, Grep, Glob, ToolSearch
    Implement, // Write, Edit
    Test,      // Bash (with test-like commands)
    Hive,      // hive_* MCP tools
    Other,
}

fn classify_tool(tool_name: &str) -> Phase {
    // Strip MCP prefix
    let name = if let Some(rest) = tool_name.strip_prefix("mcp__") {
        rest.find("__").map_or(tool_name, |i| &rest[i + 2..])
    } else {
        tool_name
    };

    if name.starts_with("hive_") {
        Phase::Hive
    } else {
        match name {
            "Read" | "Grep" | "Glob" | "ToolSearch" | "LSP" | "Agent" => Phase::Explore,
            "Write" | "Edit" | "NotebookEdit" => Phase::Implement,
            "Bash" => Phase::Test,
            _ => Phase::Other,
        }
    }
}

fn phase_char(phase: Phase) -> char {
    match phase {
        Phase::Explore => '\u{2591}',   // light shade
        Phase::Implement => '\u{2593}', // dark shade
        Phase::Test => '\u{2592}',      // medium shade
        Phase::Hive => '\u{2588}',      // full block
        Phase::Other => '\u{2588}',     // full block
    }
}

fn phase_color(phase: Phase, base_color: Color) -> Color {
    match phase {
        Phase::Explore => Color::Cyan,
        Phase::Implement => Color::Green,
        Phase::Test => Color::Magenta,
        Phase::Hive => Color::Yellow,
        Phase::Other => base_color,
    }
}

// ---------------------------------------------------------------------------
// Role colors and symbols
// ---------------------------------------------------------------------------

fn role_color(role: AgentRole) -> Color {
    match role {
        AgentRole::Coordinator => Color::Cyan,
        AgentRole::Lead => Color::Green,
        AgentRole::Worker => Color::White,
        AgentRole::Reviewer => Color::Yellow,
        AgentRole::Planner => Color::Magenta,
        AgentRole::Postmortem => Color::DarkGray,
        AgentRole::Explorer => Color::LightMagenta,
        AgentRole::Evaluator => Color::LightYellow,
    }
}

fn role_symbol(role: AgentRole) -> char {
    match role {
        AgentRole::Coordinator => '\u{2B21}',
        AgentRole::Lead => '\u{25B8}',
        AgentRole::Worker => '\u{25CB}',
        AgentRole::Reviewer => '\u{25C8}',
        AgentRole::Planner => '\u{25C6}',
        AgentRole::Postmortem => '\u{25C7}',
        AgentRole::Explorer => '\u{00BB}',
        AgentRole::Evaluator => '\u{2261}',
    }
}

// ---------------------------------------------------------------------------
// Time axis header
// ---------------------------------------------------------------------------

fn build_time_axis(total_secs: i64, bar_w: usize, label_w: usize) -> Line<'static> {
    let mut axis = vec![' '; bar_w];

    let num_ticks = 4usize.min(bar_w / 8);
    for i in 0..=num_ticks {
        let pos = if num_ticks == 0 {
            0
        } else {
            i * (bar_w.saturating_sub(1)) / num_ticks
        };
        let secs = (pos as i64 * total_secs) / bar_w.max(1) as i64;
        let label = if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m", secs / 60)
        };
        let start = pos.min(bar_w.saturating_sub(label.len()));
        for (j, ch) in label.chars().enumerate() {
            if start + j < bar_w {
                axis[start + j] = ch;
            }
        }
    }

    let label_part = " ".repeat(label_w + 1);
    let axis_str: String = axis.into_iter().collect();
    Line::from(vec![
        Span::styled(label_part, Style::default()),
        Span::styled(axis_str, Style::default().fg(Color::DarkGray)),
    ])
}

// ---------------------------------------------------------------------------
// Individual agent row — now with phase coloring and idle gaps
// ---------------------------------------------------------------------------

/// Threshold in seconds: gaps longer than this are shown as idle.
const IDLE_GAP_SECS: i64 = 30;

#[allow(clippy::too_many_arguments)]
fn render_agent_row(
    agent: &Agent,
    t_start: DateTime<Utc>,
    t_now: DateTime<Utc>,
    total_secs: i64,
    label_w: usize,
    bar_w: usize,
    spawn_times: &HashMap<String, DateTime<Utc>>,
    agent_tool_calls: &HashMap<String, Vec<AgentToolCall>>,
    queue_timestamps: &[DateTime<Utc>],
) -> Line<'static> {
    let color = role_color(agent.role);
    let symbol = role_symbol(agent.role);

    // Build label
    let max_id = label_w.saturating_sub(2);
    let id_truncated = if agent.id.len() > max_id {
        format!("{}…", &agent.id[..max_id.saturating_sub(1)])
    } else {
        agent.id.clone()
    };
    let label = format!("{symbol} {id_truncated:<width$}", width = max_id);

    // Agent time range
    let agent_start = spawn_times.get(&agent.id).copied().unwrap_or(t_start);
    let agent_end = match agent.status {
        AgentStatus::Done | AgentStatus::Failed => agent.last_completed_at.unwrap_or(t_now),
        _ => t_now,
    };

    let start_secs = (agent_start - t_start).num_seconds().max(0);
    let end_secs = (agent_end - t_start)
        .num_seconds()
        .max(start_secs + 1)
        .min(total_secs);

    let ts = total_secs.max(1) as usize;
    let bar_start = (start_secs as usize * bar_w) / ts;
    let bar_end = ((end_secs as usize * bar_w) / ts).clamp(bar_start + 1, bar_w);

    // Build per-column phase + idle data from tool calls
    let mut bar_chars: Vec<char> = vec![' '; bar_w];
    let mut bar_colors: Vec<Color> = vec![Color::Reset; bar_w];

    let is_done = matches!(agent.status, AgentStatus::Done | AgentStatus::Failed);

    if let Some(calls) = agent_tool_calls.get(&agent.id) {
        if !calls.is_empty() {
            // Map each tool call to a bar column and assign phase
            let mut column_phases: Vec<Option<Phase>> = vec![None; bar_w];

            for call in calls {
                let call_secs = (call.timestamp - t_start).num_seconds().max(0);
                let col = ((call_secs as usize) * bar_w) / ts;
                if col >= bar_start && col < bar_end {
                    let phase = classify_tool(&call.tool_name);
                    // Later phases overwrite earlier ones in same column
                    column_phases[col] = Some(phase);
                }
            }

            // Detect idle gaps: columns within agent lifetime that have no tool calls
            // and are far from the nearest call
            let mut last_call_col: Option<usize> = None;
            for (col, phase) in column_phases
                .iter()
                .enumerate()
                .take(bar_end)
                .skip(bar_start)
            {
                if phase.is_some() {
                    last_call_col = Some(col);
                }
            }

            // Build a "nearest call distance" map for idle detection
            let mut nearest_dist: Vec<usize> = vec![usize::MAX; bar_w];
            // Forward pass
            let mut last_seen = None;
            for col in bar_start..bar_end {
                if column_phases[col].is_some() {
                    last_seen = Some(col);
                }
                if let Some(lc) = last_seen {
                    nearest_dist[col] = col - lc;
                }
            }
            // Backward pass
            last_seen = None;
            for col in (bar_start..bar_end).rev() {
                if column_phases[col].is_some() {
                    last_seen = Some(col);
                }
                if let Some(lc) = last_seen {
                    nearest_dist[col] = nearest_dist[col].min(lc - col);
                }
            }

            // How many columns correspond to IDLE_GAP_SECS?
            let idle_cols = ((IDLE_GAP_SECS as usize) * bar_w) / ts;

            // Fill the bar
            for col in bar_start..bar_end {
                if let Some(phase) = column_phases[col] {
                    bar_chars[col] = phase_char(phase);
                    bar_colors[col] = if is_done {
                        Color::DarkGray
                    } else {
                        phase_color(phase, color)
                    };
                } else if nearest_dist[col] > idle_cols && idle_cols > 0 {
                    // Idle gap — use dots
                    bar_chars[col] = '\u{2504}'; // box drawings light triple dash horizontal
                    bar_colors[col] = Color::Rgb(80, 80, 80);
                } else {
                    // Active region between calls — fill with base color
                    bar_chars[col] = '\u{2588}'; // full block
                    bar_colors[col] = if is_done { Color::DarkGray } else { color };
                }
            }

            // Override: if no calls mapped to columns (very sparse), just show solid bar
            if last_call_col.is_none() {
                for col in bar_start..bar_end {
                    bar_chars[col] = '\u{2588}';
                    bar_colors[col] = if is_done { Color::DarkGray } else { color };
                }
            }
        } else {
            // No tool calls — solid bar
            for col in bar_start..bar_end {
                bar_chars[col] = '\u{2588}';
                bar_colors[col] = if is_done { Color::DarkGray } else { color };
            }
        }
    } else {
        // No tool call data — solid bar
        for col in bar_start..bar_end {
            bar_chars[col] = '\u{2588}';
            bar_colors[col] = if is_done { Color::DarkGray } else { color };
        }
    }

    // Mark merge queue submission times with '│' (only in empty space)
    for &qt in queue_timestamps {
        let q_secs = (qt - t_start).num_seconds();
        if q_secs >= 0 && q_secs < total_secs {
            let q_pos = (q_secs as usize * bar_w) / ts;
            if q_pos < bar_w && bar_chars[q_pos] == ' ' {
                bar_chars[q_pos] = '\u{2502}';
                bar_colors[q_pos] = Color::White;
            }
        }
    }

    // Status indicator suffix
    let status_char = match agent.status {
        AgentStatus::Running => '\u{25B6}',
        AgentStatus::Idle => '\u{25E6}',
        AgentStatus::Done => '\u{2713}',
        AgentStatus::Failed => '\u{2717}',
        AgentStatus::Stalled => '!',
    };

    // Build spans — group consecutive columns with same color to reduce span count
    let mut bar_spans: Vec<Span> = Vec::new();
    let mut run_start = 0;
    while run_start < bar_w {
        let run_color = bar_colors[run_start];
        let run_char = bar_chars[run_start];
        let mut run_end = run_start + 1;
        while run_end < bar_w && bar_colors[run_end] == run_color && bar_chars[run_end] == run_char
        {
            run_end += 1;
        }
        let s: String = bar_chars[run_start..run_end].iter().collect();
        bar_spans.push(Span::styled(s, Style::default().fg(run_color)));
        run_start = run_end;
    }

    let mut spans = vec![
        Span::styled(label, Style::default().fg(color)),
        Span::raw(" "),
    ];
    spans.extend(bar_spans);
    spans.push(Span::styled(
        format!(" {status_char}"),
        Style::default().fg(color),
    ));

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Dependency arrows (parent → child spawn connections)
// ---------------------------------------------------------------------------

/// Render dependency lines between parent and child agents.
/// Returns lines to insert between the time axis and agent rows.
fn build_dependency_indicators(sorted_agents: &[&Agent]) -> HashMap<usize, Vec<usize>> {
    // Map agent_id → row index
    let id_to_row: HashMap<&str, usize> = sorted_agents
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id.as_str(), i))
        .collect();

    // parent_row → [child_rows]
    let mut deps: HashMap<usize, Vec<usize>> = HashMap::new();
    for agent in sorted_agents {
        if let Some(ref parent_id) = agent.parent
            && let (Some(&parent_row), Some(&child_row)) = (
                id_to_row.get(parent_id.as_str()),
                id_to_row.get(agent.id.as_str()),
            )
        {
            deps.entry(parent_row).or_default().push(child_row);
        }
    }
    deps
}

/// Build the dependency gutter (left margin showing parent→child connections).
/// Returns a vec of strings, one per agent row, to prepend to labels.
fn build_dep_gutter(sorted_agents: &[&Agent], gutter_w: usize) -> Vec<(String, Color)> {
    let deps = build_dependency_indicators(sorted_agents);
    let num_rows = sorted_agents.len();
    let mut gutter: Vec<(String, Color)> = vec![(" ".repeat(gutter_w), Color::DarkGray); num_rows];

    if deps.is_empty() || gutter_w < 2 {
        return gutter;
    }

    // For each parent, draw vertical lines from parent to last child
    // Use columns 0..gutter_w for different nesting levels
    let mut grid = vec![vec![' '; gutter_w]; num_rows];
    let mut col_colors = vec![vec![Color::DarkGray; gutter_w]; num_rows];

    let mut next_col = 0usize;
    // Process parents in row order
    let mut parent_rows: Vec<usize> = deps.keys().copied().collect();
    parent_rows.sort();

    for &parent_row in &parent_rows {
        if next_col >= gutter_w {
            break;
        }
        let col = next_col;
        let children = &deps[&parent_row];
        if children.is_empty() {
            continue;
        }
        let max_child = *children.iter().max().unwrap();
        let parent_color = role_color(sorted_agents[parent_row].role);

        // Draw vertical line from parent to last child
        for row in (parent_row + 1)..=max_child {
            if col < gutter_w {
                grid[row][col] = '\u{2502}'; // │
                col_colors[row][col] = parent_color;
            }
        }
        // Draw branch points for each child
        for &child_row in children {
            if col < gutter_w {
                if child_row == max_child {
                    grid[child_row][col] = '\u{2514}'; // └
                } else {
                    grid[child_row][col] = '\u{251C}'; // ├
                }
                col_colors[child_row][col] = parent_color;
                // Fill rightward to connect to label
                for c in (col + 1)..gutter_w {
                    grid[child_row][c] = '\u{2500}'; // ─
                    col_colors[child_row][c] = parent_color;
                }
            }
        }

        // Mark the parent origin
        if col < gutter_w {
            grid[parent_row][col] = '\u{252C}'; // ┬
            col_colors[parent_row][col] = parent_color;
        }

        next_col += 1;
    }

    // Convert grid to strings
    for row in 0..num_rows {
        let s: String = grid[row].iter().collect();
        // Use the first non-DarkGray color found in the row
        let c = col_colors[row]
            .iter()
            .find(|&&c| c != Color::DarkGray)
            .copied()
            .unwrap_or(Color::DarkGray);
        gutter[row] = (s, c);
    }

    gutter
}

// ---------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------

fn build_legend() -> Line<'static> {
    Line::from(vec![
        Span::styled(" Phases: ", Style::default().fg(Color::DarkGray)),
        Span::styled("\u{2591}explore ", Style::default().fg(Color::Cyan)),
        Span::styled("\u{2593}implement ", Style::default().fg(Color::Green)),
        Span::styled("\u{2592}test ", Style::default().fg(Color::Magenta)),
        Span::styled("\u{2588}hive ", Style::default().fg(Color::Yellow)),
        Span::styled("\u{2504}idle ", Style::default().fg(Color::Rgb(80, 80, 80))),
        Span::styled(
            "  \u{2502}\u{2514}\u{2500}deps",
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Main render entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn render_gantt_view(
    frame: &mut Frame,
    area: Rect,
    agents: &[Agent],
    queue: &MergeQueue,
    run_meta: &Option<RunMetadata>,
    spawn_times: &HashMap<String, DateTime<Utc>>,
    agent_tool_calls: &HashMap<String, Vec<AgentToolCall>>,
    scroll: usize,
) {
    let now = Utc::now();
    let t_start = run_meta
        .as_ref()
        .map(|m| m.created_at)
        .unwrap_or_else(|| now - chrono::Duration::minutes(30));

    let total_secs = (now - t_start).num_seconds().max(60);

    let block = Block::default()
        .title(" Timeline ")
        .title_bottom(Line::from(" [t] normal view  [j/k] scroll ").right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 30 || inner.height < 3 {
        return;
    }

    // Sort agents: coordinator first, then by spawn time
    let mut sorted: Vec<&Agent> = agents.iter().collect();
    sorted.sort_by(|a, b| {
        let a_coord = a.role == AgentRole::Coordinator;
        let b_coord = b.role == AgentRole::Coordinator;
        if a_coord != b_coord {
            return b_coord.cmp(&a_coord);
        }
        let at = spawn_times.get(&a.id).copied().unwrap_or(t_start);
        let bt = spawn_times.get(&b.id).copied().unwrap_or(t_start);
        at.cmp(&bt)
    });

    // Compute dependency gutter width
    let has_deps = sorted.iter().any(|a| a.parent.is_some());
    let gutter_w: usize = if has_deps { 3 } else { 0 };

    let label_w: usize = 22;
    // +2: separator space + status char suffix, +gutter_w for dep lines
    let bar_w = (inner.width as usize).saturating_sub(label_w + gutter_w + 2);

    if bar_w < 4 {
        return;
    }

    let queue_timestamps: Vec<DateTime<Utc>> =
        queue.entries.iter().map(|e| e.submitted_at).collect();

    let dep_gutter = build_dep_gutter(&sorted, gutter_w);

    let total_agents = sorted.len();
    // -2: time axis header + legend row
    let visible_height = inner.height.saturating_sub(2) as usize;
    let scroll_clamped = scroll.min(total_agents.saturating_sub(1));

    let mut lines: Vec<Line> = Vec::new();

    // Time axis header
    let axis_prefix = " ".repeat(gutter_w);
    let axis = build_time_axis(total_secs, bar_w, label_w);
    let mut axis_spans = vec![Span::raw(axis_prefix)];
    axis_spans.extend(axis.spans);
    lines.push(Line::from(axis_spans));

    // Agent rows
    for (i, agent) in sorted
        .iter()
        .skip(scroll_clamped)
        .take(visible_height)
        .enumerate()
    {
        let row_idx = scroll_clamped + i;
        let mut row = render_agent_row(
            agent,
            t_start,
            now,
            total_secs,
            label_w,
            bar_w,
            spawn_times,
            agent_tool_calls,
            &queue_timestamps,
        );

        // Prepend dependency gutter
        if gutter_w > 0
            && let Some((gutter_str, gutter_color)) = dep_gutter.get(row_idx)
        {
            let mut new_spans = vec![Span::styled(
                gutter_str.clone(),
                Style::default().fg(*gutter_color),
            )];
            new_spans.extend(row.spans);
            row = Line::from(new_spans);
        }

        lines.push(row);
    }

    // Scroll hint
    let shown = lines.len().saturating_sub(1);
    if scroll_clamped + shown < total_agents {
        let remaining = total_agents - scroll_clamped - shown;
        lines.push(Line::from(Span::styled(
            format!("  \u{2193} {remaining} more"),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // Legend at bottom
        lines.push(build_legend());
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
