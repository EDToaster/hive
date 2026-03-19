import { Type } from "@sinclair/typebox";
import type { Static } from "@sinclair/typebox";
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { SubmitToQueueSchema } from "./schemas.js";
import type { Hive } from "../hive.js";

const EmptySchema = Type.Object({});

export function submitToQueueTool(agentId: string, hive: Hive): AgentTool<typeof SubmitToQueueSchema> {
  return {
    name: "submit_to_queue",
    label: "Submit to Merge Queue",
    description: "Submit a completed branch to the merge queue.",
    parameters: SubmitToQueueSchema,
    execute: async (_toolCallId, params: Static<typeof SubmitToQueueSchema>) => {
      hive.submitToMergeQueue(params.taskId, params.branch, agentId);
      return {
        content: [
          {
            type: "text",
            text: `Branch "${params.branch}" submitted to merge queue for task ${params.taskId}.`,
          },
        ],
        details: { taskId: params.taskId, branch: params.branch },
      };
    },
  };
}

export function processMergeQueueTool(hive: Hive): AgentTool<typeof EmptySchema> {
  return {
    name: "process_merge_queue",
    label: "Process Merge Queue",
    description: "Process the next entry in the merge queue. Coordinator only.",
    parameters: EmptySchema,
    execute: async () => {
      const result = await hive.processMergeQueue();
      return {
        content: [{ type: "text", text: result }],
        details: {},
      };
    },
  };
}
