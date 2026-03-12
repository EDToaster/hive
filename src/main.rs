mod agent;
mod cli;
#[allow(dead_code)]
mod git;
mod logging;
mod mcp;
mod output;
#[allow(dead_code)]
mod state;
mod tui;
mod types;
mod wait;

use clap::Parser;
use cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cli::cmd_init(),
        Commands::Start { spec, goal } => cli::cmd_start(spec, goal),
        Commands::Status => cli::cmd_status(),
        Commands::Agents => cli::cmd_agents(),
        Commands::Tasks { status, assignee } => cli::cmd_tasks(status, assignee),
        Commands::Messages { agent } => cli::cmd_messages(agent),
        Commands::LogTool {
            run,
            agent,
            tool,
            status,
            duration,
            args_summary,
        } => cli::cmd_log_tool(
            &run,
            &agent,
            &tool,
            &status,
            duration,
            args_summary.as_deref(),
        ),
        Commands::Heartbeat { run, agent } => cli::cmd_heartbeat(&run, &agent),
        Commands::Logs { agent } => cli::cmd_logs(agent),
        Commands::Tui => cli::cmd_tui(),
        Commands::Mcp { run, agent } => cli::cmd_mcp(&run, &agent),
        Commands::Wait { run, timeout } => cli::cmd_wait(run, timeout),
        Commands::ReviewAgent { agent_id, run } => cli::cmd_review_agent(&agent_id, run),
        Commands::ReadMessages {
            agent,
            run,
            unread,
            stop_hook,
        } => cli::cmd_read_messages(&agent, run, unread, stop_hook),
        Commands::Summary { run } => cli::cmd_summary(run),
        Commands::Cost { run } => cli::cmd_cost(run),
        Commands::History => cli::cmd_history(),
        Commands::Memory { command } => cli::cmd_memory(command),
        Commands::Explore { intent } => cli::cmd_explore(&intent),
        Commands::Mind { command } => cli::cmd_mind(command),
        Commands::AgentExit { run, agent } => cli::cmd_agent_exit(&run, &agent),
        Commands::Config => cli::cmd_config(),
        Commands::Stop => cli::cmd_stop(),
        Commands::Watch { interval } => cli::cmd_watch(interval),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
