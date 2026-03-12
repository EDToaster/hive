use serde_json::Value;
use std::fs;
use std::path::Path;

/// Phase classification for a tool use entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputPhase {
    /// Exploration: Read, Grep, Glob, LS
    Exploration,
    /// Implementation: Write, Edit, MultiEdit
    Implementation,
    /// Testing: Bash with test/cargo test/pytest/etc.
    Testing,
    /// Other tool calls
    Other,
}

impl OutputPhase {
    pub fn from_tool(name: &str, command: Option<&str>) -> Self {
        match name {
            "Read" | "Grep" | "Glob" | "LS" => Self::Exploration,
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Self::Implementation,
            "Bash" => {
                if let Some(cmd) = command {
                    let cmd_lower = cmd.to_lowercase();
                    if cmd_lower.contains("cargo test")
                        || cmd_lower.contains("pytest")
                        || cmd_lower.contains("npm test")
                        || cmd_lower.contains("make test")
                        || cmd_lower.contains("go test")
                        || (cmd_lower.contains("test") && !cmd_lower.starts_with("echo"))
                    {
                        return Self::Testing;
                    }
                }
                Self::Other
            }
            _ => Self::Other,
        }
    }
}

/// Parsed test results from a Bash test command's output.
#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    pub passed: u64,
    pub failed: u64,
    pub command: String,
}

/// Summary of files touched and test outcomes in a session.
#[derive(Debug, Clone, Default)]
pub struct OutputSummary {
    pub files_read: Vec<String>,
    pub files_written: Vec<String>,
    pub test_results: Vec<TestResult>,
}

/// A displayable entry parsed from Claude Code's stream-json NDJSON output.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEntry {
    /// Free-form text from the assistant.
    AssistantText(String),
    /// A tool invocation by the assistant.
    ToolUse {
        name: String,
        input_summary: String,
        /// Extracted file path (for Read/Write/Edit/Glob/Grep tools)
        extracted_path: Option<String>,
        /// Phase classification
        phase: OutputPhase,
    },
    /// The result returned from a tool call.
    ToolResult {
        /// First line for collapsed view
        first_line: String,
        /// Full content (up to 2000 chars) for expanded view
        full_content: String,
        /// Total line count
        line_count: usize,
    },
    /// The final result summary at the end of a session.
    Result {
        duration_ms: u64,
        cost_usd: f64,
        num_turns: u64,
        result: String,
    },
}

/// Truncate a string to at most `max` characters, appending "..." if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        // Don't split in the middle of a multi-byte character
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Produce a compact summary of a JSON value, truncated to `max` characters.
///
/// For objects, shows key=value pairs. For arrays, shows element count.
/// For scalars, shows the value directly.
pub fn summarize_json(v: &Value, max: usize) -> String {
    let summary = match v {
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => truncate(s, 40),
                        Value::Array(arr) => format!("[{} items]", arr.len()),
                        Value::Object(_) => "{...}".to_string(),
                        other => other.to_string(),
                    };
                    format!("{k}={val}")
                })
                .collect();
            parts.join(", ")
        }
        Value::Array(arr) => format!("[{} items]", arr.len()),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    truncate(&summary, max)
}

/// Read an NDJSON file and return its lines (excluding empty trailing lines).
pub fn load_output_file(path: &Path) -> Vec<String> {
    match fs::read_to_string(path) {
        Ok(contents) => contents
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Extract a file path from a tool's input JSON for known file-based tools.
fn extract_path_from_input(name: &str, input: &Value) -> Option<String> {
    match name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Glob" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(String::from),
        _ => None,
    }
}

/// Parse a Rust `cargo test` result line like:
/// "test result: ok. 42 passed; 0 failed; ..."
fn parse_rust_test_result(output: &str) -> Option<(u64, u64)> {
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("test result:") {
            // "test result: ok. 42 passed; 0 failed; ..."
            let passed = line
                .split_whitespace()
                .skip_while(|w| *w != "result:")
                .nth(2)
                .and_then(|w| w.parse::<u64>().ok())
                .unwrap_or(0);
            let failed = line
                .split(';')
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|w| w.parse::<u64>().ok())
                .unwrap_or(0);
            return Some((passed, failed));
        }
    }
    None
}

/// Parse NDJSON lines from Claude Code's `--output-format stream-json` into displayable entries.
///
/// Skips `system` lines (hooks, init). Extracts text and tool_use blocks from `assistant` messages,
/// tool_result lines, and the final `result` summary.
pub fn parse_output_lines(lines: &[String]) -> Vec<OutputEntry> {
    let mut entries = Vec::new();

    for line in lines {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match line_type {
            "system" => {
                // Skip all system lines (hook_started, hook_response, init)
                continue;
            }
            "assistant" => {
                // Extract content blocks from message.content
                if let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str())
                                    && !text.is_empty()
                                {
                                    entries.push(OutputEntry::AssistantText(text.to_string()));
                                }
                            }
                            "tool_use" => {
                                let name = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let input = block.get("input");
                                let input_summary =
                                    input.map(|i| summarize_json(i, 120)).unwrap_or_default();
                                let extracted_path =
                                    input.and_then(|i| extract_path_from_input(&name, i));
                                let command = if name == "Bash" {
                                    extracted_path.as_deref()
                                } else {
                                    None
                                };
                                let phase = OutputPhase::from_tool(&name, command);
                                entries.push(OutputEntry::ToolUse {
                                    name,
                                    input_summary,
                                    extracted_path,
                                    phase,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            "tool_result" => {
                let raw = if let Some(s) = v.get("content").and_then(|c| c.as_str()) {
                    s.to_string()
                } else if let Some(val) = v.get("content") {
                    val.to_string()
                } else {
                    String::new()
                };
                let first_line = raw.lines().next().unwrap_or("").to_string();
                let line_count = raw.lines().count();
                let full_content = truncate(&raw, 2000);
                entries.push(OutputEntry::ToolResult {
                    first_line,
                    full_content,
                    line_count,
                });
            }
            "result" => {
                let duration_ms = v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
                let cost_usd = v
                    .get("total_cost_usd")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                let num_turns = v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0);
                let result = v
                    .get("result")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();
                entries.push(OutputEntry::Result {
                    duration_ms,
                    cost_usd,
                    num_turns,
                    result,
                });
            }
            _ => {
                // Unknown line types are silently skipped
            }
        }
    }

    entries
}

/// Compute a summary of files touched and test results from parsed output entries.
pub fn compute_output_summary(entries: &[OutputEntry]) -> OutputSummary {
    let mut summary = OutputSummary::default();
    let mut pending_test_command: Option<String> = None;

    for entry in entries {
        match entry {
            OutputEntry::ToolUse {
                name,
                extracted_path,
                phase,
                ..
            } => {
                match name.as_str() {
                    "Read" => {
                        if let Some(p) = extracted_path
                            && !summary.files_read.contains(p)
                        {
                            summary.files_read.push(p.clone());
                        }
                    }
                    "Write" | "Edit" | "MultiEdit" => {
                        if let Some(p) = extracted_path
                            && !summary.files_written.contains(p)
                        {
                            summary.files_written.push(p.clone());
                        }
                    }
                    _ => {}
                }
                if *phase == OutputPhase::Testing {
                    pending_test_command = extracted_path.clone();
                } else {
                    pending_test_command = None;
                }
            }
            OutputEntry::ToolResult { full_content, .. } => {
                if let Some(ref cmd) = pending_test_command {
                    if let Some((passed, failed)) = parse_rust_test_result(full_content) {
                        summary.test_results.push(TestResult {
                            passed,
                            failed,
                            command: cmd.clone(),
                        });
                    }
                    pending_test_command = None;
                }
            }
            _ => {
                pending_test_command = None;
            }
        }
    }

    summary
}

/// Parse session_id from the init line at the start of output.jsonl.
/// This captures the session ID early (before the agent finishes), enabling crash recovery.
pub fn parse_early_session_id(output_path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(output_path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(10) {
        let line = line.ok()?;
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line)
            && json.get("type").and_then(|t| t.as_str()) == Some("system")
            && json.get("subtype").and_then(|t| t.as_str()) == Some("init")
        {
            return json
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    None
}

/// Parse session_id from a Claude Code NDJSON output file.
///
/// Scans lines in reverse looking for any JSON line containing a `session_id` field.
/// Returns the most recent session_id found, or None if the file doesn't exist or
/// contains no session_id.
pub fn parse_session_id_from_output(output_path: &Path) -> Option<String> {
    let data = fs::read_to_string(output_path).ok()?;
    for line in data.lines().rev() {
        if let Ok(json) = serde_json::from_str::<Value>(line)
            && let Some(sid) = json.get("session_id").and_then(|v| v.as_str())
        {
            return Some(sid.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn test_truncate_multibyte() {
        // Ensure we don't split in the middle of a multi-byte char
        let s = "hello\u{00e9}world"; // e with accent (2 bytes in UTF-8)
        let result = truncate(s, 6);
        // Should truncate before the multi-byte char boundary
        assert!(result.ends_with("..."));
        assert!(result.len() <= 12); // 6 + "..."
    }

    #[test]
    fn test_summarize_json_object() {
        let v: Value = serde_json::from_str(r#"{"file_path":"/src/main.rs","line":42}"#).unwrap();
        let summary = summarize_json(&v, 100);
        assert!(summary.contains("file_path="));
        assert!(summary.contains("line=42"));
    }

    #[test]
    fn test_summarize_json_array() {
        let v: Value = serde_json::from_str(r#"[1, 2, 3]"#).unwrap();
        assert_eq!(summarize_json(&v, 100), "[3 items]");
    }

    #[test]
    fn test_summarize_json_string() {
        let v: Value = serde_json::from_str(r#""hello world""#).unwrap();
        assert_eq!(summarize_json(&v, 100), "hello world");
    }

    #[test]
    fn test_summarize_json_nested_object() {
        let v: Value = serde_json::from_str(r#"{"name":"Read","input":{"a":"b"}}"#).unwrap();
        let summary = summarize_json(&v, 100);
        assert!(summary.contains("input={...}"));
    }

    #[test]
    fn test_summarize_json_with_array_value() {
        let v: Value = serde_json::from_str(r#"{"items":[1,2,3],"name":"test"}"#).unwrap();
        let summary = summarize_json(&v, 100);
        assert!(summary.contains("items=[3 items]"));
    }

    #[test]
    fn test_summarize_json_truncation() {
        let v: Value = serde_json::from_str(r#"{"key":"a very long value that should be truncated at some point if we set max low enough"}"#).unwrap();
        let summary = summarize_json(&v, 20);
        assert!(summary.ends_with("..."));
        // 20 chars + "..."
        assert!(summary.len() <= 23);
    }

    #[test]
    fn test_parse_assistant_text() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world!"}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0],
            OutputEntry::AssistantText("Hello world!".to_string())
        );
    }

    #[test]
    fn test_parse_tool_use() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::ToolUse {
                name,
                input_summary,
                extracted_path,
                phase,
            } => {
                assert_eq!(name, "Read");
                assert!(input_summary.contains("file_path="));
                assert_eq!(extracted_path.as_deref(), Some("/src/main.rs"));
                assert_eq!(phase, &OutputPhase::Exploration);
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_tool_result() {
        let lines = vec![
            r#"{"type":"tool_result","content":"File contents here...","tool_use_id":"abc123"}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::ToolResult {
                first_line,
                full_content,
                ..
            } => {
                assert_eq!(first_line, "File contents here...");
                assert_eq!(full_content, "File contents here...");
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_result_summary() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","duration_ms":2346,"total_cost_usd":0.09779,"num_turns":1,"result":"Hello!","session_id":"abc"}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                assert_eq!(*duration_ms, 2346);
                assert!((cost_usd - 0.09779).abs() < 0.0001);
                assert_eq!(*num_turns, 1);
                assert_eq!(result, "Hello!");
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_skip_system_lines() {
        let lines = vec![
            r#"{"type":"system","subtype":"hook_started","hook_id":"abc"}"#.to_string(),
            r#"{"type":"system","subtype":"hook_response","hook_id":"abc"}"#.to_string(),
            r#"{"type":"system","subtype":"init","cwd":"/home/user","session_id":"xyz"}"#
                .to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], OutputEntry::AssistantText("Hi".to_string()));
    }

    #[test]
    fn test_mixed_content_blocks() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me read the file."},{"type":"tool_use","name":"Read","input":{"file_path":"/src/lib.rs"}}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            OutputEntry::AssistantText("Let me read the file.".to_string())
        );
        match &entries[1] {
            OutputEntry::ToolUse { name, .. } => assert_eq!(name, "Read"),
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_full_session() {
        let lines = vec![
            // System lines (should be skipped)
            r#"{"type":"system","subtype":"hook_started","hook_id":"h1"}"#.to_string(),
            r#"{"type":"system","subtype":"hook_response","hook_id":"h1","output":"{}"}"#
                .to_string(),
            r#"{"type":"system","subtype":"init","cwd":"/project","session_id":"s1","tools":["Read","Write"]}"#
                .to_string(),
            // Assistant with text + tool_use
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll read the file first."},{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#
                .to_string(),
            // Tool result
            r#"{"type":"tool_result","content":"fn main() { println!(\"hello\"); }","tool_use_id":"tu1"}"#
                .to_string(),
            // Assistant response after tool
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The file contains a simple hello world program."}]}}"#
                .to_string(),
            // Final result
            r#"{"type":"result","subtype":"success","duration_ms":5000,"total_cost_usd":0.25,"num_turns":2,"result":"Analysis complete."}"#
                .to_string(),
        ];

        let entries = parse_output_lines(&lines);

        // Should have: text, tool_use, tool_result, text, result = 5 entries
        assert_eq!(entries.len(), 5);

        assert_eq!(
            entries[0],
            OutputEntry::AssistantText("I'll read the file first.".to_string())
        );
        match &entries[1] {
            OutputEntry::ToolUse {
                name,
                input_summary,
                ..
            } => {
                assert_eq!(name, "Read");
                assert!(input_summary.contains("file_path="));
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
        match &entries[2] {
            OutputEntry::ToolResult { full_content, .. } => {
                assert!(full_content.contains("fn main()"));
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
        assert_eq!(
            entries[3],
            OutputEntry::AssistantText(
                "The file contains a simple hello world program.".to_string()
            )
        );
        match &entries[4] {
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                assert_eq!(*duration_ms, 5000);
                assert!((cost_usd - 0.25).abs() < 0.0001);
                assert_eq!(*num_turns, 2);
                assert_eq!(result, "Analysis complete.");
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_json_lines_skipped() {
        let lines = vec![
            "not valid json".to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"OK"}]}}"#
                .to_string(),
            "".to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], OutputEntry::AssistantText("OK".to_string()));
    }

    #[test]
    fn test_empty_text_skipped() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":""}]}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_tool_result_long_content_truncated() {
        let long_content = "x".repeat(3000);
        let line = format!(
            r#"{{"type":"tool_result","content":"{}","tool_use_id":"t1"}}"#,
            long_content
        );
        let entries = parse_output_lines(&[line]);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::ToolResult {
                full_content,
                first_line,
                ..
            } => {
                // full_content is capped at 2000 chars + "..."
                assert!(full_content.len() <= 2003);
                assert!(full_content.ends_with("..."));
                // first_line is the first line of raw content (also truncated via full_content)
                assert!(!first_line.is_empty());
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn test_result_with_missing_fields() {
        let lines = vec![r#"{"type":"result","subtype":"success"}"#.to_string()];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                assert_eq!(*duration_ms, 0);
                assert!((cost_usd - 0.0).abs() < 0.0001);
                assert_eq!(*num_turns, 0);
                assert_eq!(result, "");
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_load_output_file_nonexistent() {
        let entries = load_output_file(Path::new("/nonexistent/path/output.jsonl"));
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_real_stream_format() {
        // Test with the real format from test-stream2.jsonl
        let lines = vec![
            r#"{"type":"system","subtype":"hook_started","hook_id":"8c51002b","hook_name":"SessionStart:startup","hook_event":"SessionStart","uuid":"c1f767f9"}"#.to_string(),
            r#"{"type":"system","subtype":"hook_response","hook_id":"8c51002b","hook_name":"SessionStart:startup","hook_event":"SessionStart","output":"{}","stderr":"","exit_code":0}"#.to_string(),
            r#"{"type":"system","subtype":"init","cwd":"/Users/howard/src/hive","session_id":"359d3e20","tools":["Bash","Read"]}"#.to_string(),
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","id":"msg_bdrk_016dAj1f8q1CNknoVhiLHeK1","type":"message","role":"assistant","content":[{"type":"text","text":"Hello!"}],"stop_reason":null,"usage":{"input_tokens":3,"output_tokens":1}},"parent_tool_use_id":null,"session_id":"359d3e20"}"#.to_string(),
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2346,"duration_api_ms":2323,"num_turns":1,"result":"Hello!","stop_reason":"end_turn","session_id":"359d3e20","total_cost_usd":0.09779,"usage":{"input_tokens":3}}"#.to_string(),
        ];

        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], OutputEntry::AssistantText("Hello!".to_string()));
        match &entries[1] {
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                assert_eq!(*duration_ms, 2346);
                assert!((cost_usd - 0.09779).abs() < 0.0001);
                assert_eq!(*num_turns, 1);
                assert_eq!(result, "Hello!");
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_type_skipped() {
        let lines = vec![
            r#"{"type":"unknown_future_type","data":"something"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"OK"}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], OutputEntry::AssistantText("OK".to_string()));
    }

    #[test]
    fn test_multiple_tool_uses_in_one_message() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"a.rs"}},{"type":"tool_use","name":"Read","input":{"file_path":"b.rs"}}]}}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 2);
        match (&entries[0], &entries[1]) {
            (
                OutputEntry::ToolUse {
                    name: n1,
                    input_summary: s1,
                    ..
                },
                OutputEntry::ToolUse {
                    name: n2,
                    input_summary: s2,
                    ..
                },
            ) => {
                assert_eq!(n1, "Read");
                assert!(s1.contains("a.rs"));
                assert_eq!(n2, "Read");
                assert!(s2.contains("b.rs"));
            }
            other => panic!("Expected two ToolUse entries, got {:?}", other),
        }
    }

    // =================================================================
    // New feature tests: OutputPhase, extracted_path, OutputSummary
    // =================================================================

    #[test]
    fn test_output_phase_from_tool_read() {
        assert_eq!(
            OutputPhase::from_tool("Read", None),
            OutputPhase::Exploration
        );
        assert_eq!(
            OutputPhase::from_tool("Grep", None),
            OutputPhase::Exploration
        );
        assert_eq!(
            OutputPhase::from_tool("Glob", None),
            OutputPhase::Exploration
        );
    }

    #[test]
    fn test_output_phase_from_tool_write() {
        assert_eq!(
            OutputPhase::from_tool("Write", None),
            OutputPhase::Implementation
        );
        assert_eq!(
            OutputPhase::from_tool("Edit", None),
            OutputPhase::Implementation
        );
        assert_eq!(
            OutputPhase::from_tool("MultiEdit", None),
            OutputPhase::Implementation
        );
    }

    #[test]
    fn test_output_phase_from_tool_bash_test() {
        assert_eq!(
            OutputPhase::from_tool("Bash", Some("cargo test --all-targets")),
            OutputPhase::Testing
        );
        assert_eq!(
            OutputPhase::from_tool("Bash", Some("npm test")),
            OutputPhase::Testing
        );
        assert_eq!(
            OutputPhase::from_tool("Bash", Some("pytest src/")),
            OutputPhase::Testing
        );
    }

    #[test]
    fn test_output_phase_from_tool_bash_other() {
        assert_eq!(
            OutputPhase::from_tool("Bash", Some("cargo build")),
            OutputPhase::Other
        );
        assert_eq!(OutputPhase::from_tool("Bash", None), OutputPhase::Other);
    }

    #[test]
    fn test_extracted_path_read() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/foo.rs"}}]}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        match &entries[0] {
            OutputEntry::ToolUse {
                extracted_path,
                phase,
                ..
            } => {
                assert_eq!(extracted_path.as_deref(), Some("/src/foo.rs"));
                assert_eq!(phase, &OutputPhase::Exploration);
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_extracted_path_write() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/src/bar.rs","content":"fn main(){}"}}]}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        match &entries[0] {
            OutputEntry::ToolUse {
                extracted_path,
                phase,
                ..
            } => {
                assert_eq!(extracted_path.as_deref(), Some("/src/bar.rs"));
                assert_eq!(phase, &OutputPhase::Implementation);
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_extracted_path_bash() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        match &entries[0] {
            OutputEntry::ToolUse {
                extracted_path,
                phase,
                ..
            } => {
                assert_eq!(extracted_path.as_deref(), Some("cargo test"));
                assert_eq!(phase, &OutputPhase::Testing);
            }
            other => panic!("Expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_result_stores_first_line_and_full_content() {
        // Use JSON-escaped newlines so the line is valid JSON
        let line = r#"{"type":"tool_result","content":"line one\nline two\nline three","tool_use_id":"t1"}"#.to_string();
        let entries = parse_output_lines(&[line]);
        assert_eq!(entries.len(), 1, "expected 1 entry");
        match &entries[0] {
            OutputEntry::ToolResult {
                first_line,
                full_content,
                line_count,
            } => {
                assert_eq!(first_line, "line one");
                assert_eq!(full_content, "line one\nline two\nline three");
                assert_eq!(*line_count, 3);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_output_summary_files() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#.to_string(),
            r#"{"type":"tool_result","content":"fn main() {}","tool_use_id":"t1"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/src/lib.rs","content":"pub fn foo() {}"}}]}}"#.to_string(),
            r#"{"type":"tool_result","content":"ok","tool_use_id":"t2"}"#.to_string(),
            // Duplicate read — should only appear once
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#.to_string(),
            r#"{"type":"tool_result","content":"fn main() {}","tool_use_id":"t3"}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        let summary = compute_output_summary(&entries);
        assert_eq!(summary.files_read, vec!["/src/main.rs"]);
        assert_eq!(summary.files_written, vec!["/src/lib.rs"]);
        assert!(summary.test_results.is_empty());
    }

    #[test]
    fn test_compute_output_summary_test_results() {
        let test_output = "running 42 tests\ntest result: ok. 42 passed; 0 failed; 0 ignored";
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#.to_string(),
            format!(
                r#"{{"type":"tool_result","content":"{}","tool_use_id":"t1"}}"#,
                test_output.replace('\n', "\\n")
            ),
        ];
        let entries = parse_output_lines(&lines);
        let summary = compute_output_summary(&entries);
        assert_eq!(summary.test_results.len(), 1);
        let tr = &summary.test_results[0];
        assert_eq!(tr.passed, 42);
        assert_eq!(tr.failed, 0);
        assert_eq!(tr.command, "cargo test");
    }

    #[test]
    fn test_compute_output_summary_empty() {
        let entries: Vec<OutputEntry> = vec![];
        let summary = compute_output_summary(&entries);
        assert!(summary.files_read.is_empty());
        assert!(summary.files_written.is_empty());
        assert!(summary.test_results.is_empty());
    }

    #[test]
    fn test_parse_rust_test_result_passing() {
        let output = "running 10 tests\ntest foo::bar ... ok\ntest result: ok. 10 passed; 0 failed; 0 ignored";
        assert_eq!(parse_rust_test_result(output), Some((10, 0)));
    }

    #[test]
    fn test_parse_rust_test_result_failing() {
        let output = "running 5 tests\ntest result: FAILED. 3 passed; 2 failed; 0 ignored";
        assert_eq!(parse_rust_test_result(output), Some((3, 2)));
    }

    #[test]
    fn test_parse_rust_test_result_none() {
        assert_eq!(parse_rust_test_result("no test output here"), None);
    }

    #[test]
    fn test_parse_session_id_from_output() {
        let dir = std::env::temp_dir().join("hive_test_session_id");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(
            &path,
            r#"{"type":"system","subtype":"init","session_id":"abc123"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}
{"type":"result","subtype":"success","session_id":"def456","duration_ms":100,"total_cost_usd":0.01,"num_turns":1,"result":"Done"}
"#,
        )
        .unwrap();
        // Should return the last session_id (scanning in reverse)
        let sid = parse_session_id_from_output(&path);
        assert_eq!(sid, Some("def456".to_string()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_parse_session_id_from_output_missing_file() {
        let sid = parse_session_id_from_output(Path::new("/nonexistent/output.jsonl"));
        assert_eq!(sid, None);
    }

    #[test]
    fn test_parse_session_id_from_output_no_session_id() {
        let dir = std::env::temp_dir().join("hive_test_no_sid");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(&path, r#"{"type":"assistant","message":{"content":[]}}"#).unwrap();
        let sid = parse_session_id_from_output(&path);
        assert_eq!(sid, None);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_parse_early_session_id() {
        let dir = std::env::temp_dir().join("hive_test_early_sid");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(
            &path,
            r#"{"type":"system","subtype":"init","session_id":"early123","cwd":"/project"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}
"#,
        )
        .unwrap();
        let sid = parse_early_session_id(&path);
        assert_eq!(sid, Some("early123".to_string()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_parse_early_session_id_missing_file() {
        let sid = parse_early_session_id(Path::new("/nonexistent/output.jsonl"));
        assert_eq!(sid, None);
    }

    // =================================================================
    // Adversarial tests: malformed NDJSON, boundary cases
    // =================================================================

    #[test]
    fn test_truncate_zero_max() {
        // Max of 0 should produce "..."
        assert_eq!(truncate("hello", 0), "...");
    }

    #[test]
    fn test_truncate_one_char() {
        assert_eq!(truncate("hello", 1), "h...");
    }

    #[test]
    fn test_truncate_with_multibyte_at_boundary() {
        // 🚀 is 4 bytes in UTF-8
        let s = "ab🚀cd";
        // Truncating at 3 would split the emoji — should back up to 2
        let result = truncate(s, 3);
        assert_eq!(result, "ab...");
    }

    #[test]
    fn test_summarize_json_null() {
        let v: Value = serde_json::from_str("null").unwrap();
        assert_eq!(summarize_json(&v, 100), "null");
    }

    #[test]
    fn test_summarize_json_boolean() {
        let v: Value = serde_json::from_str("true").unwrap();
        assert_eq!(summarize_json(&v, 100), "true");
    }

    #[test]
    fn test_summarize_json_number() {
        let v: Value = serde_json::from_str("42").unwrap();
        assert_eq!(summarize_json(&v, 100), "42");
    }

    #[test]
    fn test_summarize_json_empty_object() {
        let v: Value = serde_json::from_str("{}").unwrap();
        assert_eq!(summarize_json(&v, 100), "");
    }

    #[test]
    fn test_summarize_json_empty_array() {
        let v: Value = serde_json::from_str("[]").unwrap();
        assert_eq!(summarize_json(&v, 100), "[0 items]");
    }

    #[test]
    fn test_summarize_json_deeply_nested() {
        let v: Value = serde_json::from_str(r#"{"a":{"b":{"c":"deep"}}}"#).unwrap();
        let summary = summarize_json(&v, 100);
        assert!(summary.contains("a={...}"));
    }

    #[test]
    fn test_parse_output_all_garbage_lines() {
        let lines = vec![
            "completely invalid".to_string(),
            "{{broken json".to_string(),
            "".to_string(),
            "null".to_string(),
            "42".to_string(),
            "[]".to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_output_json_without_type() {
        let lines = vec![
            r#"{"message":"no type field"}"#.to_string(),
            r#"{"data":"something"}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        // Lines without "type" get type="" which doesn't match any case → skipped
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_output_assistant_no_content() {
        let lines = vec![
            r#"{"type":"assistant","message":{}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":null}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":"not_array"}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_output_assistant_unknown_block_type() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"image","url":"http://example.com"}]}}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_output_tool_result_with_json_content() {
        let lines = vec![
            r#"{"type":"tool_result","content":{"nested":"object"},"tool_use_id":"t1"}"#
                .to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::ToolResult { full_content, .. } => {
                assert!(full_content.contains("nested"));
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_output_tool_result_with_null_content() {
        let lines = vec![r#"{"type":"tool_result","tool_use_id":"t1"}"#.to_string()];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::ToolResult {
                first_line,
                full_content,
                ..
            } => {
                assert!(first_line.is_empty());
                assert!(full_content.is_empty());
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_output_result_with_negative_values() {
        let lines = vec![
            r#"{"type":"result","duration_ms":-1,"total_cost_usd":-0.5,"num_turns":-10,"result":"error"}"#.to_string(),
        ];
        let entries = parse_output_lines(&lines);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            OutputEntry::Result {
                duration_ms,
                cost_usd,
                num_turns,
                result,
            } => {
                // Negative values for u64 → as_u64() returns None → defaults to 0
                assert_eq!(*duration_ms, 0);
                assert!(*cost_usd < 0.0); // f64 accepts negatives
                assert_eq!(*num_turns, 0);
                assert_eq!(result, "error");
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_load_output_file_empty_file() {
        let dir = std::env::temp_dir().join("hive_test_empty_output");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(&path, "").unwrap();
        let lines = load_output_file(&path);
        assert!(lines.is_empty());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_output_file_with_only_empty_lines() {
        let dir = std::env::temp_dir().join("hive_test_empty_lines");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(&path, "\n\n\n\n").unwrap();
        let lines = load_output_file(&path);
        assert!(lines.is_empty());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_parse_session_id_with_all_garbage() {
        let dir = std::env::temp_dir().join("hive_test_garbage_sid");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(&path, "not json\nalso not json\n{\"no\":\"session_id\"}\n").unwrap();
        let sid = parse_session_id_from_output(&path);
        assert_eq!(sid, None);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_parse_early_session_id_no_init() {
        let dir = std::env::temp_dir().join("hive_test_early_no_init");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("output.jsonl");
        fs::write(
            &path,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}
{"type":"result","subtype":"success","duration_ms":100,"total_cost_usd":0.01,"num_turns":1,"result":"Done"}
"#,
        )
        .unwrap();
        let sid = parse_early_session_id(&path);
        assert_eq!(sid, None);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}
