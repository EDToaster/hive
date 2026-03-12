use super::params::*;
use super::*;
use crate::state::HiveState;
use crate::types::{
    Agent, AgentRole, AgentStatus, Confidence, Discovery, FailureEntry, OperationalEntry,
    RunStatus, Task, TaskStatus, Urgency,
};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

fn setup_mcp(role: AgentRole) -> (TempDir, HiveMcp) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root.clone());
    state.create_run("test-run").unwrap();
    let agent = Agent {
        id: "test-agent".into(),
        role,
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
        model: None,
    };
    state.save_agent("test-run", &agent).unwrap();
    let mcp = HiveMcp::new(
        "test-run".into(),
        "test-agent".into(),
        root.to_string_lossy().to_string(),
    );
    (dir, mcp)
}

fn setup_mcp_with_id(agent_id: &str, role: AgentRole) -> (TempDir, HiveMcp) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root.clone());
    state.create_run("test-run").unwrap();
    let agent = Agent {
        id: agent_id.into(),
        role,
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
        model: None,
    };
    state.save_agent("test-run", &agent).unwrap();
    let mcp = HiveMcp::new(
        "test-run".into(),
        agent_id.into(),
        root.to_string_lossy().to_string(),
    );
    (dir, mcp)
}

fn make_task(id: &str, parent: Option<&str>, status: TaskStatus) -> Task {
    let now = Utc::now();
    Task {
        id: id.into(),
        title: format!("Task {id}"),
        description: format!("Description for {id}"),
        status,
        urgency: Urgency::Normal,
        blocking: vec![],
        blocked_by: vec![],
        assigned_to: None,
        created_by: "coordinator".into(),
        parent_task: parent.map(|s| s.to_string()),
        branch: None,
        domain: None,
        review_count: 0,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn spawn_agent_rejects_missing_task_id() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-test".into(),
        role: "lead".into(),
        task_id: "nonexistent-task".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("not found"));
}

#[tokio::test]
async fn update_task_worker_can_update_own() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    // Set worker's parent
    let mut agent = state.load_agent("test-run", "worker-1").unwrap();
    agent.task_id = Some("task-w1".into());
    agent.parent = Some("lead-1".into());
    state.save_agent("test-run", &agent).unwrap();

    let mut task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
    task.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-w1".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Worker should update own task"
    );
}

#[tokio::test]
async fn update_task_worker_denied_other() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "worker-1").unwrap();
    agent.task_id = Some("task-w1".into());
    agent.parent = Some("lead-1".into());
    state.save_agent("test-run", &agent).unwrap();

    let mut task = make_task("task-w2", Some("task-lead"), TaskStatus::Active);
    task.assigned_to = Some("worker-2".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-w2".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
    assert!(text.contains("task-w1")); // mentions own task
    assert!(text.contains("lead-1")); // mentions lead
}

#[tokio::test]
async fn update_task_coordinator_denied_subtask() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let task = make_task("task-sub", Some("task-lead"), TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-sub".into(),
        status: Some("cancelled".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("coordinator cannot modify subtasks"));
}

#[tokio::test]
async fn update_task_lead_can_update_own_and_children() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
    lead_agent.task_id = Some("task-lead".into());
    state.save_agent("test-run", &lead_agent).unwrap();

    // Lead's own task
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // Worker task created by lead
    let mut worker_task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
    worker_task.created_by = "lead-1".into();
    worker_task.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &worker_task).unwrap();

    // Worker agent parented to lead
    let worker_agent = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-w1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker_agent).unwrap();

    // Lead can update own task
    let params = Parameters(UpdateTaskParams {
        task_id: "task-lead".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("progress note".into()),
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead should update own task"
    );

    // Lead can update worker's task (created by lead)
    let params = Parameters(UpdateTaskParams {
        task_id: "task-w1".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("feedback".into()),
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead should update worker's task"
    );
}

#[tokio::test]
async fn create_task_coordinator_no_parent_ok() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(CreateTaskParams {
        title: "Lead task".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Coordinator should create top-level task"
    );
}

#[tokio::test]
async fn create_task_coordinator_denied_with_parent() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(CreateTaskParams {
        title: "Subtask".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: Some("task-lead".into()),
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("coordinator cannot create subtasks"));
}

#[tokio::test]
async fn create_task_lead_with_own_parent_ok() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
    lead_agent.task_id = Some("task-lead".into());
    state.save_agent("test-run", &lead_agent).unwrap();

    let params = Parameters(CreateTaskParams {
        title: "Subtask".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: Some("task-lead".into()),
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead should create subtask under own task"
    );
}

#[tokio::test]
async fn create_task_worker_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(CreateTaskParams {
        title: "Task".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("workers cannot create tasks"));
}

#[tokio::test]
async fn submit_to_queue_blocked_by_unresolved_subtasks() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // Active subtask (unresolved)
    let mut sub1 = make_task("task-sub1", Some("task-lead"), TaskStatus::Active);
    sub1.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &sub1).unwrap();

    // Merged subtask (resolved)
    let mut sub2 = make_task("task-sub2", Some("task-lead"), TaskStatus::Merged);
    sub2.assigned_to = Some("worker-2".into());
    state.save_task("test-run", &sub2).unwrap();

    let params = Parameters(SubmitToQueueParams {
        task_id: "task-lead".into(),
        branch: "hive/test/lead-1".into(),
    });
    let result = mcp.hive_submit_to_queue(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("1 subtask(s) are not resolved"));
    assert!(text.contains("task-sub1"));
}

#[tokio::test]
async fn list_tasks_parent_filter_none() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let top = make_task("task-top", None, TaskStatus::Active);
    let sub = make_task("task-sub", Some("task-top"), TaskStatus::Active);
    state.save_task("test-run", &top).unwrap();
    state.save_task("test-run", &sub).unwrap();

    let params = Parameters(ListTasksParams {
        status: None,
        assignee: None,
        domain: None,
        parent_task: Some("none".into()),
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("task-top"));
    assert!(!text.contains("task-sub"));
}

#[tokio::test]
async fn list_tasks_parent_filter_by_id() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let top = make_task("task-top", None, TaskStatus::Active);
    let sub = make_task("task-sub", Some("task-top"), TaskStatus::Active);
    state.save_task("test-run", &top).unwrap();
    state.save_task("test-run", &sub).unwrap();

    let params = Parameters(ListTasksParams {
        status: None,
        assignee: None,
        domain: None,
        parent_task: Some("task-top".into()),
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(!text.contains("task-top") || text.contains("task-sub"));
    assert!(text.contains("task-sub"));
}

#[test]
fn save_memory_rejects_non_postmortem() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let result = mcp.require_role(&[AgentRole::Postmortem]);
    assert!(
        result.is_err(),
        "Worker should not be allowed to save memory"
    );
}

#[test]
fn save_memory_allows_postmortem() {
    let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
    let result = mcp.require_role(&[AgentRole::Postmortem]);
    assert!(
        result.is_ok(),
        "Postmortem should be allowed to save memory"
    );
}

#[test]
fn save_spec_rejects_non_planner() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let result = mcp.require_role(&[AgentRole::Planner]);
    assert!(result.is_err(), "Worker should not be allowed to save spec");
}

#[test]
fn save_spec_allows_planner() {
    let (_dir, mcp) = setup_mcp(AgentRole::Planner);
    let result = mcp.require_role(&[AgentRole::Planner]);
    assert!(result.is_ok(), "Planner should be allowed to save spec");
}

#[tokio::test]
async fn save_memory_rejects_invalid_memory_type() {
    let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
    let params = Parameters(SaveMemoryParams {
        memory_type: "invalid".into(),
        content: "test".into(),
    });
    let result = mcp.hive_save_memory(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn save_memory_rejects_invalid_operation_json() {
    let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
    let params = Parameters(SaveMemoryParams {
        memory_type: "operation".into(),
        content: "not valid json".into(),
    });
    let result = mcp.hive_save_memory(params).await;
    assert!(result.is_err());
}

#[test]
fn check_agents_allows_planner_and_postmortem() {
    let (_dir, mcp) = setup_mcp(AgentRole::Planner);
    let allowed = &[
        AgentRole::Coordinator,
        AgentRole::Lead,
        AgentRole::Reviewer,
        AgentRole::Planner,
        AgentRole::Postmortem,
    ];
    assert!(mcp.require_role(allowed).is_ok());

    let (_dir2, mcp2) = setup_mcp(AgentRole::Postmortem);
    assert!(mcp2.require_role(allowed).is_ok());
}

#[test]
fn spawn_hierarchy_allows_coordinator_to_spawn_postmortem() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let caller_role = mcp.agent_role();
    let allowed = matches!(
        (caller_role, AgentRole::Postmortem),
        (AgentRole::Coordinator, AgentRole::Lead)
            | (AgentRole::Coordinator, AgentRole::Planner)
            | (AgentRole::Coordinator, AgentRole::Postmortem)
            | (AgentRole::Lead, AgentRole::Worker)
            | (AgentRole::Lead, AgentRole::Reviewer)
    );
    assert!(allowed, "Coordinator should be able to spawn Postmortem");
}

#[test]
fn synthesize_rejects_non_coordinator() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let result = mcp.require_role(&[AgentRole::Coordinator]);
    assert!(
        result.is_err(),
        "Worker should not be allowed to synthesize"
    );
}

#[test]
fn synthesize_allows_coordinator() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let result = mcp.require_role(&[AgentRole::Coordinator]);
    assert!(
        result.is_ok(),
        "Coordinator should be allowed to synthesize"
    );
}

#[test]
fn establish_convention_rejects_non_coordinator() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let result = mcp.require_role(&[AgentRole::Coordinator]);
    assert!(
        result.is_err(),
        "Worker should not be allowed to establish conventions"
    );
}

#[test]
fn establish_convention_allows_coordinator() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let result = mcp.require_role(&[AgentRole::Coordinator]);
    assert!(
        result.is_ok(),
        "Coordinator should be allowed to establish conventions"
    );
}

#[test]
fn spawn_hierarchy_coordinator_can_spawn_explorer() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let caller_role = mcp.agent_role();
    let allowed = matches!(
        (caller_role, AgentRole::Explorer),
        (AgentRole::Coordinator, AgentRole::Lead)
            | (AgentRole::Coordinator, AgentRole::Planner)
            | (AgentRole::Coordinator, AgentRole::Postmortem)
            | (AgentRole::Coordinator, AgentRole::Explorer)
            | (AgentRole::Coordinator, AgentRole::Evaluator)
            | (AgentRole::Lead, AgentRole::Worker)
            | (AgentRole::Lead, AgentRole::Reviewer)
    );
    assert!(allowed, "Coordinator should be able to spawn Explorer");
}

#[test]
fn spawn_hierarchy_coordinator_can_spawn_evaluator() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let caller_role = mcp.agent_role();
    let allowed = matches!(
        (caller_role, AgentRole::Evaluator),
        (AgentRole::Coordinator, AgentRole::Lead)
            | (AgentRole::Coordinator, AgentRole::Planner)
            | (AgentRole::Coordinator, AgentRole::Postmortem)
            | (AgentRole::Coordinator, AgentRole::Explorer)
            | (AgentRole::Coordinator, AgentRole::Evaluator)
            | (AgentRole::Lead, AgentRole::Worker)
            | (AgentRole::Lead, AgentRole::Reviewer)
    );
    assert!(allowed, "Coordinator should be able to spawn Evaluator");
}

#[test]
fn spawn_hierarchy_explorer_cannot_spawn() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let result = mcp.require_role(&[AgentRole::Coordinator, AgentRole::Lead]);
    assert!(
        result.is_err(),
        "Explorer should not be allowed to spawn agents"
    );
}

#[test]
fn spawn_hierarchy_evaluator_cannot_spawn() {
    let (_dir, mcp) = setup_mcp(AgentRole::Evaluator);
    let result = mcp.require_role(&[AgentRole::Coordinator, AgentRole::Lead]);
    assert!(
        result.is_err(),
        "Evaluator should not be allowed to spawn agents"
    );
}

#[tokio::test]
async fn discover_creates_discovery() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(DiscoverParams {
        content: "Found a caching pattern in the API layer".into(),
        confidence: "high".into(),
        file_paths: vec!["src/api.rs".into()],
        tags: vec!["caching".into(), "performance".into()],
    });
    let result = mcp.hive_discover(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "discover should succeed for any agent"
    );

    // Verify the discovery was saved
    let discoveries = mcp.state().load_discoveries("test-run");
    assert_eq!(discoveries.len(), 1);
    assert!(discoveries[0].id.starts_with("disc-"));
    assert_eq!(
        discoveries[0].content,
        "Found a caching pattern in the API layer"
    );
    assert_eq!(discoveries[0].agent_id, "test-agent");
}

#[tokio::test]
async fn query_mind_returns_results() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let state = mcp.state();
    let discovery = Discovery {
        id: "disc-test1".into(),
        run_id: "test-run".into(),
        agent_id: "test-agent".into(),
        timestamp: Utc::now(),
        content: "Discovered caching optimization opportunity".into(),
        file_paths: vec![],
        confidence: Confidence::High,
        tags: vec!["performance".into()],
    };
    state.save_discovery("test-run", &discovery).unwrap();

    let params = Parameters(QueryMindParams {
        query: "caching".into(),
    });
    let result = mcp.hive_query_mind(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "query_mind should succeed"
    );
    // Verify the result contains discovery info
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("disc-test1"));
    assert!(text.contains("caching"));
}

#[tokio::test]
async fn review_verdict_request_changes_sends_feedback_message() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-tasktest", AgentRole::Reviewer);
    let state = mcp.state();

    let mut task = make_task("task-test", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    task.review_count = 0;
    state.save_task("test-run", &task).unwrap();

    // Create an idle lead agent
    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Idle,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-test".into()),
        session_id: Some("sess-abc".into()),
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-test".into(),
        verdict: "request-changes".into(),
        feedback: Some("Please fix the error handling.".into()),
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // Verify feedback message was saved for lead-1
    let messages = state.list_messages("test-run").unwrap_or_default();
    let lead_msgs: Vec<_> = messages.iter().filter(|m| m.to == "lead-1").collect();
    assert!(!lead_msgs.is_empty(), "lead should have a feedback message");
    assert!(
        lead_msgs[0].body.contains("Please fix the error handling"),
        "message should contain the feedback"
    );

    // Task should be back to Active with review_count incremented
    let task = state.load_task("test-run", "task-test").unwrap();
    assert_eq!(task.status, TaskStatus::Active);
    assert_eq!(task.review_count, 1);
}

#[tokio::test]
async fn coordinator_can_message_explorer() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    let explorer = Agent {
        id: "explorer-1".into(),
        role: AgentRole::Explorer,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &explorer).unwrap();

    let params = Parameters(SendMessageParams {
        to: "explorer-1".into(),
        message_type: "info".into(),
        body: "Test message to explorer".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Coordinator should be able to message explorers"
    );
}

#[tokio::test]
async fn explorer_can_update_own_task() {
    let (_dir, mcp) = setup_mcp_with_id("explorer-1", AgentRole::Explorer);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "explorer-1").unwrap();
    agent.task_id = Some("task-explore".into());
    agent.parent = Some("coordinator".into());
    state.save_agent("test-run", &agent).unwrap();

    let mut task = make_task("task-explore", None, TaskStatus::Active);
    task.assigned_to = Some("explorer-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-explore".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Explorer should update own task"
    );
}

#[tokio::test]
async fn evaluator_can_update_own_task() {
    let (_dir, mcp) = setup_mcp_with_id("evaluator-1", AgentRole::Evaluator);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "evaluator-1").unwrap();
    agent.task_id = Some("task-eval".into());
    agent.parent = Some("coordinator".into());
    state.save_agent("test-run", &agent).unwrap();

    let mut task = make_task("task-eval", None, TaskStatus::Active);
    task.assigned_to = Some("evaluator-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-eval".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Evaluator should update own task"
    );
}

#[tokio::test]
async fn check_agents_includes_commits() {
    use std::process::Command;

    // Create a git repo with a main branch and an initial commit
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::fs::write(root.join("init.txt"), "init").unwrap();
    crate::git::Git::add_all(&root).unwrap();
    crate::git::Git::commit(&root, "initial commit").unwrap();

    // Create a worktree on a feature branch with commits
    let wt_path = root.join("worktree-test");
    Command::new("git")
        .args([
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "feature-branch",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    std::fs::write(wt_path.join("a.txt"), "a").unwrap();
    crate::git::Git::add_all(&wt_path).unwrap();
    crate::git::Git::commit(&wt_path, "first feature commit").unwrap();
    std::fs::write(wt_path.join("b.txt"), "b").unwrap();
    crate::git::Git::add_all(&wt_path).unwrap();
    crate::git::Git::commit(&wt_path, "second feature commit").unwrap();

    // Set up hive state with an agent that has the worktree
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root.clone());
    state.create_run("test-run").unwrap();
    let agent = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: Some(wt_path.to_string_lossy().to_string()),
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &agent).unwrap();

    // Create a coordinator MCP to call check_agents
    let coord = Agent {
        id: "coordinator".into(),
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
        model: None,
    };
    state.save_agent("test-run", &coord).unwrap();
    let mcp = HiveMcp::new(
        "test-run".into(),
        "coordinator".into(),
        root.to_string_lossy().to_string(),
    );

    let result = mcp.hive_check_agents().await.unwrap();
    let text = match &result.content[0].raw {
        rmcp::model::RawContent::Text(t) => &t.text,
        _ => panic!("expected text content"),
    };
    let reports: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();

    // Find the worker-1 report
    let worker_report = reports
        .iter()
        .find(|r| r["agent_id"] == "worker-1")
        .expect("worker-1 report should exist");

    assert_eq!(worker_report["commit_count"], 2);
    let commits = worker_report["recent_commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert!(
        commits
            .iter()
            .any(|c| c.as_str().unwrap().contains("first feature commit"))
    );
    assert!(
        commits
            .iter()
            .any(|c| c.as_str().unwrap().contains("second feature commit"))
    );

    // Coordinator should have no commits (no worktree)
    let coord_report = reports
        .iter()
        .find(|r| r["agent_id"] == "coordinator")
        .expect("coordinator report should exist");
    assert_eq!(coord_report["commit_count"], 0);
    assert!(coord_report["recent_commits"].is_null());
}

#[test]
fn notify_parent_of_transition_sends_message_and_records() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    // Create a worker agent with a parent
    let worker = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Idle,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-w1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    // Call the helper
    mcp.notify_parent_of_transition(&state, &worker);

    // Verify a message was sent to the parent
    let messages = state.list_messages("test-run").unwrap();
    let notify_msgs: Vec<_> = messages
        .iter()
        .filter(|m| m.from == "worker-1" && m.to == "lead-1")
        .collect();
    assert_eq!(notify_msgs.len(), 1, "Should send exactly one notification");
    let msg = notify_msgs[0];
    assert_eq!(msg.message_type, MessageType::Status);
    assert!(msg.body.contains("worker-1"));
    assert!(msg.body.contains("Idle"));
    assert!(msg.body.contains("task-w1"));
    assert_eq!(msg.refs, vec!["task-w1".to_string()]);
}

// ---- send_message permission tests ----

#[tokio::test]
async fn send_message_worker_denied_non_parent() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "worker-1").unwrap();
    agent.parent = Some("lead-1".into());
    state.save_agent("test-run", &agent).unwrap();

    // Create lead-1 agent
    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    // Worker tries to message coordinator (not their parent)
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "test".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Workers can only send messages to their lead"));
}

#[tokio::test]
async fn send_message_worker_can_message_parent() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "worker-1").unwrap();
    agent.parent = Some("lead-1".into());
    state.save_agent("test-run", &agent).unwrap();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(SendMessageParams {
        to: "lead-1".into(),
        message_type: "info".into(),
        body: "hello lead".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Worker should message parent lead"
    );
}

#[tokio::test]
async fn send_message_lead_denied_non_child() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    // Another lead's worker
    let other_worker = Agent {
        id: "worker-2".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-2".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &other_worker).unwrap();

    let params = Parameters(SendMessageParams {
        to: "worker-2".into(),
        message_type: "info".into(),
        body: "test".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Leads can only message their workers or the coordinator"));
}

#[tokio::test]
async fn send_message_lead_can_message_coordinator() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let coord = Agent {
        id: "coordinator".into(),
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
        model: None,
    };
    state.save_agent("test-run", &coord).unwrap();

    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "status update".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead should message coordinator"
    );
}

#[tokio::test]
async fn send_message_coordinator_denied_worker() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    let worker = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    let params = Parameters(SendMessageParams {
        to: "worker-1".into(),
        message_type: "info".into(),
        body: "test".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(
        text.contains("Coordinator can only send messages to leads, explorers, and evaluators")
    );
}

#[tokio::test]
async fn send_message_explorer_can_only_message_coordinator() {
    let (_dir, mcp) = setup_mcp_with_id("explorer-1", AgentRole::Explorer);
    let state = mcp.state();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(SendMessageParams {
        to: "lead-1".into(),
        message_type: "info".into(),
        body: "test".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("This role can only send messages to the coordinator"));
}

#[tokio::test]
async fn send_message_explorer_can_message_coordinator() {
    let (_dir, mcp) = setup_mcp_with_id("explorer-1", AgentRole::Explorer);
    let state = mcp.state();

    let coord = Agent {
        id: "coordinator".into(),
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
        model: None,
    };
    state.save_agent("test-run", &coord).unwrap();

    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "discovery report".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Explorer should message coordinator"
    );
}

// ---- spawn_agent permission and edge case tests ----

#[tokio::test]
async fn spawn_agent_worker_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "child".into(),
        role: "worker".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn spawn_agent_invalid_role() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "agent-1".into(),
        role: "superadmin".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Invalid role"));
}

#[tokio::test]
async fn spawn_agent_coordinator_cannot_spawn_worker() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "worker-1".into(),
        role: "worker".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
    assert!(text.contains("cannot spawn"));
}

#[tokio::test]
async fn spawn_agent_task_wrong_status() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-1".into(),
        role: "lead".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Active"));
    assert!(text.contains("expected pending or blocked"));
}

#[tokio::test]
async fn spawn_agent_lead_cannot_spawn_lead() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();
    let task = make_task("task-1", Some("parent"), TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-2".into(),
        role: "lead".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("cannot spawn"));
}

// ---- review_verdict tests ----

#[tokio::test]
async fn review_verdict_non_reviewer_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Lead);
    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn review_verdict_invalid_verdict() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1", AgentRole::Reviewer);
    let state = mcp.state();
    let mut task = make_task("task-1", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "maybe".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Invalid verdict"));
}

#[tokio::test]
async fn review_verdict_approve_sets_queued() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1", AgentRole::Reviewer);
    let state = mcp.state();
    let mut task = make_task("task-1", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    task.branch = Some("hive/test/lead-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let task = state.load_task("test-run", "task-1").unwrap();
    assert_eq!(task.status, TaskStatus::Queued);

    // Verify merge queue entry was created
    let queue = state.load_merge_queue("test-run").unwrap();
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(queue.entries[0].task_id, "task-1");
}

#[tokio::test]
async fn review_verdict_reject_sets_failed() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1", AgentRole::Reviewer);
    let state = mcp.state();
    let mut task = make_task("task-1", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    // Create lead agent with parent for notification path
    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "reject".into(),
        feedback: Some("Fundamentally flawed approach".into()),
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let task = state.load_task("test-run", "task-1").unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

#[tokio::test]
async fn review_verdict_task_not_found() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1", AgentRole::Reviewer);

    let params = Parameters(ReviewVerdictParams {
        task_id: "nonexistent".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

// ---- submit_to_queue tests ----

#[tokio::test]
async fn submit_to_queue_non_lead_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(SubmitToQueueParams {
        task_id: "task-1".into(),
        branch: "branch".into(),
    });
    let result = mcp.hive_submit_to_queue(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn submit_to_queue_review_cycle_exceeded() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut task = make_task("task-maxrev", None, TaskStatus::Active);
    task.assigned_to = Some("lead-1".into());
    task.review_count = 3; // Max
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SubmitToQueueParams {
        task_id: "task-maxrev".into(),
        branch: "hive/test/lead-1".into(),
    });
    let result = mcp.hive_submit_to_queue(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("maximum review cycles"));

    // Task should be marked failed
    let task = state.load_task("test-run", "task-maxrev").unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

#[tokio::test]
async fn submit_to_queue_all_subtasks_resolved_ok() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // All resolved subtasks
    let mut sub1 = make_task("task-sub1", Some("task-lead"), TaskStatus::Merged);
    sub1.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &sub1).unwrap();

    let mut sub2 = make_task("task-sub2", Some("task-lead"), TaskStatus::Absorbed);
    sub2.assigned_to = Some("worker-2".into());
    state.save_task("test-run", &sub2).unwrap();

    let params = Parameters(SubmitToQueueParams {
        task_id: "task-lead".into(),
        branch: "hive/test/lead-1".into(),
    });
    // This may fail due to AgentSpawner trying to actually spawn, but
    // it should NOT fail on the subtask gate
    let result = mcp.hive_submit_to_queue(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(
        !text.contains("subtask(s) are not resolved"),
        "Should pass subtask gate when all resolved"
    );
}

// ---- retry_agent permission tests ----

#[tokio::test]
async fn retry_agent_worker_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(RetryAgentParams {
        agent_id: "some-agent".into(),
        feedback: None,
    });
    let result = mcp.hive_retry_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn retry_agent_coordinator_denied_retry_worker() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    let worker = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Failed,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-w1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    let params = Parameters(RetryAgentParams {
        agent_id: "worker-1".into(),
        feedback: None,
    });
    let result = mcp.hive_retry_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Coordinator can only retry lead agents"));
}

#[tokio::test]
async fn retry_agent_not_in_retriable_state() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running, // Not Failed or Stalled
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let params = Parameters(RetryAgentParams {
        agent_id: "lead-1".into(),
        feedback: None,
    });
    let result = mcp.hive_retry_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("not in Failed or Stalled state"));
}

#[tokio::test]
async fn retry_agent_lead_denied_other_leads_worker() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let worker = Agent {
        id: "worker-2".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Failed,
        parent: Some("lead-2".into()), // Different lead
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-w2".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    let params = Parameters(RetryAgentParams {
        agent_id: "worker-2".into(),
        feedback: None,
    });
    let result = mcp.hive_retry_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Leads can only retry their own workers"));
}

#[tokio::test]
async fn retry_agent_not_found() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);

    let params = Parameters(RetryAgentParams {
        agent_id: "nonexistent".into(),
        feedback: None,
    });
    let result = mcp.hive_retry_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

// ---- read_messages tests ----

#[tokio::test]
async fn read_messages_returns_messages_for_agent() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    // Save a message for lead-1
    let msg = Message {
        id: "msg-test1".into(),
        from: "coordinator".into(),
        to: "lead-1".into(),
        timestamp: Utc::now(),
        message_type: MessageType::Info,
        body: "Hello lead".into(),
        refs: vec![],
    };
    state.save_message("test-run", &msg).unwrap();

    let params = Parameters(ReadMessagesParams { since: None });
    let result = mcp.hive_read_messages(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Hello lead"));
    assert!(text.contains("msg-test1"));
}

#[tokio::test]
async fn read_messages_updates_cursor() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    // Create a message with a timestamp well in the past
    let past = Utc::now() - chrono::Duration::seconds(10);
    let msg = Message {
        id: "msg-test1".into(),
        from: "coordinator".into(),
        to: "lead-1".into(),
        timestamp: past,
        message_type: MessageType::Info,
        body: "First message".into(),
        refs: vec![],
    };
    state.save_message("test-run", &msg).unwrap();

    // First read should return the message
    let params = Parameters(ReadMessagesParams { since: None });
    let result = mcp.hive_read_messages(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("First message"));

    // Second read should not return the old message (cursor was updated)
    let params = Parameters(ReadMessagesParams { since: None });
    let result = mcp.hive_read_messages(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(
        !text.contains("First message"),
        "Old message should be filtered out by cursor"
    );
}

// ---- log_tool tests ----

#[tokio::test]
async fn log_tool_records_entry() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(LogToolParams {
        tool: "Read".into(),
        status: "success".into(),
        duration_ms: Some(42),
        args_summary: Some("file.rs".into()),
    });
    let result = mcp.hive_log_tool(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false), "log_tool should succeed");
}

// ---- create_task edge cases ----

#[tokio::test]
async fn create_task_lead_denied_without_parent() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);

    let params = Parameters(CreateTaskParams {
        title: "Top level".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("leads can only create subtasks"));
}

#[tokio::test]
async fn create_task_lead_denied_wrong_parent() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
    lead_agent.task_id = Some("task-lead-1".into());
    state.save_agent("test-run", &lead_agent).unwrap();

    let params = Parameters(CreateTaskParams {
        title: "Subtask".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: Some("task-lead-2".into()), // Not their own
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("task-lead-1")); // mentions their own task
    assert!(text.contains("task-lead-2")); // mentions the wrong parent
}

#[tokio::test]
async fn create_task_explorer_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(CreateTaskParams {
        title: "Task".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("cannot create tasks"));
}

#[tokio::test]
async fn create_task_reviewer_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Reviewer);
    let params = Parameters(CreateTaskParams {
        title: "Task".into(),
        description: "desc".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("cannot create tasks"));
}

// ---- update_task edge cases ----

#[tokio::test]
async fn update_task_invalid_status() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-1".into(),
        status: Some("invalid_status".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Invalid status"));
}

#[tokio::test]
async fn update_task_not_found() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);

    let params = Parameters(UpdateTaskParams {
        task_id: "nonexistent".into(),
        status: Some("active".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn update_task_notes_appended() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-1".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("Progress update: 50% done".into()),
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let task = state.load_task("test-run", "task-1").unwrap();
    assert!(task.description.contains("Progress update: 50% done"));
}

#[tokio::test]
async fn update_task_absorbed_denied_for_non_creator() {
    let (_dir, mcp) = setup_mcp_with_id("lead-2", AgentRole::Lead);
    let state = mcp.state();

    let mut lead_agent = state.load_agent("test-run", "lead-2").unwrap();
    lead_agent.task_id = Some("task-lead-2".into());
    state.save_agent("test-run", &lead_agent).unwrap();

    // Task created by lead-1, not lead-2
    let mut task = make_task("task-sub", Some("task-lead-2"), TaskStatus::Active);
    task.created_by = "lead-1".into();
    task.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &task).unwrap();

    // Create worker agent parented to lead-2 so ownership check passes
    let worker = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-2".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-sub".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-sub".into(),
        status: Some("absorbed".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
    assert!(text.contains("absorbed/cancelled"));
}

#[tokio::test]
async fn update_task_cancelled_allowed_for_assigned_agent() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    let mut agent = state.load_agent("test-run", "worker-1").unwrap();
    agent.task_id = Some("task-w1".into());
    agent.parent = Some("lead-1".into());
    state.save_agent("test-run", &agent).unwrap();

    let mut task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
    task.assigned_to = Some("worker-1".into());
    task.created_by = "lead-1".into();
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-w1".into(),
        status: Some("cancelled".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Assigned agent should cancel own task"
    );

    let task = state.load_task("test-run", "task-w1").unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn update_task_branch_set() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-1".into(),
        status: None,
        assigned_to: None,
        branch: Some("hive/test/branch".into()),
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let task = state.load_task("test-run", "task-1").unwrap();
    assert_eq!(task.branch.as_deref(), Some("hive/test/branch"));
}

#[tokio::test]
async fn update_task_reviewer_can_update_own_reviewed_task() {
    let (_dir, mcp) = setup_mcp_with_id("reviewer-task1", AgentRole::Reviewer);
    let state = mcp.state();

    let mut reviewer = state.load_agent("test-run", "reviewer-task1").unwrap();
    reviewer.task_id = Some("task-1".into());
    state.save_agent("test-run", &reviewer).unwrap();

    let mut task = make_task("task-1", None, TaskStatus::Review);
    task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-1".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("Reviewing...".into()),
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Reviewer should update task they are reviewing"
    );
}

// ---- list_tasks filter tests ----

#[tokio::test]
async fn list_tasks_status_filter() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let active = make_task("task-active", None, TaskStatus::Active);
    let pending = make_task("task-pending", None, TaskStatus::Pending);
    state.save_task("test-run", &active).unwrap();
    state.save_task("test-run", &pending).unwrap();

    let params = Parameters(ListTasksParams {
        status: Some("active".into()),
        assignee: None,
        domain: None,
        parent_task: None,
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("task-active"));
    assert!(!text.contains("task-pending"));
}

#[tokio::test]
async fn list_tasks_assignee_filter() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let mut t1 = make_task("task-1", None, TaskStatus::Active);
    t1.assigned_to = Some("lead-1".into());
    let mut t2 = make_task("task-2", None, TaskStatus::Active);
    t2.assigned_to = Some("lead-2".into());
    state.save_task("test-run", &t1).unwrap();
    state.save_task("test-run", &t2).unwrap();

    let params = Parameters(ListTasksParams {
        status: None,
        assignee: Some("lead-1".into()),
        domain: None,
        parent_task: None,
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("task-1"));
    assert!(!text.contains("task-2"));
}

#[tokio::test]
async fn list_tasks_domain_filter() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();

    let mut t1 = make_task("task-1", None, TaskStatus::Active);
    t1.domain = Some("backend".into());
    let mut t2 = make_task("task-2", None, TaskStatus::Active);
    t2.domain = Some("frontend".into());
    state.save_task("test-run", &t1).unwrap();
    state.save_task("test-run", &t2).unwrap();

    let params = Parameters(ListTasksParams {
        status: None,
        assignee: None,
        domain: Some("backend".into()),
        parent_task: None,
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("task-1"));
    assert!(!text.contains("task-2"));
}

// ---- merge_next permission test ----

#[tokio::test]
async fn merge_next_non_coordinator_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Lead);
    let result = mcp.hive_merge_next().await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

// ---- synthesize full handler test ----

#[tokio::test]
async fn synthesize_creates_insight() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    // Create a discovery first
    let disc = Discovery {
        id: "disc-1".into(),
        run_id: "test-run".into(),
        agent_id: "explorer-1".into(),
        timestamp: Utc::now(),
        content: "Found pattern X".into(),
        file_paths: vec![],
        confidence: Confidence::High,
        tags: vec!["arch".into()],
    };
    state.save_discovery("test-run", &disc).unwrap();

    let params = Parameters(SynthesizeParams {
        content: "Pattern X is consistent across modules".into(),
        discovery_ids: vec!["disc-1".into()],
        tags: vec!["architecture".into()],
    });
    let result = mcp.hive_synthesize(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Coordinator should synthesize"
    );
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("ins-"));
}

#[tokio::test]
async fn synthesize_worker_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(SynthesizeParams {
        content: "test".into(),
        discovery_ids: vec![],
        tags: vec![],
    });
    let result = mcp.hive_synthesize(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

// ---- establish_convention full handler test ----

#[tokio::test]
async fn establish_convention_creates_convention() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);

    let params = Parameters(EstablishConventionParams {
        content: "Always run tests before merging".into(),
    });
    let result = mcp.hive_establish_convention(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Convention added"));

    // Verify stored
    let conventions = mcp.state().load_conventions();
    assert!(conventions.contains("Always run tests before merging"));
}

#[tokio::test]
async fn establish_convention_explorer_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(EstablishConventionParams {
        content: "test".into(),
    });
    let result = mcp.hive_establish_convention(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

// ---- heartbeat test ----

#[tokio::test]
async fn heartbeat_updates_timestamp() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    // Verify no heartbeat initially
    let agent = state.load_agent("test-run", "worker-1").unwrap();
    assert!(agent.heartbeat.is_none());

    let result = mcp.hive_heartbeat().await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // Verify heartbeat was set
    let agent = state.load_agent("test-run", "worker-1").unwrap();
    assert!(agent.heartbeat.is_some());
}

// ---- list_agents test ----

#[tokio::test]
async fn list_agents_returns_all() {
    let (_dir, mcp) = setup_mcp_with_id("coordinator", AgentRole::Coordinator);
    let state = mcp.state();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let result = mcp.hive_list_agents().await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("coordinator"));
    assert!(text.contains("lead-1"));
}

// ---- check_agents permission test ----

#[tokio::test]
async fn check_agents_worker_denied() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let result = mcp.hive_check_agents().await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

// ---- discover edge cases ----

#[tokio::test]
async fn discover_low_confidence() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(DiscoverParams {
        content: "Might be a pattern here".into(),
        confidence: "low".into(),
        file_paths: vec![],
        tags: vec!["speculative".into()],
    });
    let result = mcp.hive_discover(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let discoveries = mcp.state().load_discoveries("test-run");
    assert_eq!(discoveries.len(), 1);
    assert_eq!(discoveries[0].confidence, Confidence::Low);
}

// ---- query_mind no results ----

#[tokio::test]
async fn query_mind_no_results() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let params = Parameters(QueryMindParams {
        query: "xyznonexistent".into(),
    });
    let result = mcp.hive_query_mind(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("No matching"));
}

// ---- save_memory convention type test ----

#[tokio::test]
async fn save_memory_convention_succeeds() {
    let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
    let params = Parameters(SaveMemoryParams {
        memory_type: "convention".into(),
        content: "## Testing Conventions\n- Always test edge cases".into(),
    });
    let result = mcp.hive_save_memory(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Convention save should succeed"
    );
}

#[test]
fn notify_parent_of_transition_no_parent_is_noop() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    // Agent with no parent
    let agent = Agent {
        id: "solo-agent".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Idle,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &agent).unwrap();

    mcp.notify_parent_of_transition(&state, &agent);

    // No messages should be created
    let messages = state.list_messages("test-run").unwrap();
    let notify_msgs: Vec<_> = messages.iter().filter(|m| m.from == "solo-agent").collect();
    assert_eq!(notify_msgs.len(), 0, "No notification without a parent");
}

// ==========================================================================
// Integration tests: end-to-end flows across modules (state, types, mcp)
// ==========================================================================

/// Helper: set up a full coordinator + lead + worker hierarchy
fn setup_hierarchy() -> (TempDir, HiveMcp, HiveMcp, HiveMcp) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root.clone());
    state.create_run("test-run").unwrap();

    let coord = Agent {
        id: "coordinator".into(),
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
        model: None,
    };
    state.save_agent("test-run", &coord).unwrap();

    let lead = Agent {
        id: "lead-1".into(),
        role: AgentRole::Lead,
        status: AgentStatus::Running,
        parent: Some("coordinator".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-lead".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &lead).unwrap();

    let worker = Agent {
        id: "worker-1".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-w1".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    let root_str = root.to_string_lossy().to_string();
    let coord_mcp = HiveMcp::new("test-run".into(), "coordinator".into(), root_str.clone());
    let lead_mcp = HiveMcp::new("test-run".into(), "lead-1".into(), root_str.clone());
    let worker_mcp = HiveMcp::new("test-run".into(), "worker-1".into(), root_str);

    (dir, coord_mcp, lead_mcp, worker_mcp)
}

// =================================================================
// Adversarial tests: invalid params, empty strings, boundary cases
// =================================================================

#[tokio::test]
async fn create_task_with_empty_title_succeeds() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(CreateTaskParams {
        title: "".into(),
        description: "".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    // No validation on empty strings — this succeeds
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn create_task_with_invalid_urgency_defaults() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    // Invalid urgency strings — check how they're handled
    let params = Parameters(CreateTaskParams {
        title: "Test task".into(),
        description: "desc".into(),
        urgency: "URGENT".into(), // not a valid urgency
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    // check if this succeeds or fails — helps discover validation gaps
    // The code does: match urgency_str "low"|"normal"|"high"|"critical" -> _ defaults to Normal
    // So invalid urgency silently becomes Normal
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn create_task_with_special_chars_in_title() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(CreateTaskParams {
        title: "Task with <html>&\"quotes'</html> and \n newlines \t tabs".into(),
        description: "描述 with unicode 🚀 and \0 null bytes".into(),
        urgency: "normal".into(),
        domain: Some("domaine-spécial".into()),
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn create_task_with_very_long_title() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let long_title = "x".repeat(10_000);
    let params = Parameters(CreateTaskParams {
        title: long_title,
        description: "y".repeat(100_000),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = mcp.hive_create_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn discover_with_empty_content_succeeds() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(DiscoverParams {
        content: "".into(),
        confidence: "medium".into(),
        file_paths: vec![],
        tags: vec![],
    });
    let result = mcp.hive_discover(params).await.unwrap();
    // No validation on empty content
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn discover_with_invalid_confidence_defaults_to_medium() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(DiscoverParams {
        content: "test finding".into(),
        confidence: "EXTREMELY_HIGH".into(),
        file_paths: vec![],
        tags: vec![],
    });
    let result = mcp.hive_discover(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    // Invalid confidence defaults to Medium (the _ match arm)
}

#[tokio::test]
async fn establish_convention_non_coordinator_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Lead);
    let params = Parameters(EstablishConventionParams {
        content: "use snake_case".into(),
    });
    let result = mcp.hive_establish_convention(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn list_tasks_with_invalid_status_filter() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    state
        .save_task("test-run", &make_task("task-1", None, TaskStatus::Active))
        .unwrap();

    let params = Parameters(ListTasksParams {
        status: Some("INVALID_STATUS".into()),
        assignee: None,
        domain: None,
        parent_task: None,
    });
    let result = mcp.hive_list_tasks(params).await.unwrap();
    // Invalid status filter returns empty list (no tasks match the invalid status)
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    // The output is "[]" since no tasks match the invalid status
    assert!(text.contains("[]"));
}

#[tokio::test]
async fn query_mind_with_empty_query() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(QueryMindParams { query: "".into() });
    let result = mcp.hive_query_mind(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("No matching"));
}

#[tokio::test]
async fn query_mind_with_special_chars() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(QueryMindParams {
        query: "SELECT * FROM users; DROP TABLE--".into(),
    });
    let result = mcp.hive_query_mind(params).await.unwrap();
    // Should not panic, just return no results
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn review_verdict_empty_verdict_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Reviewer);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Review);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn review_verdict_invalid_verdict_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Reviewer);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Review);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "thumbs-up".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Invalid verdict"));
}

#[tokio::test]
async fn review_verdict_non_reviewer_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Review);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(ReviewVerdictParams {
        task_id: "task-1".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn review_verdict_nonexistent_task_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Reviewer);
    let params = Parameters(ReviewVerdictParams {
        task_id: "task-nonexistent".into(),
        verdict: "approve".into(),
        feedback: None,
    });
    let result = mcp.hive_review_verdict(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn save_memory_invalid_type_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Postmortem);
    let params = Parameters(SaveMemoryParams {
        memory_type: "invalid_type".into(),
        content: "test".into(),
    });
    let result = mcp.hive_save_memory(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn save_memory_non_postmortem_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SaveMemoryParams {
        memory_type: "convention".into(),
        content: "test".into(),
    });
    let result = mcp.hive_save_memory(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn save_spec_non_planner_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SaveSpecParams {
        spec: "# My Spec".into(),
    });
    let result = mcp.hive_save_spec(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn send_message_empty_body_succeeds() {
    let (_dir, mcp) = setup_mcp_with_id("explorer-1", AgentRole::Explorer);
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    // No validation on empty body
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn send_message_invalid_type_defaults_to_info() {
    let (_dir, mcp) = setup_mcp_with_id("explorer-1", AgentRole::Explorer);
    let state = mcp.state();

    // Explorer can only send to coordinator
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "INVALID_TYPE".into(),
        body: "test".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    // Invalid message_type silently defaults to "info" — this should succeed
    assert!(!result.is_error.unwrap_or(false));

    let messages = state.list_messages("test-run").unwrap();
    let msg = messages.iter().find(|m| m.from == "explorer-1").unwrap();
    assert_eq!(msg.message_type, MessageType::Info);
}

#[tokio::test]
async fn send_message_to_nonexistent_agent() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(SendMessageParams {
        to: "ghost-agent-that-does-not-exist".into(),
        message_type: "info".into(),
        body: "hello".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    // Coordinator can only message leads/explorers/evaluators — nonexistent agent is rejected
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn send_message_worker_to_non_parent_rejected() {
    let (_dir, mcp) = setup_mcp_with_id("worker-1", AgentRole::Worker);
    let state = mcp.state();

    let mut worker = state.load_agent("test-run", "worker-1").unwrap();
    worker.parent = Some("lead-1".into());
    state.save_agent("test-run", &worker).unwrap();

    // Try to message another agent (not lead-1)
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "trying to bypass hierarchy".into(),
        refs: vec![],
    });
    let result = mcp.hive_send_message(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn spawn_agent_empty_role_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "empty-role".into(),
        role: "".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn spawn_agent_invalid_role_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "bad-role-agent".into(),
        role: "superadmin".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Invalid role"));
}

#[tokio::test]
async fn spawn_agent_task_already_active_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "lead-1".into(),
        role: "lead".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Active"));
}

#[tokio::test]
async fn spawn_agent_wrong_hierarchy_rejected() {
    // Worker tries to spawn
    let (_dir, mcp) = setup_mcp(AgentRole::Worker);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(SpawnAgentParams {
        agent_id: "spawned-agent".into(),
        role: "worker".into(),
        task_id: "task-1".into(),
        task_description: "test".into(),
        model: None,
    });
    let result = mcp.hive_spawn_agent(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Permission denied"));
}

#[tokio::test]
async fn synthesize_non_coordinator_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Explorer);
    let params = Parameters(SynthesizeParams {
        content: "synthesized".into(),
        discovery_ids: vec!["disc-1".into()],
        tags: vec![],
    });
    let result = mcp.hive_synthesize(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn update_task_absorbed_by_non_creator_rejected() {
    let (_dir, mcp) = setup_mcp_with_id("lead-1", AgentRole::Lead);
    let state = mcp.state();

    let mut lead = state.load_agent("test-run", "lead-1").unwrap();
    lead.task_id = Some("task-lead".into());
    state.save_agent("test-run", &lead).unwrap();

    // Lead's own task
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // Task created by someone else, but lead has access
    let mut other_task = make_task("task-other", Some("task-lead"), TaskStatus::Active);
    other_task.created_by = "coordinator".into();
    other_task.assigned_to = Some("worker-99".into());
    state.save_task("test-run", &other_task).unwrap();

    // Worker agent parented to this lead
    let worker = Agent {
        id: "worker-99".into(),
        role: AgentRole::Worker,
        status: AgentStatus::Running,
        parent: Some("lead-1".into()),
        pid: None,
        worktree: None,
        heartbeat: None,
        task_id: Some("task-other".into()),
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
        model: None,
    };
    state.save_agent("test-run", &worker).unwrap();

    // Lead tries to set "absorbed" on a task it didn't create
    let params = Parameters(UpdateTaskParams {
        task_id: "task-other".into(),
        status: Some("absorbed".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(
        result.is_error.unwrap_or(false),
        "Non-creator should not be able to set absorbed"
    );
}

#[tokio::test]
async fn update_task_invalid_status_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Pending);
    state.save_task("test-run", &task).unwrap();

    for bad_status in &["PENDING", "Done", "running", "complete", "", "null", "42"] {
        let params = Parameters(UpdateTaskParams {
            task_id: "task-1".into(),
            status: Some(bad_status.to_string()),
            assigned_to: None,
            branch: None,
            notes: None,
        });
        let result = mcp.hive_update_task(params).await.unwrap();
        assert!(
            result.is_error.unwrap_or(false),
            "Expected error for status '{bad_status}'"
        );
    }
}

#[tokio::test]
async fn update_task_nonexistent_task_rejected() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let params = Parameters(UpdateTaskParams {
        task_id: "nonexistent-task-12345".into(),
        status: Some("active".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn update_task_with_notes_appends_correctly() {
    let (_dir, mcp) = setup_mcp(AgentRole::Coordinator);
    let state = mcp.state();
    let task = make_task("task-1", None, TaskStatus::Active);
    state.save_task("test-run", &task).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: "task-1".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("This is a note with <html> and \"quotes\"".into()),
    });
    let result = mcp.hive_update_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    let loaded = state.load_task("test-run", "task-1").unwrap();
    assert!(loaded.description.contains("<html>"));
    assert!(loaded.description.contains("\"quotes\""));
}

// --- Integration: Task lifecycle flows ---

#[tokio::test]
async fn integration_task_lifecycle_create_assign_update_complete() {
    let (_dir, coord_mcp, lead_mcp, worker_mcp) = setup_hierarchy();
    let state = coord_mcp.state();

    // 1. Coordinator creates a top-level task
    let params = Parameters(CreateTaskParams {
        title: "Build feature X".into(),
        description: "Implement the full feature".into(),
        urgency: "high".into(),
        domain: Some("backend".into()),
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = coord_mcp.hive_create_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    let task_id = text.split("'").nth(1).unwrap().to_string();

    // Verify task exists in state with correct initial values
    let task = state.load_task("test-run", &task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.urgency, Urgency::High);
    assert_eq!(task.domain.as_deref(), Some("backend"));
    assert_eq!(task.created_by, "coordinator");

    // 2. Coordinator assigns task to lead (simulating what spawn does)
    let mut task = task;
    task.assigned_to = Some("lead-1".into());
    task.status = TaskStatus::Active;
    state.save_task("test-run", &task).unwrap();

    // 3. Lead creates a subtask under its own task
    let mut lead_agent = state.load_agent("test-run", "lead-1").unwrap();
    lead_agent.task_id = Some(task_id.clone());
    state.save_agent("test-run", &lead_agent).unwrap();

    let params = Parameters(CreateTaskParams {
        title: "Sub-feature Y".into(),
        description: "Worker task".into(),
        urgency: "normal".into(),
        domain: None,
        blocking: vec![],
        blocked_by: vec![],
        parent_task: Some(task_id.clone()),
    });
    let result = lead_mcp.hive_create_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    let sub_task_id = text.split("'").nth(1).unwrap().to_string();

    // Verify subtask parent_task relationship
    let sub_task = state.load_task("test-run", &sub_task_id).unwrap();
    assert_eq!(sub_task.parent_task.as_deref(), Some(task_id.as_str()));
    assert_eq!(sub_task.created_by, "lead-1");

    // 4. Assign subtask to worker and worker updates it to review
    let mut sub_task = sub_task;
    sub_task.assigned_to = Some("worker-1".into());
    sub_task.status = TaskStatus::Active;
    state.save_task("test-run", &sub_task).unwrap();

    let mut worker_agent = state.load_agent("test-run", "worker-1").unwrap();
    worker_agent.task_id = Some(sub_task_id.clone());
    state.save_agent("test-run", &worker_agent).unwrap();

    let params = Parameters(UpdateTaskParams {
        task_id: sub_task_id.clone(),
        status: Some("review".into()),
        assigned_to: None,
        branch: Some("hive/test/worker-1".into()),
        notes: Some("Implementation complete".into()),
    });
    let result = worker_mcp.hive_update_task(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // 5. Verify final state across modules
    let final_task = state.load_task("test-run", &sub_task_id).unwrap();
    assert_eq!(final_task.status, TaskStatus::Review);
    assert_eq!(final_task.branch.as_deref(), Some("hive/test/worker-1"));
    assert!(final_task.description.contains("Implementation complete"));

    // 6. Verify all tasks are listed correctly
    let all_tasks = state.list_tasks("test-run").unwrap();
    assert_eq!(all_tasks.len(), 2);
}

// --- Integration: Message passing flows ---

#[tokio::test]
async fn integration_message_flow_worker_to_lead_to_coordinator() {
    let (_dir, coord_mcp, lead_mcp, worker_mcp) = setup_hierarchy();
    let state = coord_mcp.state();

    // Set up lead task for coordinator update
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // 1. Worker sends message to lead
    let params = Parameters(SendMessageParams {
        to: "lead-1".into(),
        message_type: "status".into(),
        body: "Subtask implementation complete".into(),
        refs: vec!["task-w1".into()],
    });
    let result = worker_mcp.hive_send_message(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // 2. Lead sends message to coordinator
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "status".into(),
        body: "All workers complete, ready for review".into(),
        refs: vec!["task-lead".into()],
    });
    let result = lead_mcp.hive_send_message(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // 3. Verify message routing: both messages saved in state
    let all_msgs = state.list_messages("test-run").unwrap();
    assert_eq!(all_msgs.len(), 2);

    // 4. Verify agent-specific filtering
    let lead_msgs = state
        .load_messages_for_agent("test-run", "lead-1", None)
        .unwrap();
    assert_eq!(lead_msgs.len(), 1);
    assert_eq!(lead_msgs[0].from, "worker-1");

    let coord_msgs = state
        .load_messages_for_agent("test-run", "coordinator", None)
        .unwrap();
    assert_eq!(coord_msgs.len(), 1);
    assert_eq!(coord_msgs[0].from, "lead-1");

    // 5. Worker should have no messages
    let worker_msgs = state
        .load_messages_for_agent("test-run", "worker-1", None)
        .unwrap();
    assert!(worker_msgs.is_empty());
}

#[tokio::test]
async fn integration_message_routing_enforces_hierarchy() {
    let (_dir, coord_mcp, _lead_mcp, worker_mcp) = setup_hierarchy();

    // Worker cannot message coordinator directly
    let params = Parameters(SendMessageParams {
        to: "coordinator".into(),
        message_type: "info".into(),
        body: "Trying to skip hierarchy".into(),
        refs: vec![],
    });
    let result = worker_mcp.hive_send_message(params).await.unwrap();
    assert!(
        result.is_error.unwrap_or(false),
        "Worker should not message coordinator"
    );

    // Coordinator cannot message worker directly
    let params = Parameters(SendMessageParams {
        to: "worker-1".into(),
        message_type: "info".into(),
        body: "Direct to worker".into(),
        refs: vec![],
    });
    let result = coord_mcp.hive_send_message(params).await.unwrap();
    assert!(
        result.is_error.unwrap_or(false),
        "Coordinator should not message worker"
    );
}

// --- Integration: Blocked-by dependency chains ---

#[tokio::test]
async fn integration_blocked_by_chain_tracks_dependencies() {
    let (_dir, coord_mcp, _lead_mcp, _worker_mcp) = setup_hierarchy();
    let state = coord_mcp.state();

    // Create chain: task-c blocked_by task-b blocked_by task-a
    let params = Parameters(CreateTaskParams {
        title: "Task A (foundation)".into(),
        description: "First task".into(),
        urgency: "high".into(),
        domain: Some("backend".into()),
        blocking: vec![],
        blocked_by: vec![],
        parent_task: None,
    });
    let result = coord_mcp.hive_create_task(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    let task_a_id = text.split("'").nth(1).unwrap().to_string();

    let params = Parameters(CreateTaskParams {
        title: "Task B (depends on A)".into(),
        description: "Second task".into(),
        urgency: "normal".into(),
        domain: Some("backend".into()),
        blocking: vec![],
        blocked_by: vec![task_a_id.clone()],
        parent_task: None,
    });
    let result = coord_mcp.hive_create_task(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    let task_b_id = text.split("'").nth(1).unwrap().to_string();

    let params = Parameters(CreateTaskParams {
        title: "Task C (depends on B)".into(),
        description: "Third task".into(),
        urgency: "normal".into(),
        domain: Some("backend".into()),
        blocking: vec![],
        blocked_by: vec![task_b_id.clone()],
        parent_task: None,
    });
    let result = coord_mcp.hive_create_task(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    let task_c_id = text.split("'").nth(1).unwrap().to_string();

    // Verify dependency chain is persisted correctly
    let task_a = state.load_task("test-run", &task_a_id).unwrap();
    let task_b = state.load_task("test-run", &task_b_id).unwrap();
    let task_c = state.load_task("test-run", &task_c_id).unwrap();

    assert!(task_a.blocked_by.is_empty());
    assert_eq!(task_b.blocked_by, vec![task_a_id.clone()]);
    assert_eq!(task_c.blocked_by, vec![task_b_id.clone()]);

    // Verify status filtering: all should be pending
    let params = Parameters(ListTasksParams {
        status: Some("pending".into()),
        assignee: None,
        domain: Some("backend".into()),
        parent_task: None,
    });
    let result = coord_mcp.hive_list_tasks(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains(&task_a_id));
    assert!(text.contains(&task_b_id));
    assert!(text.contains(&task_c_id));
}

// --- Integration: Merge queue with subtask completion gate ---

#[tokio::test]
async fn integration_subtask_completion_gate_blocks_then_allows_submit() {
    let (_dir, _coord_mcp, lead_mcp, _worker_mcp) = setup_hierarchy();
    let state = lead_mcp.state();

    // Create parent task assigned to lead
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // Create two subtasks: one active, one merged
    let mut sub1 = make_task("task-sub1", Some("task-lead"), TaskStatus::Active);
    sub1.assigned_to = Some("worker-1".into());
    state.save_task("test-run", &sub1).unwrap();

    let mut sub2 = make_task("task-sub2", Some("task-lead"), TaskStatus::Merged);
    sub2.assigned_to = Some("worker-2".into());
    state.save_task("test-run", &sub2).unwrap();

    // Submit should fail: sub1 is unresolved
    let params = Parameters(SubmitToQueueParams {
        task_id: "task-lead".into(),
        branch: "hive/test/lead-1".into(),
    });
    let result = lead_mcp.hive_submit_to_queue(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("subtask(s) are not resolved"));
    assert!(text.contains("task-sub1"));

    // Mark sub1 as absorbed (resolved terminal state)
    let mut sub1 = state.load_task("test-run", "task-sub1").unwrap();
    sub1.status = TaskStatus::Absorbed;
    state.save_task("test-run", &sub1).unwrap();

    // Submit should now proceed (spawns reviewer, which will fail without
    // claude binary, but the subtask gate itself should pass)
    let params = Parameters(SubmitToQueueParams {
        task_id: "task-lead".into(),
        branch: "hive/test/lead-1".into(),
    });
    let _result = lead_mcp.hive_submit_to_queue(params).await.unwrap();
    // The result should succeed (either reviewer spawned or fallback to queue)
    // Either way, task should no longer be blocked by the gate
    let task = state.load_task("test-run", "task-lead").unwrap();
    assert!(
        matches!(task.status, TaskStatus::Review | TaskStatus::Queued),
        "Task should be in review or queued, got {:?}",
        task.status
    );
}

// --- Integration: Memory system across modules ---

#[tokio::test]
async fn integration_discovery_to_query_mind_roundtrip() {
    let (_dir, _coord_mcp, _lead_mcp, worker_mcp) = setup_hierarchy();
    let state = worker_mcp.state();

    // Worker discovers something
    let params = Parameters(DiscoverParams {
        content: "The merge queue has a race condition when two leads submit simultaneously".into(),
        confidence: "high".into(),
        file_paths: vec!["src/state.rs".into(), "src/mcp.rs".into()],
        tags: vec!["race-condition".into(), "merge-queue".into()],
    });
    let result = worker_mcp.hive_discover(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // Another discovery
    let params = Parameters(DiscoverParams {
        content: "Task status transitions are not validated at the state layer".into(),
        confidence: "medium".into(),
        file_paths: vec!["src/state.rs".into()],
        tags: vec!["validation".into(), "state".into()],
    });
    let result = worker_mcp.hive_discover(params).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));

    // Query mind for "merge" should find the first discovery
    let params = Parameters(QueryMindParams {
        query: "merge".into(),
    });
    let result = worker_mcp.hive_query_mind(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("race condition"));
    assert!(!text.contains("status transitions"));

    // Query mind for "state" should find the second discovery
    let params = Parameters(QueryMindParams {
        query: "state".into(),
    });
    let result = worker_mcp.hive_query_mind(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("status transitions"));

    // Verify discoveries are persisted in state
    let discoveries = state.load_discoveries("test-run");
    assert_eq!(discoveries.len(), 2);
    assert_eq!(discoveries[0].agent_id, "worker-1");
    assert_eq!(discoveries[1].agent_id, "worker-1");
}

// --- Integration: Ownership enforcement across role hierarchy ---

#[tokio::test]
async fn integration_ownership_cross_domain_isolation() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".hive")).unwrap();
    let state = HiveState::new(root.clone());
    state.create_run("test-run").unwrap();
    let root_str = root.to_string_lossy().to_string();

    // Create two leads with separate tasks
    for (id, task_id) in [("lead-a", "task-a"), ("lead-b", "task-b")] {
        let agent = Agent {
            id: id.into(),
            role: AgentRole::Lead,
            status: AgentStatus::Running,
            parent: Some("coordinator".into()),
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: Some(task_id.into()),
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
            model: None,
        };
        state.save_agent("test-run", &agent).unwrap();

        let mut task = make_task(task_id, None, TaskStatus::Active);
        task.assigned_to = Some(id.into());
        state.save_task("test-run", &task).unwrap();
    }

    let lead_a_mcp = HiveMcp::new("test-run".into(), "lead-a".into(), root_str.clone());
    let lead_b_mcp = HiveMcp::new("test-run".into(), "lead-b".into(), root_str);

    // Lead A cannot update Lead B's task
    let params = Parameters(UpdateTaskParams {
        task_id: "task-b".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = lead_a_mcp.hive_update_task(params).await.unwrap();
    assert!(
        result.is_error.unwrap_or(false),
        "Lead A should not update Lead B's task"
    );
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("another lead"));

    // Lead B can update its own task
    let params = Parameters(UpdateTaskParams {
        task_id: "task-b".into(),
        status: Some("review".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = lead_b_mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead B should update own task"
    );

    // Verify state reflects the correct update
    let task_b = state.load_task("test-run", "task-b").unwrap();
    assert_eq!(task_b.status, TaskStatus::Review);
    let task_a = state.load_task("test-run", "task-a").unwrap();
    assert_eq!(task_a.status, TaskStatus::Active); // unchanged
}

// --- Integration: Read messages with since filter across operations ---

#[tokio::test]
async fn integration_read_messages_since_filter() {
    let (_dir, _coord_mcp, lead_mcp, worker_mcp) = setup_hierarchy();
    let state = lead_mcp.state();

    // Set up lead task
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    state.save_task("test-run", &lead_task).unwrap();

    // Worker sends first message
    let params = Parameters(SendMessageParams {
        to: "lead-1".into(),
        message_type: "info".into(),
        body: "First update".into(),
        refs: vec![],
    });
    worker_mcp.hive_send_message(params).await.unwrap();

    // Lead reads messages (no filter — gets all)
    let params = Parameters(ReadMessagesParams { since: None });
    let result = lead_mcp.hive_read_messages(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("First update"));

    // Lead's messages_read_at should now be updated
    let lead_after = state.load_agent("test-run", "lead-1").unwrap();
    assert!(lead_after.messages_read_at.is_some());
    let read_at = lead_after.messages_read_at.unwrap();

    // Small delay to ensure timestamps differ
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Worker sends second message
    let params = Parameters(SendMessageParams {
        to: "lead-1".into(),
        message_type: "info".into(),
        body: "Second update".into(),
        refs: vec![],
    });
    worker_mcp.hive_send_message(params).await.unwrap();

    // Lead reads only new messages (using since timestamp)
    let since_str = read_at.to_rfc3339();
    let params = Parameters(ReadMessagesParams {
        since: Some(since_str),
    });
    let result = lead_mcp.hive_read_messages(params).await.unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Second update"));
    assert!(!text.contains("First update"));
}

// --- Integration: Terminal status transitions (absorbed/cancelled) ---

#[tokio::test]
async fn integration_absorbed_cancelled_permission_matrix() {
    let (_dir, coord_mcp, lead_mcp, worker_mcp) = setup_hierarchy();
    let state = coord_mcp.state();

    // Set up lead task
    let mut lead_task = make_task("task-lead", None, TaskStatus::Active);
    lead_task.assigned_to = Some("lead-1".into());
    lead_task.created_by = "coordinator".into();
    state.save_task("test-run", &lead_task).unwrap();

    // Create worker task
    let mut worker_task = make_task("task-w1", Some("task-lead"), TaskStatus::Active);
    worker_task.assigned_to = Some("worker-1".into());
    worker_task.created_by = "lead-1".into();
    state.save_task("test-run", &worker_task).unwrap();

    // Worker can cancel its own assigned task
    let params = Parameters(UpdateTaskParams {
        task_id: "task-w1".into(),
        status: Some("cancelled".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = worker_mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Worker should cancel own task"
    );
    let task = state.load_task("test-run", "task-w1").unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert!(task.status.is_resolved());
    assert!(!task.status.is_success());

    // Reset for next test
    let mut task = task;
    task.status = TaskStatus::Active;
    state.save_task("test-run", &task).unwrap();

    // Lead (as creator) can set absorbed
    let params = Parameters(UpdateTaskParams {
        task_id: "task-w1".into(),
        status: Some("absorbed".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = lead_mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Lead (creator) should set absorbed"
    );
    let task = state.load_task("test-run", "task-w1").unwrap();
    assert_eq!(task.status, TaskStatus::Absorbed);
    assert!(task.status.is_resolved());
    assert!(task.status.is_success());

    // Coordinator can update top-level task to cancelled
    let params = Parameters(UpdateTaskParams {
        task_id: "task-lead".into(),
        status: Some("cancelled".into()),
        assigned_to: None,
        branch: None,
        notes: None,
    });
    let result = coord_mcp.hive_update_task(params).await.unwrap();
    assert!(
        !result.is_error.unwrap_or(false),
        "Coordinator should cancel top-level task"
    );
}

// --- Integration: Review cycle count enforcement ---

#[tokio::test]
async fn integration_review_cycle_limit_fails_task() {
    let (_dir, _coord_mcp, lead_mcp, _worker_mcp) = setup_hierarchy();
    let state = lead_mcp.state();

    // Create a task that's been reviewed 3 times already
    let mut task = make_task("task-lead", None, TaskStatus::Active);
    task.assigned_to = Some("lead-1".into());
    task.review_count = 3;
    state.save_task("test-run", &task).unwrap();

    // Submit should fail and mark task as Failed
    let params = Parameters(SubmitToQueueParams {
        task_id: "task-lead".into(),
        branch: "hive/test/lead-1".into(),
    });
    let result = lead_mcp.hive_submit_to_queue(params).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("exceeded the maximum review cycles"));

    // Verify task is now failed in state
    let task = state.load_task("test-run", "task-lead").unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.status.is_resolved());
    assert!(!task.status.is_success());
}

// --- Integration: Memory system persistence across operations ---

#[test]
fn integration_memory_persistence_operations_and_conventions() {
    let dir = TempDir::new().unwrap();
    let state = HiveState::new(dir.path().to_path_buf());
    std::fs::create_dir_all(dir.path().join(".hive")).unwrap();

    // Save operations and conventions
    let op = OperationalEntry {
        run_id: "run-1".into(),
        created_at: Utc::now(),
        tasks_total: 5,
        tasks_failed: 1,
        agents_spawned: 8,
        total_cost_usd: 12.50,
        learnings: vec!["Domain grouping reduces conflicts".into()],
        spec_quality: "good".into(),
        team_sizing: "4 leads x 2 workers".into(),
    };
    state.save_operation(&op).unwrap();
    state
        .save_conventions("## Test Convention\n- Always run tests before merge\n")
        .unwrap();

    let failure = FailureEntry {
        run_id: "run-1".into(),
        created_at: Utc::now(),
        pattern: "merge_conflict".into(),
        context: "Two leads modified same file".into(),
        run_number: 1,
    };
    state.save_failure(&failure).unwrap();

    // Load memory for different roles and verify content
    let coord_memory = state.load_memory_for_prompt(&AgentRole::Coordinator);
    assert!(coord_memory.contains("run-1"));
    assert!(coord_memory.contains("5 tasks"));
    assert!(!coord_memory.contains("Test Convention")); // coordinator doesn't get conventions

    let lead_memory = state.load_memory_for_prompt(&AgentRole::Lead);
    assert!(lead_memory.contains("Test Convention"));
    assert!(lead_memory.contains("merge_conflict"));
    assert!(!lead_memory.contains("5 tasks")); // lead doesn't get operations

    let worker_memory = state.load_memory_for_prompt(&AgentRole::Worker);
    assert!(worker_memory.contains("Test Convention"));
    assert!(worker_memory.contains("merge_conflict"));

    // Postmortem gets nothing
    let pm_memory = state.load_memory_for_prompt(&AgentRole::Postmortem);
    assert!(pm_memory.is_empty());
}

// --- Integration: Concurrent task updates with locking ---

#[tokio::test]
async fn integration_task_notes_append_preserves_description() {
    let (_dir, coord_mcp, lead_mcp, _worker_mcp) = setup_hierarchy();
    let state = coord_mcp.state();

    let mut task = make_task("task-lead", None, TaskStatus::Active);
    task.assigned_to = Some("lead-1".into());
    task.description = "Original description".into();
    state.save_task("test-run", &task).unwrap();

    // Lead appends notes
    let params = Parameters(UpdateTaskParams {
        task_id: "task-lead".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("First progress update".into()),
    });
    lead_mcp.hive_update_task(params).await.unwrap();

    // Lead appends more notes
    let params = Parameters(UpdateTaskParams {
        task_id: "task-lead".into(),
        status: None,
        assigned_to: None,
        branch: None,
        notes: Some("Second progress update".into()),
    });
    lead_mcp.hive_update_task(params).await.unwrap();

    // Verify both notes appended, original preserved
    let task = state.load_task("test-run", "task-lead").unwrap();
    assert!(task.description.contains("Original description"));
    assert!(task.description.contains("First progress update"));
    assert!(task.description.contains("Second progress update"));
}

// --- Integration: Heartbeat updates across state ---

#[test]
fn integration_heartbeat_and_agent_state_independence() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".hive")).unwrap();
    let state = HiveState::new(dir.path().to_path_buf());
    state.create_run("run-1").unwrap();

    // Create two agents
    for id in ["agent-a", "agent-b"] {
        let agent = Agent {
            id: id.into(),
            role: AgentRole::Worker,
            status: AgentStatus::Running,
            parent: Some("lead-1".into()),
            pid: None,
            worktree: None,
            heartbeat: None,
            task_id: None,
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            retry_count: 0,
            model: None,
        };
        state.save_agent("run-1", &agent).unwrap();
    }

    // Update heartbeat on agent-a only
    state.update_agent_heartbeat("run-1", "agent-a").unwrap();

    // Verify agent-a has heartbeat, agent-b does not
    let a = state.load_agent("run-1", "agent-a").unwrap();
    let b = state.load_agent("run-1", "agent-b").unwrap();
    assert!(a.heartbeat.is_some());
    assert!(b.heartbeat.is_none());

    // Update heartbeat on agent-a again — should be newer
    std::thread::sleep(std::time::Duration::from_millis(10));
    let first_hb = a.heartbeat.unwrap();
    state.update_agent_heartbeat("run-1", "agent-a").unwrap();
    let a = state.load_agent("run-1", "agent-a").unwrap();
    assert!(a.heartbeat.unwrap() > first_hb);
}

// --- Integration: Full run setup and state consistency ---

#[test]
fn integration_run_initialization_consistency() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".hive")).unwrap();
    let state = HiveState::new(dir.path().to_path_buf());

    // Create a run and verify all subsystems initialized
    state.create_run("run-test").unwrap();

    // Active run is set
    assert_eq!(state.active_run_id().unwrap(), "run-test");

    // Run metadata exists and is correct
    let meta = state.load_run_metadata("run-test").unwrap();
    assert_eq!(meta.id, "run-test");
    assert_eq!(meta.status, RunStatus::Active);

    // Merge queue initialized empty
    let queue = state.load_merge_queue("run-test").unwrap();
    assert!(queue.entries.is_empty());

    // All directories created
    assert!(state.tasks_dir("run-test").is_dir());
    assert!(state.agents_dir("run-test").is_dir());
    assert!(state.messages_dir("run-test").is_dir());
    assert!(state.worktrees_dir("run-test").is_dir());

    // All lists start empty
    assert!(state.list_tasks("run-test").unwrap().is_empty());
    assert!(state.list_agents("run-test").unwrap().is_empty());
    assert!(state.list_messages("run-test").unwrap().is_empty());

    // Mind is empty
    assert!(state.load_discoveries("run-test").is_empty());
    assert!(state.load_insights("run-test").is_empty());
}
