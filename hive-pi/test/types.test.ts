import { describe, it, expect } from "vitest";
import {
  type Task,
  type AgentInfo,
  type HiveMessage,
  type RunState,
  TaskStatus,
  AgentRole,
  AgentStatus,
  isTerminalTaskStatus,
  isSuccessTaskStatus,
} from "../src/types.js";

describe("TaskStatus", () => {
  it("terminal statuses are correct", () => {
    expect(isTerminalTaskStatus(TaskStatus.Merged)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Failed)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Cancelled)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Active)).toBe(false);
    expect(isTerminalTaskStatus(TaskStatus.Pending)).toBe(false);
  });

  it("success statuses are correct", () => {
    expect(isSuccessTaskStatus(TaskStatus.Merged)).toBe(true);
    expect(isSuccessTaskStatus(TaskStatus.Cancelled)).toBe(true);
    expect(isSuccessTaskStatus(TaskStatus.Failed)).toBe(false);
  });
});

describe("AgentRole", () => {
  it("has three roles for MVP", () => {
    expect(Object.values(AgentRole)).toEqual(["coordinator", "lead", "worker"]);
  });
});
