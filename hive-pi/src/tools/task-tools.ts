import type { AgentTool } from "@mariozechner/pi-agent-core";
import type { Static } from "@sinclair/typebox";
import { CreateTaskSchema, UpdateTaskSchema, ListTasksSchema } from "./schemas.js";
import { TaskStatus } from "../types.js";
import type { Hive } from "../hive.js";

export function createTaskTool(agentId: string, hive: Hive): AgentTool<typeof CreateTaskSchema> {
  return {
    name: "create_task",
    label: "Create Task",
    description: "Create a new task with title, description, and optional dependencies.",
    parameters: CreateTaskSchema,
    execute: async (_toolCallId, params: Static<typeof CreateTaskSchema>) => {
      const task = hive.taskManager.create({
        title: params.title,
        description: params.description,
        blockedBy: params.blockedBy,
        domain: params.domain,
      });
      const text = `Created task ${task.id}: "${task.title}" [${task.status}]`;
      return { content: [{ type: "text", text }], details: task };
    },
  };
}

export function updateTaskTool(agentId: string, hive: Hive): AgentTool<typeof UpdateTaskSchema> {
  return {
    name: "update_task",
    label: "Update Task",
    description: "Update a task's status or append notes.",
    parameters: UpdateTaskSchema,
    execute: async (_toolCallId, params: Static<typeof UpdateTaskSchema>) => {
      let task = hive.taskManager.get(params.taskId);
      if (params.status) {
        task = hive.taskManager.updateStatus(params.taskId, params.status as TaskStatus);
      }
      if (params.notes) {
        task = hive.stateManager.updateTask(params.taskId, {
          description: task.description + "\n\n" + params.notes,
        });
      }
      const text = `Updated task ${task.id}: [${task.status}]`;
      return { content: [{ type: "text", text }], details: task };
    },
  };
}

export function listTasksTool(agentId: string, hive: Hive): AgentTool<typeof ListTasksSchema> {
  return {
    name: "list_tasks",
    label: "List Tasks",
    description: "List tasks, optionally filtered by status or assignee.",
    parameters: ListTasksSchema,
    execute: async (_toolCallId, params: Static<typeof ListTasksSchema>) => {
      let tasks = hive.taskManager.listAll();
      if (params.status) {
        tasks = tasks.filter((t) => t.status === params.status);
      }
      if (params.assignee) {
        tasks = tasks.filter((t) => t.assignedTo === params.assignee);
      }
      const summary = tasks
        .map(
          (t) =>
            `${t.id}: "${t.title}" [${t.status}] assigned=${t.assignedTo ?? "none"}`
        )
        .join("\n");
      return {
        content: [{ type: "text", text: summary || "No tasks found." }],
        details: tasks,
      };
    },
  };
}
