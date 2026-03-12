use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::Instant;

use super::tree::{TaskTreeNode, TreeNode};
use super::{pane_row_index, Overlay, Pane, TuiState, DOUBLE_CLICK_MS};

pub(super) fn handle_mouse(
    ui: &mut TuiState,
    mouse: MouseEvent,
    tree_nodes: &[TreeNode],
    task_tree_nodes: &[TaskTreeNode],
) {
    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        // --- Scroll wheel: target pane under cursor ---
        MouseEventKind::ScrollDown => {
            if ui.overlay.is_some() && ui.overlay_area.contains((col, row).into()) {
                // Scroll down in overlay (toward bottom)
                if ui.output_scroll > 0 {
                    ui.output_auto_scroll = false;
                    ui.output_scroll = ui.output_scroll.saturating_sub(3);
                    if ui.output_scroll == 0 {
                        ui.output_auto_scroll = true;
                    }
                }
            } else if ui.swarm_area.contains((col, row).into()) {
                let max = tree_nodes.len().saturating_sub(1);
                let next = ui.swarm_selected.map_or(0, |i| (i + 1).min(max));
                ui.swarm_selected = Some(next);
                ui.selected_agent_filter = tree_nodes.get(next).map(|n| n.agent_id.clone());
            } else if ui.spec_area.width > 0 && ui.spec_area.contains((col, row).into()) {
                ui.spec_scroll = ui.spec_scroll.saturating_add(3);
            } else if ui.tasks_area.contains((col, row).into()) {
                let max = task_tree_nodes.len().saturating_sub(1);
                let next = ui.tasks_selected.map_or(0, |i| (i + 1).min(max));
                ui.tasks_selected = Some(next);
            } else if ui.activity_area.contains((col, row).into()) {
                ui.activity_auto_scroll = false;
                ui.activity_scroll = ui.activity_scroll.saturating_add(3);
            }
        }
        MouseEventKind::ScrollUp => {
            if ui.overlay.is_some() && ui.overlay_area.contains((col, row).into()) {
                ui.output_auto_scroll = false;
                ui.output_scroll = ui.output_scroll.saturating_add(3);
            } else if ui.swarm_area.contains((col, row).into()) {
                if let Some(i) = ui.swarm_selected {
                    let next = i.saturating_sub(1);
                    ui.swarm_selected = Some(next);
                    ui.selected_agent_filter = tree_nodes.get(next).map(|n| n.agent_id.clone());
                }
            } else if ui.spec_area.width > 0 && ui.spec_area.contains((col, row).into()) {
                ui.spec_scroll = ui.spec_scroll.saturating_sub(3);
            } else if ui.tasks_area.contains((col, row).into()) {
                if let Some(i) = ui.tasks_selected {
                    ui.tasks_selected = Some(i.saturating_sub(1));
                }
            } else if ui.activity_area.contains((col, row).into()) {
                ui.activity_auto_scroll = false;
                ui.activity_scroll = ui.activity_scroll.saturating_sub(3);
            }
        }

        // --- Left click ---
        MouseEventKind::Down(MouseButton::Left) => {
            let now = Instant::now();

            // Click outside overlay dismisses it
            if ui.overlay.is_some() {
                if !ui.overlay_area.contains((col, row).into()) {
                    ui.overlay = None;
                    ui.last_click = None;
                    return;
                }
                // Clicks inside overlay are consumed
                return;
            }

            // Detect double-click
            let is_double = ui.last_click.is_some_and(|(lc, lr, lt)| {
                lc == col && lr == row && now.duration_since(lt).as_millis() < DOUBLE_CLICK_MS
            });

            // Click in Swarm pane
            if ui.swarm_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Swarm;
                if let Some(idx) = pane_row_index(ui.swarm_area, row)
                    && idx < tree_nodes.len()
                {
                    ui.swarm_selected = Some(idx);
                    ui.selected_agent_filter = tree_nodes.get(idx).map(|n| n.agent_id.clone());

                    let node = &tree_nodes[idx];

                    // Check if click is on the collapse/expand toggle indicator
                    if node.has_children {
                        let toggle_col_start = ui.swarm_area.x + 1 + node.prefix.len() as u16;
                        let toggle_col_end = toggle_col_start + 2;
                        if col >= toggle_col_start && col < toggle_col_end {
                            if ui.collapsed_agents.contains(&node.agent_id) {
                                ui.collapsed_agents.remove(&node.agent_id);
                            } else {
                                ui.collapsed_agents.insert(node.agent_id.clone());
                            }
                        }
                    }

                    // Double-click opens detail overlay
                    if is_double {
                        ui.overlay = Some(Overlay::Agent(node.agent_id.clone()));
                    }
                }
            }
            // Click in Tasks pane (extra -1 for table header row)
            else if ui.tasks_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Tasks;
                if let Some(raw) = pane_row_index(ui.tasks_area, row)
                    && raw > 0
                    && (raw - 1) < task_tree_nodes.len()
                {
                    let idx = raw - 1;
                    ui.tasks_selected = Some(idx);

                    let node = &task_tree_nodes[idx];

                    // Check if click is on the collapse/expand toggle indicator
                    if node.has_children {
                        let toggle_col_start = ui.tasks_area.x + 1 + node.prefix.len() as u16;
                        let toggle_col_end = toggle_col_start + 2;
                        if col >= toggle_col_start && col < toggle_col_end {
                            if ui.collapsed_tasks.contains(&node.task_id) {
                                ui.collapsed_tasks.remove(&node.task_id);
                            } else {
                                ui.collapsed_tasks.insert(node.task_id.clone());
                            }
                        }
                    }

                    // Double-click opens task detail overlay
                    if is_double {
                        ui.overlay = Some(Overlay::Task(node.task_id.clone()));
                    }
                }
            }
            // Click in Activity pane
            else if ui.activity_area.contains((col, row).into()) {
                ui.focused_pane = Pane::Activity;
            }

            // Record click for double-click detection
            ui.last_click = Some((col, row, now));
        }

        // Ignore move, drag, and other mouse events for performance
        _ => {}
    }
}
