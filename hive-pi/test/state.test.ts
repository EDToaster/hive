import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { StateManager } from "../src/state.js";
import { TaskStatus, AgentRole, AgentStatus } from "../src/types.js";
import type { RunState, Task } from "../src/types.js";

describe("StateManager", () => {
  let tmpDir: string;
  let stateManager: StateManager;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    stateManager = new StateManager(tmpDir);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("initializes a new run", () => {
    const state = stateManager.initRun("test-run", "spec.md");
    expect(state.runId).toBe("test-run");
    expect(state.specFile).toBe("spec.md");
    expect(state.status).toBe("running");
    expect(Object.keys(state.tasks)).toHaveLength(0);
  });

  it("persists and loads state", () => {
    stateManager.initRun("test-run", "spec.md");
    stateManager.addTask({
      id: "t1",
      title: "Test task",
      description: "Do something",
      status: TaskStatus.Pending,
      blockedBy: [],
      createdAt: Date.now(),
      updatedAt: Date.now(),
    });
    stateManager.save();

    // Create a new StateManager pointing at same directory
    const loaded = new StateManager(tmpDir);
    loaded.load();
    const state = loaded.getState();
    expect(state.tasks["t1"].title).toBe("Test task");
  });

  it("appends agent messages to JSONL", () => {
    stateManager.initRun("test-run", "spec.md");
    stateManager.appendAgentMessage("agent-1", {
      role: "user",
      content: "Hello",
      timestamp: Date.now(),
    });
    stateManager.appendAgentMessage("agent-1", {
      role: "assistant",
      content: [{ type: "text", text: "Hi" }],
      api: "anthropic-messages",
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      stopReason: "stop",
      timestamp: Date.now(),
    });

    const messages = stateManager.loadAgentMessages("agent-1");
    expect(messages).toHaveLength(2);
    expect(messages[0].role).toBe("user");
    expect(messages[1].role).toBe("assistant");
  });
});
