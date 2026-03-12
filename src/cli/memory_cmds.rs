use crate::state::HiveState;
use super::MemoryCommands;

pub fn cmd_memory(command: Option<MemoryCommands>) -> Result<(), String> {
    let state = HiveState::discover()?;
    match command {
        None => {
            let ops = state.load_operations();
            let conventions = state.load_conventions();
            let failures = state.load_failures();
            let conv_lines = conventions.lines().filter(|l| !l.trim().is_empty()).count();
            println!("Memory:");
            println!("  Operations: {} entries", ops.len());
            println!("  Conventions: {} lines", conv_lines);
            println!("  Failures: {} entries", failures.len());
            Ok(())
        }
        Some(MemoryCommands::Show) => {
            let ops = state.load_operations();
            let conventions = state.load_conventions();
            let failures = state.load_failures();

            println!("=== Operations ({}) ===", ops.len());
            for op in &ops {
                println!("{}", serde_json::to_string_pretty(op).unwrap_or_default());
            }

            println!("\n=== Conventions ===");
            if conventions.is_empty() {
                println!("(none)");
            } else {
                println!("{}", conventions);
            }

            println!("\n=== Failures ({}) ===", failures.len());
            for f in &failures {
                println!("{}", serde_json::to_string_pretty(f).unwrap_or_default());
            }
            Ok(())
        }
        Some(MemoryCommands::Prune) => {
            state.prune_memory()?;
            println!("Memory pruned.");
            Ok(())
        }
    }
}

pub fn cmd_mind(command: Option<super::MindCommands>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    match command {
        None => {
            let discoveries = state.load_discoveries(&run_id);
            let insights = state.load_insights(&run_id);
            println!("Hive Mind for run {run_id}:");
            println!("  Discoveries: {}", discoveries.len());
            println!("  Insights: {}", insights.len());

            if !discoveries.is_empty() {
                println!("\n--- Recent Discoveries ---");
                for disc in discoveries.iter().rev().take(5).rev() {
                    let content_preview = if disc.content.len() > 80 {
                        &disc.content[..80]
                    } else {
                        &disc.content
                    };
                    println!(
                        "  {} [{}] by {} ({:?}): {}",
                        disc.id,
                        disc.tags.join(", "),
                        disc.agent_id,
                        disc.confidence,
                        content_preview
                    );
                }
            }

            if !insights.is_empty() {
                println!("\n--- Insights ---");
                for ins in &insights {
                    println!("  {} [{}]: {}", ins.id, ins.tags.join(", "), ins.content);
                    println!("    Based on: {}", ins.discovery_ids.join(", "));
                }
            }

            Ok(())
        }
        Some(super::MindCommands::Query { query }) => {
            let result = state.query_mind(&run_id, &query);
            println!("Hive Mind query: \"{}\"", query);
            println!(
                "Found {} discoveries, {} insights",
                result.discoveries.len(),
                result.insights.len()
            );

            if !result.discoveries.is_empty() {
                println!("\n--- Matching Discoveries ---");
                for disc in &result.discoveries {
                    println!(
                        "  {} [{}] by {} ({:?})",
                        disc.id,
                        disc.tags.join(", "),
                        disc.agent_id,
                        disc.confidence
                    );
                    println!("    {}", disc.content);
                    if !disc.file_paths.is_empty() {
                        println!("    Files: {}", disc.file_paths.join(", "));
                    }
                }
            }

            if !result.insights.is_empty() {
                println!("\n--- Matching Insights ---");
                for ins in &result.insights {
                    println!("  {} [{}]: {}", ins.id, ins.tags.join(", "), ins.content);
                    println!("    Based on: {}", ins.discovery_ids.join(", "));
                }
            }

            Ok(())
        }
    }
}
