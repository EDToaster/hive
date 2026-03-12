use super::HiveMcp;
use super::params::{DiscoverParams, EstablishConventionParams, QueryMindParams, SynthesizeParams};
use crate::types::{AgentRole, Confidence, Discovery, Insight};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData as McpError, tool, tool_router};

#[tool_router(router = mind_tool_router, vis = "pub(crate)")]
impl HiveMcp {
    #[tool(
        description = "Record a discovery to the Hive Mind knowledge space. Any agent can call this."
    )]
    pub(crate) async fn hive_discover(
        &self,
        params: Parameters<DiscoverParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let id = format!("disc-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let confidence = match p.confidence.as_str() {
            "low" => Confidence::Low,
            "high" => Confidence::High,
            _ => Confidence::Medium,
        };
        let discovery = Discovery {
            id: id.clone(),
            run_id: self.run_id.clone(),
            agent_id: self.agent_id.clone(),
            timestamp: Utc::now(),
            content: p.content.clone(),
            file_paths: p.file_paths.clone(),
            confidence,
            tags: p.tags.clone(),
        };
        match self.state().save_discovery(&self.run_id, &discovery) {
            Ok(()) => {
                let preview = if p.content.len() > 80 {
                    format!("{}...", &p.content[..80])
                } else {
                    p.content.clone()
                };
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Recorded discovery '{id}': {preview}"
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Search the Hive Mind knowledge space by keyword. Any agent can call this."
    )]
    pub(crate) async fn hive_query_mind(
        &self,
        params: Parameters<QueryMindParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.state().query_mind(&self.run_id, &params.0.query);
        if result.discoveries.is_empty() && result.insights.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No matching discoveries or insights found.",
            )]));
        }

        let mut output = String::new();
        if !result.discoveries.is_empty() {
            output.push_str("## Discoveries\n\n");
            for d in &result.discoveries {
                output.push_str(&format!("### {}\n", d.id));
                output.push_str(&format!("- **Agent:** {}\n", d.agent_id));
                output.push_str(&format!("- **Confidence:** {:?}\n", d.confidence));
                if !d.tags.is_empty() {
                    output.push_str(&format!("- **Tags:** {}\n", d.tags.join(", ")));
                }
                if !d.file_paths.is_empty() {
                    output.push_str(&format!("- **Files:** {}\n", d.file_paths.join(", ")));
                }
                output.push_str(&format!("\n{}\n\n", d.content));
            }
        }
        if !result.insights.is_empty() {
            output.push_str("## Insights\n\n");
            for i in &result.insights {
                output.push_str(&format!("### {}\n", i.id));
                if !i.tags.is_empty() {
                    output.push_str(&format!("- **Tags:** {}\n", i.tags.join(", ")));
                }
                output.push_str(&format!(
                    "- **Discovery IDs:** {}\n",
                    i.discovery_ids.join(", ")
                ));
                output.push_str(&format!("\n{}\n\n", i.content));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Synthesize discoveries into an insight. Coordinator-only.")]
    pub(crate) async fn hive_synthesize(
        &self,
        params: Parameters<SynthesizeParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
            return Ok(result);
        }
        let p = &params.0;
        let id = format!("ins-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let insight = Insight {
            id: id.clone(),
            run_id: self.run_id.clone(),
            timestamp: Utc::now(),
            content: p.content.clone(),
            discovery_ids: p.discovery_ids.clone(),
            tags: p.tags.clone(),
        };
        match self.state().save_insight(&self.run_id, &insight) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Synthesized insight '{id}'"
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(description = "Add a convention to the shared conventions memory. Coordinator-only.")]
    pub(crate) async fn hive_establish_convention(
        &self,
        params: Parameters<EstablishConventionParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
            return Ok(result);
        }
        let existing = self.state().load_conventions();
        let updated = if existing.is_empty() {
            format!("- {}\n", params.0.content)
        } else {
            format!("{}\n- {}\n", existing, params.0.content)
        };
        match self.state().save_conventions(&updated) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Convention added: {}",
                params.0.content
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}
