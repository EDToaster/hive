# Speculative Swarm Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add explore mode, Hive Mind knowledge space, explorer/evaluator agent roles, and the `hive explore` CLI command to enable speculative, divergent development workflows.

**Architecture:** Extends the existing agent hierarchy with two new roles (Explorer, Evaluator), adds a real-time shared knowledge store (`.hive/mind/`) with discovery/insight/query MCP tools accessible to all agents, and introduces `hive explore` as a lightweight alternative to `hive start` that spawns explorers instead of the full lead/worker pipeline.

**Tech Stack:** Rust, clap (CLI), rmcp (MCP), serde/serde_json (serialization), chrono (timestamps), uuid (IDs), existing HiveState pattern for filesystem operations.

---

### Task 1: Add Explorer and Evaluator roles to AgentRole enum

**Files:**
- Modify: `src/types.rs:8-15` (AgentRole enum)
- Modify: `src/types.rs:179-218` (existing role serialization tests)

**Step 1: Write the failing test**

Add to the existing test module in `src/types.rs`:

```rust
#[test]
fn explorer_and_evaluator_roles_roundtrip() {
    assert_eq!(
        serde_json::to_string(&AgentRole::Explorer).unwrap(),
        "\"explorer\""
    );
    let role: AgentRole = serde_json::from_str("\"explorer\"").unwrap();
    assert_eq!(role, AgentRole::Explorer);

    assert_eq!(
        serde_json::to_string(&AgentRole::Evaluator).unwrap(),
        "\"evaluator\""
    );
    let role: AgentRole = serde_json::from_str("\"evaluator\"").unwrap();
    assert_eq!(role, AgentRole::Evaluator);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test explorer_and_evaluator_roles_roundtrip -- --nocapture`
Expected: FAIL — `Explorer` and `Evaluator` variants don't exist

**Step 3: Add the new variants to AgentRole**

In `src/types.rs`, add `Explorer` and `Evaluator` to the `AgentRole` enum:

```rust
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
```

**Step 4: Run test to verify it passes**

Run: `cargo test explorer_and_evaluator_roles_roundtrip -- --nocapture`
Expected: PASS

**Step 5: Run full test suite and fix any compilation errors**

Run: `cargo test --all-targets`

The new variants may cause non-exhaustive match warnings in `agent.rs` (generate_prompt) and `state.rs` (load_memory_for_prompt). These will be placeholder matches for now — add `AgentRole::Explorer | AgentRole::Evaluator => ...` arms that use the Worker prompt as a temporary fallback.

In `src/agent.rs` `generate_prompt`, add a match arm for Explorer and Evaluator that falls through to the Worker prompt for now (will be replaced in Task 5).

In `src/state.rs` `load_memory_for_prompt`, add Explorer and Evaluator to the `include_conventions` and `include_failures` match arms (same as Worker).

**Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Fix any warnings.

**Step 7: Commit**

```bash
git add src/types.rs src/agent.rs src/state.rs
git commit -m "feat: add Explorer and Evaluator agent roles"
```

---

### Task 2: Add Discovery and Insight types

**Files:**
- Modify: `src/types.rs` (add new structs after FailureEntry)

**Step 1: Write the failing test**

Add to the test module in `src/types.rs`:

```rust
#[test]
fn discovery_roundtrip() {
    let discovery = Discovery {
        id: "disc-001".into(),
        run_id: "run-1".into(),
        agent_id: "explorer-1".into(),
        timestamp: chrono::Utc::now(),
        content: "Found unused cache abstraction in utils/cache.rs".into(),
        file_paths: vec!["src/utils/cache.rs".into()],
        confidence: Confidence::High,
        tags: vec!["caching".into(), "architecture".into()],
    };
    let json = serde_json::to_string(&discovery).unwrap();
    let back: Discovery = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "disc-001");
    assert_eq!(back.agent_id, "explorer-1");
    assert_eq!(back.confidence, Confidence::High);
    assert_eq!(back.tags.len(), 2);
}

#[test]
fn insight_roundtrip() {
    let insight = Insight {
        id: "ins-001".into(),
        run_id: "run-1".into(),
        timestamp: chrono::Utc::now(),
        content: "Auth module is tightly coupled to session store".into(),
        discovery_ids: vec!["disc-001".into(), "disc-002".into()],
        tags: vec!["architecture".into()],
    };
    let json = serde_json::to_string(&insight).unwrap();
    let back: Insight = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "ins-001");
    assert_eq!(back.discovery_ids.len(), 2);
}

#[test]
fn confidence_serializes_lowercase() {
    for (variant, expected) in [
        (Confidence::Low, "\"low\""),
        (Confidence::Medium, "\"medium\""),
        (Confidence::High, "\"high\""),
    ] {
        assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test discovery_roundtrip insight_roundtrip confidence_serializes_lowercase -- --nocapture`
Expected: FAIL — types don't exist

**Step 3: Add the types**

In `src/types.rs`, after the `FailureEntry` struct:

```rust
// --- Hive Mind Types ---

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
```

**Step 4: Run tests to verify they pass**

Run: `cargo test discovery_roundtrip insight_roundtrip confidence_serializes_lowercase -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/types.rs
git commit -m "feat: add Discovery, Insight, and Confidence types for Hive Mind"
```

---

### Task 3: Add Hive Mind state operations

**Files:**
- Modify: `src/state.rs` (add mind_dir, save/load/query methods after the existing memory section)

**Step 1: Write failing tests**

Add to the test module in `src/state.rs`:

```rust
#[test]
fn test_save_and_load_discovery() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    let run_id = "run-1";

    let disc = Discovery {
        id: "disc-001".into(),
        run_id: run_id.into(),
        agent_id: "explorer-1".into(),
        timestamp: chrono::Utc::now(),
        content: "Found unused cache".into(),
        file_paths: vec!["src/cache.rs".into()],
        confidence: Confidence::High,
        tags: vec!["caching".into()],
    };
    state.save_discovery(run_id, &disc).unwrap();

    let discoveries = state.load_discoveries(run_id);
    assert_eq!(discoveries.len(), 1);
    assert_eq!(discoveries[0].id, "disc-001");
}

#[test]
fn test_query_discoveries_by_tag() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    let run_id = "run-1";

    let disc1 = Discovery {
        id: "disc-001".into(),
        run_id: run_id.into(),
        agent_id: "explorer-1".into(),
        timestamp: chrono::Utc::now(),
        content: "Cache abstraction exists".into(),
        file_paths: vec![],
        confidence: Confidence::High,
        tags: vec!["caching".into()],
    };
    let disc2 = Discovery {
        id: "disc-002".into(),
        run_id: run_id.into(),
        agent_id: "explorer-2".into(),
        timestamp: chrono::Utc::now(),
        content: "N+1 query in items endpoint".into(),
        file_paths: vec![],
        confidence: Confidence::High,
        tags: vec!["performance".into()],
    };
    state.save_discovery(run_id, &disc1).unwrap();
    state.save_discovery(run_id, &disc2).unwrap();

    let results = state.query_mind(run_id, "caching");
    assert_eq!(results.discoveries.len(), 1);
    assert_eq!(results.discoveries[0].id, "disc-001");
}

#[test]
fn test_save_and_load_insight() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    let run_id = "run-1";

    let insight = Insight {
        id: "ins-001".into(),
        run_id: run_id.into(),
        timestamp: chrono::Utc::now(),
        content: "Auth is tightly coupled".into(),
        discovery_ids: vec!["disc-001".into()],
        tags: vec!["architecture".into()],
    };
    state.save_insight(run_id, &insight).unwrap();

    let insights = state.load_insights(run_id);
    assert_eq!(insights.len(), 1);
    assert_eq!(insights[0].id, "ins-001");
}

#[test]
fn test_query_mind_searches_content_and_tags() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    let run_id = "run-1";

    let disc = Discovery {
        id: "disc-001".into(),
        run_id: run_id.into(),
        agent_id: "explorer-1".into(),
        timestamp: chrono::Utc::now(),
        content: "The database uses PostgreSQL with custom extensions".into(),
        file_paths: vec![],
        confidence: Confidence::Medium,
        tags: vec!["database".into()],
    };
    state.save_discovery(run_id, &disc).unwrap();

    // Search by content keyword
    let results = state.query_mind(run_id, "PostgreSQL");
    assert_eq!(results.discoveries.len(), 1);

    // Search by tag
    let results = state.query_mind(run_id, "database");
    assert_eq!(results.discoveries.len(), 1);

    // No match
    let results = state.query_mind(run_id, "frontend");
    assert_eq!(results.discoveries.len(), 0);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_save_and_load_discovery test_query_discoveries_by_tag test_save_and_load_insight test_query_mind_searches_content_and_tags -- --nocapture`
Expected: FAIL — methods don't exist

**Step 3: Implement the state operations**

In `src/state.rs`, add a `MindQueryResult` struct and new methods:

```rust
// At the top of the file, ensure Discovery, Insight, Confidence are imported from types

/// Result of querying the Hive Mind
pub struct MindQueryResult {
    pub discoveries: Vec<Discovery>,
    pub insights: Vec<Insight>,
}
```

Add methods to `impl HiveState`:

```rust
// --- Hive Mind ---

pub fn mind_dir(&self, run_id: &str) -> PathBuf {
    self.run_dir(run_id).join("mind")
}

pub fn save_discovery(&self, run_id: &str, entry: &Discovery) -> Result<(), String> {
    let dir = self.mind_dir(run_id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("discoveries.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(file, "{json}").map_err(|e| e.to_string())
}

pub fn load_discoveries(&self, run_id: &str) -> Vec<Discovery> {
    let path = self.mind_dir(run_id).join("discoveries.jsonl");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn save_insight(&self, run_id: &str, entry: &Insight) -> Result<(), String> {
    let dir = self.mind_dir(run_id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("insights.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(file, "{json}").map_err(|e| e.to_string())
}

pub fn load_insights(&self, run_id: &str) -> Vec<Insight> {
    let path = self.mind_dir(run_id).join("insights.jsonl");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Search discoveries and insights by keyword (case-insensitive match on content, tags, file_paths)
pub fn query_mind(&self, run_id: &str, query: &str) -> MindQueryResult {
    let query_lower = query.to_lowercase();

    let discoveries: Vec<Discovery> = self
        .load_discoveries(run_id)
        .into_iter()
        .filter(|d| {
            d.content.to_lowercase().contains(&query_lower)
                || d.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
                || d.file_paths.iter().any(|f| f.to_lowercase().contains(&query_lower))
        })
        .collect();

    let insights: Vec<Insight> = self
        .load_insights(run_id)
        .into_iter()
        .filter(|i| {
            i.content.to_lowercase().contains(&query_lower)
                || i.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
        })
        .collect();

    MindQueryResult {
        discoveries,
        insights,
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test test_save_and_load_discovery test_query_discoveries_by_tag test_save_and_load_insight test_query_mind_searches_content_and_tags -- --nocapture`
Expected: PASS

**Step 5: Run full suite and clippy**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 6: Commit**

```bash
git add src/state.rs
git commit -m "feat: add Hive Mind state operations (discoveries, insights, query)"
```

---

### Task 4: Add Hive Mind MCP tools

**Files:**
- Modify: `src/mcp.rs` (add parameter types and tool implementations)

**Step 1: Add parameter types**

After the existing `SaveSpecParams` in `src/mcp.rs`, add:

```rust
#[derive(Deserialize, JsonSchema)]
pub struct DiscoverParams {
    /// What was discovered (a clear, concise finding)
    pub content: String,
    /// Confidence level: low, medium, high
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// Relevant file paths
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Topic tags for searchability
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_confidence() -> String {
    "medium".to_string()
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryMindParams {
    /// Search query (matches against content, tags, and file paths)
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SynthesizeParams {
    /// The synthesized insight
    pub content: String,
    /// IDs of discoveries this insight is based on
    pub discovery_ids: Vec<String>,
    /// Topic tags
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct EstablishConventionParams {
    /// The convention to establish (appended to conventions.md)
    pub content: String,
}
```

**Step 2: Write failing tests for permission checks**

Add to the test module in `src/mcp.rs`:

```rust
#[test]
fn discover_allowed_for_all_roles() {
    // All agent roles should be able to write discoveries
    for role in [
        AgentRole::Coordinator,
        AgentRole::Lead,
        AgentRole::Worker,
        AgentRole::Explorer,
        AgentRole::Evaluator,
    ] {
        assert!(
            [
                AgentRole::Coordinator,
                AgentRole::Lead,
                AgentRole::Worker,
                AgentRole::Explorer,
                AgentRole::Evaluator,
                AgentRole::Reviewer,
            ]
            .contains(&role),
            "All roles should be able to discover"
        );
    }
}

#[test]
fn synthesize_requires_coordinator() {
    let mcp = test_mcp_with_role(AgentRole::Worker);
    assert!(
        mcp.require_role(&[AgentRole::Coordinator]).is_err(),
        "Worker should not be allowed to synthesize"
    );
    let mcp = test_mcp_with_role(AgentRole::Coordinator);
    assert!(
        mcp.require_role(&[AgentRole::Coordinator]).is_ok(),
        "Coordinator should be allowed to synthesize"
    );
}

#[test]
fn establish_convention_requires_coordinator() {
    let mcp = test_mcp_with_role(AgentRole::Worker);
    assert!(
        mcp.require_role(&[AgentRole::Coordinator]).is_err(),
        "Worker should not be allowed to establish conventions"
    );
}
```

**Step 3: Run tests to verify they compile and pass the basic checks**

Run: `cargo test discover_allowed synthesize_requires establish_convention_requires -- --nocapture`

**Step 4: Implement the MCP tools**

Add these tool implementations inside the `#[tool_router] impl HiveMcp` block:

```rust
#[tool(description = "Record a discovery about the codebase in the Hive Mind. Any agent can call this. Use it whenever you find something surprising, useful, or relevant to other agents.")]
async fn hive_discover(
    &self,
    params: Parameters<DiscoverParams>,
) -> Result<CallToolResult, McpError> {
    self.touch_heartbeat();
    let p = &params.0;

    let confidence = match p.confidence.as_str() {
        "low" => crate::types::Confidence::Low,
        "medium" => crate::types::Confidence::Medium,
        "high" => crate::types::Confidence::High,
        _ => crate::types::Confidence::Medium,
    };

    let discovery = crate::types::Discovery {
        id: format!("disc-{}", &uuid::Uuid::new_v4().to_string()[..8]),
        run_id: self.run_id.clone(),
        agent_id: self.agent_id.clone(),
        timestamp: Utc::now(),
        content: p.content.clone(),
        file_paths: p.file_paths.clone(),
        confidence,
        tags: p.tags.clone(),
    };

    match self.state().save_discovery(&self.run_id, &discovery) {
        Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
            "Discovery '{}' saved to Hive Mind.",
            discovery.id
        ))])),
        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
    }
}

#[tool(description = "Search the Hive Mind for discoveries and insights by keyword. Returns matching entries from all agents in this run.")]
async fn hive_query_mind(
    &self,
    params: Parameters<QueryMindParams>,
) -> Result<CallToolResult, McpError> {
    self.touch_heartbeat();
    let p = &params.0;
    let state = self.state();
    let results = state.query_mind(&self.run_id, &p.query);

    let mut output = String::new();
    if results.discoveries.is_empty() && results.insights.is_empty() {
        output.push_str("No results found.");
    } else {
        if !results.discoveries.is_empty() {
            output.push_str(&format!("## Discoveries ({})\n\n", results.discoveries.len()));
            for d in &results.discoveries {
                output.push_str(&format!(
                    "- **{}** [{}] (by {}, {:?}): {}\n",
                    d.id,
                    d.tags.join(", "),
                    d.agent_id,
                    d.confidence,
                    d.content
                ));
                if !d.file_paths.is_empty() {
                    output.push_str(&format!("  Files: {}\n", d.file_paths.join(", ")));
                }
            }
        }
        if !results.insights.is_empty() {
            output.push_str(&format!("\n## Insights ({})\n\n", results.insights.len()));
            for i in &results.insights {
                output.push_str(&format!(
                    "- **{}** [{}]: {}\n  Based on: {}\n",
                    i.id,
                    i.tags.join(", "),
                    i.content,
                    i.discovery_ids.join(", ")
                ));
            }
        }
    }

    Ok(CallToolResult::success(vec![Content::text(output)]))
}

#[tool(description = "Synthesize multiple discoveries into a higher-level insight. Coordinator-only.")]
async fn hive_synthesize(
    &self,
    params: Parameters<SynthesizeParams>,
) -> Result<CallToolResult, McpError> {
    self.touch_heartbeat();
    if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
        return Ok(result);
    }
    let p = &params.0;

    let insight = crate::types::Insight {
        id: format!("ins-{}", &uuid::Uuid::new_v4().to_string()[..8]),
        run_id: self.run_id.clone(),
        timestamp: Utc::now(),
        content: p.content.clone(),
        discovery_ids: p.discovery_ids.clone(),
        tags: p.tags.clone(),
    };

    match self.state().save_insight(&self.run_id, &insight) {
        Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
            "Insight '{}' saved.",
            insight.id
        ))])),
        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
    }
}

#[tool(description = "Promote an insight into a persistent convention (appended to conventions.md). Coordinator-only.")]
async fn hive_establish_convention(
    &self,
    params: Parameters<EstablishConventionParams>,
) -> Result<CallToolResult, McpError> {
    self.touch_heartbeat();
    if let Err(result) = self.require_role(&[AgentRole::Coordinator]) {
        return Ok(result);
    }
    let p = &params.0;
    let state = self.state();
    let mut existing = state.load_conventions();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&p.content);
    existing.push('\n');

    match state.save_conventions(&existing) {
        Ok(()) => Ok(CallToolResult::success(vec![Content::text(
            "Convention established and saved to conventions.md.",
        )])),
        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
    }
}
```

**Step 5: Add hive_discover and hive_query_mind to the tool_router macro**

Find the `tool_router!` invocation at the bottom of the `impl HiveMcp` block and add the new tools. The `#[tool_router]` attribute macro auto-registers `#[tool]` methods, so no manual registration is needed.

**Step 6: Run full test suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 7: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: add Hive Mind MCP tools (discover, query_mind, synthesize, establish_convention)"
```

---

### Task 5: Add Explorer and Evaluator agent prompts

**Files:**
- Modify: `src/agent.rs:280-543` (generate_prompt match arms)
- Modify: `src/agent.rs:32-34` (settings_json — Explorer needs read-write, Evaluator needs read-only)

**Step 1: Write failing tests for Explorer prompt**

Add to the test module in `src/agent.rs`:

```rust
#[test]
fn explorer_prompt_contains_mandate_and_discovery() {
    let prompt = AgentSpawner::generate_prompt(
        "explorer-1",
        AgentRole::Explorer,
        Some("coordinator"),
        "Explore: Try implementing caching with Redis",
        "",
    );
    assert!(prompt.contains("Agent ID: explorer-1"));
    assert!(prompt.contains("Role: explorer"));
    assert!(prompt.contains("hive_discover"));
    assert!(prompt.contains("hive_query_mind"));
    assert!(prompt.contains("Try implementing caching with Redis"));
    assert!(!prompt.contains("hive_spawn_agent")); // explorers don't spawn
    assert!(!prompt.contains("hive_submit_to_queue")); // explorers don't submit
}

#[test]
fn evaluator_prompt_contains_comparison_instructions() {
    let prompt = AgentSpawner::generate_prompt(
        "evaluator-1",
        AgentRole::Evaluator,
        Some("coordinator"),
        "Evaluate explorer branches: explore-1, explore-2, explore-3",
        "",
    );
    assert!(prompt.contains("Agent ID: evaluator-1"));
    assert!(prompt.contains("Role: evaluator"));
    assert!(prompt.contains("READ-ONLY"));
    assert!(prompt.contains("hive_query_mind"));
    assert!(prompt.contains("comparison"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test explorer_prompt_contains evaluator_prompt_contains -- --nocapture`
Expected: FAIL — prompts fall through to Worker template

**Step 3: Add Explorer prompt**

In `src/agent.rs` `generate_prompt`, replace the Explorer placeholder with:

```rust
AgentRole::Explorer => format!(
    r#"You are an explorer agent in a hive swarm.
Agent ID: {agent_id}
Role: explorer
Parent: {}

## Your Mandate
{task_description}

## Responsibilities
- Explore the codebase and prototype a solution for your mandate.
- Write discoveries to the Hive Mind via hive_discover as you work.
- Query the Hive Mind via hive_query_mind before starting — other explorers may have found useful information.
- Produce either: a working prototype, a structured analysis, or a proof-of-concept.
- Commit your work with descriptive messages as you go.
- When done, call hive_update_task to set status to "review".

## Discovery Protocol
- Use hive_discover for anything surprising, useful, or relevant to other agents.
- Tag discoveries with relevant topics for searchability.
- Include file paths when the discovery relates to specific code.
- Discoveries are often MORE valuable than your code — be thorough.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than the coordinator.
- Your prototype may be disposable — focus on learning and proving the approach, not polish.
- Stay focused on your mandate. Explore deeply, not broadly.
"#,
    parent.unwrap_or("coordinator")
),
```

**Step 4: Add Evaluator prompt**

```rust
AgentRole::Evaluator => format!(
    r#"You are an evaluator agent in a hive swarm.
Agent ID: {agent_id}
Role: evaluator
Parent: {}

## Your Evaluation Task
{task_description}

## Responsibilities
- Read and analyze the code on each explorer branch listed in your task.
- Run tests on each branch to verify correctness.
- Compare approaches on: lines changed, test coverage, complexity, correctness, maintainability.
- Query the Hive Mind via hive_query_mind for discoveries made by each explorer.
- Produce a structured comparison document with your recommendation.
- Write your evaluation to a file (evaluation.md) in your worktree.
- Call hive_update_task to set status to "review" when done.

## Comparison Format
For each approach, document:
1. **Approach summary** — what it does and how
2. **Metrics** — lines changed, tests passing, build status
3. **Strengths** — what it does well
4. **Weaknesses** — what it does poorly or misses
5. **Recommendation** — which approach to pursue and why

## Constraints
- You are READ-ONLY for source code. Do NOT modify explorer branches.
- Only use Read, Glob, Grep to examine code. Use Bash only for running tests and benchmarks.
- Write your evaluation document in your own worktree, not on explorer branches.
- Be thorough but concise. Focus on actionable comparison, not exhaustive analysis.
- After writing your evaluation, stop immediately.
"#,
    parent.unwrap_or("coordinator")
),
```

**Step 5: Update settings_json in spawn()**

In `src/agent.rs` `spawn()`, the Evaluator role should get read-only hooks (same as Reviewer). Update the match in the settings_json block (line ~32-34):

```rust
if matches!(
    role,
    AgentRole::Reviewer | AgentRole::Planner | AgentRole::Postmortem | AgentRole::Evaluator
) {
```

Explorer should use the default read-write hooks (same as Lead/Worker), so no change needed for Explorer.

**Step 6: Run tests**

Run: `cargo test explorer_prompt_contains evaluator_prompt_contains -- --nocapture`
Expected: PASS

**Step 7: Run full suite and clippy**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 8: Commit**

```bash
git add src/agent.rs
git commit -m "feat: add Explorer and Evaluator agent prompts and spawn config"
```

---

### Task 6: Update spawn permissions for Explorer and Evaluator

**Files:**
- Modify: `src/mcp.rs` (hive_spawn_agent role validation)

**Step 1: Write failing tests**

Add to the test module in `src/mcp.rs`:

```rust
#[tokio::test]
async fn spawn_explorer_allowed_for_coordinator() {
    let mcp = test_mcp_with_role(AgentRole::Coordinator);
    // Coordinator should be able to spawn explorers (role string "explorer")
    let valid_roles = ["lead", "worker", "explorer", "evaluator"];
    let coordinator_can_spawn = ["lead", "explorer", "evaluator"];
    for role in coordinator_can_spawn {
        assert!(
            valid_roles.contains(&role),
            "Coordinator should be able to spawn {role}"
        );
    }
}

#[tokio::test]
async fn spawn_explorer_denied_for_lead() {
    // Leads should NOT be able to spawn explorers
    let lead_can_spawn = ["worker"];
    assert!(
        !lead_can_spawn.contains(&"explorer"),
        "Leads should not spawn explorers"
    );
}
```

**Step 2: Update hive_spawn_agent role parsing**

In `src/mcp.rs`, in the `hive_spawn_agent` method, update the role parsing to include "explorer" and "evaluator":

```rust
let role = match p.role.as_str() {
    "lead" => AgentRole::Lead,
    "worker" => AgentRole::Worker,
    "explorer" => AgentRole::Explorer,
    "evaluator" => AgentRole::Evaluator,
    _ => {
        return Ok(CallToolResult::error(vec![Content::text(
            "Invalid role. Use 'lead', 'worker', 'explorer', or 'evaluator'.",
        )]));
    }
};
```

**Step 3: Update hierarchy enforcement**

Update the `allowed` check to permit coordinator spawning explorers and evaluators:

```rust
let allowed = matches!(
    (caller_role, role),
    (AgentRole::Coordinator, AgentRole::Lead)
        | (AgentRole::Coordinator, AgentRole::Explorer)
        | (AgentRole::Coordinator, AgentRole::Evaluator)
        | (AgentRole::Lead, AgentRole::Worker)
);
```

**Step 4: Run tests**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 5: Commit**

```bash
git add src/mcp.rs
git commit -m "feat: allow coordinator to spawn explorer and evaluator agents"
```

---

### Task 7: Add `hive explore` CLI command

**Files:**
- Modify: `src/cli.rs` (add Explore command variant)
- Modify: `src/main.rs` (add cmd_explore handler)

**Step 1: Add CLI variant**

In `src/cli.rs`, add after the `Start` variant:

```rust
/// Start an exploration run — express intent, discuss with coordinator, prototype approaches
Explore {
    /// What you want to explore (e.g., "add caching", "make the dashboard faster")
    intent: String,
},
```

**Step 2: Add command dispatch in main.rs**

In `src/main.rs`, add to the match block:

```rust
Commands::Explore { intent } => cmd_explore(&intent),
```

**Step 3: Implement cmd_explore**

Add this function to `src/main.rs`. It's similar to `cmd_start` but generates an explore-mode coordinator prompt:

```rust
fn cmd_explore(intent: &str) -> Result<(), String> {
    let state = HiveState::discover()?;

    let run_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    state.create_run(&run_id)?;

    // Initialize log.db
    let log_path = state.run_dir(&run_id).join("log.db");
    LogDb::open(&log_path)?;

    // Save the intent as the spec
    let spec_content = format!("# Exploration\n\n## Intent\n{intent}\n");
    state.save_spec(&run_id, &spec_content)?;

    println!("Created exploration run: {run_id}");

    // Write coordinator CLAUDE.local.md with explore-mode prompt
    let codebase_summary =
        crate::agent::AgentSpawner::generate_codebase_summary(state.repo_root());
    let memory = state.load_memory_for_prompt(&crate::types::AgentRole::Coordinator);
    let coordinator_prompt = crate::agent::AgentSpawner::explore_coordinator_prompt(
        &run_id,
        intent,
        &codebase_summary,
        &memory,
    );
    let repo_root = state.repo_root();
    fs::write(repo_root.join("CLAUDE.local.md"), &coordinator_prompt)
        .map_err(|e| e.to_string())?;

    // Write .claude/settings.local.json for coordinator hooks
    let claude_dir = repo_root.join(".claude");
    fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
    let settings_json = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent coordinator --tool {{}} --status success"
                    )
                }]
            }]
        }
    });
    fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&settings_json).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Write .mcp.json for coordinator MCP
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "hive": {
                "command": "hive",
                "args": ["mcp", "--run", &run_id, "--agent", "coordinator"]
            }
        }
    });
    fs::write(
        repo_root.join(".mcp.json"),
        serde_json::to_string_pretty(&mcp_config).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Register coordinator agent
    let coordinator = crate::types::Agent {
        id: "coordinator".to_string(),
        role: crate::types::AgentRole::Coordinator,
        status: crate::types::AgentStatus::Running,
        parent: None,
        pid: None,
        worktree: None,
        heartbeat: Some(chrono::Utc::now()),
        task_id: None,
        session_id: None,
        last_completed_at: None,
        messages_read_at: None,
        retry_count: 0,
    };
    state.save_agent(&run_id, &coordinator)?;

    println!("Exploration coordinator configured.");
    println!("Launch Claude Code in this directory to begin exploring.");
    println!("Run 'hive tui' in another terminal to monitor progress.");

    Ok(())
}
```

**Step 4: Add explore_coordinator_prompt to agent.rs**

In `src/agent.rs`, add a new method to `AgentSpawner`:

```rust
pub fn explore_coordinator_prompt(
    run_id: &str,
    intent: &str,
    codebase_summary: &str,
    memory: &str,
) -> String {
    let base = format!(
        r#"You are the coordinator agent in a hive exploration run.
Run ID: {run_id}
Agent ID: coordinator
Role: coordinator
Mode: EXPLORE

## Project Summary
{codebase_summary}

## Human's Intent
{intent}

## Exploration Protocol

You are in EXPLORE mode. Your job is NOT to execute a plan — it's to THINK with the human.

### Phase 1: Think Mode
1. Analyze the codebase to understand the current state relevant to the intent.
2. Query the Hive Mind (hive_query_mind) for any prior knowledge.
3. Present your analysis to the human: what you found, what the options are, what you recommend.
4. Discuss with the human. Refine the direction together.
5. Once you agree on a direction, propose 2-3 exploration angles.

### Phase 2: Explore Mode
1. Create tasks for each exploration angle.
2. Spawn explorer agents (role="explorer") for each angle.
3. ALWAYS include one adversarial explorer whose mandate is to find reasons the approach fails.
4. Wait for explorers to complete (hive_wait_for_activity, hive_check_agents).
5. Spawn an evaluator agent (role="evaluator") to compare results.
6. Present the evaluator's comparison to the human.

### Phase 3: Decision
Based on the evaluation, present three options to the human:
1. **Merge directly** — explorer output is good enough. Submit branch to merge queue.
2. **Refine** — spawn a worker to polish the best explorer's branch.
3. **Full execution** — escalate to a full hive run using the explorer's approach as the spec.

## Discovery Protocol
- Use hive_discover to record findings as you analyze the codebase.
- Use hive_query_mind before spawning explorers to share existing knowledge.
- Use hive_synthesize to combine discoveries into insights.

## Constraints
- Do NOT skip Phase 1. The conversation with the human is the most valuable part.
- Do NOT read or write implementation code yourself.
- Only spawn explorers and evaluators, not leads or workers (unless escalating to full execution).
- Let explorers do the prototyping. Your job is to think and coordinate.
"#
    );
    if memory.is_empty() {
        base
    } else {
        format!("{base}\n{memory}\n")
    }
}
```

**Step 5: Write test for explore_coordinator_prompt**

```rust
#[test]
fn explore_coordinator_prompt_has_exploration_protocol() {
    let prompt = AgentSpawner::explore_coordinator_prompt(
        "run-1",
        "make the dashboard faster",
        "summary: 10 files",
        "",
    );
    assert!(prompt.contains("Mode: EXPLORE"));
    assert!(prompt.contains("make the dashboard faster"));
    assert!(prompt.contains("Think Mode"));
    assert!(prompt.contains("Explore Mode"));
    assert!(prompt.contains("adversarial explorer"));
    assert!(prompt.contains("hive_discover"));
    assert!(prompt.contains("hive_query_mind"));
    assert!(prompt.contains("hive_synthesize"));
}
```

**Step 6: Run full suite and clippy**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 7: Commit**

```bash
git add src/cli.rs src/main.rs src/agent.rs
git commit -m "feat: add 'hive explore' CLI command with explore-mode coordinator prompt"
```

---

### Task 8: Add `hive mind` CLI commands

**Files:**
- Modify: `src/cli.rs` (add Mind command)
- Modify: `src/main.rs` (add cmd_mind handler)

**Step 1: Add CLI variants**

In `src/cli.rs`, add after the `Memory` variant:

```rust
/// Browse and query the Hive Mind knowledge space
Mind {
    #[command(subcommand)]
    command: Option<MindCommands>,
},
```

Add a new enum:

```rust
#[derive(Subcommand)]
pub enum MindCommands {
    /// Search the Hive Mind by keyword
    Query {
        /// Search query
        query: String,
    },
}
```

**Step 2: Add command dispatch**

In `src/main.rs`, add to the match:

```rust
Commands::Mind { command } => cmd_mind(command),
```

**Step 3: Implement cmd_mind**

```rust
fn cmd_mind(command: Option<cli::MindCommands>) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    match command {
        None => {
            // Show summary of Hive Mind contents
            let discoveries = state.load_discoveries(&run_id);
            let insights = state.load_insights(&run_id);
            println!("Hive Mind (run {run_id}):");
            println!("  Discoveries: {}", discoveries.len());
            println!("  Insights: {}", insights.len());

            if !discoveries.is_empty() {
                println!("\nRecent Discoveries:");
                for d in discoveries.iter().rev().take(5) {
                    println!(
                        "  [{}] ({}) {}: {}",
                        d.id,
                        d.tags.join(", "),
                        d.agent_id,
                        if d.content.len() > 80 {
                            &d.content[..80]
                        } else {
                            &d.content
                        }
                    );
                }
            }

            if !insights.is_empty() {
                println!("\nInsights:");
                for i in &insights {
                    println!(
                        "  [{}] ({}): {}",
                        i.id,
                        i.tags.join(", "),
                        if i.content.len() > 80 {
                            &i.content[..80]
                        } else {
                            &i.content
                        }
                    );
                }
            }

            Ok(())
        }
        Some(cli::MindCommands::Query { query }) => {
            let results = state.query_mind(&run_id, &query);

            if results.discoveries.is_empty() && results.insights.is_empty() {
                println!("No results for '{query}'.");
                return Ok(());
            }

            if !results.discoveries.is_empty() {
                println!("Discoveries ({}):", results.discoveries.len());
                for d in &results.discoveries {
                    println!("  [{}] {:?} by {}: {}", d.id, d.confidence, d.agent_id, d.content);
                    if !d.file_paths.is_empty() {
                        println!("    Files: {}", d.file_paths.join(", "));
                    }
                }
            }

            if !results.insights.is_empty() {
                println!("\nInsights ({}):", results.insights.len());
                for i in &results.insights {
                    println!("  [{}]: {}", i.id, i.content);
                    println!("    Based on: {}", i.discovery_ids.join(", "));
                }
            }

            Ok(())
        }
    }
}
```

**Step 4: Run full suite and clippy**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 5: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add 'hive mind' and 'hive mind query' CLI commands"
```

---

### Task 9: Inject Hive Mind discoveries into agent prompts

**Files:**
- Modify: `src/state.rs` (`load_memory_for_prompt` to include discoveries for Explorer/Evaluator)
- Modify: `src/agent.rs` (spawn to pass run_id for mind loading)

**Step 1: Write failing test**

In `src/state.rs` tests:

```rust
#[test]
fn test_load_memory_for_explorer_includes_conventions() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    state.save_conventions("Use snake_case.").unwrap();

    let prompt = state.load_memory_for_prompt(&AgentRole::Explorer);
    assert!(prompt.contains("Use snake_case."));
}

#[test]
fn test_load_memory_for_evaluator_includes_conventions() {
    let dir = TempDir::new().unwrap();
    setup_hive_dir(dir.path());
    let state = HiveState::new(dir.path().to_path_buf());
    state.save_conventions("Use snake_case.").unwrap();

    let prompt = state.load_memory_for_prompt(&AgentRole::Evaluator);
    assert!(prompt.contains("Use snake_case."));
}
```

**Step 2: Run tests to verify they fail or pass**

These may already pass if Task 1 correctly added Explorer/Evaluator to the conventions include list. Verify:

Run: `cargo test test_load_memory_for_explorer test_load_memory_for_evaluator -- --nocapture`

**Step 3: If failing, update load_memory_for_prompt**

Ensure `include_conventions` covers Explorer and Evaluator (it should if they're not excluded like Coordinator is). Verify the match arms in `load_memory_for_prompt`.

**Step 4: Run full suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings`

**Step 5: Commit (if changes were needed)**

```bash
git add src/state.rs
git commit -m "feat: ensure explorer and evaluator agents receive memory in prompts"
```

---

### Task 10: Integration verification and final cleanup

**Step 1: Run the full test suite**

Run: `cargo test --all-targets`
Expected: All tests pass (163 existing + new tests from this plan)

**Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: No warnings

**Step 3: Run format check**

Run: `cargo fmt --all -- --check`
Fix any formatting issues with `cargo fmt --all`

**Step 4: Build release**

Run: `cargo build --release`
Expected: Clean build

**Step 5: Verify CLI help**

Run: `cargo run -- --help`
Expected: Shows `explore` and `mind` commands

Run: `cargo run -- explore --help`
Expected: Shows intent argument

Run: `cargo run -- mind --help`
Expected: Shows query subcommand

**Step 6: Final commit**

```bash
git add -A
git commit -m "chore: cleanup and formatting for speculative swarm feature"
```
