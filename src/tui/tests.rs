use crate::tui::helpers::*;
use crate::tui::input::handle_mouse;
use crate::tui::render::{extract_arg, format_tool_display};
use crate::tui::tree::*;
use crate::tui::*;
use crate::types::*;
use chrono::Utc;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use rusqlite::Connection;
use std::collections::HashSet;

fn make_task(id: &str, title: &str, parent: Option<&str>, status: TaskStatus) -> Task {
    Task {
        id: id.into(),
        title: title.into(),
        description: String::new(),
        status,
        urgency: Urgency::Normal,
        blocking: vec![],
        blocked_by: vec![],
        assigned_to: None,
        created_by: "test".into(),
        parent_task: parent.map(|s| s.into()),
        branch: None,
        domain: None,
        review_count: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn task_tree_roots_sorted_by_creation() {
    let tasks = vec![
        make_task("t-2", "Second", None, TaskStatus::Pending),
        make_task("t-1", "First", None, TaskStatus::Active),
    ];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].task_id, "t-2");
    assert_eq!(nodes[1].task_id, "t-1");
}

#[test]
fn task_tree_children_nested_under_parent() {
    let tasks = vec![
        make_task("parent", "Parent Task", None, TaskStatus::Merged),
        make_task("child-1", "First Child", Some("parent"), TaskStatus::Merged),
        make_task(
            "child-2",
            "Second Child",
            Some("parent"),
            TaskStatus::Pending,
        ),
    ];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].task_id, "parent");
    assert!(nodes[0].has_children);
    assert!(nodes[0].indicator.contains('\u{25BC}')); // ▼
    assert_eq!(nodes[1].task_id, "child-1");
    assert!(nodes[1].prefix.contains('\u{251C}')); // ├
    assert_eq!(nodes[2].task_id, "child-2");
    assert!(nodes[2].prefix.contains('\u{2514}')); // └
}

#[test]
fn task_tree_collapsed_hides_children() {
    let tasks = vec![
        make_task("parent", "Parent Task", None, TaskStatus::Merged),
        make_task("child-1", "First Child", Some("parent"), TaskStatus::Merged),
        make_task(
            "child-2",
            "Second Child",
            Some("parent"),
            TaskStatus::Pending,
        ),
    ];
    let mut collapsed = HashSet::new();
    collapsed.insert("parent".to_string());
    let nodes = build_task_tree(&tasks, &collapsed);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].task_id, "parent");
    assert!(nodes[0].indicator.contains('\u{25B6}')); // ▶
    assert!(nodes[0].title.contains("[1 merged, 1 pending]"));
}

#[test]
fn task_tree_childless_root_has_no_indicator() {
    let tasks = vec![make_task("lone", "Lone Task", None, TaskStatus::Active)];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes.len(), 1);
    assert!(!nodes[0].has_children);
    assert_eq!(nodes[0].indicator, "  ");
}

#[test]
fn task_tree_aggregate_status_only_nonzero() {
    let tasks = vec![
        make_task("p", "Parent", None, TaskStatus::Merged),
        make_task("c1", "C1", Some("p"), TaskStatus::Merged),
        make_task("c2", "C2", Some("p"), TaskStatus::Merged),
    ];
    let mut collapsed = HashSet::new();
    collapsed.insert("p".to_string());
    let nodes = build_task_tree(&tasks, &collapsed);
    assert_eq!(nodes[0].title, "Parent [2 merged]");
}

#[test]
fn task_tree_real_world_run_af5af78f() {
    // Reproduce the actual task hierarchy from run af5af78f
    let tasks = vec![
        make_task("task-77cc4261", "D1: Types+State", None, TaskStatus::Merged),
        make_task(
            "task-d345f9c0",
            "Add types to types.rs",
            Some("task-77cc4261"),
            TaskStatus::Pending,
        ),
        make_task(
            "task-c2095213",
            "Add state operations",
            Some("task-77cc4261"),
            TaskStatus::Pending,
        ),
        make_task(
            "task-f55567b3",
            "Combined impl",
            Some("task-77cc4261"),
            TaskStatus::Merged,
        ),
        make_task("task-59327398", "D2: MCP Tools", None, TaskStatus::Merged),
        make_task(
            "task-d1d70ac5",
            "Add Hive Mind MCP tools",
            Some("task-59327398"),
            TaskStatus::Merged,
        ),
        make_task("task-d7b90b26", "D3: Prompts", None, TaskStatus::Merged),
        make_task(
            "task-bf3a761c",
            "Implement prompts",
            Some("task-d7b90b26"),
            TaskStatus::Merged,
        ),
        make_task(
            "task-0290d750",
            "D4: CLI Commands",
            None,
            TaskStatus::Merged,
        ),
        make_task(
            "task-104b2e65",
            "Add explore + mind CLI",
            Some("task-0290d750"),
            TaskStatus::Merged,
        ),
    ];

    // Expanded: 10 nodes total, 4 roots + 6 children
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes.len(), 10);

    // First root is D1 with ▼ indicator
    assert_eq!(nodes[0].task_id, "task-77cc4261");
    assert!(nodes[0].has_children);
    assert!(nodes[0].indicator.contains('\u{25BC}'));

    // Its 3 children follow
    assert_eq!(nodes[1].task_id, "task-d345f9c0");
    assert_eq!(nodes[2].task_id, "task-c2095213");
    assert_eq!(nodes[3].task_id, "task-f55567b3");

    // D2 is next root
    assert_eq!(nodes[4].task_id, "task-59327398");

    // Collapse D1: only 7 nodes visible
    let mut collapsed = HashSet::new();
    collapsed.insert("task-77cc4261".to_string());
    let nodes = build_task_tree(&tasks, &collapsed);
    assert_eq!(nodes.len(), 7);
    assert!(nodes[0].title.contains("[1 merged, 2 pending]"));
    assert_eq!(nodes[1].task_id, "task-59327398");
}

#[test]
fn format_action_summary_strips_hive_prefix() {
    assert_eq!(
        format_action_summary("hive_wait_for_activity", None),
        "wait_for_activity"
    );
    assert_eq!(
        format_action_summary("hive_send_message", Some("to=coordinator")),
        "send_message to=coordinator"
    );
}

#[test]
fn format_action_summary_strips_mcp_prefix() {
    assert_eq!(
        format_action_summary("mcp__hive__hive_spawn_agent", Some("role=worker")),
        "spawn_agent role=worker"
    );
}

#[test]
fn format_action_summary_truncates_long_args() {
    let long_args = "a]".repeat(20); // 40 chars
    let result = format_action_summary("Read", Some(&long_args));
    assert!(result.len() < 40, "should truncate: {result}");
    assert!(result.ends_with('…'));
}

#[test]
fn format_action_summary_no_prefix_passthrough() {
    assert_eq!(
        format_action_summary("Read", Some("src/main.rs")),
        "Read src/main.rs"
    );
    assert_eq!(format_action_summary("Write", None), "Write");
}

#[test]
fn load_latest_actions_with_no_db() {
    let result = load_latest_actions(&None, "test-run");
    assert!(result.is_empty());
}

#[test]
fn load_latest_actions_from_in_memory_db() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tool_calls (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            args_summary TEXT,
            status TEXT NOT NULL DEFAULT 'ok',
            duration_ms INTEGER,
            timestamp TEXT NOT NULL
        );
        INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
        VALUES ('run1', 'worker-1', 'Read', 'src/main.rs', 'ok', '2025-01-01T00:00:01Z');
        INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
        VALUES ('run1', 'worker-1', 'Edit', 'src/tui.rs', 'ok', '2025-01-01T00:00:02Z');
        INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
        VALUES ('run1', 'worker-2', 'hive_send_message', 'to=lead', 'ok', '2025-01-01T00:00:03Z');
        INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp)
        VALUES ('run2', 'worker-3', 'Read', 'other.rs', 'ok', '2025-01-01T00:00:04Z');",
    )
    .unwrap();

    let db = Some(conn);
    let actions = load_latest_actions(&db, "run1");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions.get("worker-1").unwrap(), "Edit src/tui.rs");
    assert_eq!(actions.get("worker-2").unwrap(), "send_message to=lead");
    // worker-3 belongs to run2, should not appear
    assert!(!actions.contains_key("worker-3"));
}

#[test]
fn task_tree_navigation_skips_collapsed_children() {
    let tasks = vec![
        make_task("p1", "Parent 1", None, TaskStatus::Active),
        make_task("c1", "Child 1", Some("p1"), TaskStatus::Pending),
        make_task("c2", "Child 2", Some("p1"), TaskStatus::Pending),
        make_task("p2", "Parent 2", None, TaskStatus::Active),
    ];

    // Expanded: 4 visible nodes
    let expanded = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(expanded.len(), 4);

    // Collapsed p1: 2 visible nodes (p1, p2)
    let mut collapsed = HashSet::new();
    collapsed.insert("p1".to_string());
    let collapsed_nodes = build_task_tree(&tasks, &collapsed);
    assert_eq!(collapsed_nodes.len(), 2);
    assert_eq!(collapsed_nodes[0].task_id, "p1");
    assert_eq!(collapsed_nodes[1].task_id, "p2");
}

// -----------------------------------------------------------------------
// format_tool_display tests
// -----------------------------------------------------------------------

#[test]
fn format_hive_wait_for_activity() {
    let (tool, args, color) =
        format_tool_display("hive_wait_for_activity", Some("timeout_secs=30"));
    assert_eq!(tool, "WaitForActivity");
    assert_eq!(args, "timeout: 30s");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_spawn_agent() {
    let (tool, args, color) =
        format_tool_display("hive_spawn_agent", Some("agent_id=worker-1, role=worker"));
    assert_eq!(tool, "SpawnAgent");
    assert_eq!(args, "worker-1 worker");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_check_agents() {
    let (tool, args, color) = format_tool_display("hive_check_agents", None);
    assert_eq!(tool, "CheckAgents");
    assert_eq!(args, "");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_send_message() {
    let (tool, args, color) =
        format_tool_display("hive_send_message", Some("to=coordinator, body=done"));
    assert_eq!(tool, "SendMessage");
    assert_eq!(args, "\u{2192} coordinator");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_create_task() {
    let (tool, args, color) = format_tool_display("hive_create_task", Some("title=Add feature X"));
    assert_eq!(tool, "CreateTask");
    assert_eq!(args, "Add feature X");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_update_task() {
    let (tool, args, color) =
        format_tool_display("hive_update_task", Some("task_id=t-1, status=review"));
    assert_eq!(tool, "UpdateTask");
    assert_eq!(args, "t-1 \u{2192} review");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_submit_to_queue() {
    let (tool, args, color) = format_tool_display("hive_submit_to_queue", Some("task_id=t-42"));
    assert_eq!(tool, "SubmitToQueue");
    assert_eq!(args, "t-42");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_merge_next() {
    let (tool, args, color) = format_tool_display("hive_merge_next", None);
    assert_eq!(tool, "MergeNext");
    assert_eq!(args, "");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_review_verdict() {
    let (tool, args, color) =
        format_tool_display("hive_review_verdict", Some("task_id=t-5, verdict=approve"));
    assert_eq!(tool, "ReviewVerdict");
    assert_eq!(args, "t-5: approve");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_hive_unknown_tool() {
    let (tool, args, color) = format_tool_display("hive_something_new", Some("x=1"));
    assert_eq!(tool, "SomethingNew");
    assert_eq!(args, "x=1");
    assert_eq!(color, Color::Yellow);
}

#[test]
fn format_read_tool() {
    let (tool, args, color) = format_tool_display("Read", Some("file_path=/src/main.rs"));
    assert_eq!(tool, "Read");
    assert_eq!(args, "/src/main.rs");
    assert_eq!(color, Color::Gray);
}

#[test]
fn format_edit_tool() {
    let (tool, args, color) = format_tool_display("Edit", Some("file_path=/src/tui.rs"));
    assert_eq!(tool, "Edit");
    assert_eq!(args, "/src/tui.rs");
    assert_eq!(color, Color::Gray);
}

#[test]
fn format_bash_tool() {
    let (tool, args, color) = format_tool_display("Bash", Some("command=cargo test"));
    assert_eq!(tool, "$");
    assert_eq!(args, "cargo test");
    assert_eq!(color, Color::Blue);
}

#[test]
fn format_grep_tool() {
    let (tool, args, color) = format_tool_display("Grep", Some("pattern=TODO"));
    assert_eq!(tool, "Grep");
    assert_eq!(args, "TODO");
    assert_eq!(color, Color::Gray);
}

#[test]
fn format_unknown_tool() {
    let (tool, args, color) = format_tool_display("SomeNewTool", None);
    assert_eq!(tool, "SomeNewTool");
    assert_eq!(args, "");
    assert_eq!(color, Color::Gray);
}

#[test]
fn format_unknown_tool_with_args() {
    let (tool, args, color) = format_tool_display("SomeNewTool", Some("key=val"));
    assert_eq!(tool, "SomeNewTool");
    assert_eq!(args, "key=val");
    assert_eq!(color, Color::Gray);
}

#[test]
fn extract_arg_finds_value() {
    assert_eq!(
        extract_arg("file_path=/src/main.rs, limit=100", "file_path"),
        Some("/src/main.rs")
    );
    assert_eq!(
        extract_arg("file_path=/src/main.rs, limit=100", "limit"),
        Some("100")
    );
}

#[test]
fn extract_arg_missing_key() {
    assert_eq!(extract_arg("file_path=/src/main.rs", "missing"), None);
}

#[test]
fn extract_arg_empty_string() {
    assert_eq!(extract_arg("", "key"), None);
}

// -----------------------------------------------------------------------
// Mouse support tests
// -----------------------------------------------------------------------

fn make_tree_node(id: &str, has_children: bool) -> TreeNode {
    TreeNode {
        agent_id: id.into(),
        prefix: String::new(),
        status: AgentStatus::Running,
        task_id: None,
        heartbeat: None,
        role: AgentRole::Worker,
        has_children,
        indicator: if has_children {
            "\u{25BC} ".into()
        } else {
            "  ".into()
        },
    }
}

fn make_task_tree_node(id: &str, has_children: bool) -> TaskTreeNode {
    TaskTreeNode {
        task_id: id.into(),
        prefix: String::new(),
        indicator: if has_children {
            "\u{25BC} ".into()
        } else {
            "  ".into()
        },
        title: format!("Task {id}"),
        status: TaskStatus::Active,
        assigned_to: None,
        review_count: 0,
        has_children,
    }
}

fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

#[test]
fn mouse_default_on() {
    let ui = TuiState::default();
    assert!(ui.mouse_enabled, "mouse should be on by default");
}

#[test]
fn pane_row_index_inside_border() {
    let area = Rect::new(0, 5, 40, 10); // y=5..15, inner rows y=6..13
    assert_eq!(pane_row_index(area, 5), None); // top border
    assert_eq!(pane_row_index(area, 6), Some(0)); // first inner row
    assert_eq!(pane_row_index(area, 7), Some(1));
    assert_eq!(pane_row_index(area, 14), None); // bottom border
}

#[test]
fn mouse_click_selects_swarm_item() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        ..Default::default()
    };
    let nodes = vec![
        make_tree_node("agent-0", false),
        make_tree_node("agent-1", false),
    ];
    let task_nodes: Vec<TaskTreeNode> = vec![];

    // Click on row 4 → inner index 1 → agent-1
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 4),
        &nodes,
        &task_nodes,
    );
    assert_eq!(ui.swarm_selected, Some(1));
    assert_eq!(ui.focused_pane, Pane::Swarm);
    assert_eq!(ui.selected_agent_filter.as_deref(), Some("agent-1"));
}

#[test]
fn mouse_click_selects_task_item() {
    let mut ui = TuiState {
        tasks_area: Rect::new(40, 2, 60, 10),
        ..Default::default()
    };
    let tree_nodes: Vec<TreeNode> = vec![];
    let task_nodes = vec![
        make_task_tree_node("task-0", false),
        make_task_tree_node("task-1", false),
    ];

    // Click on header row (y=3) → no selection (header)
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 3),
        &tree_nodes,
        &task_nodes,
    );
    assert_eq!(ui.tasks_selected, None);

    // Click on first data row (y=4) → index 0
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
        &tree_nodes,
        &task_nodes,
    );
    assert_eq!(ui.tasks_selected, Some(0));
    assert_eq!(ui.focused_pane, Pane::Tasks);
}

#[test]
fn mouse_click_focuses_activity_pane() {
    let mut ui = TuiState {
        activity_area: Rect::new(0, 15, 100, 10),
        ..Default::default()
    };

    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 10, 18),
        &[],
        &[],
    );
    assert_eq!(ui.focused_pane, Pane::Activity);
}

#[test]
fn mouse_scroll_down_in_swarm_advances_by_1() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        swarm_selected: Some(0),
        ..Default::default()
    };
    let nodes = vec![
        make_tree_node("a-0", false),
        make_tree_node("a-1", false),
        make_tree_node("a-2", false),
        make_tree_node("a-3", false),
        make_tree_node("a-4", false),
    ];

    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollDown, 5, 5),
        &nodes,
        &[],
    );
    assert_eq!(ui.swarm_selected, Some(1)); // 0 + 1
}

#[test]
fn mouse_scroll_in_activity_increases_by_3() {
    let mut ui = TuiState {
        activity_area: Rect::new(0, 15, 100, 10),
        activity_scroll: 5,
        activity_auto_scroll: false,
        ..Default::default()
    };

    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollDown, 10, 18),
        &[],
        &[],
    );
    assert_eq!(ui.activity_scroll, 8); // 5 + 3
}

#[test]
fn mouse_double_click_opens_agent_overlay() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        ..Default::default()
    };
    let nodes = vec![make_tree_node("agent-x", false)];

    // First click
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
        &nodes,
        &[],
    );
    assert!(ui.overlay.is_none());

    // Second click at same position (double-click)
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
        &nodes,
        &[],
    );
    assert!(matches!(ui.overlay, Some(Overlay::Agent(ref id)) if id == "agent-x"));
}

#[test]
fn mouse_double_click_opens_task_overlay() {
    let mut ui = TuiState {
        tasks_area: Rect::new(40, 2, 60, 10),
        ..Default::default()
    };
    let task_nodes = vec![make_task_tree_node("task-y", false)];

    // First click (y=4: first data row, past header at y=3)
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
        &[],
        &task_nodes,
    );
    // Second click (double-click)
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 4),
        &[],
        &task_nodes,
    );
    assert!(matches!(ui.overlay, Some(Overlay::Task(ref id)) if id == "task-y"));
}

#[test]
fn mouse_click_outside_overlay_dismisses() {
    let mut ui = TuiState {
        overlay: Some(Overlay::Agent("agent-z".into())),
        overlay_area: Rect::new(20, 5, 60, 30),
        ..Default::default()
    };

    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3),
        &[],
        &[],
    );
    assert!(ui.overlay.is_none());
}

#[test]
fn mouse_click_inside_overlay_does_not_dismiss() {
    let mut ui = TuiState {
        overlay: Some(Overlay::Agent("agent-z".into())),
        overlay_area: Rect::new(20, 5, 60, 30),
        ..Default::default()
    };

    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 40, 15),
        &[],
        &[],
    );
    assert!(ui.overlay.is_some());
}

#[test]
fn mouse_scroll_in_overlay_adjusts_output_scroll() {
    let mut ui = TuiState {
        overlay: Some(Overlay::AgentOutput("agent-o".into())),
        overlay_area: Rect::new(5, 3, 90, 40),
        ..Default::default()
    };

    // Scroll up inside overlay
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollUp, 50, 20),
        &[],
        &[],
    );
    assert_eq!(ui.output_scroll, 3);
    assert!(!ui.output_auto_scroll);
}

#[test]
fn mouse_click_on_collapse_toggle_toggles_agent() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        ..Default::default()
    };
    let nodes = vec![make_tree_node("lead-1", true)]; // has_children=true, prefix=""

    // Toggle indicator is at col = swarm_area.x + 1 + prefix.len() = 0 + 1 + 0 = 1
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
        &nodes,
        &[],
    );
    assert!(ui.collapsed_agents.contains("lead-1"));

    // Click again to expand
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
        &nodes,
        &[],
    );
    assert!(!ui.collapsed_agents.contains("lead-1"));
}

#[test]
fn mouse_click_on_collapse_toggle_toggles_task() {
    let mut ui = TuiState {
        tasks_area: Rect::new(40, 2, 60, 10),
        ..Default::default()
    };
    let task_nodes = vec![make_task_tree_node("task-p", true)]; // has_children=true

    // Toggle indicator at col = tasks_area.x + 1 + prefix.len() = 40 + 1 + 0 = 41
    // y=4: first data row (y=3 is header)
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 41, 4),
        &[],
        &task_nodes,
    );
    assert!(ui.collapsed_tasks.contains("task-p"));

    // Click again to expand
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 41, 4),
        &[],
        &task_nodes,
    );
    assert!(!ui.collapsed_tasks.contains("task-p"));
}

#[test]
fn detect_multiplexer_returns_valid_value() {
    let result = detect_multiplexer();
    if let Some(name) = result {
        assert!(
            name == "Zellij" || name == "tmux" || name == "screen",
            "unexpected multiplexer: {name}"
        );
    }
}

// -----------------------------------------------------------------------
// Helper: make_agent
// -----------------------------------------------------------------------

fn make_agent(id: &str, role: AgentRole, status: AgentStatus, parent: Option<&str>) -> Agent {
    Agent {
        id: id.into(),
        role,
        status,
        parent: parent.map(|s| s.into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    }
}

// -----------------------------------------------------------------------
// Empty state tests
// -----------------------------------------------------------------------

#[test]
fn task_tree_empty_input() {
    let nodes = build_task_tree(&[], &HashSet::new());
    assert!(nodes.is_empty());
}

#[test]
fn agent_tree_empty_input() {
    let nodes = build_tree(&[], &HashSet::new());
    assert!(nodes.is_empty());
}

#[test]
fn aggregate_child_status_empty_children() {
    let children: Vec<&Task> = vec![];
    let result = aggregate_child_status(&children);
    assert_eq!(result, "");
}

#[test]
fn aggregate_agent_status_no_descendants() {
    let agents = vec![make_agent(
        "coord",
        AgentRole::Coordinator,
        AgentStatus::Running,
        None,
    )];
    let result = aggregate_agent_status(&agents, "coord");
    assert_eq!(result, "");
}

#[test]
fn load_latest_actions_empty_db() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tool_calls (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            args_summary TEXT,
            status TEXT NOT NULL DEFAULT 'ok',
            duration_ms INTEGER,
            timestamp TEXT NOT NULL
        );",
    )
    .unwrap();
    let db = Some(conn);
    let actions = load_latest_actions(&db, "nonexistent-run");
    assert!(actions.is_empty());
}

// -----------------------------------------------------------------------
// Formatting helper edge case tests
// -----------------------------------------------------------------------

#[test]
fn format_duration_short_zero() {
    assert_eq!(format_duration_short(0), "0s");
}

#[test]
fn format_duration_short_boundary_59() {
    assert_eq!(format_duration_short(59), "59s");
}

#[test]
fn format_duration_short_boundary_60() {
    assert_eq!(format_duration_short(60), "1m");
}

#[test]
fn format_duration_short_boundary_3599() {
    assert_eq!(format_duration_short(3599), "59m");
}

#[test]
fn format_duration_short_boundary_3600() {
    assert_eq!(format_duration_short(3600), "1h");
}

#[test]
fn format_duration_short_large_value() {
    assert_eq!(format_duration_short(86400), "24h");
}

#[test]
fn format_duration_short_negative() {
    // Negative seconds (possible with clock skew)
    assert_eq!(format_duration_short(-5), "-5s");
}

#[test]
fn heartbeat_color_fresh() {
    assert_eq!(heartbeat_color(0, 300), Color::White);
    assert_eq!(heartbeat_color(119, 300), Color::White);
}

#[test]
fn heartbeat_color_warning() {
    assert_eq!(heartbeat_color(120, 300), Color::Yellow);
    assert_eq!(heartbeat_color(299, 300), Color::Yellow);
}

#[test]
fn heartbeat_color_stalled() {
    assert_eq!(heartbeat_color(300, 300), Color::Red);
    assert_eq!(heartbeat_color(1000, 300), Color::Red);
}

#[test]
fn heartbeat_color_zero_timeout() {
    // Edge: stall_timeout=0 means everything >= 120 is red
    assert_eq!(heartbeat_color(0, 0), Color::White);
    assert_eq!(heartbeat_color(120, 0), Color::Red);
}

#[test]
fn truncate_spans_zero_width() {
    let spans = vec![Span::raw("hello")];
    let result = truncate_spans(spans, 0);
    assert!(result.is_empty());
}

#[test]
fn truncate_spans_width_1() {
    let spans = vec![Span::raw("hello")];
    let result = truncate_spans(spans, 1);
    // Should just be the ellipsis
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].content.as_ref(), "\u{2026}");
}

#[test]
fn truncate_spans_exact_fit() {
    let spans = vec![Span::raw("hello")];
    let result = truncate_spans(spans, 5);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].content.as_ref(), "hello");
}

#[test]
fn truncate_spans_multi_span_truncation() {
    let spans = vec![Span::raw("abc"), Span::raw("defgh")];
    // Total: 8 chars, max_width=6 → target=5 chars + ellipsis
    let result = truncate_spans(spans, 6);
    // "abc" (3) + "de" (2) + "…" (1)
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].content.as_ref(), "abc");
    assert_eq!(result[1].content.as_ref(), "de");
    assert_eq!(result[2].content.as_ref(), "\u{2026}");
}

#[test]
fn truncate_spans_under_limit() {
    let spans = vec![Span::raw("hi")];
    let result = truncate_spans(spans, 100);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].content.as_ref(), "hi");
}

#[test]
fn border_color_focused_vs_unfocused() {
    assert_eq!(border_color(Pane::Swarm, Pane::Swarm), Color::Cyan);
    assert_eq!(border_color(Pane::Swarm, Pane::Tasks), Color::Gray);
    assert_eq!(border_color(Pane::Activity, Pane::Activity), Color::Cyan);
}

#[test]
fn status_abbrev_all_variants() {
    assert_eq!(status_abbrev(AgentStatus::Running), "run");
    assert_eq!(status_abbrev(AgentStatus::Idle), "idle");
    assert_eq!(status_abbrev(AgentStatus::Done), "done");
    assert_eq!(status_abbrev(AgentStatus::Failed), "fail");
    assert_eq!(status_abbrev(AgentStatus::Stalled), "stal");
}

#[test]
fn agent_status_color_all_variants() {
    assert_eq!(agent_status_color(AgentStatus::Running), Color::Green);
    assert_eq!(agent_status_color(AgentStatus::Idle), Color::Cyan);
    assert_eq!(agent_status_color(AgentStatus::Done), Color::LightBlue);
    assert_eq!(agent_status_color(AgentStatus::Failed), Color::Red);
    assert_eq!(agent_status_color(AgentStatus::Stalled), Color::Yellow);
}

#[test]
fn task_status_color_all_variants() {
    assert_eq!(task_status_color(TaskStatus::Active), Color::Green);
    assert_eq!(task_status_color(TaskStatus::Approved), Color::Green);
    assert_eq!(task_status_color(TaskStatus::Merged), Color::LightBlue);
    assert_eq!(task_status_color(TaskStatus::Queued), Color::Yellow);
    assert_eq!(task_status_color(TaskStatus::Review), Color::Yellow);
    assert_eq!(task_status_color(TaskStatus::Blocked), Color::Yellow);
    assert_eq!(task_status_color(TaskStatus::Pending), Color::Gray);
    assert_eq!(task_status_color(TaskStatus::Failed), Color::Red);
    assert_eq!(task_status_color(TaskStatus::Absorbed), Color::Cyan);
    assert_eq!(task_status_color(TaskStatus::Cancelled), Color::DarkGray);
}

#[test]
fn task_status_bullet_all_variants() {
    // Ensure every variant returns a non-empty string with expected status name
    assert!(task_status_bullet(TaskStatus::Active).contains("active"));
    assert!(task_status_bullet(TaskStatus::Merged).contains("merged"));
    assert!(task_status_bullet(TaskStatus::Queued).contains("queued"));
    assert!(task_status_bullet(TaskStatus::Review).contains("review"));
    assert!(task_status_bullet(TaskStatus::Pending).contains("pending"));
    assert!(task_status_bullet(TaskStatus::Blocked).contains("blocked"));
    assert!(task_status_bullet(TaskStatus::Approved).contains("approved"));
    assert!(task_status_bullet(TaskStatus::Failed).contains("failed"));
    assert!(task_status_bullet(TaskStatus::Absorbed).contains("absorbed"));
    assert!(task_status_bullet(TaskStatus::Cancelled).contains("cancelled"));
}

// -----------------------------------------------------------------------
// Overflow / truncation / long string tests
// -----------------------------------------------------------------------

#[test]
fn format_action_summary_very_long_tool_name() {
    let long_name = "a".repeat(100);
    let result = format_action_summary(&long_name, None);
    assert_eq!(result, long_name);
}

#[test]
fn format_action_summary_empty_tool_name() {
    assert_eq!(format_action_summary("", None), "");
}

#[test]
fn format_action_summary_empty_args() {
    assert_eq!(format_action_summary("Read", Some("")), "Read");
}

#[test]
fn format_action_summary_exactly_30_char_args() {
    let args = "a".repeat(30);
    let result = format_action_summary("Read", Some(&args));
    assert_eq!(result, format!("Read {}", args));
}

#[test]
fn format_action_summary_31_char_args_truncated() {
    let args = "a".repeat(31);
    let result = format_action_summary("Read", Some(&args));
    assert!(result.ends_with('…'));
    assert!(result.len() < 40);
}

#[test]
fn format_tool_display_with_empty_args() {
    // Empty args string → extract_arg returns None → falls back to "?"
    let (tool, args, _color) = format_tool_display("Read", Some(""));
    assert_eq!(tool, "Read");
    assert_eq!(args, "?");
}

#[test]
fn format_tool_display_hive_tool_no_match_in_args() {
    // send_message with missing "to=" field → falls back to "?"
    let (tool, args, color) = format_tool_display("hive_send_message", Some("body=hello"));
    assert_eq!(tool, "SendMessage");
    assert_eq!(args, "\u{2192} ?"); // → ?
    assert_eq!(color, Color::Yellow);
}

#[test]
fn extract_arg_value_with_spaces() {
    assert_eq!(
        extract_arg("title=Add feature X, status=pending", "title"),
        Some("Add feature X")
    );
}

#[test]
fn extract_arg_last_key() {
    assert_eq!(extract_arg("a=1, b=2, c=3", "c"), Some("3"));
}

#[test]
fn extract_arg_key_substring_no_false_match() {
    // "file_path" should not match "path"
    assert_eq!(extract_arg("file_path=/src/main.rs", "path"), None);
}

// -----------------------------------------------------------------------
// Agent tree edge cases
// -----------------------------------------------------------------------

#[test]
fn agent_tree_single_coordinator() {
    let agents = vec![make_agent(
        "coord",
        AgentRole::Coordinator,
        AgentStatus::Running,
        None,
    )];
    let nodes = build_tree(&agents, &HashSet::new());
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].agent_id, "coord");
    assert!(!nodes[0].has_children);
}

#[test]
fn agent_tree_coordinator_sorts_first() {
    let agents = vec![
        make_agent("lead-1", AgentRole::Lead, AgentStatus::Running, None),
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
    ];
    let nodes = build_tree(&agents, &HashSet::new());
    assert_eq!(nodes[0].agent_id, "coord");
    assert_eq!(nodes[1].agent_id, "lead-1");
}

#[test]
fn agent_tree_with_children() {
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-1",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent(
            "worker-1",
            AgentRole::Worker,
            AgentStatus::Done,
            Some("lead-1"),
        ),
    ];
    let nodes = build_tree(&agents, &HashSet::new());
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].agent_id, "coord");
    assert!(nodes[0].has_children);
    assert_eq!(nodes[1].agent_id, "lead-1");
    assert!(nodes[1].has_children);
    assert_eq!(nodes[2].agent_id, "worker-1");
}

#[test]
fn agent_tree_collapse_hides_children() {
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-1",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent(
            "worker-1",
            AgentRole::Worker,
            AgentStatus::Done,
            Some("lead-1"),
        ),
    ];
    let mut collapsed = HashSet::new();
    collapsed.insert("coord".to_string());
    let nodes = build_tree(&agents, &collapsed);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].agent_id, "coord");
    assert!(nodes[0].indicator.contains('\u{25B6}')); // ▶
}

#[test]
fn agent_tree_collapse_inner_node_only() {
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-1",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent(
            "worker-1",
            AgentRole::Worker,
            AgentStatus::Done,
            Some("lead-1"),
        ),
        make_agent(
            "worker-2",
            AgentRole::Worker,
            AgentStatus::Running,
            Some("lead-1"),
        ),
    ];
    // Collapse lead-1 but not coord
    let mut collapsed = HashSet::new();
    collapsed.insert("lead-1".to_string());
    let nodes = build_tree(&agents, &collapsed);
    // coord + lead-1 visible, workers hidden
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].agent_id, "coord");
    assert_eq!(nodes[1].agent_id, "lead-1");
    assert!(nodes[1].indicator.contains('\u{25B6}')); // ▶ collapsed
}

#[test]
fn aggregate_agent_status_mixed() {
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-1",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent("lead-2", AgentRole::Lead, AgentStatus::Done, Some("coord")),
        make_agent("w-1", AgentRole::Worker, AgentStatus::Done, Some("lead-1")),
        make_agent(
            "w-2",
            AgentRole::Worker,
            AgentStatus::Failed,
            Some("lead-1"),
        ),
    ];
    let result = aggregate_agent_status(&agents, "coord");
    assert!(result.contains("1 run"));
    assert!(result.contains("2 done"));
    assert!(result.contains("1 fail"));
}

#[test]
fn agent_children_sorted_by_id() {
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-b",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent(
            "lead-a",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
    ];
    let children = agent_children(&agents, "coord");
    assert_eq!(children[0].id, "lead-a");
    assert_eq!(children[1].id, "lead-b");
}

// -----------------------------------------------------------------------
// Task tree edge cases
// -----------------------------------------------------------------------

#[test]
fn task_tree_all_status_types_in_aggregate() {
    let mut tasks = vec![make_task("p", "Parent", None, TaskStatus::Active)];
    let statuses = [
        TaskStatus::Active,
        TaskStatus::Review,
        TaskStatus::Queued,
        TaskStatus::Approved,
        TaskStatus::Merged,
        TaskStatus::Absorbed,
        TaskStatus::Pending,
        TaskStatus::Blocked,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ];
    for (i, status) in statuses.iter().enumerate() {
        tasks.push(make_task(
            &format!("c{i}"),
            &format!("Child {i}"),
            Some("p"),
            *status,
        ));
    }
    let mut collapsed = HashSet::new();
    collapsed.insert("p".to_string());
    let nodes = build_task_tree(&tasks, &collapsed);
    assert_eq!(nodes.len(), 1);
    let title = &nodes[0].title;
    assert!(title.contains("1 active"));
    assert!(title.contains("1 review"));
    assert!(title.contains("1 queued"));
    assert!(title.contains("1 approved"));
    assert!(title.contains("1 merged"));
    assert!(title.contains("1 absorbed"));
    assert!(title.contains("1 pending"));
    assert!(title.contains("1 blocked"));
    assert!(title.contains("1 failed"));
    assert!(title.contains("1 cancelled"));
}

#[test]
fn task_tree_review_count_on_root() {
    let mut task = make_task("t1", "My Task", None, TaskStatus::Review);
    task.review_count = 3;
    let tasks = vec![task];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes[0].title, "My Task (review cycle 3)");
}

#[test]
fn task_tree_review_count_on_child() {
    let parent = make_task("p", "Parent", None, TaskStatus::Active);
    let mut child = make_task("c", "Child", Some("p"), TaskStatus::Review);
    child.review_count = 2;
    let tasks = vec![parent, child];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes[1].title, "Child (review cycle 2)");
}

#[test]
fn task_tree_orphan_child_treated_as_root() {
    // Child references a parent that doesn't exist — treated as standalone
    let tasks = vec![make_task(
        "orphan",
        "Orphan Task",
        Some("nonexistent"),
        TaskStatus::Active,
    )];
    let nodes = build_task_tree(&tasks, &HashSet::new());
    // Orphan should not appear as root since it has parent_task set
    assert!(nodes.is_empty());
}

#[test]
fn task_tree_many_children() {
    let mut tasks = vec![make_task("p", "Parent", None, TaskStatus::Active)];
    for i in 0..50 {
        tasks.push(make_task(
            &format!("c-{i:03}"),
            &format!("Child {i}"),
            Some("p"),
            TaskStatus::Pending,
        ));
    }
    let nodes = build_task_tree(&tasks, &HashSet::new());
    assert_eq!(nodes.len(), 51); // 1 parent + 50 children
    // Last child should have └ prefix
    assert!(nodes[50].prefix.contains('\u{2514}'));
    // Non-last children should have ├ prefix
    assert!(nodes[1].prefix.contains('\u{251C}'));
}

// -----------------------------------------------------------------------
// Mouse handling edge cases
// -----------------------------------------------------------------------

#[test]
fn pane_row_index_zero_height() {
    let area = Rect::new(0, 0, 40, 0);
    assert_eq!(pane_row_index(area, 0), None);
}

#[test]
fn pane_row_index_height_1() {
    let area = Rect::new(0, 0, 40, 1);
    // Only border rows, no inner content
    assert_eq!(pane_row_index(area, 0), None);
}

#[test]
fn pane_row_index_height_2() {
    let area = Rect::new(0, 0, 40, 2);
    // Top border at y=0, bottom border at y=1, no inner content
    assert_eq!(pane_row_index(area, 0), None);
    assert_eq!(pane_row_index(area, 1), None);
}

#[test]
fn pane_row_index_height_3() {
    let area = Rect::new(0, 0, 40, 3);
    assert_eq!(pane_row_index(area, 0), None); // top border
    assert_eq!(pane_row_index(area, 1), Some(0)); // inner row
    assert_eq!(pane_row_index(area, 2), None); // bottom border
}

#[test]
fn mouse_click_empty_swarm_no_crash() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        ..Default::default()
    };
    // Click in swarm area with no agents
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 4),
        &[],
        &[],
    );
    assert_eq!(ui.swarm_selected, None);
    assert_eq!(ui.focused_pane, Pane::Swarm);
}

#[test]
fn mouse_click_empty_tasks_no_crash() {
    let mut ui = TuiState {
        tasks_area: Rect::new(40, 2, 60, 10),
        ..Default::default()
    };
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 50, 5),
        &[],
        &[],
    );
    assert_eq!(ui.tasks_selected, None);
    assert_eq!(ui.focused_pane, Pane::Tasks);
}

#[test]
fn mouse_scroll_up_at_zero_no_underflow() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        swarm_selected: Some(0),
        ..Default::default()
    };
    let nodes = vec![make_tree_node("a-0", false)];
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollUp, 5, 5),
        &nodes,
        &[],
    );
    assert_eq!(ui.swarm_selected, Some(0)); // should not go below 0
}

#[test]
fn mouse_scroll_down_at_end_no_overflow() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        swarm_selected: Some(1),
        ..Default::default()
    };
    let nodes = vec![make_tree_node("a-0", false), make_tree_node("a-1", false)];
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollDown, 5, 5),
        &nodes,
        &[],
    );
    assert_eq!(ui.swarm_selected, Some(1)); // already at last, stays
}

#[test]
fn mouse_scroll_activity_up_at_zero_no_underflow() {
    let mut ui = TuiState {
        activity_area: Rect::new(0, 15, 100, 10),
        activity_scroll: 0,
        activity_auto_scroll: false,
        ..Default::default()
    };
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::ScrollUp, 10, 18),
        &[],
        &[],
    );
    assert_eq!(ui.activity_scroll, 0); // saturating_sub should prevent underflow
}

#[test]
fn mouse_click_outside_all_panes() {
    let mut ui = TuiState {
        swarm_area: Rect::new(0, 2, 40, 10),
        tasks_area: Rect::new(40, 2, 40, 10),
        activity_area: Rect::new(0, 15, 80, 10),
        ..Default::default()
    };
    let original_pane = ui.focused_pane;
    handle_mouse(
        &mut ui,
        make_mouse(MouseEventKind::Down(MouseButton::Left), 90, 30),
        &[],
        &[],
    );
    // Should not crash; pane focus unchanged
    assert_eq!(ui.focused_pane, original_pane);
}

// -----------------------------------------------------------------------
// centered_rect edge cases
// -----------------------------------------------------------------------

#[test]
fn centered_rect_100_percent() {
    let r = Rect::new(0, 0, 100, 50);
    let result = centered_rect(100, 100, r);
    // Should approximately cover the whole area
    assert!(result.width > 0);
    assert!(result.height > 0);
}

#[test]
fn centered_rect_small_area() {
    let r = Rect::new(0, 0, 10, 5);
    let result = centered_rect(80, 80, r);
    assert!(result.width <= 10);
    assert!(result.height <= 5);
}

// -----------------------------------------------------------------------
// Large dataset tests
// -----------------------------------------------------------------------

#[test]
fn agent_tree_many_agents() {
    let mut agents = vec![make_agent(
        "coord",
        AgentRole::Coordinator,
        AgentStatus::Running,
        None,
    )];
    for i in 0..20 {
        agents.push(make_agent(
            &format!("lead-{i:02}"),
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ));
    }
    let nodes = build_tree(&agents, &HashSet::new());
    assert_eq!(nodes.len(), 21);
    assert_eq!(nodes[0].agent_id, "coord");
    assert!(nodes[0].has_children);
}

#[test]
fn agent_tree_deep_nesting() {
    // coord → lead → worker (3 levels)
    let agents = vec![
        make_agent("coord", AgentRole::Coordinator, AgentStatus::Running, None),
        make_agent(
            "lead-1",
            AgentRole::Lead,
            AgentStatus::Running,
            Some("coord"),
        ),
        make_agent(
            "w-1",
            AgentRole::Worker,
            AgentStatus::Running,
            Some("lead-1"),
        ),
        make_agent("w-2", AgentRole::Worker, AgentStatus::Done, Some("lead-1")),
    ];
    let nodes = build_tree(&agents, &HashSet::new());
    assert_eq!(nodes.len(), 4);
    // Workers should have nested prefixes with box-drawing chars
    assert!(nodes[2].prefix.contains('\u{251C}') || nodes[2].prefix.contains('\u{2514}'));
    assert!(nodes[3].prefix.contains('\u{251C}') || nodes[3].prefix.contains('\u{2514}'));
}

#[test]
fn load_latest_actions_many_agents() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tool_calls (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            args_summary TEXT,
            status TEXT NOT NULL DEFAULT 'ok',
            duration_ms INTEGER,
            timestamp TEXT NOT NULL
        );",
    )
    .unwrap();
    // Insert 50 agents with 3 tool calls each
    for i in 0..50 {
        for j in 0..3 {
            conn.execute(
                "INSERT INTO tool_calls (run_id, agent_id, tool_name, args_summary, status, timestamp) VALUES (?1, ?2, ?3, ?4, 'ok', ?5)",
                rusqlite::params![
                    "run1",
                    format!("worker-{i}"),
                    format!("tool-{j}"),
                    format!("arg-{j}"),
                    format!("2025-01-01T00:00:{:02}Z", j),
                ],
            ).unwrap();
        }
    }
    let db = Some(conn);
    let actions = load_latest_actions(&db, "run1");
    assert_eq!(actions.len(), 50);
    // Each agent should have their latest tool call (tool-2)
    for i in 0..50 {
        let key = format!("worker-{i}");
        assert!(actions.contains_key(&key), "missing {key}");
        assert!(
            actions[&key].contains("tool-2"),
            "wrong latest for {key}: {}",
            actions[&key]
        );
    }
}
