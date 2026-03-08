use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// --- Agent Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Coordinator,
    Lead,
    Worker,
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
}
