/**
 * Edge case tests — adversarial audit of hive-pi.
 *
 * Tests are grouped by bug category. Each test documents a real defect
 * found during code review.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";
import { TaskManager } from "../src/task-manager.js";
import { StateManager } from "../src/state.js";
import { TaskStatus } from "../src/types.js";

// ─── Mock pi-agent-core ───────────────────────────────────────────────────────
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
    get state() { return this._state; }
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
import { AgentRole } from "../src/types.js";

// ─── Helpers ──────────────────────────────────────────────────────────────────
function makeTmpDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "hive-edge-"));
}

function setupGitRepo(): string {
  const dir = makeTmpDir();
  execSync("git init", { cwd: dir });
  execSync("git config user.email 'test@test.com'", { cwd: dir });
  execSync("git config user.name 'Test'", { cwd: dir });
  fs.writeFileSync(path.join(dir, "README.md"), "# Test");
  execSync("git add -A && git commit -m 'init'", { cwd: dir });
  return dir;
}

// ─── Bug 1: Blocked tasks stuck when dependency FAILS ───────────────────────
describe("Bug 1: blocked tasks stuck when dependency fails", () => {
  let tmpDir: string;
  let stateManager: StateManager;
  let taskManager: TaskManager;

  beforeEach(() => {
    tmpDir = makeTmpDir();
    stateManager = new StateManager(tmpDir);
    stateManager.initRun("test-run", "spec.md");
    taskManager = new TaskManager(stateManager);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("should unblock dependents when a dependency fails", () => {
    const taskA = taskManager.create({ title: "A", description: "Dependency" });
    const taskB = taskManager.create({
      title: "B",
      description: "Depends on A",
      blockedBy: [taskA.id],
    });

    expect(taskB.status).toBe(TaskStatus.Blocked);

    taskManager.updateStatus(taskA.id, TaskStatus.Active);
    taskManager.updateStatus(taskA.id, TaskStatus.Failed);

    const updatedB = taskManager.get(taskB.id);
    expect(updatedB.status).toBe(TaskStatus.Pending);
  });

  it("should remove failed task from blockedBy list", () => {
    const taskA = taskManager.create({ title: "A", description: "dep 1" });
    const taskC = taskManager.create({ title: "C", description: "dep 2" });
    const taskB = taskManager.create({
      title: "B",
      description: "depends on A and C",
      blockedBy: [taskA.id, taskC.id],
    });

    taskManager.updateStatus(taskA.id, TaskStatus.Active);
    taskManager.updateStatus(taskA.id, TaskStatus.Review);
    taskManager.updateStatus(taskA.id, TaskStatus.Merged);

    expect(taskManager.get(taskB.id).status).toBe(TaskStatus.Blocked);
    expect(taskManager.get(taskB.id).blockedBy).toContain(taskC.id);

    taskManager.updateStatus(taskC.id, TaskStatus.Active);
    taskManager.updateStatus(taskC.id, TaskStatus.Failed);

    const updatedB = taskManager.get(taskB.id);
    expect(updatedB.blockedBy).not.toContain(taskC.id);
    expect(updatedB.status).toBe(TaskStatus.Pending);
  });

  it("stays blocked if a sibling dependency is still active", () => {
    const taskA = taskManager.create({ title: "A", description: "dep 1" });
    const taskC = taskManager.create({ title: "C", description: "dep 2 (active)" });
    const taskB = taskManager.create({
      title: "B",
      description: "depends on A and C",
      blockedBy: [taskA.id, taskC.id],
    });

    taskManager.updateStatus(taskA.id, TaskStatus.Active);
    taskManager.updateStatus(taskA.id, TaskStatus.Failed);

    taskManager.updateStatus(taskC.id, TaskStatus.Active);
    const updatedB = taskManager.get(taskB.id);
    expect(updatedB.status).toBe(TaskStatus.Blocked);
  });
});

// ─── Bug 2: Worker without valid parent gets own worktree ────────────────────
describe("Bug 2: worker without valid parent gets own worktree", () => {
  let repoDir: string;
  let hive: Hive;

  beforeEach(() => {
    repoDir = setupGitRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Spec");
  });

  afterEach(async () => {
    try { await hive.stop(); } catch {}
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("should throw when worker's parentAgent does not exist in state", async () => {
    const task = hive.taskManager.create({ title: "subtask", description: "do it" });
    hive.taskManager.updateStatus(task.id, TaskStatus.Active);

    await expect(
      hive.spawnAgent({
        agentId: "worker-orphan",
        role: AgentRole.Worker,
        taskId: task.id,
        parentAgent: "lead-does-not-exist",
      })
    ).rejects.toThrow();
  });
});

// ─── Bug 3: Partial JSONL write causes parse crash ───────────────────────────
describe("Bug 3: partial JSONL line causes parse crash", () => {
  let tmpDir: string;
  let stateManager: StateManager;

  beforeEach(() => {
    tmpDir = makeTmpDir();
    stateManager = new StateManager(tmpDir);
    stateManager.initRun("test-run", "spec.md");
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("should handle truncated last line in messages.jsonl gracefully", () => {
    stateManager.appendAgentMessage("agent-1", {
      role: "user",
      content: "First message",
      timestamp: 1000,
    });
    stateManager.appendAgentMessage("agent-1", {
      role: "user",
      content: "Second message",
      timestamp: 2000,
    });

    const filePath = path.join(tmpDir, "agents", "agent-1", "messages.jsonl");
    fs.appendFileSync(filePath, '{"role":"user","content":"truncated');

    expect(() => stateManager.loadAgentMessages("agent-1")).not.toThrow();
    const messages = stateManager.loadAgentMessages("agent-1");
    expect(messages).toHaveLength(2);
    expect(messages[0].content).toBe("First message");
    expect(messages[1].content).toBe("Second message");
  });

  it("should handle completely empty JSONL file gracefully", () => {
    const agentDir = path.join(tmpDir, "agents", "agent-2");
    fs.mkdirSync(agentDir, { recursive: true });
    fs.writeFileSync(path.join(agentDir, "messages.jsonl"), "");

    expect(() => stateManager.loadAgentMessages("agent-2")).not.toThrow();
    const messages = stateManager.loadAgentMessages("agent-2");
    expect(messages).toHaveLength(0);
  });
});

// ─── Bug 4: Concurrent processMergeQueue double-processes entries ────────────
describe("Bug 4: concurrent merge queue processing", () => {
  let repoDir: string;
  let hive: Hive;

  beforeEach(() => {
    repoDir = setupGitRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Spec");
  });

  afterEach(async () => {
    try { await hive.stop(); } catch {}
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("second processMergeQueue call should not reprocess a merging entry", async () => {
    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    hive.taskManager.updateStatus(task.id, TaskStatus.Active);
    hive.taskManager.updateStatus(task.id, TaskStatus.Review);

    const lead = await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
    });

    fs.writeFileSync(path.join(lead.worktree!, "feature.txt"), "done");
    execSync("git add -A && git commit -m 'add feature'", { cwd: lead.worktree! });

    hive.submitToMergeQueue(task.id, lead.branch!, "lead-1");
    hive.stateManager.updateMergeQueueEntry(task.id, { status: "merging" });

    const result = await hive.processMergeQueue();
    expect(result).toContain("No pending");
  });

  it("does not corrupt state when processMergeQueue is called on an already-merged branch", async () => {
    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    hive.taskManager.updateStatus(task.id, TaskStatus.Active);
    hive.taskManager.updateStatus(task.id, TaskStatus.Review);

    const lead = await hive.spawnAgent({
      agentId: "lead-2",
      role: AgentRole.Lead,
      taskId: task.id,
    });
    fs.writeFileSync(path.join(lead.worktree!, "f2.txt"), "done");
    execSync("git add -A && git commit -m 'f2'", { cwd: lead.worktree! });

    hive.submitToMergeQueue(task.id, lead.branch!, "lead-2");
    hive.stateManager.updateMergeQueueEntry(task.id, { status: "merging" });

    const r1 = await hive.processMergeQueue();
    expect(r1).toContain("No pending");

    const stuckTask = hive.taskManager.get(task.id);
    expect(stuckTask.status).toBe(TaskStatus.Review); // stuck, never merges
  });
});

// ─── Bug 5: stop() persists status ───────────────────────────────────────────
describe("Bug 5: stop() updates status via direct mutation", () => {
  let repoDir: string;
  let hive: Hive;

  beforeEach(() => {
    repoDir = setupGitRepo();
    hive = new Hive({ repoRoot: repoDir });
    hive.initRun("# Spec");
  });

  afterEach(() => {
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("stop() persists status change to disk", async () => {
    await hive.stop();

    const loaded = new StateManager(path.join(repoDir, ".hive"));
    loaded.load();
    expect(loaded.getState().status).toBe("stopped");
  });
});

// ─── Bug 6: isSuccessTaskStatus includes Cancelled ───────────────────────────
describe("Bug 6: cancelling a dependency unblocks dependents (undocumented behavior)", () => {
  let tmpDir: string;
  let stateManager: StateManager;
  let taskManager: TaskManager;

  beforeEach(() => {
    tmpDir = makeTmpDir();
    stateManager = new StateManager(tmpDir);
    stateManager.initRun("test-run", "spec.md");
    taskManager = new TaskManager(stateManager);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("documents that cancelling a blocking task unblocks the dependent", () => {
    const taskA = taskManager.create({ title: "A", description: "dep" });
    const taskB = taskManager.create({
      title: "B",
      description: "depends on A",
      blockedBy: [taskA.id],
    });

    expect(taskB.status).toBe(TaskStatus.Blocked);
    taskManager.updateStatus(taskA.id, TaskStatus.Cancelled);

    const updatedB = taskManager.get(taskB.id);
    expect(updatedB.status).toBe(TaskStatus.Pending);
  });
});
