import type { AgentTool } from "@mariozechner/pi-agent-core";
import { AgentRole } from "../types.js";
import type { Hive } from "../hive.js";
import { createTaskTool, updateTaskTool, listTasksTool } from "./task-tools.js";
import { sendMessageTool, readMessagesTool } from "./message-tools.js";
import { spawnAgentTool } from "./spawn-tools.js";
import { submitToQueueTool, processMergeQueueTool } from "./merge-tools.js";
import { discoverTool } from "./discover-tool.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyTool = AgentTool<any>;

export function getToolsForRole(
  role: AgentRole,
  agentId: string,
  hive: Hive
): AgentTool[] {
  const tools: AnyTool[] = [];

  switch (role) {
    case AgentRole.Coordinator:
      tools.push(createTaskTool(agentId, hive));
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(spawnAgentTool(agentId, role, hive));
      tools.push(sendMessageTool(agentId, hive));
      tools.push(readMessagesTool(agentId, hive));
      tools.push(processMergeQueueTool(hive));
      tools.push(discoverTool(agentId, hive));
      break;

    case AgentRole.Lead:
      tools.push(createTaskTool(agentId, hive));
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(spawnAgentTool(agentId, role, hive));
      tools.push(sendMessageTool(agentId, hive));
      tools.push(readMessagesTool(agentId, hive));
      tools.push(submitToQueueTool(agentId, hive));
      tools.push(discoverTool(agentId, hive));
      break;

    case AgentRole.Worker:
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(sendMessageTool(agentId, hive));
      tools.push(readMessagesTool(agentId, hive));
      tools.push(discoverTool(agentId, hive));
      break;
  }

  return tools;
}

export {
  createTaskTool,
  updateTaskTool,
  listTasksTool,
  sendMessageTool,
  readMessagesTool,
  spawnAgentTool,
  submitToQueueTool,
  processMergeQueueTool,
  discoverTool,
};
