import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { TaskManager } from "../src/task-manager.js";
import { StateManager } from "../src/state.js";
import { TaskStatus } from "../src/types.js";

describe("TaskManager", () => {
  let tmpDir: string;
  let stateManager: StateManager;
  let taskManager: TaskManager;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    stateManager = new StateManager(tmpDir);
    stateManager.initRun("test-run", "spec.md");
    taskManager = new TaskManager(stateManager);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("creates a task", () => {
    const task = taskManager.create({
      title: "Build feature",
      description: "Implement X",
    });
    expect(task.id).toBeDefined();
    expect(task.status).toBe(TaskStatus.Pending);
    expect(task.title).toBe("Build feature");
  });

  it("creates a task with blocked_by", () => {
    const t1 = taskManager.create({ title: "First", description: "Do first" });
    const t2 = taskManager.create({
      title: "Second",
      description: "Do second",
      blockedBy: [t1.id],
    });
    expect(t2.status).toBe(TaskStatus.Blocked);
    expect(t2.blockedBy).toEqual([t1.id]);
  });

  it("unblocks tasks when dependencies complete", () => {
    const t1 = taskManager.create({ title: "First", description: "Do first" });
    const t2 = taskManager.create({
      title: "Second",
      description: "Do second",
      blockedBy: [t1.id],
    });
    expect(t2.status).toBe(TaskStatus.Blocked);

    // Properly transition t1 through to Merged (can't skip straight from Pending)
    taskManager.updateStatus(t1.id, TaskStatus.Active);
    taskManager.updateStatus(t1.id, TaskStatus.Review);
    taskManager.updateStatus(t1.id, TaskStatus.Merged);

    const updated = taskManager.get(t2.id);
    expect(updated.status).toBe(TaskStatus.Pending);
  });

  it("lists tasks by status", () => {
    taskManager.create({ title: "A", description: "a" });
    taskManager.create({ title: "B", description: "b" });
    const t3 = taskManager.create({ title: "C", description: "c" });
    taskManager.updateStatus(t3.id, TaskStatus.Active);

    expect(taskManager.listByStatus(TaskStatus.Pending)).toHaveLength(2);
    expect(taskManager.listByStatus(TaskStatus.Active)).toHaveLength(1);
  });

  it("rejects invalid status transitions", () => {
    const t = taskManager.create({ title: "Task", description: "d" });
    // Can't go from Pending directly to Merged
    expect(() => taskManager.updateStatus(t.id, TaskStatus.Merged)).toThrow();
  });
});
