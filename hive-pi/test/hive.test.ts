import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";

// Mock pi-agent-core so no real LLM calls happen
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

import { Hive, transformContext, CONTEXT_CHAR_THRESHOLD, CONTEXT_KEEP_RECENT } from "../src/hive.js";
import { TaskStatus, AgentRole, AgentStatus } from "../src/types.js";
import type { AgentMessage } from "@mariozechner/pi-agent-core";

describe("Hive", () => {
  let repoDir: string;
  let hive: Hive;

  function setupRepo(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    execSync("git init", { cwd: dir });
    execSync("git config user.email 'test@test.com'", { cwd: dir });
    execSync("git config user.name 'Test'", { cwd: dir });
    fs.writeFileSync(path.join(dir, "README.md"), "# Test");
    fs.mkdirSync(path.join(dir, "src"), { recursive: true });
    fs.writeFileSync(path.join(dir, "src", "main.ts"), "// main");
    execSync("git add -A && git commit -m 'init'", { cwd: dir });
    return dir;
  }

  beforeEach(() => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });
  });

  afterEach(async () => {
    try {
      await hive.stop();
    } catch {}
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("initializes a run", () => {
    const runId = hive.initRun("# Spec\n- Task 1: Do something");
    const state = hive.stateManager.getState();
    expect(state.status).toBe("running");
    expect(state.runId).toBe(runId);
  });

  it("spawns a coordinator agent (no worktree)", async () => {
    hive.initRun("# Spec");
    const agent = await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    expect(agent.role).toBe(AgentRole.Coordinator);
    expect(agent.status).toBe(AgentStatus.Running);
    expect(agent.worktree).toBeUndefined();
  });

  it("spawns a lead with a worktree", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Backend", description: "Build API" });
    const agent = await hive.spawnAgent({
      agentId: "lead-backend",
      role: AgentRole.Lead,
      taskId: task.id,
      parentAgent: "coordinator",
    });
    expect(agent.worktree).toBeDefined();
    expect(fs.existsSync(agent.worktree!)).toBe(true);
    expect(agent.branch).toBeDefined();
  });

  it("workers share parent lead's worktree", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    const workerTask = hive.taskManager.create({
      title: "Subtask",
      description: "Do it",
      parentTask: task.id,
    });

    const lead = await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
    });
    hive.taskManager.updateStatus(workerTask.id, TaskStatus.Active);
    const worker = await hive.spawnAgent({
      agentId: "worker-1",
      role: AgentRole.Worker,
      taskId: workerTask.id,
      parentAgent: "lead-1",
    });

    // Worker should share lead's worktree
    expect(worker.worktree).toBe(lead.worktree);
    expect(worker.branch).toBe(lead.branch);
  });

  it("sends a message between agents", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Work", description: "Do it" });
    await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
      parentAgent: "coordinator",
    });

    hive.sendMessage("coordinator", "lead-1", "Start working");

    const messages = hive.stateManager.getState().messages;
    expect(messages).toHaveLength(1);
    expect(messages[0].from).toBe("coordinator");
    expect(messages[0].to).toBe("lead-1");
    expect(messages[0].body).toBe("Start working");
  });

  it("getMessages filters by recipient and since timestamp", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Work", description: "Do it" });
    await hive.spawnAgent({ agentId: "lead-1", role: AgentRole.Lead, taskId: task.id });

    const before = Date.now();
    hive.sendMessage("coord", "lead-1", "Message 1");
    await new Promise((r) => setTimeout(r, 5));
    const after = Date.now();
    hive.sendMessage("coord", "lead-1", "Message 2");
    hive.sendMessage("coord", "lead-2", "Not for lead-1");

    // All messages for lead-1
    const allForLead1 = hive.getMessages("lead-1");
    expect(allForLead1).toHaveLength(2);

    // Messages since 'after' — only Message 2
    const newMessages = hive.getMessages("lead-1", after);
    expect(newMessages).toHaveLength(1);
    expect(newMessages[0].body).toBe("Message 2");

    // Messages for lead-2 — only 1
    const lead2Messages = hive.getMessages("lead-2");
    expect(lead2Messages).toHaveLength(1);
  });

  it("submits to merge queue and processes successfully", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    hive.taskManager.updateStatus(task.id, TaskStatus.Active);
    hive.taskManager.updateStatus(task.id, TaskStatus.Review);

    const lead = await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
    });

    // Make a change in lead's worktree
    fs.writeFileSync(path.join(lead.worktree!, "feature.txt"), "done");
    execSync("git add -A && git commit -m 'add feature'", { cwd: lead.worktree! });

    hive.submitToMergeQueue(task.id, lead.branch!, "lead-1");
    const result = await hive.processMergeQueue();
    expect(result).toContain("Merged");

    // Task should now be merged
    const updated = hive.taskManager.get(task.id);
    expect(updated.status).toBe(TaskStatus.Merged);
  });

  it("processMergeQueue returns message when queue is empty", async () => {
    hive.initRun("# Spec");
    const result = await hive.processMergeQueue();
    expect(result).toContain("No pending");
  });

  // ── transformContext tests ──────────────────────────────────────────────────

  it("transformContext returns messages unchanged when under threshold", async () => {
    const messages: AgentMessage[] = [
      { role: "user", content: "hello", timestamp: 1 },
      { role: "user", content: "world", timestamp: 2 },
    ];
    const result = await transformContext(messages);
    expect(result).toEqual(messages);
  });

  it("transformContext returns messages unchanged when not enough to truncate", async () => {
    const bigContent = "x".repeat(CONTEXT_CHAR_THRESHOLD + 1);
    const messages: AgentMessage[] = Array.from({ length: CONTEXT_KEEP_RECENT + 1 }, (_, i) => ({
      role: "user" as const,
      content: i === 0 ? bigContent : "short",
      timestamp: i,
    }));
    const result = await transformContext(messages);
    expect(result).toHaveLength(messages.length);
  });

  it("transformContext truncates middle messages when over threshold", async () => {
    const msgCount = CONTEXT_KEEP_RECENT + 10;
    const chunkSize = Math.ceil(CONTEXT_CHAR_THRESHOLD / msgCount) + 1;
    const messages: AgentMessage[] = Array.from({ length: msgCount }, (_, i) => ({
      role: "user" as const,
      content: "y".repeat(chunkSize),
      timestamp: i,
    }));

    const result = await transformContext(messages);
    expect(result[0]).toEqual(messages[0]);
    const marker = result[1] as any;
    expect(marker.role).toBe("user");
    expect(marker.content).toMatch(/\[context truncated — \d+ earlier messages removed\]/);
    const tail = result.slice(2);
    expect(tail).toHaveLength(CONTEXT_KEEP_RECENT);
    expect(tail[0]).toEqual(messages[msgCount - CONTEXT_KEEP_RECENT]);
    expect(result).toHaveLength(CONTEXT_KEEP_RECENT + 2);
  });

  it("transformContext marker contains correct removed count", async () => {
    const msgCount = CONTEXT_KEEP_RECENT + 5;
    const chunkSize = Math.ceil(CONTEXT_CHAR_THRESHOLD / msgCount) + 1;
    const messages: AgentMessage[] = Array.from({ length: msgCount }, (_, i) => ({
      role: "user" as const,
      content: "z".repeat(chunkSize),
      timestamp: i,
    }));

    const result = await transformContext(messages);
    const marker = result[1] as any;
    const expectedRemoved = msgCount - 1 - CONTEXT_KEEP_RECENT;
    expect(marker.content).toContain(`${expectedRemoved} earlier messages removed`);
  });

  it("transformContext preserves most recent messages in order", async () => {
    const msgCount = CONTEXT_KEEP_RECENT + 10;
    const chunkSize = Math.ceil(CONTEXT_CHAR_THRESHOLD / msgCount) + 1;
    const messages: AgentMessage[] = Array.from({ length: msgCount }, (_, i) => ({
      role: "user" as const,
      content: `msg-${i}-${"a".repeat(chunkSize)}`,
      timestamp: i,
    }));

    const result = await transformContext(messages);
    const tail = result.slice(2);
    for (let i = 0; i < CONTEXT_KEEP_RECENT; i++) {
      const expectedMsg = messages[msgCount - CONTEXT_KEEP_RECENT + i] as any;
      const actualMsg = tail[i] as any;
      expect(actualMsg.content).toBe(expectedMsg.content);
    }
  });

  it("recordDiscovery writes to discoveries.jsonl", async () => {
    hive.initRun("# Spec");
    hive.recordDiscovery("agent-1", {
      content: "Found something interesting",
      confidence: "high",
      tags: ["architecture"],
    });

    const filePath = path.join(repoDir, ".hive", "discoveries.jsonl");
    expect(fs.existsSync(filePath)).toBe(true);
    const content = fs.readFileSync(filePath, "utf-8").trim();
    const entry = JSON.parse(content);
    expect(entry.agentId).toBe("agent-1");
    expect(entry.content).toBe("Found something interesting");
    expect(entry.confidence).toBe("high");
    expect(entry.tags).toContain("architecture");
  });
});
