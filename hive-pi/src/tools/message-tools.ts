import type { AgentTool } from "@mariozechner/pi-agent-core";
import type { Static } from "@sinclair/typebox";
import { SendMessageSchema, ReadMessagesSchema } from "./schemas.js";
import type { Hive } from "../hive.js";

export function sendMessageTool(agentId: string, hive: Hive): AgentTool<typeof SendMessageSchema> {
  return {
    name: "send_message",
    label: "Send Message",
    description: "Send a message to another agent. Delivered in real-time via steering.",
    parameters: SendMessageSchema,
    execute: async (_toolCallId, params: Static<typeof SendMessageSchema>) => {
      hive.sendMessage(agentId, params.to, params.body);
      return {
        content: [{ type: "text", text: `Message sent to ${params.to}.` }],
        details: { from: agentId, to: params.to },
      };
    },
  };
}

export function readMessagesTool(agentId: string, hive: Hive): AgentTool<typeof ReadMessagesSchema> {
  return {
    name: "read_messages",
    label: "Read Messages",
    description: "Read messages sent to this agent.",
    parameters: ReadMessagesSchema,
    execute: async (_toolCallId, params: Static<typeof ReadMessagesSchema>) => {
      const messages = hive.getMessages(agentId, params.since);
      if (messages.length === 0) {
        return {
          content: [{ type: "text", text: "No new messages." }],
          details: [],
        };
      }
      const summary = messages
        .map((m) => `[${new Date(m.timestamp).toISOString()}] from ${m.from}: ${m.body}`)
        .join("\n");
      return {
        content: [{ type: "text", text: summary }],
        details: messages,
      };
    },
  };
}
