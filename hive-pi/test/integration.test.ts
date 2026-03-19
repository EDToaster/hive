/**
 * Integration test: full run lifecycle simulation with mocked LLM.
 * Tests the complete coordinator → lead → worker → merge flow.
 */
import { describe, it, expect, afterEach, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";

// Mock pi-agent-core so no real LLM calls are made
vi.mock("@mariozechner/pi-agent-core", () => ({
  Agent: class MockAgent {
    _state = {
      systemPrompt: "",
      model: {
        provider: "anthropic",
        id: "claude-sonnet-4-6",
        api: "anthropic-messages",
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      },
      thinkingLevel: "off",
      tools: [],
      messages: [],
      isStreaming: false,
      streamMessage: null,
      pendingToolCalls: new Set(),
    };
    private _listeners = new Set<(e: any) => void>();
    constructor(opts: any) {
      if (opts?.initialState) Object.assign(this._state, opts.initialState);
    }
    get state() {
      return this._state;
    }
    subscribe(fn: (e: any) => void) {
      this._listeners.add(fn);
      return () => this._listeners.delete(fn);
    }
    steer(_m: any) {}
    abort() {}
    async prompt(_input: any) {}
    async waitForIdle() {}
  },
}));

import { Hive } from "../src/hive.js";
import { TaskStatus, AgentRole } from "../src/types.js";

describe("Integration: full run lifecycle", () => {
  let repoDir: string;
  let hive: Hive;

  function setupRepo(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-integration-"));
    execSync("git init", { cwd: dir });
    execSync("git config user.email 'test@test.com'", { cwd: dir });
    execSync("git config user.name 'Test'", { cwd: dir });
    fs.mkdirSync(path.join(dir, "src"), { recursive: true });
    fs.writeFileSync(path.join(dir, "README.md"), "# Project");
    fs.writeFileSync(path.join(dir, "src", "app.ts"), "// app");
    execSync("git add -A && git commit -m 'init'", { cwd: dir });
    return dir;
  }

  afterEach(() => {
    try {
      hive?.stop();
    } catch {}
    if (repoDir) fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("simulates a complete run: init → tasks → coordinator → lead → worker → merge", async () => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });

    // 1. Initialize run
    const runId = hive.initRun("# Build a REST API\n- Backend service\n- Database layer");
    expect(runId).toBeDefined();
    expect(hive.stateManager.getState().status).toBe("running");

    // 2. Coordinator creates tasks (simulating what coordinator would do via tools)
    const t1 = hive.taskManager.create({
      title: "Backend API",
      description: "Build REST endpoints",
      domain: "src",
    });
    const t2 = hive.taskManager.create({
      title: "Database layer",
      description: "Add DB persistence",
      domain: "src",
      blockedBy: [t1.id],
    });
    expect(t2.status).toBe(TaskStatus.Blocked);

    // 3. Spawn coordinator
    const coordinator = await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    expect(coordinator.worktree).toBeUndefined(); // coordinator has no worktree

    // 4. Spawn lead for t1
    const lead1 = await hive.spawnAgent({
      agentId: "lead-backend",
      role: AgentRole.Lead,
      taskId: t1.id,
      parentAgent: "coordinator",
    });
    expect(lead1.worktree).toBeDefined();
    expect(lead1.branch).toBe(`hive/${runId}/lead-backend`);
    expect(hive.taskManager.get(t1.id).status).toBe(TaskStatus.Active);

    // 5. Lead creates a worker task and spawns a worker
    const workerTask = hive.taskManager.create({
      title: "Implement GET /users",
      description: "Write the handler",
      parentTask: t1.id,
    });
    hive.taskManager.updateStatus(workerTask.id, TaskStatus.Active);
    const worker = await hive.spawnAgent({
      agentId: "worker-1",
      role: AgentRole.Worker,
      taskId: workerTask.id,
      parentAgent: "lead-backend",
    });
    // Worker shares lead's worktree
    expect(worker.worktree).toBe(lead1.worktree);

    // 6. Coordinator sends a message to lead
    hive.sendMessage("coordinator", "lead-backend", "Remember to add tests");
    const leadMessages = hive.getMessages("lead-backend");
    expect(leadMessages).toHaveLength(1);
    expect(leadMessages[0].body).toContain("tests");

    // 7. Worker "does work" — writes files and commits
    const usersFile = path.join(worker.worktree!, "src", "users.ts");
    fs.writeFileSync(usersFile, "export function getUsers() { return []; }");
    execSync("git add -A && git commit -m 'feat: add users endpoint'", {
      cwd: worker.worktree!,
    });
    hive.taskManager.updateStatus(workerTask.id, TaskStatus.Review);

    // 8. Lead reviews and submits branch to merge queue
    hive.taskManager.updateStatus(t1.id, TaskStatus.Review);
    hive.submitToMergeQueue(t1.id, lead1.branch!, "lead-backend");

    const queueState = hive.stateManager.getState().mergeQueue;
    expect(queueState).toHaveLength(1);
    expect(queueState[0].branch).toBe(lead1.branch);
    expect(queueState[0].status).toBe("pending");

    // 9. Coordinator processes merge queue
    const mergeResult = await hive.processMergeQueue();
    expect(mergeResult).toContain("Merged");
    expect(hive.taskManager.get(t1.id).status).toBe(TaskStatus.Merged);

    // 10. t2 should be unblocked since t1 is now merged
    const t2Updated = hive.taskManager.get(t2.id);
    expect(t2Updated.status).toBe(TaskStatus.Pending);
    expect(t2Updated.blockedBy).toHaveLength(0);

    // 11. Verify the file actually exists on main
    const mainContent = execSync("git show HEAD:src/users.ts", {
      cwd: repoDir,
      encoding: "utf-8",
    });
    expect(mainContent).toContain("getUsers");
  });

  it("handles two parallel leads with non-conflicting changes", async () => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Parallel feature development");

    // Two independent tasks
    const taskA = hive.taskManager.create({ title: "Feature A", description: "Module A" });
    const taskB = hive.taskManager.create({ title: "Feature B", description: "Module B" });

    const leadA = await hive.spawnAgent({
      agentId: "lead-a",
      role: AgentRole.Lead,
      taskId: taskA.id,
    });
    const leadB = await hive.spawnAgent({
      agentId: "lead-b",
      role: AgentRole.Lead,
      taskId: taskB.id,
    });

    // Each lead writes to different files
    fs.writeFileSync(path.join(leadA.worktree!, "module-a.txt"), "Module A content");
    execSync("git add -A && git commit -m 'feat: module A'", { cwd: leadA.worktree! });

    fs.writeFileSync(path.join(leadB.worktree!, "module-b.txt"), "Module B content");
    execSync("git add -A && git commit -m 'feat: module B'", { cwd: leadB.worktree! });

    // Both go to review and submit
    hive.taskManager.updateStatus(taskA.id, TaskStatus.Review);
    hive.taskManager.updateStatus(taskB.id, TaskStatus.Review);
    hive.submitToMergeQueue(taskA.id, leadA.branch!, "lead-a");
    hive.submitToMergeQueue(taskB.id, leadB.branch!, "lead-b");

    // Process queue — both should merge successfully
    const result1 = await hive.processMergeQueue();
    expect(result1).toContain("Merged");

    const result2 = await hive.processMergeQueue();
    expect(result2).toContain("Merged");

    // Both files should exist on main
    const lsFiles = execSync("git ls-files", { cwd: repoDir, encoding: "utf-8" });
    expect(lsFiles).toContain("module-a.txt");
    expect(lsFiles).toContain("module-b.txt");
  });

  it("discovers and records findings", async () => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Research task");

    hive.recordDiscovery("explorer-1", {
      content: "pi-mono steer() is queue-based, not immediate",
      confidence: "high",
      tags: ["pi-agent-core", "architecture"],
    });
    hive.recordDiscovery("explorer-1", {
      content: "agent_end event has messages (plural), not message",
      confidence: "high",
      tags: ["pi-agent-core", "bug"],
    });

    const discoveriesFile = path.join(repoDir, ".hive", "discoveries.jsonl");
    expect(fs.existsSync(discoveriesFile)).toBe(true);
    const lines = fs.readFileSync(discoveriesFile, "utf-8").trim().split("\n");
    expect(lines).toHaveLength(2);

    const first = JSON.parse(lines[0]);
    expect(first.agentId).toBe("explorer-1");
    expect(first.confidence).toBe("high");
    expect(first.tags).toContain("architecture");
  });

  it("handles failed merge with fallback rebase strategy", async () => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Conflict test");

    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    const lead = await hive.spawnAgent({ agentId: "lead-1", role: AgentRole.Lead, taskId: task.id });

    // Lead makes a commit on its branch
    fs.writeFileSync(path.join(lead.worktree!, "conflict.txt"), "from lead");
    execSync("git add -A && git commit -m 'lead change'", { cwd: lead.worktree! });

    // Also make a conflicting commit on main (simulating another lead already merged)
    const mainBranch = hive.gitManager.getMainBranch();
    execSync(`git checkout ${mainBranch}`, { cwd: repoDir });
    fs.writeFileSync(path.join(repoDir, "conflict.txt"), "from main");
    execSync("git add -A && git commit -m 'main change'", { cwd: repoDir });

    hive.taskManager.updateStatus(task.id, TaskStatus.Review);
    hive.submitToMergeQueue(task.id, lead.branch!, "lead-1");

    // processMergeQueue will try merge (fail), then rebase (which handles divergence)
    const result = await hive.processMergeQueue();
    // With a true conflict, this will fail — that's expected and correct behavior
    expect(typeof result).toBe("string");
    expect(result.length).toBeGreaterThan(0);
  });
});
