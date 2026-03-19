import type { AgentTool } from "@mariozechner/pi-agent-core";
import type { Static } from "@sinclair/typebox";
import { SpawnAgentSchema } from "./schemas.js";
import { AgentRole } from "../types.js";
import type { Hive } from "../hive.js";

export function spawnAgentTool(
  agentId: string,
  callerRole: AgentRole,
  hive: Hive
): AgentTool<typeof SpawnAgentSchema> {
  return {
    name: "spawn_agent",
    label: "Spawn Agent",
    description: "Spawn a new agent (lead or worker) bound to a task.",
    parameters: SpawnAgentSchema,
    execute: async (_toolCallId, params: Static<typeof SpawnAgentSchema>) => {
      // Permission check
      const role = params.role as AgentRole;
      if (callerRole === AgentRole.Coordinator && role !== AgentRole.Lead) {
        return {
          content: [{ type: "text", text: "Error: Coordinator can only spawn leads." }],
          details: {},
        };
      }
      if (callerRole === AgentRole.Lead && role !== AgentRole.Worker) {
        return {
          content: [{ type: "text", text: "Error: Lead can only spawn workers." }],
          details: {},
        };
      }

      const agent = await hive.spawnAgent({
        agentId: params.agentId,
        role,
        taskId: params.taskId,
        parentAgent: agentId,
        model: params.model,
      });

      return {
        content: [
          {
            type: "text",
            text: `Spawned ${role} agent "${agent.id}" for task ${params.taskId}.`,
          },
        ],
        details: agent,
      };
    },
  };
}
