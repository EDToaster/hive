import type { AgentTool } from "@mariozechner/pi-agent-core";
import type { Static } from "@sinclair/typebox";
import { DiscoverSchema } from "./schemas.js";
import type { Hive } from "../hive.js";

export function discoverTool(agentId: string, hive: Hive): AgentTool<typeof DiscoverSchema> {
  return {
    name: "discover",
    label: "Record Discovery",
    description:
      "Record a significant discovery or insight to the shared knowledge base.",
    parameters: DiscoverSchema,
    execute: async (_toolCallId, params: Static<typeof DiscoverSchema>) => {
      hive.recordDiscovery(agentId, {
        content: params.content,
        confidence: params.confidence ?? "medium",
        tags: params.tags ?? [],
      });
      return {
        content: [{ type: "text", text: "Discovery recorded." }],
        details: { agentId, content: params.content },
      };
    },
  };
}
