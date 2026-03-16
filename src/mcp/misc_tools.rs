use super::HiveMcp;
use super::params::SaveMemoryParams;
use crate::types::{AgentRole, FailureEntry, OperationalEntry};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = misc_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(
        description = "Get cost summary for the current run — token usage and estimated cost per agent"
    )]
    pub(crate) async fn hive_run_cost(&self) -> Result<CallToolResult, McpError> {
        let state = self.state();
        let agents = match state.list_agents(&self.run_id) {
            Ok(a) => a,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let mut lines = Vec::new();
        let mut total_cost = 0.0;
        let mut total_input = 0u64;
        let mut total_output = 0u64;

        lines.push(format!(
            "{:<24} {:<14} {:>10} {:>10} {:>10}",
            "Agent", "Role", "Input", "Output", "Cost"
        ));
        lines.push("-".repeat(72));

        for agent in &agents {
            if let Some(cost) = state.load_agent_cost(&self.run_id, &agent.id) {
                total_cost += cost.cost_usd;
                total_input += cost.input_tokens;
                total_output += cost.output_tokens;
                lines.push(format!(
                    "{:<24} {:<14} {:>9.1}k {:>9.1}k {:>9}",
                    agent.id,
                    format!("{:?}", agent.role).to_lowercase(),
                    cost.input_tokens as f64 / 1000.0,
                    cost.output_tokens as f64 / 1000.0,
                    format!("${:.2}", cost.cost_usd),
                ));
            }
        }

        if total_cost == 0.0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No cost data available (agents still running?).",
            )]));
        }

        lines.push("-".repeat(72));
        lines.push(format!(
            "{:<24} {:<14} {:>9.1}k {:>9.1}k {:>9}",
            "TOTAL",
            "",
            total_input as f64 / 1000.0,
            total_output as f64 / 1000.0,
            format!("${:.2}", total_cost),
        ));

        // Add budget info if configured
        let config = state.load_config();
        if let Some(budget) = config.budget_usd {
            let pct = (total_cost / budget * 100.0).min(999.9);
            lines.push(format!("\nBudget: ${:.2} ({:.1}% used)", budget, pct));
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    #[tool(
        description = "Save a memory entry (operation, convention, or failure). Postmortem-only."
    )]
    pub(crate) async fn hive_save_memory(
        &self,
        params: Parameters<SaveMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Postmortem]) {
            return Ok(result);
        }
        let p = &params.0;
        let state = self.state();
        match p.memory_type.as_str() {
            "operation" => {
                let entry: OperationalEntry = serde_json::from_str(&p.content).map_err(|e| {
                    McpError::invalid_params(format!("Invalid operation JSON: {e}"), None)
                })?;
                state
                    .save_operation(&entry)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved operation entry.",
                )]))
            }
            "convention" => {
                state
                    .save_conventions(&p.content)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved conventions.",
                )]))
            }
            "failure" => {
                let entry: FailureEntry = serde_json::from_str(&p.content).map_err(|e| {
                    McpError::invalid_params(format!("Invalid failure JSON: {e}"), None)
                })?;
                state
                    .save_failure(&entry)
                    .map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text(
                    "Saved failure entry.",
                )]))
            }
            _ => Ok(CallToolResult::error(vec![Content::text(
                "Invalid memory_type. Use 'operation', 'convention', or 'failure'.",
            )])),
        }
    }
}
