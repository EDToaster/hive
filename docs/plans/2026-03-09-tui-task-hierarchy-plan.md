# TUI Task Hierarchy Implementation Plan
**Goal:** Replace the flat task table in the TUI with a collapsible tree showing parent/child task relationships.

**Architecture:** Add a `TaskTreeNode` struct and `build_task_tree` function (mirroring the existing agent tree pattern), a `collapsed_tasks: HashSet<String>` in `TuiState`, and rewrite `render_tasks_pane` to render from the flattened tree. Navigation (`j/k`, `Enter`, `Space`) operates on visible nodes only.

**Tech Stack:** Rust, ratatui (TUI framework), existing `Task` type with `parent_task: Option<String>`.

**Design doc:** `docs/plans/2026-03-09-tui-task-hierarchy-design.md`

---

### Task 1: Add TaskTreeNode struct and collapsed_tasks state

**Files:**
- Modify: `src/tui.rs:32-53` (TuiState)
- Modify: `src/tui.rs:1` (imports)

**Step 1: Add HashSet import**

At line 1, add `use std::collections::HashSet;` alongside the existing imports. Place it after line 13 (`use std::time::{Duration, Instant};`):

```rust
use std::collections::HashSet;
```

**Step 2: Add TaskTreeNode struct**

After the `TreeNode` struct block (after line 103), add:

```rust
// ---------------------------------------------------------------------------
// Flattened tree node (for tasks pane)
// ---------------------------------------------------------------------------

struct TaskTreeNode {
    task_id: String,
    prefix: String,        // box-drawing connector prefix
    indicator: String,     // "▼ " / "▶ " / "  " (collapse indicator)
    title: String,         // task title, with aggregate suffix when collapsed
    status: TaskStatus,
    assigned_to: Option<String>,
    review_count: u32,
    has_children: bool,
}
```

**Step 3: Add collapsed_tasks to TuiState**

Add field to `TuiState` struct (after line 39):

```rust
collapsed_tasks: HashSet<String>,
```

Add to `Default` impl (after line 51):

```rust
collapsed_tasks: HashSet::new(),
```

**Step 4: Verify it compiles**

Run: `cargo build 2>&1 | head -20`
Expected: compiles (TaskTreeNode is unused for now, that's fine)

**Step 5: Commit**

```
feat(tui): add TaskTreeNode struct and collapsed_tasks state
```

---

### Task 2: Implement build_task_tree function

**Files:**
- Modify: `src/tui.rs` (after the TaskTreeNode struct)

**Step 1: Write the test**

Add to the bottom of `src/tui.rs`, before the closing of the file. Create a test module if one doesn't exist:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
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
            make_task("child-2", "Second Child", Some("parent"), TaskStatus::Pending),
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
            make_task("child-2", "Second Child", Some("parent"), TaskStatus::Pending),
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
        let tasks = vec![
            make_task("lone", "Lone Task", None, TaskStatus::Active),
        ];
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
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --all-targets 2>&1 | grep "task_tree"`
Expected: FAIL — `build_task_tree` not found

**Step 3: Implement build_task_tree**

Add after the `TaskTreeNode` struct:

```rust
fn build_task_tree(tasks: &[Task], collapsed: &HashSet<String>) -> Vec<TaskTreeNode> {
    let mut nodes = Vec::new();

    // Partition into roots and children
    let roots: Vec<&Task> = tasks.iter().filter(|t| t.parent_task.is_none()).collect();
    let mut children_map: std::collections::HashMap<&str, Vec<&Task>> =
        std::collections::HashMap::new();
    for task in tasks {
        if let Some(ref parent_id) = task.parent_task {
            children_map
                .entry(parent_id.as_str())
                .or_default()
                .push(task);
        }
    }

    // Sort children by created_at within each group
    for children in children_map.values_mut() {
        children.sort_by_key(|t| t.created_at);
    }

    // Roots are already sorted by created_at from state.list_tasks
    for root in &roots {
        let children = children_map.get(root.id.as_str());
        let has_children = children.is_some_and(|c| !c.is_empty());
        let is_collapsed = collapsed.contains(&root.id);

        let indicator = if !has_children {
            "  ".to_string()
        } else if is_collapsed {
            "\u{25B6} ".to_string() // ▶
        } else {
            "\u{25BC} ".to_string() // ▼
        };

        let title = if is_collapsed && has_children {
            let aggregate = aggregate_child_status(children.unwrap());
            format!("{} [{}]", root.title, aggregate)
        } else if root.review_count > 0 {
            format!("{} (review cycle {})", root.title, root.review_count)
        } else {
            root.title.clone()
        };

        nodes.push(TaskTreeNode {
            task_id: root.id.clone(),
            prefix: String::new(),
            indicator,
            title,
            status: root.status,
            assigned_to: root.assigned_to.clone(),
            review_count: root.review_count,
            has_children,
        });

        // Add children if expanded
        if has_children && !is_collapsed {
            let kids = children.unwrap();
            for (i, child) in kids.iter().enumerate() {
                let is_last = i == kids.len() - 1;
                let connector = if is_last {
                    "  \u{2514}\u{2500} " // └─
                } else {
                    "  \u{251C}\u{2500} " // ├─
                };

                let child_title = if child.review_count > 0 {
                    format!("{} (review cycle {})", child.title, child.review_count)
                } else {
                    child.title.clone()
                };

                nodes.push(TaskTreeNode {
                    task_id: child.id.clone(),
                    prefix: connector.to_string(),
                    indicator: String::new(),
                    title: child_title,
                    status: child.status,
                    assigned_to: child.assigned_to.clone(),
                    review_count: child.review_count,
                    has_children: false,
                });
            }
        }
    }

    nodes
}

fn aggregate_child_status(children: &[&Task]) -> String {
    // Order: active, review, queued, pending, merged, failed
    let counts: Vec<(TaskStatus, &str)> = vec![
        (TaskStatus::Active, "active"),
        (TaskStatus::Review, "review"),
        (TaskStatus::Queued, "queued"),
        (TaskStatus::Pending, "pending"),
        (TaskStatus::Merged, "merged"),
        (TaskStatus::Approved, "approved"),
        (TaskStatus::Blocked, "blocked"),
        (TaskStatus::Failed, "failed"),
    ];

    let mut parts = Vec::new();
    for (status, label) in &counts {
        let n = children.iter().filter(|t| t.status == *status).count();
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    parts.join(", ")
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --all-targets 2>&1 | grep -E "(task_tree|test result)"`
Expected: all 5 task_tree tests pass

**Step 5: Commit**

```
feat(tui): implement build_task_tree with collapse and aggregate status
```

---

### Task 3: Rewrite render_tasks_pane to use task tree

**Files:**
- Modify: `src/tui.rs:794-848` (render_tasks_pane)

**Step 1: Write the test**

Add to the test module:

```rust
#[test]
fn task_tree_real_world_run_af5af78f() {
    // Reproduce the actual task hierarchy from run af5af78f
    let tasks = vec![
        make_task("task-77cc4261", "D1: Types+State", None, TaskStatus::Merged),
        make_task("task-d345f9c0", "Add types to types.rs", Some("task-77cc4261"), TaskStatus::Pending),
        make_task("task-c2095213", "Add state operations", Some("task-77cc4261"), TaskStatus::Pending),
        make_task("task-f55567b3", "Combined impl", Some("task-77cc4261"), TaskStatus::Merged),
        make_task("task-59327398", "D2: MCP Tools", None, TaskStatus::Merged),
        make_task("task-d1d70ac5", "Add Hive Mind MCP tools", Some("task-59327398"), TaskStatus::Merged),
        make_task("task-d7b90b26", "D3: Prompts", None, TaskStatus::Merged),
        make_task("task-bf3a761c", "Implement prompts", Some("task-d7b90b26"), TaskStatus::Merged),
        make_task("task-0290d750", "D4: CLI Commands", None, TaskStatus::Merged),
        make_task("task-104b2e65", "Add explore + mind CLI", Some("task-0290d750"), TaskStatus::Merged),
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
```

**Step 2: Run test to verify it passes** (build_task_tree already implemented)

Run: `cargo test task_tree_real_world 2>&1`
Expected: PASS

**Step 3: Update render_tasks_pane signature and body**

Replace `render_tasks_pane` (lines 794-848) with:

```rust
fn render_tasks_pane(frame: &mut Frame, area: Rect, tasks: &[Task], ui: &TuiState) {
    let tree_nodes = build_task_tree(tasks, &ui.collapsed_tasks);

    let rows: Vec<Row> = tree_nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let stripe = if i % 2 == 0 {
                Style::default().bg(Color::Rgb(45, 45, 55))
            } else {
                Style::default()
            };

            let id_cell = format!("{}{}{}", node.indicator, node.prefix, node.task_id);
            let assigned = node.assigned_to.as_deref().unwrap_or("--");

            Row::new(vec![
                Cell::from(id_cell),
                Cell::from(Span::styled(
                    task_status_bullet(node.status),
                    Style::default().fg(task_status_color(node.status)),
                )),
                Cell::from(assigned.to_string()),
                Cell::from(node.title.clone()),
            ])
            .style(stripe)
        })
        .collect();

    let widths = [
        Constraint::Min(18), // wider to fit indicator + prefix + ID
        Constraint::Min(12),
        Constraint::Min(10),
        Constraint::Min(20),
    ];

    let bc = border_color(ui.focused_pane, Pane::Tasks);
    let block = Block::default()
        .title(" Tasks ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(bc));

    let mut table_state = TableState::default();
    table_state.select(ui.tasks_selected);

    let table = Table::new(rows, widths)
        .block(block)
        .header(
            Row::new(vec!["ID", "Status", "Assigned", "Title"])
                .bold()
                .style(Style::default().fg(Color::White)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(55, 55, 90)));

    frame.render_stateful_widget(table, area, &mut table_state);
}
```

**Step 4: Verify it compiles and all tests pass**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: all pass, no warnings

**Step 5: Commit**

```
feat(tui): render task tree with hierarchy and collapse indicators
```

---

### Task 4: Add Space toggle and fix j/k navigation for visible nodes

**Files:**
- Modify: `src/tui.rs:488-571` (input handling in run_tui_loop)
- Modify: `src/tui.rs:392-401` (data loading section, build task tree earlier)

**Step 1: Write the test**

Add to test module:

```rust
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
```

**Step 2: Run test to verify it passes**

Run: `cargo test task_tree_navigation 2>&1`
Expected: PASS (build_task_tree already handles this)

**Step 3: Build task tree alongside agents in run_tui_loop**

In the data loading section (around line 401, after `let tree_nodes = build_tree(&agents);`), add:

```rust
let task_tree_nodes = build_task_tree(&tasks, &ui.collapsed_tasks);
let task_tree_len = task_tree_nodes.len();
```

**Step 4: Update j/k navigation for Tasks pane**

Replace the Tasks arm in `KeyCode::Char('j') | KeyCode::Down` (lines 520-523):

```rust
Pane::Tasks => {
    let max = task_tree_len.saturating_sub(1);
    let next = ui.tasks_selected.map_or(0, |i| (i + 1).min(max));
    ui.tasks_selected = Some(next);
}
```

The `KeyCode::Char('k') | KeyCode::Up` Tasks arm (lines 539-542) stays the same — it already uses `saturating_sub(1)`.

**Step 5: Update Enter for Tasks pane to use tree nodes**

Replace the Tasks arm in `KeyCode::Enter` (lines 560-565):

```rust
Pane::Tasks => {
    if let Some(i) = ui.tasks_selected
        && let Some(node) = task_tree_nodes.get(i)
    {
        ui.overlay = Some(Overlay::Task(node.task_id.clone()));
    }
}
```

**Step 6: Add Space handler**

After the `KeyCode::Char('G')` arm (line 551), add:

```rust
KeyCode::Char(' ') => {
    if ui.focused_pane == Pane::Tasks {
        if let Some(i) = ui.tasks_selected
            && let Some(node) = task_tree_nodes.get(i)
            && node.has_children
        {
            if ui.collapsed_tasks.contains(&node.task_id) {
                ui.collapsed_tasks.remove(&node.task_id);
            } else {
                ui.collapsed_tasks.insert(node.task_id.clone());
            }
        }
    }
}
```

**Step 7: Verify everything compiles and tests pass**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`
Expected: all pass

**Step 8: Commit**

```
feat(tui): add Space to toggle task collapse, fix navigation for tree
```

---

### Task 5: Final verification and formatting

**Files:**
- All changes in `src/tui.rs`

**Step 1: Run full test suite**

Run: `cargo test --all-targets`
Expected: all tests pass (190 existing + 6 new = 196)

**Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings

**Step 3: Run fmt**

Run: `cargo fmt --all`

**Step 4: Commit if fmt changed anything**

```
fmt
```
