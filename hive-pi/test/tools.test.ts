import { describe, it, expect } from "vitest";
import { getToolsForRole } from "../src/tools/index.js";
import { AgentRole } from "../src/types.js";
import type { Hive } from "../src/hive.js";

describe("getToolsForRole", () => {
  // Use a mock hive context — we just need to test tool set composition
  const mockHive = {} as Hive;

  it("coordinator gets spawn, task, message, and merge tools", () => {
    const tools = getToolsForRole(AgentRole.Coordinator, "coord-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("create_task");
    expect(names).toContain("update_task");
    expect(names).toContain("list_tasks");
    expect(names).toContain("spawn_agent");
    expect(names).toContain("send_message");
    expect(names).toContain("read_messages");
    expect(names).toContain("process_merge_queue");
    expect(names).toContain("discover");
  });

  it("lead gets task, spawn, message, submit tools but not process_merge_queue", () => {
    const tools = getToolsForRole(AgentRole.Lead, "lead-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("create_task");
    expect(names).toContain("spawn_agent");
    expect(names).toContain("send_message");
    expect(names).toContain("read_messages");
    expect(names).toContain("submit_to_queue");
    expect(names).toContain("discover");
    expect(names).not.toContain("process_merge_queue");
  });

  it("worker gets update_task, list_tasks, send_message, read_messages, discover — no spawn/create/submit", () => {
    const tools = getToolsForRole(AgentRole.Worker, "worker-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("update_task");
    expect(names).toContain("list_tasks");
    expect(names).toContain("send_message");
    expect(names).toContain("read_messages");
    expect(names).toContain("discover");
    expect(names).not.toContain("create_task");
    expect(names).not.toContain("spawn_agent");
    expect(names).not.toContain("submit_to_queue");
  });

  it("all tools have required AgentTool fields", () => {
    for (const role of [AgentRole.Coordinator, AgentRole.Lead, AgentRole.Worker]) {
      const tools = getToolsForRole(role, "test-agent", mockHive);
      for (const tool of tools) {
        expect(typeof tool.name).toBe("string");
        expect(typeof tool.label).toBe("string");
        expect(typeof tool.description).toBe("string");
        expect(tool.parameters).toBeDefined();
        expect(typeof tool.execute).toBe("function");
      }
    }
  });

  it("coordinator does not have submit_to_queue", () => {
    const tools = getToolsForRole(AgentRole.Coordinator, "coord-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).not.toContain("submit_to_queue");
  });
});
