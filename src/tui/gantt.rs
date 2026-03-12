use crate::types::*;
use chrono::{DateTime, Utc};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::HashMap;

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
        AgentRole::Coordinator => '⬡',
        AgentRole::Lead => '▸',
        AgentRole::Worker => '○',
        AgentRole::Reviewer => '◈',
        AgentRole::Planner => '◆',
        AgentRole::Postmortem => '◇',
        AgentRole::Explorer => '»',
        AgentRole::Evaluator => '≡',
    }
}

// ---------------------------------------------------------------------------
// Time axis header
// ---------------------------------------------------------------------------

fn build_time_axis(total_secs: i64, bar_w: usize, label_w: usize) -> Line<'static> {
    let mut axis = vec![' '; bar_w];

    // Place 4 tick labels evenly across the bar
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
        // Place label left-aligned at pos, clamped to bar_w
        let start = pos.min(bar_w.saturating_sub(label.len()));
        for (j, ch) in label.chars().enumerate() {
            if start + j < bar_w {
                axis[start + j] = ch;
            }
        }
    }

    let label_part = " ".repeat(label_w + 1); // +1 for separator space
    let axis_str: String = axis.into_iter().collect();
    Line::from(vec![
        Span::styled(label_part, Style::default()),
        Span::styled(axis_str, Style::default().fg(Color::DarkGray)),
    ])
}

// ---------------------------------------------------------------------------
// Individual agent row
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render_agent_row(
    agent: &Agent,
    t_start: DateTime<Utc>,
    t_now: DateTime<Utc>,
    total_secs: i64,
    label_w: usize,
    bar_w: usize,
    spawn_times: &HashMap<String, DateTime<Utc>>,
    queue_timestamps: &[DateTime<Utc>],
) -> Line<'static> {
    let color = role_color(agent.role);
    let symbol = role_symbol(agent.role);

    // Build label: symbol + id, padded/truncated to label_w
    let max_id = label_w.saturating_sub(2); // symbol + space
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

    // Convert to bar column positions
    let ts = total_secs.max(1) as usize;
    let bar_start = (start_secs as usize * bar_w) / ts;
    let bar_end = ((end_secs as usize * bar_w) / ts).clamp(bar_start + 1, bar_w);

    // Build the bar buffer
    let mut bar: Vec<char> = vec![' '; bar_w];

    // Fill agent lifetime with block chars
    for cell in bar.iter_mut().take(bar_end).skip(bar_start) {
        *cell = '█';
    }

    // Mark merge queue submission times with '│' (only in empty space)
    for &qt in queue_timestamps {
        let q_secs = (qt - t_start).num_seconds();
        if q_secs >= 0 && q_secs < total_secs {
            let q_pos = (q_secs as usize * bar_w) / ts;
            if q_pos < bar_w && bar[q_pos] == ' ' {
                bar[q_pos] = '│';
            }
        }
    }

    // Status indicator suffix
    let status_char = match agent.status {
        AgentStatus::Running => '▶',
        AgentStatus::Idle => '◦',
        AgentStatus::Done => '✓',
        AgentStatus::Failed => '✗',
        AgentStatus::Stalled => '!',
    };

    let bar_color = match agent.status {
        AgentStatus::Done => Color::DarkGray,
        AgentStatus::Failed => Color::Red,
        AgentStatus::Stalled => Color::Yellow,
        _ => color,
    };

    let bar_str: String = bar.into_iter().collect();

    Line::from(vec![
        Span::styled(label, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(bar_str, Style::default().fg(bar_color)),
        Span::styled(format!(" {status_char}"), Style::default().fg(color)),
    ])
}

// ---------------------------------------------------------------------------
// Main render entry point
// ---------------------------------------------------------------------------

pub(super) fn render_gantt_view(
    frame: &mut Frame,
    area: Rect,
    agents: &[Agent],
    queue: &MergeQueue,
    run_meta: &Option<RunMetadata>,
    spawn_times: &HashMap<String, DateTime<Utc>>,
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

    // label_w: fixed left column width (role symbol + space + agent id)
    let label_w: usize = 22;
    // +2: 1 separator space + 1 status char suffix
    let bar_w = (inner.width as usize).saturating_sub(label_w + 2);

    if bar_w < 4 {
        return;
    }

    // Queue submission timestamps for vertical markers
    let queue_timestamps: Vec<DateTime<Utc>> =
        queue.entries.iter().map(|e| e.submitted_at).collect();

    // Sort: coordinator first, then by spawn time
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

    let total_agents = sorted.len();
    let visible_height = inner.height.saturating_sub(1) as usize; // -1 for time axis header
    let scroll_clamped = scroll.min(total_agents.saturating_sub(1));

    let mut lines: Vec<Line> = Vec::new();

    // Time axis header row
    lines.push(build_time_axis(total_secs, bar_w, label_w));

    // Agent rows (scrolled)
    for agent in sorted.iter().skip(scroll_clamped).take(visible_height) {
        lines.push(render_agent_row(
            agent,
            t_start,
            now,
            total_secs,
            label_w,
            bar_w,
            spawn_times,
            &queue_timestamps,
        ));
    }

    // Scroll hint at bottom if more agents below
    let shown = lines.len().saturating_sub(1); // -1 for header
    if scroll_clamped + shown < total_agents {
        let remaining = total_agents - scroll_clamped - shown;
        lines.push(Line::from(Span::styled(
            format!("  ↓ {remaining} more"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
