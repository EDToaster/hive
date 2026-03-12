use crate::types::*;
use ratatui::prelude::*;
use ratatui::widgets::*;

pub(super) fn status_abbrev(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Running => "run",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Failed => "fail",
        AgentStatus::Stalled => "stal",
    }
}

pub(super) fn agent_status_color(s: AgentStatus) -> Color {
    match s {
        AgentStatus::Running => Color::Green,
        AgentStatus::Idle => Color::Cyan,
        AgentStatus::Done => Color::LightBlue,
        AgentStatus::Failed => Color::Red,
        AgentStatus::Stalled => Color::Yellow,
    }
}

pub(super) fn task_status_color(s: TaskStatus) -> Color {
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

pub(super) fn task_status_bullet(s: TaskStatus) -> &'static str {
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

pub(super) fn format_duration_short(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

pub(super) fn heartbeat_color(age_secs: i64, stall_timeout: i64) -> Color {
    if age_secs < 120 {
        Color::White
    } else if age_secs < stall_timeout {
        Color::Yellow
    } else {
        Color::Red
    }
}

pub(super) fn truncate_spans(spans: Vec<Span<'_>>, max_width: usize) -> Vec<Span<'_>> {
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

pub(super) fn border_color(focused: super::Pane, this: super::Pane) -> Color {
    if focused == this {
        Color::Cyan
    } else {
        Color::Gray
    }
}

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
