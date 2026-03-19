import { describe, it, expect } from "vitest";
import { coordinatorPrompt, leadPrompt, workerPrompt } from "../src/prompts/index.js";
import { TaskStatus, AgentRole } from "../src/types.js";
import type { Task } from "../src/types.js";

function makeTask(overrides: Partial<Task> = {}): Task {
  return {
    id: "t-abc12345",
    title: "Build feature",
    description: "Implement X and Y",
    status: TaskStatus.Active,
    blockedBy: [],
    createdAt: Date.now(),
    updatedAt: Date.now(),
    ...overrides,
  };
}

describe("coordinatorPrompt", () => {
  it("includes run ID", () => {
    const prompt = coordinatorPrompt("run-abc123", "spec content", []);
    expect(prompt).toContain("run-abc123");
  });

  it("includes spec content", () => {
    const prompt = coordinatorPrompt("run-1", "Build a todo app", []);
    expect(prompt).toContain("Build a todo app");
  });

  it("lists tasks with status", () => {
    const tasks = [
      makeTask({ id: "t-1", title: "Frontend", status: TaskStatus.Active }),
      makeTask({ id: "t-2", title: "Backend", status: TaskStatus.Pending }),
    ];
    const prompt = coordinatorPrompt("run-1", "spec", tasks);
    expect(prompt).toContain("t-1");
    expect(prompt).toContain("Frontend");
    expect(prompt).toContain("active");
    expect(prompt).toContain("t-2");
    expect(prompt).toContain("Backend");
    expect(prompt).toContain("pending");
  });

  it("shows blocked-by info for blocked tasks", () => {
    const tasks = [makeTask({ id: "t-2", blockedBy: ["t-1"] })];
    const prompt = coordinatorPrompt("run-1", "spec", tasks);
    expect(prompt).toContain("t-1");
    expect(prompt).toContain("blocked by");
  });

  it("shows placeholder when no tasks", () => {
    const prompt = coordinatorPrompt("run-1", "spec", []);
    expect(prompt).toContain("No tasks yet");
  });

  it("contains role constraints", () => {
    const prompt = coordinatorPrompt("run-1", "spec", []);
    expect(prompt).toContain("lead");
    expect(prompt).toContain("spawn");
    expect(prompt).toContain("merge queue");
  });
});

describe("leadPrompt", () => {
  it("includes agent ID and task info", () => {
    const task = makeTask({ title: "API layer", description: "Build REST endpoints" });
    const prompt = leadPrompt("lead-api", task, "/worktrees/lead-api");
    expect(prompt).toContain("lead-api");
    expect(prompt).toContain("API layer");
    expect(prompt).toContain("Build REST endpoints");
  });

  it("includes worktree path", () => {
    const task = makeTask();
    const prompt = leadPrompt("lead-1", task, "/tmp/worktrees/lead-1");
    expect(prompt).toContain("/tmp/worktrees/lead-1");
  });

  it("includes branch when assigned", () => {
    const task = makeTask({ branch: "hive/run-1/lead-backend" });
    const prompt = leadPrompt("lead-1", task, "/worktrees/lead-1");
    expect(prompt).toContain("hive/run-1/lead-backend");
  });

  it("includes domain when set", () => {
    const task = makeTask({ domain: "src/backend" });
    const prompt = leadPrompt("lead-1", task, "/worktrees/lead-1");
    expect(prompt).toContain("src/backend");
  });

  it("contains role constraints (no spawning workers directly for non-lead)", () => {
    const task = makeTask();
    const prompt = leadPrompt("lead-1", task, "/worktrees/lead-1");
    expect(prompt).toContain("worker");
    expect(prompt).toContain("submit_to_queue");
    expect(prompt).toContain("spawn");
  });
});

describe("workerPrompt", () => {
  it("includes agent ID and task info", () => {
    const task = makeTask({ title: "Write tests", description: "Add unit tests for module X" });
    const prompt = workerPrompt("worker-1", task, "/worktrees/worker-1");
    expect(prompt).toContain("worker-1");
    expect(prompt).toContain("Write tests");
    expect(prompt).toContain("Add unit tests for module X");
  });

  it("includes worktree path", () => {
    const task = makeTask();
    const prompt = workerPrompt("worker-1", task, "/tmp/worktrees/worker-1");
    expect(prompt).toContain("/tmp/worktrees/worker-1");
  });

  it("includes domain scope when set", () => {
    const task = makeTask({ domain: "src/auth" });
    const prompt = workerPrompt("worker-1", task, "/worktrees/worker-1");
    expect(prompt).toContain("src/auth");
  });

  it("contains constraints: no spawn, no submit", () => {
    const task = makeTask();
    const prompt = workerPrompt("worker-1", task, "/worktrees/worker-1");
    expect(prompt).toContain("Do NOT spawn other agents");
    expect(prompt).toContain("Do NOT submit to the merge queue");
  });

  it("instructs to update task to review when done", () => {
    const task = makeTask();
    const prompt = workerPrompt("worker-1", task, "/worktrees/worker-1");
    expect(prompt).toContain("review");
    expect(prompt).toContain("update_task");
  });
});
