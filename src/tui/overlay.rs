use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::*;

use super::Overlay;
use super::helpers::{centered_rect, format_duration_short};

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
    output_expanded_entries: &std::collections::HashSet<usize>,
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
                output_expanded_entries,
            );
        }
        Overlay::Help => {
            let area = centered_rect(70, 85, frame.area());
            frame.render_widget(Clear, area);
            render_help_overlay(frame, area);
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

fn tool_phase_color(phase: &crate::output::OutputPhase) -> Color {
    use crate::output::OutputPhase;
    match phase {
        OutputPhase::Exploration => Color::Cyan,
        OutputPhase::Implementation => Color::Green,
        OutputPhase::Testing => Color::Magenta,
        OutputPhase::Other => Color::Yellow,
    }
}

fn render_agent_output_overlay(
    frame: &mut Frame,
    area: Rect,
    agent_id: &str,
    path: &std::path::Path,
    scroll: usize,
    auto_scroll: bool,
    expanded_entries: &std::collections::HashSet<usize>,
) {
    use crate::output::{
        OutputEntry, OutputPhase, compute_output_summary, load_output_file, parse_output_lines,
    };

    let raw_lines = load_output_file(path);
    let entries = parse_output_lines(&raw_lines);
    let summary = compute_output_summary(&entries);

    let mut lines: Vec<Line> = Vec::new();

    // ── Summary header ────────────────────────────────────────────────────────
    if !summary.files_read.is_empty()
        || !summary.files_written.is_empty()
        || !summary.test_results.is_empty()
    {
        lines.push(Line::from(Span::styled(
            " ─── Summary ───────────────────────────────────────────",
            Style::default().fg(Color::DarkGray),
        )));

        if !summary.files_written.is_empty() {
            let file_list = summary
                .files_written
                .iter()
                .map(|f| {
                    // Show just the filename, not full path
                    std::path::Path::new(f)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(f.as_str())
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(vec![
                Span::styled(" ✎ ", Style::default().fg(Color::Green)),
                Span::styled(
                    format!("wrote: {file_list}"),
                    Style::default().fg(Color::Green),
                ),
            ]));
        }

        if !summary.files_read.is_empty() {
            let count = summary.files_read.len();
            let show: Vec<&str> = summary
                .files_read
                .iter()
                .take(4)
                .map(|f| {
                    std::path::Path::new(f.as_str())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(f.as_str())
                })
                .collect();
            let file_list = if count > 4 {
                format!("{} (+{})", show.join(", "), count - 4)
            } else {
                show.join(", ")
            };
            lines.push(Line::from(vec![
                Span::styled(" 👁 ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("read: {file_list}"),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }

        for tr in &summary.test_results {
            let (color, icon) = if tr.failed == 0 {
                (Color::Green, "✓")
            } else {
                (Color::Red, "✗")
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), Style::default().fg(color)),
                Span::styled(
                    format!("tests: {} passed, {} failed", tr.passed, tr.failed),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        lines.push(Line::from(Span::styled(
            " ────────────────────────────────────────────────────────",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }

    // ── Entries ───────────────────────────────────────────────────────────────
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let mut prev_phase: Option<&OutputPhase> = None;

        for (idx, entry) in entries.iter().enumerate() {
            match entry {
                OutputEntry::AssistantText(text) => {
                    prev_phase = None;
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
                    phase,
                    ..
                } => {
                    // Phase separator when phase changes
                    let show_separator = prev_phase.is_some_and(|p| p != phase);
                    if show_separator {
                        let label = match phase {
                            OutputPhase::Exploration => "── exploration ──",
                            OutputPhase::Implementation => "── implementation ──",
                            OutputPhase::Testing => "── testing ──",
                            OutputPhase::Other => "────────────────",
                        };
                        lines.push(Line::from(Span::styled(
                            format!(" {label}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                    prev_phase = Some(phase);

                    let color = tool_phase_color(phase);
                    lines.push(Line::from(vec![
                        Span::styled("▶ ", Style::default().fg(color)),
                        Span::styled(
                            name.as_str(),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" {input_summary}"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                OutputEntry::ToolResult {
                    first_line,
                    full_content,
                    line_count,
                } => {
                    let is_expanded = expanded_entries.contains(&idx);
                    if is_expanded {
                        for l in full_content.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("   {l}"),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                        if *line_count > full_content.lines().count() {
                            lines.push(Line::from(Span::styled(
                                format!("   … (truncated, {} total lines)", line_count),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                        lines.push(Line::from(Span::styled(
                            "   [Enter] collapse",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    } else {
                        let extra = if *line_count > 1 {
                            format!(" (+{} lines, Enter to expand)", line_count - 1)
                        } else {
                            String::new()
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("   {first_line}"),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(
                                extra,
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ]));
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
                        "── Session Complete ──",
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
    // Extra visual rows from wrapping
    let wrap_width = area.width.saturating_sub(2) as usize;
    let wrap_extra: usize = lines
        .iter()
        .map(|line| {
            let w: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if wrap_width == 0 {
                0
            } else {
                w.saturating_sub(1) / wrap_width
            }
        })
        .sum();
    let max_scroll = (logical_lines + wrap_extra).saturating_sub(visible);
    let effective_scroll = if auto_scroll {
        max_scroll
    } else {
        max_scroll.saturating_sub(scroll)
    };

    let block = Block::default()
        .title(format!(" Output: {} ", agent_id))
        .title_bottom(
            Line::from(" [j/k] scroll  [Enter] expand  [G] follow  [Esc] close ").right_aligned(),
        )
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

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    fn section(title: &str) -> Line<'static> {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                title.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    }
    fn key(keys: &str, desc: &str) -> Line<'static> {
        let pad = 18usize.saturating_sub(keys.len());
        Line::from(vec![
            Span::styled(
                format!("  {keys}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(pad)),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ])
    }
    fn blank() -> Line<'static> {
        Line::from("")
    }

    let lines: Vec<Line> = vec![
        blank(),
        section("Navigation"),
        key("Tab", "Cycle panes (Swarm → Tasks → Activity)"),
        key("1 / 2 / 3", "Jump to Swarm / Tasks / Activity pane"),
        key("j / ↓", "Move selection down"),
        key("k / ↑", "Move selection up"),
        key("Enter", "Open detail overlay for selected item"),
        key("Space", "Collapse / expand selected node"),
        key("G", "Activity: jump to bottom (follow mode)"),
        blank(),
        section("Search & Filter  (Activity pane)"),
        key("/", "Open search bar — type to filter incrementally"),
        key("Esc", "Clear search query / close overlay"),
        key("Enter", "Commit search — keep filter, exit typing mode"),
        key("Backspace", "Delete last character from search query"),
        key("f", "Cycle filter preset: all → running → failed"),
        blank(),
        section("Swarm pane"),
        key("Enter", "Open agent detail overlay"),
        key("o", "Open agent output viewer"),
        key("Space", "Collapse / expand agent children"),
        blank(),
        section("Agent output viewer"),
        key("j / ↓", "Scroll down"),
        key("k / ↑", "Scroll up"),
        key("G", "Jump to bottom (follow mode)"),
        key("Esc / q / o", "Close viewer"),
        blank(),
        section("Mouse (when enabled)"),
        key("Click", "Focus pane / select item"),
        key("Double-click", "Open detail overlay"),
        key("Scroll wheel", "Scroll within the pane under cursor"),
        blank(),
        section("Other"),
        key("m", "Toggle mouse capture on / off"),
        key("?", "Show this help overlay"),
        key("q", "Quit"),
        blank(),
        Line::from(Span::styled(
            "                           [any key] close ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .title(" \u{2b21} Hive Keyboard Reference ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}
