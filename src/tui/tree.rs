use crate::types::*;
use chrono::{DateTime, Utc};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Flattened tree node (for swarm pane)
// ---------------------------------------------------------------------------

pub(super) struct TreeNode {
    pub agent_id: String,
    pub prefix: String,
    pub status: AgentStatus,
    pub task_id: Option<String>,
    pub heartbeat: Option<DateTime<Utc>>,
    pub role: AgentRole,
    pub has_children: bool,
    pub indicator: String,
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// Flattened tree node (for tasks pane)
// ---------------------------------------------------------------------------

pub(super) struct TaskTreeNode {
    pub task_id: String,
    pub prefix: String,    // box-drawing connector prefix
    pub indicator: String, // "▼ " / "▶ " / "  " (collapse indicator)
    pub title: String,     // task title, with aggregate suffix when collapsed
    pub status: TaskStatus,
    pub assigned_to: Option<String>,
    #[allow(dead_code)] // read in tests; will be used by Space-toggle (Task 4)
    pub review_count: u32,
    pub has_children: bool,
}

pub(super) fn build_task_tree(tasks: &[Task], collapsed: &HashSet<String>) -> Vec<TaskTreeNode> {
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

pub(super) fn aggregate_child_status(children: &[&Task]) -> String {
    let counts: Vec<(TaskStatus, &str)> = vec![
        (TaskStatus::Active, "active"),
        (TaskStatus::Review, "review"),
        (TaskStatus::Queued, "queued"),
        (TaskStatus::Approved, "approved"),
        (TaskStatus::Merged, "merged"),
        (TaskStatus::Absorbed, "absorbed"),
        (TaskStatus::Pending, "pending"),
        (TaskStatus::Blocked, "blocked"),
        (TaskStatus::Failed, "failed"),
        (TaskStatus::Cancelled, "cancelled"),
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

pub(super) fn build_tree(agents: &[Agent], collapsed: &HashSet<String>) -> Vec<TreeNode> {
    let mut nodes = Vec::new();
    let mut roots: Vec<&Agent> = agents.iter().filter(|a| a.parent.is_none()).collect();
    roots.sort_by(|a, b| {
        let ord_a = if a.role == AgentRole::Coordinator {
            0
        } else {
            1
        };
        let ord_b = if b.role == AgentRole::Coordinator {
            0
        } else {
            1
        };
        ord_a.cmp(&ord_b).then(a.id.cmp(&b.id))
    });
    for root in &roots {
        add_agent_node(&mut nodes, agents, root, "", "", collapsed);
    }
    nodes
}

pub(super) fn agent_children<'a>(agents: &'a [Agent], agent_id: &str) -> Vec<&'a Agent> {
    let mut children: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(agent_id))
        .collect();
    children.sort_by(|a, b| a.id.cmp(&b.id));
    children
}

pub(super) fn aggregate_agent_status(agents: &[Agent], agent_id: &str) -> String {
    // Recursively collect all descendants
    let mut descendants = Vec::new();
    let mut frontier = vec![agent_id];
    while let Some(id) = frontier.pop() {
        for a in agents {
            if a.parent.as_deref() == Some(id) {
                descendants.push(a);
                frontier.push(&a.id);
            }
        }
    }
    let statuses = [
        (AgentStatus::Running, "run"),
        (AgentStatus::Idle, "idle"),
        (AgentStatus::Done, "done"),
        (AgentStatus::Failed, "fail"),
        (AgentStatus::Stalled, "stal"),
    ];
    let mut parts = Vec::new();
    for (status, label) in &statuses {
        let n = descendants.iter().filter(|a| a.status == *status).count();
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    parts.join(", ")
}

fn add_agent_node(
    nodes: &mut Vec<TreeNode>,
    agents: &[Agent],
    agent: &Agent,
    this_prefix: &str,
    child_prefix: &str,
    collapsed: &HashSet<String>,
) {
    let children = agent_children(agents, &agent.id);
    let has_children = !children.is_empty();
    let is_collapsed = collapsed.contains(&agent.id);

    let indicator = if !has_children {
        String::new()
    } else if is_collapsed {
        "\u{25B6} ".to_string() // ▶
    } else {
        "\u{25BC} ".to_string() // ▼
    };

    nodes.push(TreeNode {
        agent_id: agent.id.clone(),
        prefix: this_prefix.to_string(),
        status: agent.status,
        task_id: agent.task_id.clone(),
        heartbeat: agent.heartbeat,
        role: agent.role,
        has_children,
        indicator,
        model: agent.model.clone(),
    });

    if !is_collapsed {
        for (i, child) in children.iter().enumerate() {
            let is_last = i == children.len() - 1;
            let connector = if is_last {
                "\u{2514}\u{2500} "
            } else {
                "\u{251C}\u{2500} "
            };
            let this_pref = format!("{child_prefix}{connector}");
            let continuation = if is_last { "   " } else { "\u{2502}  " };
            let next_pref = format!("{child_prefix}{continuation}");
            add_agent_node(nodes, agents, child, &this_pref, &next_pref, collapsed);
        }
    }
}
