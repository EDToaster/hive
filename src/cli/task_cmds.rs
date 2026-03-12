use crate::state::HiveState;

pub fn cmd_tasks(status_filter: Option<String>, assignee_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let tasks = state.list_tasks(&run_id)?;

    let filtered: Vec<_> = tasks
        .iter()
        .filter(|t| {
            if let Some(ref s) = status_filter {
                let status_str = serde_json::to_value(t.status)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                if status_str.as_deref() != Some(s.as_str()) {
                    return false;
                }
            }
            if let Some(ref a) = assignee_filter
                && t.assigned_to.as_deref() != Some(a.as_str())
            {
                return false;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No tasks match filters.");
        return Ok(());
    }

    for task in &filtered {
        println!(
            "{:<12} {:?} [{:?}] assigned={} - {}",
            task.id,
            task.status,
            task.urgency,
            task.assigned_to.as_deref().unwrap_or("-"),
            task.title,
        );
    }
    Ok(())
}
