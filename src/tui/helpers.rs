use crate::types::*;
use ratatui::prelude::*;

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

// ---------------------------------------------------------------------------
// Animation utilities
// ---------------------------------------------------------------------------

/// Braille spinner frames — one per tick, cycling at 1 Hz.
pub(super) const BRAILLE_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Return the braille spinner character for the current tick.
pub(super) fn spinner_char(tick: u64) -> char {
    BRAILLE_SPINNER[(tick as usize) % BRAILLE_SPINNER.len()]
}

/// Linear interpolation between two u8 values, t in 0..=255.
fn lerp_u8(a: u8, b: u8, t: u8) -> u8 {
    let a = a as u32;
    let b = b as u32;
    let t = t as u32;
    ((a * (255 - t) + b * t) / 255) as u8
}

/// Smooth triangle-wave pulse between two RGB colors.
/// `period` is the cycle length in ticks (full back-and-forth).
pub(super) fn pulse_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), tick: u64, period: u64) -> Color {
    let period = period.max(2);
    let phase = tick % period;
    let half = period / 2;
    let t = if phase <= half { phase } else { period - phase };
    let blend = ((t * 255) / half.max(1)) as u8;
    Color::Rgb(
        lerp_u8(c1.0, c2.0, blend),
        lerp_u8(c1.1, c2.1, blend),
        lerp_u8(c1.2, c2.2, blend),
    )
}

/// Heartbeat color with a slow pulse effect based on freshness.
/// Fresh (<120s): breathes between bright white-green and mid-green.
/// Aging (approaching stall): pulses amber to orange.
/// Stalled: delegates to static `heartbeat_color` (solid red).
pub(super) fn heartbeat_color_animated(age_secs: i64, stall_timeout: i64, tick: u64) -> Color {
    if age_secs < 120 {
        pulse_rgb((200, 255, 200), (40, 180, 40), tick, 6)
    } else if age_secs < stall_timeout {
        pulse_rgb((255, 210, 40), (255, 110, 0), tick, 4)
    } else {
        heartbeat_color(age_secs, stall_timeout)
    }
}

/// Border color for the swarm pane, with a brief flash when any agent
/// recently changed status (drives sci-fi "alert" feel).
pub(super) fn animated_border_color(
    focused: super::Pane,
    this: super::Pane,
    tick: u64,
    last_change_tick: Option<u64>,
) -> Color {
    if let Some(change_tick) = last_change_tick {
        let age = tick.wrapping_sub(change_tick);
        if age < 4 {
            // Flash: bright cyan → teal → normal over 4 ticks
            let flash_colors: &[(u8, u8, u8)] = &[
                (255, 255, 120), // tick+0: bright yellow-white
                (80, 255, 220),  // tick+1: electric cyan
                (40, 200, 160),  // tick+2: teal
                (0, 160, 120),   // tick+3: dim teal fading out
            ];
            let (r, g, b) = flash_colors[age as usize];
            return Color::Rgb(r, g, b);
        }
    }
    border_color(focused, this)
}

/// Gradient color for the title "HIVE" text — slow cycle through cyan→blue→cyan.
pub(super) fn title_gradient_color(tick: u64) -> Color {
    pulse_rgb((0, 220, 255), (80, 80, 255), tick, 10)
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
