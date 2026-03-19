import { Type } from "@sinclair/typebox";

export const CreateTaskSchema = Type.Object({
  title: Type.String({ description: "Short title for the task" }),
  description: Type.String({ description: "Detailed description of the work" }),
  blockedBy: Type.Optional(
    Type.Array(Type.String(), { description: "Task IDs this is blocked by" })
  ),
  domain: Type.Optional(
    Type.String({ description: "Filesystem path for sparse checkout scope" })
  ),
});

export const UpdateTaskSchema = Type.Object({
  taskId: Type.String({ description: "Task ID to update" }),
  status: Type.Optional(
    Type.String({ description: "New status: active, review, failed, cancelled" })
  ),
  notes: Type.Optional(
    Type.String({ description: "Notes to append to task description" })
  ),
});

export const ListTasksSchema = Type.Object({
  status: Type.Optional(Type.String({ description: "Filter by status" })),
  assignee: Type.Optional(
    Type.String({ description: "Filter by assigned agent" })
  ),
});

export const SpawnAgentSchema = Type.Object({
  agentId: Type.String({
    description: "Unique agent ID (e.g., 'lead-backend', 'worker-1')",
  }),
  role: Type.String({ description: "Role: 'lead' or 'worker'" }),
  taskId: Type.String({ description: "Task ID to bind this agent to" }),
  model: Type.Optional(
    Type.String({
      description: "Model override (e.g., 'anthropic/claude-sonnet-4-6')",
    })
  ),
});

export const SendMessageSchema = Type.Object({
  to: Type.String({ description: "Recipient agent ID" }),
  body: Type.String({ description: "Message body" }),
});

export const ReadMessagesSchema = Type.Object({
  since: Type.Optional(
    Type.Number({ description: "Unix ms timestamp — only return messages after this" })
  ),
});

export const SubmitToQueueSchema = Type.Object({
  taskId: Type.String({ description: "Task ID of the completed work" }),
  branch: Type.String({ description: "Branch name to merge" }),
});

export const DiscoverSchema = Type.Object({
  content: Type.String({ description: "The discovery content" }),
  confidence: Type.Optional(
    Type.Union(
      [
        Type.Literal("low"),
        Type.Literal("medium"),
        Type.Literal("high"),
      ],
      { description: "Confidence level: low, medium, or high" }
    )
  ),
  tags: Type.Optional(
    Type.Array(Type.String(), { description: "Categorization tags" })
  ),
});
