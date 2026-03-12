use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct SpawnAgentParams {
    /// Agent ID (unique name like "lead-backend" or "worker-001")
    pub agent_id: String,
    /// Role: "lead" or "worker"
    pub role: String,
    /// Task ID to bind this agent to
    pub task_id: String,
    /// Task description for the agent
    pub task_description: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Short title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Urgency: low, normal, high, critical
    #[serde(default = "default_urgency")]
    pub urgency: String,
    /// Optional domain tag
    pub domain: Option<String>,
    /// Optional list of task IDs this blocks
    #[serde(default)]
    pub blocking: Vec<String>,
    /// Optional list of task IDs blocking this
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Optional parent task ID
    pub parent_task: Option<String>,
}

fn default_urgency() -> String {
    "normal".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateTaskParams {
    /// Task ID to update
    pub task_id: String,
    /// New status
    pub status: Option<String>,
    /// Agent ID to assign to
    pub assigned_to: Option<String>,
    /// Branch name
    pub branch: Option<String>,
    /// Optional notes to append to task description (for context handoff)
    pub notes: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTasksParams {
    /// Filter by status
    pub status: Option<String>,
    /// Filter by assignee
    pub assignee: Option<String>,
    /// Filter by domain
    pub domain: Option<String>,
    /// Filter by parent task. Use "none" for top-level only, or a task ID for that task's subtasks.
    pub parent_task: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// Recipient agent ID
    pub to: String,
    /// Message type: info, request, status, task-suggestion
    #[serde(default = "default_message_type")]
    pub message_type: String,
    /// Message body
    pub body: String,
    /// Optional task ID references
    #[serde(default)]
    pub refs: Vec<String>,
}

fn default_message_type() -> String {
    "info".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct SubmitToQueueParams {
    /// Task ID of the approved work
    pub task_id: String,
    /// Branch name to merge
    pub branch: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct WaitForActivityParams {
    /// Timeout in seconds (default 60)
    #[serde(default = "default_wait_timeout")]
    pub timeout_secs: u64,
}

fn default_wait_timeout() -> u64 {
    60
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewAgentParams {
    /// Agent ID to review
    pub agent_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LogToolParams {
    /// Tool name
    pub tool: String,
    /// Status: success or error
    pub status: String,
    /// Duration in milliseconds
    pub duration_ms: Option<i64>,
    /// Optional args summary
    pub args_summary: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadMessagesParams {
    /// Only return messages newer than this timestamp (ISO 8601). If omitted, returns unread messages since last read or last idle.
    pub since: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RetryAgentParams {
    /// Agent ID of the failed agent to retry
    pub agent_id: String,
    /// Optional feedback about what went wrong, appended to the new agent's prompt
    pub feedback: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReviewVerdictParams {
    /// Task ID being reviewed
    pub task_id: String,
    /// Verdict: "approve", "request-changes", or "reject"
    pub verdict: String,
    /// Feedback message (required for request-changes and reject)
    pub feedback: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveMemoryParams {
    /// Memory type: "operation", "convention", or "failure"
    pub memory_type: String,
    /// Content: JSON string for operation/failure entries, markdown for conventions
    pub content: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveSpecParams {
    /// Full spec markdown content
    pub spec: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiscoverParams {
    /// Content of the discovery
    pub content: String,
    /// Confidence level: "low", "medium", or "high"
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// File paths related to this discovery
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_confidence() -> String {
    "medium".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryMindParams {
    /// Search query (keywords)
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SynthesizeParams {
    /// Insight content synthesized from discoveries
    pub content: String,
    /// IDs of discoveries being synthesized
    pub discovery_ids: Vec<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct EstablishConventionParams {
    /// Convention to add
    pub content: String,
}
