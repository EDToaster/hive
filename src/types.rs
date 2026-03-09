use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// --- Agent Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Coordinator,
    Lead,
    Worker,
    Reviewer,
    Planner,
    Postmortem,
    Explorer,
    Evaluator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Idle,
    Done,
    Failed,
    Stalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub parent: Option<String>,
    pub pid: Option<u32>,
    pub worktree: Option<String>,
    pub heartbeat: Option<DateTime<Utc>>,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub messages_read_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub retry_count: u32,
}

// --- Task Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Active,
    Blocked,
    Review,
    Approved,
    Queued,
    Merged,
    Failed,
    Absorbed,
    Cancelled,
}

impl TaskStatus {
    /// A task is "resolved" if it's in a terminal state — no further action needed.
    #[allow(dead_code)]
    pub fn is_resolved(&self) -> bool {
        matches!(
            self,
            TaskStatus::Merged
                | TaskStatus::Failed
                | TaskStatus::Absorbed
                | TaskStatus::Cancelled
        )
    }

    /// A task completed its purpose (successfully or by absorption).
    #[allow(dead_code)]
    pub fn is_success(&self) -> bool {
        matches!(self, TaskStatus::Merged | TaskStatus::Absorbed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub urgency: Urgency,
    pub blocking: Vec<String>,
    pub blocked_by: Vec<String>,
    pub assigned_to: Option<String>,
    pub created_by: String,
    pub parent_task: Option<String>,
    pub branch: Option<String>,
    pub domain: Option<String>,
    #[serde(default)]
    pub review_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// --- Message Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageType {
    Info,
    Request,
    Status,
    TaskSuggestion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub to: String,
    pub timestamp: DateTime<Utc>,
    pub message_type: MessageType,
    pub body: String,
    pub refs: Vec<String>,
}

// --- Merge Queue ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub task_id: String,
    pub branch: String,
    pub submitted_by: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueue {
    pub entries: Vec<MergeQueueEntry>,
}

// --- Run ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub status: RunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Active,
    Completed,
    Failed,
}

// --- Cost Tracking ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub session_duration_secs: u64,
}

// --- Run Memory ---

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationalEntry {
    pub run_id: String,
    pub created_at: DateTime<Utc>,
    pub tasks_total: u32,
    pub tasks_failed: u32,
    pub agents_spawned: u32,
    pub total_cost_usd: f64,
    pub learnings: Vec<String>,
    pub spec_quality: String,
    pub team_sizing: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEntry {
    pub run_id: String,
    pub created_at: DateTime<Utc>,
    pub pattern: String,
    pub context: String,
    pub run_number: u32,
}

// --- Hive Mind ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    #[serde(default)]
    pub file_paths: Vec<String>,
    pub confidence: Confidence,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    pub id: String,
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub discovery_ids: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MindQueryResult {
    pub discoveries: Vec<Discovery>,
    pub insights: Vec<Insight>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_role_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&AgentRole::Coordinator).unwrap(),
            "\"coordinator\""
        );
        assert_eq!(serde_json::to_string(&AgentRole::Lead).unwrap(), "\"lead\"");
        assert_eq!(
            serde_json::to_string(&AgentRole::Worker).unwrap(),
            "\"worker\""
        );

        let role: AgentRole = serde_json::from_str("\"coordinator\"").unwrap();
        assert_eq!(role, AgentRole::Coordinator);

        assert_eq!(
            serde_json::to_string(&AgentRole::Reviewer).unwrap(),
            "\"reviewer\""
        );
        let role: AgentRole = serde_json::from_str("\"reviewer\"").unwrap();
        assert_eq!(role, AgentRole::Reviewer);

        assert_eq!(
            serde_json::to_string(&AgentRole::Planner).unwrap(),
            "\"planner\""
        );
        let role: AgentRole = serde_json::from_str("\"planner\"").unwrap();
        assert_eq!(role, AgentRole::Planner);

        assert_eq!(
            serde_json::to_string(&AgentRole::Postmortem).unwrap(),
            "\"postmortem\""
        );
        let role: AgentRole = serde_json::from_str("\"postmortem\"").unwrap();
        assert_eq!(role, AgentRole::Postmortem);
    }

    #[test]
    fn agent_status_serializes_lowercase() {
        for (variant, expected) in [
            (AgentStatus::Running, "\"running\""),
            (AgentStatus::Idle, "\"idle\""),
            (AgentStatus::Done, "\"done\""),
            (AgentStatus::Failed, "\"failed\""),
            (AgentStatus::Stalled, "\"stalled\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    #[test]
    fn task_status_all_variants_roundtrip() {
        let variants = [
            TaskStatus::Pending,
            TaskStatus::Active,
            TaskStatus::Blocked,
            TaskStatus::Review,
            TaskStatus::Approved,
            TaskStatus::Queued,
            TaskStatus::Merged,
            TaskStatus::Failed,
            TaskStatus::Absorbed,
            TaskStatus::Cancelled,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let back: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn urgency_all_variants_roundtrip() {
        for (variant, expected) in [
            (Urgency::Low, "\"low\""),
            (Urgency::Normal, "\"normal\""),
            (Urgency::High, "\"high\""),
            (Urgency::Critical, "\"critical\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    #[test]
    fn message_type_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&MessageType::TaskSuggestion).unwrap(),
            "\"task-suggestion\""
        );
        let back: MessageType = serde_json::from_str("\"task-suggestion\"").unwrap();
        assert_eq!(back, MessageType::TaskSuggestion);
        assert_eq!(
            serde_json::to_string(&MessageType::Info).unwrap(),
            "\"info\""
        );
    }

    #[test]
    fn run_status_all_variants_roundtrip() {
        for (variant, expected) in [
            (RunStatus::Active, "\"active\""),
            (RunStatus::Completed, "\"completed\""),
            (RunStatus::Failed, "\"failed\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
            let back: RunStatus = serde_json::from_str(expected).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn agent_struct_roundtrip() {
        let agent = Agent {
            id: "agent-1".into(),
            role: AgentRole::Worker,
            status: AgentStatus::Running,
            parent: Some("lead-1".into()),
            pid: Some(12345),
            worktree: Some("/tmp/wt".into()),
            heartbeat: Some(chrono::Utc::now()),
            task_id: Some("task-1".into()),
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        let json = serde_json::to_string(&agent).unwrap();
        let back: Agent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "agent-1");
        assert_eq!(back.role, AgentRole::Worker);
        assert_eq!(back.pid, Some(12345));
    }

    #[test]
    fn agent_struct_with_null_optionals() {
        let agent = Agent {
            id: "coord-1".into(),
            role: AgentRole::Coordinator,
            status: AgentStatus::Running,
            parent: None,
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
        };
        let json = serde_json::to_string(&agent).unwrap();
        let back: Agent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, AgentRole::Coordinator);
        assert!(back.parent.is_none());
        assert!(back.pid.is_none());
    }

    #[test]
    fn task_struct_roundtrip() {
        let now = chrono::Utc::now();
        let task = Task {
            id: "task-1".into(),
            title: "Implement feature X".into(),
            description: "Build the thing".into(),
            status: TaskStatus::Pending,
            urgency: Urgency::High,
            blocking: vec!["task-2".into()],
            blocked_by: vec![],
            assigned_to: None,
            created_by: "coordinator".into(),
            parent_task: None,
            branch: None,
            domain: Some("backend".into()),
            review_count: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&task).unwrap();
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, TaskStatus::Pending);
        assert_eq!(back.urgency, Urgency::High);
        assert_eq!(back.blocking, vec!["task-2"]);
    }

    #[test]
    fn task_without_review_count_defaults_to_zero() {
        let json = r#"{"id":"t1","title":"test","description":"d","status":"pending","urgency":"normal","blocking":[],"blocked_by":[],"assigned_to":null,"created_by":"coord","parent_task":null,"branch":null,"domain":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;
        let task: Task = serde_json::from_str(json).unwrap();
        assert_eq!(task.review_count, 0);
    }

    #[test]
    fn message_struct_roundtrip() {
        let msg = Message {
            id: "msg-1".into(),
            from: "lead-1".into(),
            to: "coordinator".into(),
            timestamp: chrono::Utc::now(),
            message_type: MessageType::Status,
            body: "All tasks complete".into(),
            refs: vec!["task-1".into(), "task-2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.message_type, MessageType::Status);
        assert_eq!(back.refs.len(), 2);
    }

    #[test]
    fn merge_queue_roundtrip() {
        let queue = MergeQueue {
            entries: vec![MergeQueueEntry {
                task_id: "task-1".into(),
                branch: "hive/run1/lead-1".into(),
                submitted_by: "lead-1".into(),
                submitted_at: chrono::Utc::now(),
            }],
        };
        let json = serde_json::to_string(&queue).unwrap();
        let back: MergeQueue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].task_id, "task-1");
    }

    #[test]
    fn run_metadata_roundtrip() {
        let run = RunMetadata {
            id: "run-abc".into(),
            created_at: chrono::Utc::now(),
            status: RunStatus::Active,
        };
        let json = serde_json::to_string(&run).unwrap();
        let back: RunMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "run-abc");
        assert_eq!(back.status, RunStatus::Active);
    }

    #[test]
    fn operational_entry_roundtrip() {
        let entry = OperationalEntry {
            run_id: "run-1".into(),
            created_at: chrono::Utc::now(),
            tasks_total: 10,
            tasks_failed: 2,
            agents_spawned: 5,
            total_cost_usd: 3.50,
            learnings: vec!["lesson 1".into(), "lesson 2".into()],
            spec_quality: "good".into(),
            team_sizing: "appropriate".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: OperationalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.run_id, "run-1");
        assert_eq!(back.tasks_total, 10);
        assert_eq!(back.tasks_failed, 2);
        assert_eq!(back.agents_spawned, 5);
        assert!((back.total_cost_usd - 3.50).abs() < f64::EPSILON);
        assert_eq!(back.learnings.len(), 2);
        assert_eq!(back.spec_quality, "good");
        assert_eq!(back.team_sizing, "appropriate");
    }

    #[test]
    fn failure_entry_roundtrip() {
        let entry = FailureEntry {
            run_id: "run-1".into(),
            created_at: chrono::Utc::now(),
            pattern: "timeout on large files".into(),
            context: "worker-3 failed processing".into(),
            run_number: 5,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: FailureEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.run_id, "run-1");
        assert_eq!(back.pattern, "timeout on large files");
        assert_eq!(back.context, "worker-3 failed processing");
        assert_eq!(back.run_number, 5);
    }

    #[test]
    fn explorer_evaluator_role_roundtrip() {
        for (variant, expected) in [
            (AgentRole::Explorer, "\"explorer\""),
            (AgentRole::Evaluator, "\"evaluator\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
            let back: AgentRole = serde_json::from_str(expected).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn confidence_all_variants_roundtrip() {
        for (variant, expected) in [
            (Confidence::Low, "\"low\""),
            (Confidence::Medium, "\"medium\""),
            (Confidence::High, "\"high\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
            let back: Confidence = serde_json::from_str(expected).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn discovery_struct_roundtrip() {
        let disc = Discovery {
            id: "disc-abc".into(),
            run_id: "run-1".into(),
            agent_id: "explorer-1".into(),
            timestamp: chrono::Utc::now(),
            content: "Found interesting pattern".into(),
            file_paths: vec!["src/main.rs".into(), "src/lib.rs".into()],
            confidence: Confidence::High,
            tags: vec!["architecture".into(), "pattern".into()],
        };
        let json = serde_json::to_string(&disc).unwrap();
        let back: Discovery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "disc-abc");
        assert_eq!(back.run_id, "run-1");
        assert_eq!(back.agent_id, "explorer-1");
        assert_eq!(back.content, "Found interesting pattern");
        assert_eq!(back.file_paths.len(), 2);
        assert_eq!(back.confidence, Confidence::High);
        assert_eq!(back.tags.len(), 2);
    }

    #[test]
    fn discovery_default_fields() {
        let json = r#"{"id":"d1","run_id":"r1","agent_id":"a1","timestamp":"2026-01-01T00:00:00Z","content":"test","confidence":"medium"}"#;
        let disc: Discovery = serde_json::from_str(json).unwrap();
        assert!(disc.file_paths.is_empty());
        assert!(disc.tags.is_empty());
    }

    #[test]
    fn insight_struct_roundtrip() {
        let insight = Insight {
            id: "ins-abc".into(),
            run_id: "run-1".into(),
            timestamp: chrono::Utc::now(),
            content: "Synthesized finding".into(),
            discovery_ids: vec!["disc-1".into(), "disc-2".into()],
            tags: vec!["summary".into()],
        };
        let json = serde_json::to_string(&insight).unwrap();
        let back: Insight = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "ins-abc");
        assert_eq!(back.content, "Synthesized finding");
        assert_eq!(back.discovery_ids.len(), 2);
        assert_eq!(back.tags, vec!["summary"]);
    }

    #[test]
    fn mind_query_result_roundtrip() {
        let result = MindQueryResult {
            discoveries: vec![Discovery {
                id: "disc-1".into(),
                run_id: "run-1".into(),
                agent_id: "explorer-1".into(),
                timestamp: chrono::Utc::now(),
                content: "Found something".into(),
                file_paths: vec![],
                confidence: Confidence::Low,
                tags: vec![],
            }],
            insights: vec![Insight {
                id: "ins-1".into(),
                run_id: "run-1".into(),
                timestamp: chrono::Utc::now(),
                content: "Key insight".into(),
                discovery_ids: vec!["disc-1".into()],
                tags: vec![],
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: MindQueryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.discoveries.len(), 1);
        assert_eq!(back.insights.len(), 1);
        assert_eq!(back.discoveries[0].id, "disc-1");
        assert_eq!(back.insights[0].id, "ins-1");
    }

    #[test]
    fn absorbed_cancelled_roundtrip() {
        for (variant, expected) in [
            (TaskStatus::Absorbed, "\"absorbed\""),
            (TaskStatus::Cancelled, "\"cancelled\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
            let back: TaskStatus = serde_json::from_str(expected).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn is_resolved_returns_true_for_terminal_statuses() {
        assert!(TaskStatus::Merged.is_resolved());
        assert!(TaskStatus::Failed.is_resolved());
        assert!(TaskStatus::Absorbed.is_resolved());
        assert!(TaskStatus::Cancelled.is_resolved());

        assert!(!TaskStatus::Pending.is_resolved());
        assert!(!TaskStatus::Active.is_resolved());
        assert!(!TaskStatus::Review.is_resolved());
        assert!(!TaskStatus::Queued.is_resolved());
    }

    #[test]
    fn is_success_returns_true_for_successful_statuses() {
        assert!(TaskStatus::Merged.is_success());
        assert!(TaskStatus::Absorbed.is_success());

        assert!(!TaskStatus::Failed.is_success());
        assert!(!TaskStatus::Cancelled.is_success());
        assert!(!TaskStatus::Active.is_success());
    }
}
