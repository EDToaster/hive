use crate::state::HiveState;
use crate::types::Message;

pub fn cmd_messages(agent_filter: Option<String>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;
    let messages = state.list_messages(&run_id)?;

    let filtered: Vec<_> = messages
        .iter()
        .filter(|m| {
            if let Some(ref a) = agent_filter {
                return m.from == *a || m.to == *a;
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        println!("No messages.");
        return Ok(());
    }

    for msg in &filtered {
        println!(
            "[{}] {} -> {} ({:?}): {}",
            msg.timestamp.format("%H:%M:%S"),
            msg.from,
            msg.to,
            msg.message_type,
            msg.body,
        );
    }
    Ok(())
}

pub fn cmd_read_messages(
    agent_id: &str,
    run: Option<String>,
    unread: bool,
    stop_hook: bool,
) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = match run {
        Some(r) => r,
        None => state.active_run_id()?,
    };

    let (messages, _) = read_messages_inner(&state, &run_id, agent_id, unread, stop_hook)?;

    if stop_hook {
        if messages.is_empty() {
            Ok(())
        } else {
            let json = serde_json::to_string_pretty(&messages).map_err(|e| e.to_string())?;
            eprintln!("Unread messages for agent {}:\n{}", agent_id, json);
            std::process::exit(2);
        }
    } else {
        let json = serde_json::to_string_pretty(&messages).map_err(|e| e.to_string())?;
        println!("{}", json);
        Ok(())
    }
}

/// Core logic for reading messages. Returns the messages and whether the cursor was updated.
/// Extracted for testability (cmd_read_messages calls process::exit).
pub fn read_messages_inner(
    state: &HiveState,
    run_id: &str,
    agent_id: &str,
    unread: bool,
    stop_hook: bool,
) -> Result<(Vec<Message>, bool), String> {
    let mut agent = state.load_agent(run_id, agent_id)?;

    let since = if unread {
        // Use max(messages_read_at, last_completed_at) as the "unread" cursor
        match (agent.messages_read_at, agent.last_completed_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    } else {
        None
    };

    let messages = state.load_messages_for_agent(run_id, agent_id, since)?;

    // Advance read cursor when stop_hook delivers messages into the agent's conversation
    let cursor_updated = stop_hook && !messages.is_empty();
    if cursor_updated {
        agent.messages_read_at = Some(chrono::Utc::now());
        state.save_agent(run_id, &agent)?;
    }

    Ok((messages, cursor_updated))
}
