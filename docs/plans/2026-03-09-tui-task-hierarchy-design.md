# TUI Task Hierarchy Design

**Goal:** Replace the flat task table in the TUI with a collapsible tree that shows parent/child relationships, provides at-a-glance domain progress, and keeps the view manageable as task counts grow.

## Problem

The TUI's Tasks pane renders all tasks as a flat table with columns: ID, Status, Assigned, Title. There's no visual distinction between coordinator-created domain tasks and lead-created implementation subtasks, no grouping, and no way to see "is this domain done?" without scanning every row.

### Real-world data (run af5af78f, 10 tasks)

```
task-77cc4261 (D1: Types+State) -- coordinator-created parent
  task-d345f9c0 (types.rs)      -- lead-created, never worked (superseded)
  task-c2095213 (state.rs)      -- lead-created, never worked (superseded)
  task-f55567b3 (combined impl) -- lead-created, actually merged

task-59327398 (D2: MCP Tools) -- coordinator-created parent
  task-d1d70ac5 (impl)         -- merged

task-d7b90b26 (D3: Prompts) -- coordinator-created parent
  task-bf3a761c (impl)         -- merged

task-0290d750 (D4: CLI) -- coordinator-created parent
  task-104b2e65 (impl)         -- merged
```

In the flat view, all 10 tasks looked identical. The two superseded subtasks in D1 were indistinguishable from real work.

## Design: Collapsible Tree, Open by Default

### Visual layout

The collapse indicator and tree connectors go in the first column, before the task ID.

**Expanded (default):**
```
           ID             Status      Assigned             Title
  v task-77cc4261  * merged    --                   D1: Types+State
    +- task-d345f  o pending   --                   Add types to types.rs
    +- task-c2095  o pending   --                   Add state operations
    +- task-f5556  * merged    worker-types-state   Combined impl
  v task-59327398  * merged    --                   D2: MCP Tools
    +- task-d1d70  * merged    worker-mcp-mind      Add Hive Mind MCP tools
  v task-d7b90b26  * merged    --                   D3: Prompts
    +- task-bf3a7  * merged    worker-prompts       Implement prompts
  v task-0290d750  * merged    --                   D4: CLI Commands
    +- task-104b2  * merged    worker-cli-cmds      Add explore + mind CLI
```

Note: `v`/`>` are the Unicode arrows, `*`/`o` are the existing status bullets, `+-` are box-drawing connectors. ASCII used here for portability.

**Collapsed (after pressing Space on D1):**
```
           ID             Status      Assigned             Title
  > task-77cc4261  * merged    --                   D1: Types+State [1/3 merged, 2 pending]
  v task-59327398  * merged    --                   D2: MCP Tools
    +- task-d1d70  * merged    worker-mcp-mind      Add Hive Mind MCP tools
```

### Interaction

- **Space** toggles collapse/expand on the selected task (only tasks with children)
- **Enter** still opens the detail overlay (unchanged)
- **j/k** navigation skips hidden children when a parent is collapsed
- Leaf tasks and childless root tasks: Space is a no-op

### Aggregate status for collapsed parents

When collapsed, append a child status summary to the title: `[N merged, M pending, ...]`. Count children by status, only show non-zero counts. Order: active, review, queued, pending, merged, failed.

## Data Model

No changes. The `Task` struct already has `parent_task: Option<String>` which is populated by leads when creating subtasks. All hierarchy information is already in the data.

## TUI State Changes

Add one field to `TuiState`:

```rust
collapsed_tasks: HashSet<String>,  // task IDs whose children are hidden
```

Initialized empty (all expanded by default).

## Rendering Changes

### render_tasks_pane

Replace the flat `Vec<Row>` construction with a tree-building step:

1. **Partition** tasks into roots (`parent_task.is_none()`) and children (grouped by `parent_task`).
2. **Sort** roots by `created_at`. Sort children within each group by `created_at`.
3. **Flatten with prefixes** -- Walk the tree depth-first. For each task, compute a prefix string:
   - Root tasks with children: `"v "` or `"> "` (expanded/collapsed indicator)
   - Root tasks without children: `"  "` (padding for alignment)
   - Child tasks: `"  +- "` / `"  +- "` (box-drawing connectors, same logic as swarm pane's `add_children`)
4. **Skip children** of collapsed parents.
5. **Aggregate status** -- For collapsed parents, count children by status and append `[N status, M status, ...]` to the title cell.
6. **Render** using the same `Table` widget with the same columns. The prefix goes in the ID cell, before the task ID.

### Input handling

In the `KeyCode::Char(' ')` arm (new), when focused on Tasks pane:
- Get selected task ID
- If the task has children: toggle its presence in `collapsed_tasks`
- If no children: no-op

In `j/k` navigation for Tasks pane:
- Build a `visible_indices` list that excludes children of collapsed parents
- Navigate within that list instead of the raw task list

## What Doesn't Change

- Task overlay (`render_task_overlay`) -- unchanged, still shows full detail including parent_task, blocked_by, etc.
- Stats bar -- unchanged, counts all tasks regardless of hierarchy
- Activity stream -- unchanged
- Swarm pane -- unchanged
- No new types, no state.rs changes, no MCP changes

## Implementation Notes

- The tree-building logic can be extracted from the existing swarm pane's `build_tree`/`add_children` pattern, adapted for tasks instead of agents.
- Task hierarchy is only one level deep in practice (coordinator creates parents, leads create children). The implementation should support arbitrary depth but optimize for the common case.
- The `tasks_selected` index in TuiState should reference the visible (post-filtering) list, not the raw task list, so that j/k navigation works correctly with collapsed parents.
