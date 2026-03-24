use crate::types::*;
use chrono::{DateTime, Utc};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashMap;

use super::AgentToolCall;

// ---------------------------------------------------------------------------
// Time compression — collapse idle gaps > 5 minutes
// ---------------------------------------------------------------------------

/// Minimum gap duration (in seconds) before we collapse it.
const COLLAPSE_THRESHOLD_SECS: i64 = 300; // 5 minutes

/// How many display-seconds a collapsed gap occupies (visual width of the marker).
const COLLAPSED_DISPLAY_SECS: i64 = 15;

/// A gap in the timeline that will be collapsed.
#[derive(Clone, Debug)]
struct CollapsedGap {
    /// Start of the gap in real seconds from t_start
    real_start: i64,
    /// End of the gap in real seconds from t_start
    real_end: i64,
}

/// Maps real elapsed seconds to display seconds, compressing idle gaps.
struct TimeMapper {
    gaps: Vec<CollapsedGap>,
    /// Total display seconds after compression
    pub display_total: i64,
}

impl TimeMapper {
    /// Build a TimeMapper by finding global idle gaps in tool call activity.
    fn new(
        total_secs: i64,
        agent_tool_calls: &HashMap<String, Vec<AgentToolCall>>,
        t_start: DateTime<Utc>,
    ) -> Self {
        // Collect all tool call timestamps as seconds from t_start
        let mut all_secs: Vec<i64> = agent_tool_calls
            .values()
            .flat_map(|calls| {
                calls
                    .iter()
                    .map(|c| (c.timestamp - t_start).num_seconds().max(0))
            })
            .collect();
        all_secs.sort_unstable();
        all_secs.dedup();

        let mut gaps = Vec::new();

        if !all_secs.is_empty() {
            // Check gap from start to first call
            if all_secs[0] > COLLAPSE_THRESHOLD_SECS {
                gaps.push(CollapsedGap {
                    real_start: 0,
                    real_end: all_secs[0],
                });
            }
            // Check gaps between consecutive calls
            for window in all_secs.windows(2) {
                let gap = window[1] - window[0];
                if gap > COLLAPSE_THRESHOLD_SECS {
                    gaps.push(CollapsedGap {
                        real_start: window[0],
                        real_end: window[1],
                    });
                }
            }
            // Check gap from last call to now
            if let Some(&last) = all_secs.last()
                && total_secs - last > COLLAPSE_THRESHOLD_SECS
            {
                gaps.push(CollapsedGap {
                    real_start: last,
                    real_end: total_secs,
                });
            }
        }

        // Compute display_total: real_total minus removed time plus collapsed markers
        let removed: i64 = gaps
            .iter()
            .map(|g| (g.real_end - g.real_start) - COLLAPSED_DISPLAY_SECS)
            .sum();
        let display_total = (total_secs - removed).max(60);

        Self {
            gaps,
            display_total,
        }
    }

    /// Map a real-seconds offset to a display-seconds offset.
    fn map(&self, real_secs: i64) -> i64 {
        let mut removed = 0i64;
        for gap in &self.gaps {
            if real_secs <= gap.real_start {
                break;
            }
            let gap_duration = gap.real_end - gap.real_start;
            if real_secs >= gap.real_end {
                // Past this gap entirely — account for the collapsed portion
                removed += gap_duration - COLLAPSED_DISPLAY_SECS;
            } else {
                // Inside a gap — map linearly within the collapsed marker
                let frac = (real_secs - gap.real_start) as f64 / gap_duration.max(1) as f64;
                let marker_pos = (frac * COLLAPSED_DISPLAY_SECS as f64) as i64;
                return (gap.real_start - removed) + marker_pos;
            }
        }
        real_secs - removed
    }

    /// Map a real-seconds offset to a virtual sub-column index.
    fn to_vcol(&self, real_secs: i64, virt_w: usize) -> usize {
        let display_secs = self.map(real_secs);
        let dt = self.display_total.max(1) as usize;
        (display_secs.max(0) as usize * virt_w) / dt
    }

    /// Return the virtual sub-column ranges occupied by collapse markers.
    fn collapse_marker_ranges(&self, virt_w: usize) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        for gap in &self.gaps {
            let start = self.to_vcol(gap.real_start, virt_w);
            let end = self.to_vcol(gap.real_end, virt_w);
            if end > start {
                ranges.push((start, end));
            }
        }
        ranges
    }
}

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

/// Convert a pair of sub-column colors into a half-block terminal cell.
/// Each terminal cell encodes two virtual columns via fg (left) and bg (right).
fn half_block_cell(left: Color, right: Color) -> (char, Style) {
    match (left, right) {
        (Color::Reset, Color::Reset) => (' ', Style::default().bg(Color::Reset)),
        (l, r) if l == r => ('\u{2588}', Style::default().fg(l).bg(Color::Reset)),
        (Color::Reset, r) => ('\u{2590}', Style::default().fg(r).bg(Color::Reset)),
        (l, Color::Reset) => ('\u{258C}', Style::default().fg(l).bg(Color::Reset)),
        (l, r) => ('\u{258C}', Style::default().fg(l).bg(r)),
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
        AgentRole::Postmortem => '\u{25C7}',
        AgentRole::Explorer => '\u{00BB}',
        AgentRole::Evaluator => '\u{2261}',
    }
}

// ---------------------------------------------------------------------------
// Time axis header
// ---------------------------------------------------------------------------

fn build_time_axis(
    total_secs: i64,
    bar_w: usize,
    label_w: usize,
    mapper: &TimeMapper,
) -> Line<'static> {
    // Per-cell character and highlight flag (true = tick label, false = collapse/empty)
    let mut axis = vec![(' ', false); bar_w];

    // Helper: format a real-seconds value as a tick label
    let fmt_tick = |secs: i64| -> String {
        if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m", secs / 60)
        }
    };

    // Helper: place a tick label at a cell position, returns true if placed
    let place_tick = |axis: &mut Vec<(char, bool)>, pos: usize, label: &str| {
        let start = pos.min(bar_w.saturating_sub(label.len()));
        // Check for overlap with existing tick labels
        let can_place = (start..start + label.len())
            .all(|j| j < bar_w && !axis[j].1);
        if !can_place {
            return;
        }
        for (j, ch) in label.chars().enumerate() {
            if start + j < bar_w {
                axis[start + j] = (ch, true);
            }
        }
    };

    // 1) Priority ticks: at the end of each collapsed section (where activity resumes)
    for gap in &mapper.gaps {
        let pos = mapper.to_vcol(gap.real_end, bar_w);
        let label = fmt_tick(gap.real_end);
        place_tick(&mut axis, pos, &label);
    }

    // 2) Fill remaining space with evenly-spaced ticks
    let num_ticks = 4usize.min(bar_w / 8);
    for i in 0..=num_ticks {
        let pos = if num_ticks == 0 {
            0
        } else {
            i * (bar_w.saturating_sub(1)) / num_ticks
        };
        let display_secs = (pos as i64 * mapper.display_total) / bar_w.max(1) as i64;
        let secs = reverse_map_approx(mapper, display_secs, total_secs);
        let label = fmt_tick(secs);
        place_tick(&mut axis, pos, &label);
    }

    // 3) Mark collapse regions on the axis (only cells not occupied by ticks)
    let collapse_ranges = mapper.collapse_marker_ranges(bar_w * 2);
    for (vs, ve) in &collapse_ranges {
        let cell_start = vs / 2;
        let cell_end = ve.div_ceil(2);
        for item in axis.iter_mut().take(cell_end.min(bar_w)).skip(cell_start) {
            if !item.1 {
                *item = ('\u{2508}', false); // ┈ dashed line
            }
        }
    }

    // Build spans with tick labels brighter than background
    let label_part = " ".repeat(label_w + 1);
    let mut spans = vec![Span::styled(label_part, Style::default())];

    let mut run_start = 0;
    while run_start < bar_w {
        let is_tick = axis[run_start].1;
        let mut run_end = run_start + 1;
        while run_end < bar_w && axis[run_end].1 == is_tick {
            run_end += 1;
        }
        let s: String = axis[run_start..run_end].iter().map(|(c, _)| c).collect();
        let style = if is_tick {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(s, style));
        run_start = run_end;
    }

    Line::from(spans)
}

/// Approximate reverse mapping: given display_secs, find real_secs.
fn reverse_map_approx(mapper: &TimeMapper, display_secs: i64, total_secs: i64) -> i64 {
    // Binary search for the real_secs that maps closest to display_secs
    let mut lo = 0i64;
    let mut hi = total_secs;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if mapper.map(mid) < display_secs {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
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
    mapper: &TimeMapper,
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

    // Virtual sub-columns: 2 per terminal cell for half-block rendering (2x resolution)
    let virt_w = bar_w * 2;

    let start_secs = (agent_start - t_start).num_seconds().max(0);
    let end_secs = (agent_end - t_start)
        .num_seconds()
        .max(start_secs + 1)
        .min(total_secs);

    let vstart = mapper.to_vcol(start_secs, virt_w);
    let vend = mapper.to_vcol(end_secs, virt_w).clamp(vstart + 1, virt_w);

    let is_done = matches!(agent.status, AgentStatus::Done | AgentStatus::Failed);
    let base = if is_done { Color::DarkGray } else { color };

    // Build per-virtual-sub-column colors (Color::Reset = empty/outside lifetime)
    let mut sub_colors: Vec<Color> = vec![Color::Reset; virt_w];

    let has_mapped_calls = if let Some(calls) = agent_tool_calls.get(&agent.id) {
        if !calls.is_empty() {
            let mut column_phases: Vec<Option<Phase>> = vec![None; virt_w];

            for call in calls {
                let call_secs = (call.timestamp - t_start).num_seconds().max(0);
                let col = mapper.to_vcol(call_secs, virt_w);
                if col >= vstart && col < vend {
                    column_phases[col] = Some(classify_tool(&call.tool_name));
                }
            }

            let any_mapped = column_phases[vstart..vend].iter().any(|p| p.is_some());

            if any_mapped {
                // Nearest call distance for idle detection
                let mut nearest_dist: Vec<usize> = vec![usize::MAX; virt_w];
                let mut last_seen = None;
                let mut last_call_col = vstart;
                for col in vstart..vend {
                    if column_phases[col].is_some() {
                        last_seen = Some(col);
                        last_call_col = col;
                    }
                    if let Some(lc) = last_seen {
                        nearest_dist[col] = col - lc;
                    }
                }
                last_seen = None;
                for col in (vstart..vend).rev() {
                    if column_phases[col].is_some() {
                        last_seen = Some(col);
                    }
                    if let Some(lc) = last_seen {
                        nearest_dist[col] = nearest_dist[col].min(lc - col);
                    }
                }

                // Use display_total for idle threshold calc (since columns are now compressed)
                let idle_cols =
                    ((IDLE_GAP_SECS as usize) * virt_w / mapper.display_total.max(1) as usize)
                        .max(1);

                for col in vstart..vend {
                    if let Some(phase) = column_phases[col] {
                        sub_colors[col] =
                            if is_done { Color::DarkGray } else { phase_color(phase, color) };
                    } else if col > last_call_col {
                        // After last tool call — leave empty (Color::Reset)
                    } else if nearest_dist[col] > idle_cols {
                        // Idle gap between tool calls
                        sub_colors[col] = Color::Rgb(80, 80, 80);
                    } else {
                        // Active region between nearby calls
                        sub_colors[col] = base;
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !has_mapped_calls {
        sub_colors[vstart..vend].fill(base);
    }

    // Mark collapsed regions — override with a distinct pattern
    let collapse_ranges = mapper.collapse_marker_ranges(virt_w);
    for (cs, ce) in &collapse_ranges {
        for (col, color) in sub_colors.iter_mut().enumerate().take(*ce.min(&virt_w)).skip(*cs) {
            // Only mark within the agent's active range for visual clarity
            if col >= vstart && col < vend && *color != Color::Reset {
                *color = Color::Rgb(60, 60, 90); // distinct dim blue-gray
            }
        }
    }

    // Merge queue markers — track which terminal cells are queue markers
    let mut queue_cells: Vec<bool> = vec![false; bar_w];
    for &qt in queue_timestamps {
        let q_secs = (qt - t_start).num_seconds();
        if q_secs >= 0 && q_secs < total_secs {
            let q_vcol = mapper.to_vcol(q_secs, virt_w);
            let cell = q_vcol / 2;
            if cell < bar_w
                && sub_colors[cell * 2] == Color::Reset
                && sub_colors[cell * 2 + 1] == Color::Reset
            {
                queue_cells[cell] = true;
            }
        }
    }

    // Collapse marker cells — show ┊ pattern in collapsed regions
    let mut collapse_cells: Vec<bool> = vec![false; bar_w];
    for (cs, ce) in &collapse_ranges {
        let cell_start = cs / 2;
        let cell_end = ce.div_ceil(2);
        for item in collapse_cells.iter_mut().take(cell_end.min(bar_w)).skip(cell_start) {
            *item = true;
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

    // Convert virtual sub-columns to half-block terminal cells, grouping runs
    let mut bar_spans: Vec<Span> = Vec::new();
    let mut run_start = 0;
    while run_start < bar_w {
        let (ch, style) = if queue_cells[run_start] {
            ('\u{2502}', Style::default().fg(Color::White))
        } else if collapse_cells[run_start]
            && sub_colors[run_start * 2] == Color::Reset
            && sub_colors[run_start * 2 + 1] == Color::Reset
        {
            ('\u{250A}', Style::default().fg(Color::Rgb(80, 80, 120))) // ┊ for empty collapse
        } else {
            half_block_cell(sub_colors[run_start * 2], sub_colors[run_start * 2 + 1])
        };

        let mut run_end = run_start + 1;
        while run_end < bar_w {
            let (ch2, style2) = if queue_cells[run_end] {
                ('\u{2502}', Style::default().fg(Color::White))
            } else if collapse_cells[run_end]
                && sub_colors[run_end * 2] == Color::Reset
                && sub_colors[run_end * 2 + 1] == Color::Reset
            {
                ('\u{250A}', Style::default().fg(Color::Rgb(80, 80, 120)))
            } else {
                half_block_cell(sub_colors[run_end * 2], sub_colors[run_end * 2 + 1])
            };
            if ch2 != ch || style2 != style {
                break;
            }
            run_end += 1;
        }

        let s: String = std::iter::repeat_n(ch, run_end - run_start).collect();
        bar_spans.push(Span::styled(s, style));
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
        Span::styled("\u{2588}explore ", Style::default().fg(Color::Cyan)),
        Span::styled("\u{2588}implement ", Style::default().fg(Color::Green)),
        Span::styled("\u{2588}test ", Style::default().fg(Color::Magenta)),
        Span::styled("\u{2588}hive ", Style::default().fg(Color::Yellow)),
        Span::styled("\u{2588}idle ", Style::default().fg(Color::Rgb(80, 80, 80))),
        Span::styled(
            "\u{250A}skip ",
            Style::default().fg(Color::Rgb(80, 80, 120)),
        ),
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
        .title_bottom(
            Line::from(vec![
                Span::raw(" "),
                Span::styled("[t]", Style::default().fg(Color::White)),
                Span::styled(" normal view  ", Style::default().fg(Color::Gray)),
                Span::styled("[j/k]", Style::default().fg(Color::White)),
                Span::styled(" scroll ", Style::default().fg(Color::Gray)),
            ])
            .right_aligned(),
        )
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

    // Build time mapper to collapse idle gaps > 5 minutes
    let mapper = TimeMapper::new(total_secs, agent_tool_calls, t_start);

    let dep_gutter = build_dep_gutter(&sorted, gutter_w);

    let total_agents = sorted.len();
    // -2: time axis header + legend row
    let visible_height = inner.height.saturating_sub(2) as usize;
    let scroll_clamped = scroll.min(total_agents.saturating_sub(1));

    let mut lines: Vec<Line> = Vec::new();

    // Time axis header
    let axis_prefix = " ".repeat(gutter_w);
    let axis = build_time_axis(total_secs, bar_w, label_w, &mapper);
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
            &mapper,
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
