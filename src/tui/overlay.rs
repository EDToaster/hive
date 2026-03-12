use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::*;

use super::helpers::{centered_rect, format_duration_short};
use super::Overlay;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_overlay(
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
