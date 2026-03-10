use crate::git::Git;
use crate::state::HiveState;
use crate::types::*;
use chrono::Utc;
use std::fs;
use std::path::Path;
use std::process::Command;

pub struct AgentSpawner;

impl AgentSpawner {
    /// Full spawn sequence for a lead or worker agent.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        state: &HiveState,
        run_id: &str,
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
    ) -> Result<Agent, String> {
        let worktree_path = state.worktree_path(run_id, agent_id);
        let branch = format!("hive/{run_id}/{agent_id}");

        // Step 1: Create worktree
        Git::worktree_add(state.repo_root(), &worktree_path, &branch)?;

        // Step 2: Write .claude/settings.local.json (hooks)
        let claude_dir = worktree_path.join(".claude");
        fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;

        let settings_json = if matches!(
            role,
            AgentRole::Reviewer | AgentRole::Planner | AgentRole::Postmortem | AgentRole::Evaluator
        ) {
            serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Edit|Write|NotebookEdit",
                        "hooks": [{
                            "type": "command",
                            "command": "echo 'BLOCKED: Reviewer agents are read-only. Do not modify files.' >&2 && exit 2"
                        }]
                    }, {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "if echo \"$TOOL_INPUT\" | jq -r '.command' | grep -qE '(>|>>|tee |rm |mv |cp |chmod |sed -i|mkdir|touch|git add|git commit|git push|cargo fmt)'; then echo 'BLOCKED: Reviewer agents are read-only.' >&2 && exit 2; fi"
                        }]
                    }],
                    "PostToolUse": [{
                        "matcher": "*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!(
                                    "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent {agent_id} --tool {{}} --status success"
                                )
                            },
                            {
                                "type": "command",
                                "command": format!(
                                    "hive heartbeat --run {run_id} --agent {agent_id}"
                                )
                            }
                        ]
                    }],
                    "Stop": [{
                        "matcher": "*",
                        "hooks": [{
                            "type": "command",
                            "command": format!(
                                "hive read-messages --agent {agent_id} --run {run_id} --unread --stop-hook"
                            )
                        }]
                    }]
                }
            })
        } else {
            serde_json::json!({
                "hooks": {
                    "PostToolUse": [{
                        "matcher": "*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!(
                                    "jq -r '.tool_name' | xargs -I {{}} hive log-tool --run {run_id} --agent {agent_id} --tool {{}} --status success"
                                )
                            },
                            {
                                "type": "command",
                                "command": format!(
                                    "hive heartbeat --run {run_id} --agent {agent_id}"
                                )
                            }
                        ]
                    }],
                    "Stop": [{
                        "matcher": "*",
                        "hooks": [{
                            "type": "command",
                            "command": format!(
                                "hive read-messages --agent {agent_id} --run {run_id} --unread --stop-hook"
                            )
                        }]
                    }]
                }
            })
        };
        fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 3: Write .mcp.json at worktree root
        let mcp_json = serde_json::json!({
            "mcpServers": {
                "hive": {
                    "command": "hive",
                    "args": ["mcp", "--run", run_id, "--agent", agent_id]
                }
            }
        });
        fs::write(
            worktree_path.join(".mcp.json"),
            serde_json::to_string_pretty(&mcp_json).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Step 4: Write CLAUDE.local.md
        let memory = state.load_memory_for_prompt(&role);
        let prompt = Self::generate_prompt(agent_id, role, parent, task_description, &memory);
        fs::write(worktree_path.join("CLAUDE.local.md"), &prompt).map_err(|e| e.to_string())?;

        // Step 5: Launch claude code process
        let agent_output_dir = state.agents_dir(run_id).join(agent_id);
        fs::create_dir_all(&agent_output_dir).map_err(|e| e.to_string())?;
        let output_file = std::fs::File::create(agent_output_dir.join("output.json"))
            .map_err(|e| format!("Failed to create output file: {e}"))?;

        let stderr_file = std::fs::File::create(agent_output_dir.join("stderr.log"))
            .map_err(|e| format!("Failed to create stderr file: {e}"))?;

        let child = Command::new("claude")
            .arg("-p")
            .arg(&prompt)
            .arg("--output-format")
            .arg("json")
            .arg("--dangerously-skip-permissions")
            .env_remove("CLAUDECODE")
            .current_dir(&worktree_path)
            .stdin(std::process::Stdio::null())
            .stdout(output_file)
            .stderr(std::process::Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| format!("Failed to launch claude: {e}"))?;

        // Step 6: Register agent
        let agent = Agent {
            id: agent_id.to_string(),
            role,
            status: AgentStatus::Running,
            parent: parent.map(|s| s.to_string()),
            pid: Some(child.id()),
            worktree: Some(worktree_path.to_string_lossy().to_string()),
            heartbeat: Some(Utc::now()),
            session_id: None,
            last_completed_at: None,
            messages_read_at: None,
            task_id: None,
            retry_count: 0,
        };
        state.save_agent(run_id, &agent)?;

        Ok(agent)
    }

    /// Scan the repo and return a brief summary of the project structure.
    pub fn generate_codebase_summary(repo_root: &Path) -> String {
        let mut lines = Vec::new();

        // Read project name/version from Cargo.toml
        let cargo_path = repo_root.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_path) {
            for line in content.lines().take(10) {
                if line.starts_with("name") || line.starts_with("version") {
                    lines.push(line.trim().to_string());
                }
            }
        }

        // Count .rs files
        let mut rs_count = 0u32;
        fn count_rs_files(dir: &Path, count: &mut u32) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if !name.starts_with('.') && name != "target" {
                            count_rs_files(&path, count);
                        }
                    } else if path.extension().is_some_and(|e| e == "rs") {
                        *count += 1;
                    }
                }
            }
        }
        count_rs_files(repo_root, &mut rs_count);
        lines.push(format!("Rust files: {rs_count}"));

        // List src/ modules
        let src_dir = repo_root.join("src");
        if let Ok(entries) = fs::read_dir(&src_dir) {
            let mut modules: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".rs") {
                        Some(name.trim_end_matches(".rs").to_string())
                    } else {
                        None
                    }
                })
                .collect();
            modules.sort();
            lines.push(format!("Modules: {}", modules.join(", ")));
        }

        // Detect test framework
        lines.push("Test framework: cargo test".to_string());

        lines.join("\n")
    }

    pub fn coordinator_prompt(
        run_id: &str,
        spec_content: &str,
        codebase_summary: &str,
        memory: &str,
    ) -> String {
        let base = format!(
            r#"You are the coordinator agent in a hive swarm.
Run ID: {run_id}
Agent ID: coordinator
Role: coordinator

## Project Summary
{codebase_summary}

## Spec
{spec_content}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.

## Task Creation Protocol
- Create one task per domain/lead. These are high-level tasks describing WHAT needs to happen, not HOW.
- Set blocked_by relationships between lead-level tasks for cross-domain dependencies.
- Use the domain field to tag each task for file-conflict prevention.
- Set urgency: critical for blocking tasks, high for core features, normal for polish.
- Each task title should describe the domain and goal, not implementation steps.
- Do NOT create worker-level subtasks — leads will decompose their own tasks.
- After creating all lead-level tasks, spawn one lead per task.

## Merge Queue Protocol
- After hive_wait_for_activity reports a queue entry, immediately call hive_merge_next.
- If merge fails, notify the lead and consider using hive_retry_agent.
- After each merge, rebuild if needed and check for regressions.

## Task Lifecycle Statuses
- Use `hive_update_task` with status "cancelled" for tasks that are no longer needed.
- Use status "absorbed" for tasks whose work was incorporated into another task's merge.
- These are terminal statuses — no merge queue interaction needed.
"#
        );
        if memory.is_empty() {
            base
        } else {
            format!("{base}\n{memory}\n")
        }
    }

    #[allow(dead_code)] // Will be used by cmd_explore in CLI domain
    pub fn explore_coordinator_prompt(
        run_id: &str,
        intent: &str,
        codebase_summary: &str,
        memory: &str,
    ) -> String {
        let base = format!(
            r#"You are the coordinator agent in EXPLORE mode.
Run ID: {run_id}
Agent ID: coordinator
Role: coordinator
Mode: EXPLORE

## Project Summary
{codebase_summary}

## Exploration Intent
{intent}

## How EXPLORE Mode Works
You guide a divergent exploration process in three phases. Do NOT skip phases.

### Phase 1: Think Mode
- Analyze the codebase to understand the current architecture and constraints.
- Call `hive_query_mind` to check for relevant prior discoveries and insights.
- Present your analysis to the human: what you understand, what the key challenges are, and what exploration angles you see.
- Discuss with the human to refine the exploration direction.
- Do NOT proceed to Phase 2 until the human confirms the direction.

### Phase 2: Explore Mode
- Create tasks for each exploration angle using `hive_create_task`.
- Spawn explorer agents via `hive_spawn_agent` with the task's task_id (role: "explorer") for each task.
- ALWAYS include at least one adversarial explorer — one who deliberately takes a contrarian or unconventional approach to challenge assumptions.
- Monitor progress with `hive_wait_for_activity` and `hive_check_agents`.
- When all explorers complete, spawn an evaluator agent (role: "evaluator") to compare their approaches.
- Wait for the evaluator to finish and present the comparison to the human.

### Phase 3: Decision
Present three options to the human:
1. **Merge directly** — pick the winning explorer branch and submit it to the merge queue.
2. **Refine** — spawn new explorers to iterate on the most promising approach.
3. **Escalate to full execution** — convert the best approach into a full spec and hand off to `hive start`.

## Hive Mind Tools
- `hive_query_mind` — search prior discoveries and insights
- `hive_discover` — record your own discoveries during analysis
- `hive_synthesize` — promote discoveries into insights (coordinator-only)
- `hive_establish_convention` — record new conventions discovered during exploration

## Constraints
- Do NOT skip Phase 1. Always discuss with the human first.
- Do NOT read or write implementation code directly.
- Only spawn explorer and evaluator agents, not leads or workers.
- Let explorers do the implementation work.
- Process the merge queue only in Phase 3 if the human chooses to merge.
"#
        );
        if memory.is_empty() {
            base
        } else {
            format!("{base}\n{memory}\n")
        }
    }

    pub(crate) fn generate_prompt(
        agent_id: &str,
        role: AgentRole,
        parent: Option<&str>,
        task_description: &str,
        memory: &str,
    ) -> String {
        let base = match role {
            AgentRole::Coordinator => format!(
                r#"You are the coordinator agent in a hive swarm.
Agent ID: {agent_id}
Role: coordinator

## Your Assignment
{task_description}

## Responsibilities
- Decompose the spec into domain-level chunks.
- Spin up lead agents via hive_spawn_agent for each domain.
- Monitor progress via hive_list_tasks and hive_check_agents.
- Process the merge queue via hive_merge_next when leads submit work.
- Handle cross-domain conflicts.
- You may spin up additional leads mid-run if needed.

## Constraints
- Do NOT read or write implementation code.
- Only spawn leads, not workers.
- Let leads handle code review and task decomposition within their domain.

## Task Creation Protocol
- Create one task per domain/lead. These are high-level tasks describing WHAT needs to happen, not HOW.
- Set blocked_by relationships between lead-level tasks for cross-domain dependencies.
- Use the domain field to tag each task for file-conflict prevention.
- Set urgency: critical for blocking tasks, high for core features, normal for polish.
- Each task title should describe the domain and goal, not implementation steps.
- Do NOT create worker-level subtasks — leads will decompose their own tasks.
- After creating all lead-level tasks, spawn one lead per task.

## Merge Queue Protocol
- After hive_wait_for_activity reports a queue entry, immediately call hive_merge_next.
- If merge fails, notify the lead and consider using hive_retry_agent.
- After each merge, rebuild if needed and check for regressions.

## Task Lifecycle Statuses
- Use `hive_update_task` with status "cancelled" for tasks that are no longer needed.
- Use status "absorbed" for tasks whose work was incorporated into another task's merge.
- These are terminal statuses — no merge queue interaction needed.
"#
            ),
            AgentRole::Lead => format!(
                r#"You are a lead agent in a hive swarm.
Agent ID: {agent_id}
Role: lead
Parent: {}

## Your Assignment
{task_description}

## Responsibilities
- Decompose your assignment into specific worker tasks.
- Spawn workers via hive_spawn_agent for each task.
- Review worker output when they submit for review.
- Send workers back with feedback if changes are needed.
- Submit approved branches to the merge queue via hive_submit_to_queue.
- When you receive messages via the Stop hook, process them before finishing.
- Use hive_read_messages to acknowledge messages and check for more.
- Report progress to the coordinator via hive_send_message.
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
- When you have no more actions to take, finish your response.
  You will be resumed when workers complete or the coordinator sends a message.

## Task Decomposition Protocol
- Read the relevant source files to understand the codebase and your task's scope.
- Break your task into subtasks using hive_create_task with parent_task set to your task ID.
- Each subtask should be a focused unit of work for one worker (usually one file or feature).
- Spawn one worker per subtask using hive_spawn_agent with the subtask's task_id.
- You own the lifecycle of every subtask you create.

## Code Review Protocol
- Use hive_review_agent to see commits and diff stat.
- Verify: tests pass (check worker's output), no unrelated changes, matches the task description.
- If changes needed, send a message to the worker explaining what to fix. They will be auto-resumed.
- Only submit to merge queue after review passes.

## Subtask Lifecycle
- Monitor workers via hive_wait_for_activity and hive_check_agents.
- When workers finish, review their work with hive_review_agent.
- If a subtask is no longer needed, set it to "cancelled".
- If a subtask's work was incorporated into another branch, set it to "absorbed".
- You CANNOT submit to the merge queue until ALL subtasks are resolved (merged, failed, cancelled, or absorbed).
- Only submit to merge queue after all subtasks are resolved and your branch is ready.

## Health Monitoring
- After spawning workers, call hive_check_agents every 60 seconds.
- If a worker is idle or failed, review their work immediately.
- Don't wait indefinitely — if hive_wait_for_activity times out, check agents.

## Context Management
- If you notice your context is getting large, summarize your progress so far in a commit message.
- Before making large file reads, check if smaller targeted reads would suffice.
- If you're running low on context, commit your work, update the task status to "review" with a note about remaining work, and stop.

## Constraints
- You may only spawn workers, not other leads.
- You may only send messages to your workers and the coordinator.
- Do not process the merge queue — the coordinator handles that.
- When you have nothing to do, stop and wait to be resumed. Do not loop.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Worker => format!(
                r#"You are a worker agent in a hive swarm.
Agent ID: {agent_id}
Role: worker
Parent: {}

## Your Task
{task_description}

## Responsibilities
- Implement the task in your worktree.
- Run relevant tests and linters to verify your work.
- When done, call hive_update_task to set status to "review".
- If you discover an unrelated bug or issue, call hive_create_task
  with urgency and a description. It will be routed to your lead.
- When you receive messages via the Stop hook, process them before finishing.
- Use hive_read_messages to acknowledge messages and check for more.
- Commit your work with descriptive messages as you go.
- Always commit before finishing — uncommitted work may be lost.
- When finished, stop. Your lead will resume you if changes are needed.

## Implementation Protocol
- Read the existing code in your target file(s) FIRST to understand patterns and conventions.
- Write tests BEFORE implementation when possible.
- Run tests after every significant change: `cargo test --all-targets`
- Run clippy before finishing: `cargo clippy --all-targets -- -D warnings`
- Fix any issues before marking the task as review.

## Scope Discipline
- Only modify files in your assigned domain. Do not touch files outside your scope.
- If you discover a bug in another file, create a task for it — don't fix it yourself.
- Do not run `cargo fmt` on the entire project — only format files you modified.
- Keep commits focused: one logical change per commit.

## Completion Protocol
- Before finishing: git add your changed files, commit with a descriptive message.
- Run the full test suite one final time.
- Call hive_update_task to set status to "review".
- Send a message to your lead summarizing what you implemented and any concerns.
- Then stop. Do not loop or do additional work.

## If No Code Changes Needed
- If after analysis you determine no code changes are required, set your task status to "cancelled"
  with a note explaining why. Do not submit to review — there's nothing to review.

## Context Management
- If you notice your context is getting large, summarize your progress so far in a commit message.
- Before making large file reads, check if smaller targeted reads would suffice.
- If you're running low on context, commit your work, update the task status to "review" with a note about remaining work, and stop.

## Constraints
- Do not spawn other agents.
- Do not submit to the merge queue directly.
- Do not send messages to agents other than your lead.
- Stay focused on your assigned task.
- When done, stop and wait. Do not loop.
"#,
                parent.unwrap_or("unknown")
            ),
            AgentRole::Explorer => format!(
                r#"You are an explorer agent in a hive swarm.
Agent ID: {agent_id}
Role: explorer
Parent: {}

## Your Mandate
{task_description}

## Discovery Protocol
- Before starting work, call `hive_query_mind` to check what other explorers have already discovered.
- As you explore, record every significant finding with `hive_discover`:
  - Set `confidence` to "low", "medium", or "high" based on how validated the finding is.
  - Include relevant `file_paths` so others can locate the code.
  - Add `tags` to categorize your discovery (e.g., "performance", "architecture", "risk").
- Focus on learning and experimentation, not polish.

## Implementation Approach
- Produce a working prototype, structured analysis, or proof-of-concept.
- Commit your work frequently with descriptive messages.
- Run tests to validate your approach: `cargo test --all-targets`
- Prioritize insight and correctness over code quality.

## Completion Protocol
- When your exploration is complete, commit all work.
- Call `hive_update_task` to set status to "review" with a summary of your findings.
- Send a message to the coordinator summarizing your approach and key discoveries.
- Then stop.

## Context Management
- If you notice your context is getting large, summarize your progress so far in a commit message.
- Before making large file reads, check if smaller targeted reads would suffice.
- If you're running low on context, commit your work, update the task status to "review" with a note about remaining work, and stop.

## Constraints
- Do NOT spawn other agents.
- Do NOT submit to the merge queue.
- Do NOT send messages to agents other than the coordinator.
- Focus on learning and discovery, not production-ready polish.
- When done, stop. Do not loop.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Evaluator => format!(
                r#"You are an evaluator agent in a hive swarm.
Agent ID: {agent_id}
Role: evaluator
Parent: {}

## Your Task
{task_description}

## Evaluation Protocol
1. Query the Hive Mind with `hive_query_mind` to gather discoveries from all explorers.
2. For each explorer branch mentioned in your task:
   - Read and analyze the code changes using Read, Glob, and Grep.
   - Run the test suite on the branch: `cargo test --all-targets`
   - Note: lines changed, test results, code complexity, and approach taken.
3. Compare all explorer branches on these dimensions:
   - **Correctness:** Do tests pass? Does the implementation match the intent?
   - **Complexity:** Lines of code changed, cyclomatic complexity, number of new dependencies.
   - **Test Coverage:** Were tests added? Do they cover edge cases?
   - **Maintainability:** Is the code clear and well-structured?
   - **Innovation:** Did the explorer discover novel approaches or insights?

## Output
Write a structured comparison to `evaluation.md` in your worktree with:
- Summary of each explorer's approach
- Side-by-side comparison table
- Recommendation with justification
- Risks and trade-offs for each approach

## Completion Protocol
- After writing evaluation.md, commit your work.
- Call `hive_update_task` to set status to "review".
- Send a message to the coordinator with your recommendation.
- Then stop.

## Constraints
- You are READ-ONLY for source code. Do NOT modify any source files.
- Only use Read, Glob, Grep, and Bash (for running tests) to examine code.
- You may write ONLY to evaluation.md in your own worktree.
- Do NOT spawn other agents.
- Do NOT submit to the merge queue.
- When done, stop. Do not loop.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Reviewer => format!(
                r#"You are a reviewer agent in a hive swarm.
Agent ID: {agent_id}
Role: reviewer
Parent: {}

## Your Review Task
{task_description}

## Responsibilities
- Review the code changes on this branch against the task description.
- Evaluate: correctness, completeness, code quality, scope discipline.
- Check that tests were added/updated and pass.
- Check that no unrelated files were modified.
- Submit your verdict via hive_review_verdict.

## Verdict Options
- **approve**: Code correctly implements the task, tests pass, no issues.
- **request-changes**: Code has specific issues that need fixing. Provide clear, actionable feedback.
- **reject**: Fundamentally wrong approach or task cannot be completed this way.

## Constraints
- You are READ-ONLY. Do NOT modify any files. Do NOT use Edit, Write, or Bash to change files.
- Only use Read, Glob, Grep to examine code.
- Use hive MCP tools only: hive_review_verdict, hive_read_messages, hive_list_tasks.
- Review the diff by reading the changed files and comparing to the task intent.
- Be thorough but concise. Focus on correctness over style.
- After submitting your verdict, stop immediately.
"#,
                parent.unwrap_or("coordinator")
            ),
            AgentRole::Planner => format!(
                r#"You are a planner agent in a hive swarm.
Agent ID: {agent_id}
Role: planner

## Goal
{task_description}

## Instructions
You are a READ-ONLY agent. Your job is to analyze the codebase and write a detailed implementation spec.

### Codebase Analysis
1. Read `Cargo.toml` to understand dependencies and project metadata.
2. Read `src/` to understand module structure — list every module and its responsibility.
3. Identify public APIs, key data types, and important traits.
4. Read existing tests to understand test patterns and conventions.
5. Check for any existing CLAUDE.md or documentation for project conventions.
6. If `.hive/memory/` exists, read run memory for patterns from previous runs.

### Spec Format
Write a spec in this exact format:
- **Goal:** One paragraph describing what to build.
- **Implementation Details:** Detailed technical description of changes needed.
- **Lead Decomposition:** Break the work into domain-level chunks, one per lead agent. Each chunk should specify:
  - Domain name
  - Files to modify (with clear boundaries — no file should appear in two domains)
  - Specific changes needed in each file
  - Dependencies on other chunks (merge ordering)
- **File Boundaries:** A table showing which files belong to which lead.
- **Merge Ordering:** Which leads must merge first due to type/API dependencies.

### Completion
When your spec is complete, call `hive_save_spec` with the full spec content.
Then stop immediately.

## Constraints
- You are READ-ONLY. Do NOT modify any files. Do NOT use Edit, Write, or Bash to change files.
- Only use Read, Glob, Grep to examine code.
- Use hive_save_spec to save your spec when done.
- After saving the spec, stop immediately.
"#
            ),
            AgentRole::Postmortem => format!(
                r#"You are a post-mortem analysis agent in a hive swarm.
Agent ID: {agent_id}
Role: postmortem

## Your Task
Analyze the completed run and extract learnings for future runs.

## Analysis Steps
1. Call `hive_list_tasks` to get all tasks — note which succeeded, failed, and why.
2. Call `hive_list_agents` to see all agents — note retry counts, stalls, and failures.
3. Call `hive_run_cost` to get token usage and cost data.
4. Read any available agent output files for error details.

## What to Analyze
- **Failure patterns:** What went wrong? Were there recurring issues (merge conflicts, test failures, scope creep)?
- **Token efficiency:** Which agents used the most tokens? Were any wasteful?
- **Spec quality:** Was the spec clear enough? Did leads need to ask for clarification?
- **Team sizing:** Were there too many or too few leads/workers? Did any domain need more parallelism?

## Memory Entries to Write
Use `hive_save_memory` for each entry type:

1. **operational** entry: Summary of the run with task counts, agent counts, costs, and key learnings.
2. **conventions** entry: Any codebase conventions discovered (naming, patterns, testing approaches).
3. **failure** entries: One per distinct failure pattern observed.

## Constraints
- You are READ-ONLY for code files. Do NOT modify source files.
- Use `hive_save_memory` to write memory entries.
- Be concise and actionable in your analysis — future agents will read this.
- After saving all memory entries, stop immediately.
"#
            ),
        };
        if memory.is_empty() {
            base
        } else {
            format!("{base}\n{memory}\n")
        }
    }

    /// Check if an agent process is still alive by PID.
    /// Uses waitpid(WNOHANG) first to reap zombies, then falls back to kill(0).
    pub fn is_alive(pid: u32) -> bool {
        // First, try to reap the process if it's a zombie child of ours.
        // waitpid returns: pid if reaped, 0 if still running, -1 on error (not our child).
        let mut status: libc::c_int = 0;
        let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
        if result == pid as i32 {
            // Process was a zombie child and we just reaped it — it's dead
            return false;
        }
        if result == 0 {
            // Process is our child and still running
            return true;
        }
        // result == -1: not our child (or invalid pid). Fall back to kill(0).
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_prompt_contains_role_and_id() {
        let prompt = AgentSpawner::generate_prompt(
            "coord-1",
            AgentRole::Coordinator,
            None,
            "Build a REST API",
            "",
        );
        assert!(prompt.contains("Agent ID: coord-1"));
        assert!(prompt.contains("Role: coordinator"));
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("Decompose the spec"));
        assert!(prompt.contains("Do NOT read or write implementation code"));
    }

    #[test]
    fn lead_prompt_contains_parent() {
        let prompt = AgentSpawner::generate_prompt(
            "lead-1",
            AgentRole::Lead,
            Some("coord-1"),
            "Handle backend domain",
            "",
        );
        assert!(prompt.contains("Agent ID: lead-1"));
        assert!(prompt.contains("Role: lead"));
        assert!(prompt.contains("Parent: coord-1"));
        assert!(prompt.contains("Handle backend domain"));
        assert!(prompt.contains("Spawn workers"));
        assert!(prompt.contains("Submit approved branches"));
        assert!(prompt.contains("Commit your work"));
        assert!(prompt.contains("Stop hook"));
    }

    #[test]
    fn lead_prompt_defaults_parent_to_coordinator() {
        let prompt = AgentSpawner::generate_prompt("lead-1", AgentRole::Lead, None, "task", "");
        assert!(prompt.contains("Parent: coordinator"));
    }

    #[test]
    fn worker_prompt_contains_parent_and_constraints() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement login endpoint",
            "",
        );
        assert!(prompt.contains("Agent ID: worker-1"));
        assert!(prompt.contains("Role: worker"));
        assert!(prompt.contains("Parent: lead-1"));
        assert!(prompt.contains("Implement login endpoint"));
        assert!(prompt.contains("Do not spawn other agents"));
        assert!(prompt.contains("Do not submit to the merge queue"));
        assert!(prompt.contains("Commit your work"));
        assert!(prompt.contains("Stop hook"));
    }

    #[test]
    fn worker_prompt_defaults_parent_to_unknown() {
        let prompt = AgentSpawner::generate_prompt("worker-1", AgentRole::Worker, None, "task", "");
        assert!(prompt.contains("Parent: unknown"));
    }

    #[test]
    fn context_management_prompt_in_lead() {
        let prompt =
            AgentSpawner::generate_prompt("lead-1", AgentRole::Lead, Some("coord-1"), "task", "");
        assert!(prompt.contains("## Context Management"));
        assert!(prompt.contains("commit your work, update the task status"));
    }

    #[test]
    fn context_management_prompt_in_worker() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "task",
            "",
        );
        assert!(prompt.contains("## Context Management"));
        assert!(prompt.contains("commit your work, update the task status"));
    }

    #[test]
    fn context_management_prompt_not_in_coordinator() {
        let prompt =
            AgentSpawner::generate_prompt("coord-1", AgentRole::Coordinator, None, "task", "");
        assert!(!prompt.contains("## Context Management"));
    }

    #[test]
    fn reviewer_prompt_contains_readonly_constraints() {
        let prompt = AgentSpawner::generate_prompt(
            "reviewer-1",
            AgentRole::Reviewer,
            Some("lead-1"),
            "Review the changes for task-123",
            "",
        );
        assert!(prompt.contains("Agent ID: reviewer-1"));
        assert!(prompt.contains("Role: reviewer"));
        assert!(prompt.contains("READ-ONLY"));
        assert!(prompt.contains("hive_review_verdict"));
        assert!(prompt.contains("Do NOT modify any files"));
    }

    #[test]
    fn is_alive_returns_true_for_current_process() {
        assert!(AgentSpawner::is_alive(std::process::id()));
    }

    #[test]
    fn is_alive_returns_false_for_bogus_pid() {
        assert!(!AgentSpawner::is_alive(99999999));
    }

    #[test]
    #[allow(clippy::zombie_processes)] // Intentionally creating a zombie to test reaping
    fn is_alive_reaps_zombie_child() {
        // Spawn a child that exits immediately, creating a zombie
        let child = std::process::Command::new("true")
            .spawn()
            .expect("failed to spawn 'true'");
        let pid = child.id();

        // Wait briefly for the child to exit and become a zombie
        std::thread::sleep(std::time::Duration::from_millis(100));

        // is_alive should reap the zombie and return false
        assert!(!AgentSpawner::is_alive(pid));
    }

    #[test]
    fn coordinator_prompt_has_task_creation_and_merge_protocols() {
        let prompt = AgentSpawner::generate_prompt(
            "coord-1",
            AgentRole::Coordinator,
            None,
            "Build something",
            "",
        );
        assert!(prompt.contains("## Task Creation Protocol"));
        assert!(prompt.contains("one task per domain/lead"));
        assert!(prompt.contains("Do NOT create worker-level subtasks"));
        assert!(!prompt.contains("Create ALL tasks FIRST"));
        assert!(prompt.contains("## Merge Queue Protocol"));
        assert!(prompt.contains("hive_merge_next"));
    }

    #[test]
    fn coordinator_prompt_fn_has_protocols_and_summary() {
        let prompt =
            AgentSpawner::coordinator_prompt("run-1", "spec here", "summary: 10 rust files", "");
        assert!(prompt.contains("## Project Summary"));
        assert!(prompt.contains("summary: 10 rust files"));
        assert!(prompt.contains("## Task Creation Protocol"));
        assert!(prompt.contains("## Merge Queue Protocol"));
    }

    #[test]
    fn lead_prompt_has_decomposition_review_health() {
        let prompt = AgentSpawner::generate_prompt(
            "lead-1",
            AgentRole::Lead,
            Some("coord-1"),
            "Handle backend",
            "",
        );
        assert!(prompt.contains("## Task Decomposition Protocol"));
        assert!(prompt.contains("Break your task into subtasks"));
        assert!(prompt.contains("parent_task set to your task ID"));
        assert!(prompt.contains("CANNOT submit to the merge queue until ALL subtasks are resolved"));
        assert!(prompt.contains("## Code Review Protocol"));
        assert!(prompt.contains("hive_review_agent"));
        assert!(prompt.contains("## Health Monitoring"));
        assert!(prompt.contains("hive_check_agents every 60 seconds"));
    }

    #[test]
    fn worker_prompt_has_implementation_scope_completion() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement feature",
            "",
        );
        assert!(prompt.contains("## Implementation Protocol"));
        assert!(prompt.contains("Write tests BEFORE implementation"));
        assert!(prompt.contains("## Scope Discipline"));
        assert!(prompt.contains("Only modify files in your assigned domain"));
        assert!(prompt.contains("## Completion Protocol"));
        assert!(prompt.contains("Run the full test suite one final time"));
    }

    #[test]
    fn generate_codebase_summary_returns_nonempty() {
        let tmp = std::env::temp_dir().join("hive_test_summary");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(
            tmp.join("Cargo.toml"),
            "name = \"test-project\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(tmp.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.join("src/lib.rs"), "").unwrap();

        let summary = AgentSpawner::generate_codebase_summary(&tmp);
        assert!(!summary.is_empty());
        assert!(summary.contains("test-project"));
        assert!(summary.contains("Rust files:"));
        assert!(summary.contains("Modules:"));
        assert!(summary.contains("main"));
        assert!(summary.contains("lib"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_memory_injection_appended_to_prompt() {
        let memory = "## Project Memory\n\n### Conventions\nUse snake_case everywhere.";
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement feature",
            memory,
        );
        assert!(prompt.contains("## Project Memory"));
        assert!(prompt.contains("Use snake_case everywhere."));
    }

    #[test]
    fn test_memory_injection_skipped_when_empty() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement feature",
            "",
        );
        assert!(!prompt.contains("Project Memory"));
    }

    #[test]
    fn test_coordinator_prompt_with_memory() {
        let memory = "## Project Memory\n\n### Recent Operations\nLast run had 5 tasks.";
        let prompt = AgentSpawner::coordinator_prompt(
            "run-1",
            "spec here",
            "summary: 10 rust files",
            memory,
        );
        assert!(prompt.contains("## Project Memory"));
        assert!(prompt.contains("Last run had 5 tasks."));
    }

    #[test]
    fn test_planner_prompt_has_detailed_instructions() {
        let prompt = AgentSpawner::generate_prompt(
            "planner-1",
            AgentRole::Planner,
            None,
            "Add WebSocket support",
            "",
        );
        assert!(prompt.contains("Role: planner"));
        assert!(prompt.contains("## Goal"));
        assert!(prompt.contains("Add WebSocket support"));
        assert!(prompt.contains("Codebase Analysis"));
        assert!(prompt.contains("Spec Format"));
        assert!(prompt.contains("hive_save_spec"));
        assert!(prompt.contains("READ-ONLY"));
    }

    #[test]
    fn test_postmortem_prompt_has_detailed_instructions() {
        let prompt = AgentSpawner::generate_prompt(
            "postmortem-1",
            AgentRole::Postmortem,
            None,
            "Analyze run",
            "",
        );
        assert!(prompt.contains("Role: postmortem"));
        assert!(prompt.contains("Analysis Steps"));
        assert!(prompt.contains("hive_list_tasks"));
        assert!(prompt.contains("hive_save_memory"));
        assert!(prompt.contains("Failure patterns"));
        assert!(prompt.contains("Token efficiency"));
    }

    #[test]
    fn test_postmortem_prompt_ignores_memory() {
        let memory = "## Project Memory\n\nSome memory content.";
        let prompt = AgentSpawner::generate_prompt(
            "postmortem-1",
            AgentRole::Postmortem,
            None,
            "Analyze run",
            memory,
        );
        // Postmortem still gets memory appended (memory filtering is done by load_memory_for_prompt
        // which returns empty for Postmortem), but if passed directly it should still append
        assert!(prompt.contains("Project Memory"));
    }

    #[test]
    fn test_explorer_prompt_full() {
        let prompt = AgentSpawner::generate_prompt(
            "explorer-1",
            AgentRole::Explorer,
            Some("coordinator"),
            "Explore alternative caching strategies",
            "",
        );
        assert!(prompt.contains("Agent ID: explorer-1"));
        assert!(prompt.contains("Role: explorer"));
        assert!(prompt.contains("Parent: coordinator"));
        assert!(prompt.contains("Explore alternative caching strategies"));
        assert!(prompt.contains("hive_discover"));
        assert!(prompt.contains("hive_query_mind"));
        assert!(prompt.contains("hive_update_task"));
        assert!(prompt.contains("## Context Management"));
        // Must NOT have spawn or merge queue access
        assert!(!prompt.contains("hive_spawn_agent"));
        assert!(!prompt.contains("hive_submit_to_queue"));
    }

    #[test]
    fn test_evaluator_prompt_full() {
        let prompt = AgentSpawner::generate_prompt(
            "evaluator-1",
            AgentRole::Evaluator,
            Some("coordinator"),
            "Evaluate explorer branches",
            "",
        );
        assert!(prompt.contains("Agent ID: evaluator-1"));
        assert!(prompt.contains("Role: evaluator"));
        assert!(prompt.contains("Parent: coordinator"));
        assert!(prompt.contains("READ-ONLY"));
        assert!(prompt.contains("evaluation.md"));
        assert!(prompt.contains("hive_query_mind"));
        assert!(prompt.contains("hive_update_task"));
        // Must NOT have spawn or merge queue access
        assert!(!prompt.contains("hive_spawn_agent"));
        assert!(!prompt.contains("hive_submit_to_queue"));
    }

    #[test]
    fn test_explore_coordinator_prompt_has_phases() {
        let prompt = AgentSpawner::explore_coordinator_prompt(
            "run-1",
            "Explore caching strategies",
            "summary: 10 rust files",
            "",
        );
        assert!(prompt.contains("EXPLORE"));
        assert!(prompt.contains("run-1"));
        assert!(prompt.contains("Explore caching strategies"));
        assert!(prompt.contains("summary: 10 rust files"));
        assert!(prompt.contains("Think Mode") || prompt.contains("Phase 1"));
        assert!(prompt.contains("adversarial"));
        assert!(prompt.contains("hive_query_mind"));
        assert!(prompt.contains("hive_discover"));
        assert!(prompt.contains("hive_synthesize"));
    }

    #[test]
    fn test_explore_coordinator_prompt_with_memory() {
        let memory = "## Project Memory\n\nSome prior insights.";
        let prompt = AgentSpawner::explore_coordinator_prompt("run-1", "intent", "summary", memory);
        assert!(prompt.contains("Project Memory"));
        assert!(prompt.contains("Some prior insights."));
    }

    #[test]
    fn test_explore_coordinator_prompt_without_memory() {
        let prompt = AgentSpawner::explore_coordinator_prompt("run-1", "intent", "summary", "");
        assert!(!prompt.contains("Project Memory"));
    }

    #[test]
    fn worker_prompt_has_no_code_changes_section() {
        let prompt = AgentSpawner::generate_prompt(
            "worker-1",
            AgentRole::Worker,
            Some("lead-1"),
            "Implement feature",
            "",
        );
        assert!(prompt.contains("## If No Code Changes Needed"));
        assert!(prompt.contains("Do not submit to review"));
    }

    #[test]
    fn coordinator_prompt_fn_has_new_task_creation_text() {
        let prompt =
            AgentSpawner::coordinator_prompt("run-1", "spec here", "summary", "");
        assert!(prompt.contains("Do NOT create worker-level subtasks"));
        assert!(!prompt.contains("Create ALL tasks FIRST"));
    }
}
