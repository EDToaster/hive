# TUI Agent Output Viewer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a live agent output viewer to the TUI, accessible via hotkey when an agent is selected in the swarm pane, parsing stream-json NDJSON and rendering formatted conversation output.

**Architecture:** New `Overlay::AgentOutput` variant opens a full-screen scrollable view. On each TUI tick, it re-reads the agent's `output.jsonl`, parses NDJSON lines into displayable entries (assistant text, tool calls, tool results, final summary), and renders them with color-coded formatting. `j`/`k` scrolls, `G` jumps to bottom (tail mode), `Esc` closes.

**Tech Stack:** Rust, ratatui, serde_json, existing TUI infrastructure in `src/tui.rs`

---

### Task 1: Add NDJSON Output Parser

**Files:**
- Create: `src/output.rs`
- Modify: `src/main.rs` (add `mod output;`)

This module parses `output.jsonl` stream-json into a Vec of displayable entries.

**Step 1: Write the failing test**

In `src/output.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assistant_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world"}]}}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], OutputEntry::AssistantText(t) if t == "Hello world"));
    }

    #[test]
    fn parse_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/foo/bar.rs"}}]}}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], OutputEntry::ToolUse { name, .. } if name == "Read"));
    }

    #[test]
    fn parse_tool_result() {
        let line = r#"{"type":"tool_result","content":"file contents here","tool_use_id":"abc123"}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], OutputEntry::ToolResult { .. }));
    }

    #[test]
    fn parse_result_summary() {
        let line = r#"{"type":"result","subtype":"success","duration_ms":5000,"total_cost_usd":0.15,"num_turns":3,"result":"Done!","session_id":"abc"}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], OutputEntry::Result { .. }));
    }

    #[test]
    fn skips_system_lines() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn mixed_content_blocks() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me read the file."},{"type":"tool_use","name":"Read","input":{"file_path":"/foo.rs"}}]}}"#;
        let entries = parse_output_lines(&[line.to_string()]);
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], OutputEntry::AssistantText(_)));
        assert!(matches!(&entries[1], OutputEntry::ToolUse { .. }));
    }

    #[test]
    fn parse_full_session() {
        let lines = vec![
            r#"{"type":"system","subtype":"init","session_id":"abc"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll read the file."},{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#.to_string(),
            r#"{"type":"tool_result","content":"fn main() {}","tool_use_id":"tu1"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The file contains a main function."}]}}"#.to_string(),
            r#"{"type":"result","subtype":"success","duration_ms":5000,"total_cost_usd":0.15,"num_turns":2,"result":"Done","session_id":"abc"}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 4); // init skipped
        assert!(matches!(&entries[0], OutputEntry::AssistantText(_)));
        assert!(matches!(&entries[1], OutputEntry::ToolUse { .. }));
        assert!(matches!(&entries[2], OutputEntry::ToolResult { .. }));
        assert!(matches!(&entries[3], OutputEntry::Result { .. }));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test output::tests -v`
Expected: FAIL — module doesn't exist yet

**Step 3: Write the implementation**

In `src/output.rs`:

```rust
use serde_json::Value;

#[derive(Debug)]
pub enum OutputEntry {
    AssistantText(String),
    ToolUse {
        name: String,
        input_summary: String, // truncated first-line summary of input
    },
    ToolResult {
        content: String, // truncated
    },
    Result {
        duration_ms: u64,
        cost_usd: f64,
        num_turns: u64,
        result: String, // truncated final result text
    },
}

pub fn parse_output_lines(lines: &[String]) -> Vec<OutputEntry> {
    let mut entries = Vec::new();
    for line in lines {
        let json: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match msg_type {
            "assistant" => {
                if let Some(content) = json
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                    entries.push(OutputEntry::AssistantText(text.to_string()));
                                }
                            }
                            "tool_use" => {
                                let name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let input_summary = block
                                    .get("input")
                                    .map(|v| summarize_json(v, 120))
                                    .unwrap_or_default();
                                entries.push(OutputEntry::ToolUse {
                                    name,
                                    input_summary,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            "tool_result" => {
                let content = json
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                entries.push(OutputEntry::ToolResult {
                    content: truncate(&content, 500),
                });
            }
            "result" => {
                entries.push(OutputEntry::Result {
                    duration_ms: json
                        .get("duration_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    cost_usd: json
                        .get("total_cost_usd")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                    num_turns: json
                        .get("num_turns")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    result: json
                        .get("result")
                        .and_then(|v| v.as_str())
                        .map(|s| truncate(s, 200))
                        .unwrap_or_default(),
                });
            }
            _ => {} // skip "system" and unknown types
        }
    }
    entries
}

pub fn load_output_file(path: &std::path::Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(data) => data.lines().map(|l| l.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn summarize_json(v: &Value, max: usize) -> String {
    let s = v.to_string();
    truncate(&s, max)
}
```

Add to `src/main.rs`:
```rust
mod output;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test output::tests -v`
Expected: all 7 tests PASS

**Step 5: Commit**

```bash
git add src/output.rs src/main.rs
git commit -m "feat: add NDJSON stream-json output parser"
```

---

### Task 2: Add AgentOutput Overlay Variant and Hotkey

**Files:**
- Modify: `src/tui.rs:28-31` (Overlay enum)
- Modify: `src/tui.rs:33-42` (TuiState — add output_scroll)
- Modify: `src/tui.rs:660-749` (key handling)

**Step 1: Write the failing test**

No unit test needed here — this is pure UI wiring. We'll test via manual verification.

**Step 2: Add `AgentOutput` overlay variant and `output_scroll` state**

In `src/tui.rs`, modify the `Overlay` enum:

```rust
#[derive(Clone)]
enum Overlay {
    Agent(String),
    Task(String),
    AgentOutput(String), // agent_id — live output viewer
}
```

Add to `TuiState`:

```rust
struct TuiState {
    // ... existing fields ...
    output_scroll: usize,
    output_auto_scroll: bool,
}
```

Default both to `0` and `true`.

**Step 3: Add 'o' hotkey in key handling**

In the key handling section, add a new match arm for `KeyCode::Char('o')`:

```rust
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
```

**Step 4: Handle j/k/G/Esc within AgentOutput overlay**

When the overlay is `AgentOutput`, the existing `Esc` handler already clears it. But we need j/k/G to control `output_scroll` instead of the pane scrolling. Modify the key handling so that when `ui.overlay` is `Some(Overlay::AgentOutput(_))`, j/k/G operate on `output_scroll`/`output_auto_scroll`:

```rust
// At the top of key handling, before pane-specific handling:
if let Some(Overlay::AgentOutput(_)) = &ui.overlay {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            ui.overlay = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            ui.output_auto_scroll = false;
            ui.output_scroll = ui.output_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            ui.output_auto_scroll = false;
            ui.output_scroll = ui.output_scroll.saturating_sub(1);
        }
        KeyCode::Char('G') => {
            ui.output_auto_scroll = true;
        }
        _ => {}
    }
    continue; // skip normal key handling
}
```

**Step 5: Run clippy and verify no warnings**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean

**Step 6: Commit**

```bash
git add src/tui.rs
git commit -m "feat: add AgentOutput overlay variant and 'o' hotkey"
```

---

### Task 3: Render Agent Output Overlay

**Files:**
- Modify: `src/tui.rs:1178-1193` (render_overlay — add AgentOutput match arm)
- Modify: `src/tui.rs` (add render_agent_output_overlay function)
- Modify: `src/tui.rs:547` (run_tui_loop signature — needs access to state for agents_dir)

The rendering reads `output.jsonl` on each frame, parses it, and renders formatted output.

**Step 1: Wire up render_overlay**

In `render_overlay`, the function currently takes `agents` and `tasks`. It also needs `state` and `run_id` to locate the output file. Modify the signature:

```rust
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
```

Add the new match arm:

```rust
Overlay::AgentOutput(agent_id) => {
    let output_path = state
        .agents_dir(run_id)
        .join(agent_id)
        .join("output.jsonl");
    render_agent_output_overlay(frame, area, agent_id, &output_path, output_scroll, output_auto_scroll);
}
```

**Step 2: Implement render_agent_output_overlay**

```rust
fn render_agent_output_overlay(
    frame: &mut Frame,
    area: Rect,
    agent_id: &str,
    output_path: &std::path::Path,
    scroll: usize,
    auto_scroll: bool,
) {
    use crate::output::{OutputEntry, load_output_file, parse_output_lines};

    let raw_lines = load_output_file(output_path);
    let entries = parse_output_lines(&raw_lines);

    let mut lines: Vec<Line> = Vec::new();

    for entry in &entries {
        match entry {
            OutputEntry::AssistantText(text) => {
                // Wrap text and render in default color
                for l in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!(" {l}"),
                        Style::default().fg(Color::White),
                    )));
                }
                lines.push(Line::from("")); // blank separator
            }
            OutputEntry::ToolUse { name, input_summary } => {
                lines.push(Line::from(vec![
                    Span::styled(" ▶ ", Style::default().fg(Color::Yellow)),
                    Span::styled(name.clone(), Style::default().fg(Color::Yellow).bold()),
                    Span::styled(
                        format!(" {input_summary}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            OutputEntry::ToolResult { content } => {
                // Show first few lines of result, dimmed
                for (i, l) in content.lines().enumerate() {
                    if i >= 5 {
                        lines.push(Line::from(Span::styled(
                            format!("   ... ({} more lines)", content.lines().count() - 5),
                            Style::default().fg(Color::DarkGray),
                        )));
                        break;
                    }
                    lines.push(Line::from(Span::styled(
                        format!("   {l}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " ── Session Complete ──",
                    Style::default().fg(Color::Green).bold(),
                )));
                lines.push(Line::from(format!(
                    "  Duration: {:.1}s  Cost: ${:.4}  Turns: {num_turns}",
                    *duration_ms as f64 / 1000.0,
                    cost_usd
                )));
                if !result.is_empty() {
                    lines.push(Line::from(""));
                    for l in result.lines() {
                        lines.push(Line::from(format!("  {l}")));
                    }
                }
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " [j/k] scroll  [G] follow  [Esc] close",
        Style::default().fg(Color::Gray),
    )));

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize; // minus border
    let effective_scroll = if auto_scroll {
        total_lines.saturating_sub(visible_height)
    } else {
        scroll.min(total_lines.saturating_sub(visible_height))
    };

    let block = Block::default()
        .title(format!(" Output: {} ", agent_id))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((effective_scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}
```

**Step 3: Use full screen area instead of centered_rect for output overlay**

The output overlay should use more screen real estate than the agent/task detail overlays. Use a 90% width, 90% height centered rect:

```rust
Overlay::AgentOutput(agent_id) => {
    let area = centered_rect(90, 90, frame.area());
    frame.render_widget(Clear, area);
    // ... render call
}
```

Keep the existing `centered_rect(60, 80, ...)` for Agent and Task overlays.

**Step 4: Update the render_overlay call site**

In the main render loop (~line 649), pass the additional parameters:

```rust
if let Some(ref overlay) = ui.overlay {
    render_overlay(
        frame,
        overlay,
        &agents,
        &tasks,
        &state,
        run_id,
        ui.output_scroll,
        ui.output_auto_scroll,
    );
}
```

**Step 5: Run clippy and manual test**

Run: `cargo clippy --all-targets -- -D warnings`
Run: `cargo test --all-targets`

**Step 6: Commit**

```bash
git add src/tui.rs
git commit -m "feat: render live agent output in TUI overlay"
```

---

### Task 4: Add Help Text Hint for 'o' Hotkey

**Files:**
- Modify: `src/tui.rs` (status bar / help line rendering)

**Step 1: Find the help/status bar rendering**

Look for where keybindings are displayed (likely in a footer or title bar). Add `[o] output` to the swarm pane hints.

**Step 2: Add hint**

In the swarm pane title or footer, add the `o` hotkey hint alongside existing ones like `[Enter] detail`:

```rust
// In the swarm pane block title or footer:
" [Enter] detail  [o] output  [j/k] navigate "
```

**Step 3: Verify and commit**

Run: `cargo clippy --all-targets -- -D warnings`

```bash
git add src/tui.rs
git commit -m "feat: add 'o' hotkey hint to swarm pane"
```

---

### Task 5: Update output_scroll on Auto-Scroll Tick

**Files:**
- Modify: `src/tui.rs` (TuiState — auto-scroll sync)

**Step 1: Ensure output_scroll tracks end when auto_scroll is true**

The `render_agent_output_overlay` already handles auto-scroll by computing `effective_scroll` from total lines. However, if the user scrolls up with `k` and then presses `G`, we need to reset. This is already handled in Task 2's key handling (`KeyCode::Char('G')` sets `output_auto_scroll = true`).

No additional code needed — verify by manual testing that:
1. Opening output with `o` starts at the bottom (auto-scroll)
2. `k` scrolls up and disables auto-scroll
3. `G` re-enables auto-scroll (jumps back to bottom)
4. New output lines appear at the bottom when auto-scrolling

**Step 2: Commit (if any tweaks needed)**

```bash
git add src/tui.rs
git commit -m "fix: ensure output auto-scroll tracks latest output"
```
