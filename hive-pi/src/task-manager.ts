import * as crypto from "node:crypto";
import type { StateManager } from "./state.js";
import type { Task } from "./types.js";
import { TaskStatus, isTerminalTaskStatus, isSuccessTaskStatus } from "./types.js";

export interface CreateTaskParams {
  title: string;
  description: string;
  parentTask?: string;
  blockedBy?: string[];
  domain?: string;
}

// Valid status transitions
const VALID_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  [TaskStatus.Pending]: [TaskStatus.Active, TaskStatus.Cancelled, TaskStatus.Blocked],
  [TaskStatus.Blocked]: [TaskStatus.Pending, TaskStatus.Cancelled],
  [TaskStatus.Active]: [TaskStatus.Review, TaskStatus.Failed, TaskStatus.Cancelled],
  [TaskStatus.Review]: [TaskStatus.Merged, TaskStatus.Failed, TaskStatus.Active],
  [TaskStatus.Merged]: [],
  [TaskStatus.Failed]: [TaskStatus.Pending], // retry
  [TaskStatus.Cancelled]: [],
};

export class TaskManager {
  constructor(private state: StateManager) {}

  create(params: CreateTaskParams): Task {
    const hasBlockers = params.blockedBy && params.blockedBy.length > 0;
    const task: Task = {
      id: `t-${crypto.randomUUID().slice(0, 8)}`,
      title: params.title,
      description: params.description,
      status: hasBlockers ? TaskStatus.Blocked : TaskStatus.Pending,
      parentTask: params.parentTask,
      blockedBy: params.blockedBy ?? [],
      domain: params.domain,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    };
    this.state.addTask(task);
    return task;
  }

  get(taskId: string): Task {
    const task = this.state.getState().tasks[taskId];
    if (!task) throw new Error(`Task not found: ${taskId}`);
    return task;
  }

  updateStatus(taskId: string, newStatus: TaskStatus): Task {
    const task = this.get(taskId);
    const valid = VALID_TRANSITIONS[task.status];
    if (!valid.includes(newStatus)) {
      throw new Error(
        `Invalid transition: ${task.status} → ${newStatus} for task ${taskId}`
      );
    }

    const updated = this.state.updateTask(taskId, { status: newStatus });

    // If this task just reached a terminal success state, unblock dependents
    if (isTerminalTaskStatus(newStatus) && isSuccessTaskStatus(newStatus)) {
      this.unblockDependents(taskId);
    }

    return updated;
  }

  assign(taskId: string, agentId: string): Task {
    return this.state.updateTask(taskId, { assignedTo: agentId });
  }

  listByStatus(status: TaskStatus): Task[] {
    return Object.values(this.state.getState().tasks).filter(
      (t) => t.status === status
    );
  }

  listAll(): Task[] {
    return Object.values(this.state.getState().tasks);
  }

  private unblockDependents(completedTaskId: string): void {
    const allTasks = Object.values(this.state.getState().tasks);
    for (const task of allTasks) {
      if (task.status !== TaskStatus.Blocked) continue;
      if (!task.blockedBy.includes(completedTaskId)) continue;

      const remaining = task.blockedBy.filter((id) => {
        const dep = this.state.getState().tasks[id];
        return dep && !isTerminalTaskStatus(dep.status);
      });

      if (remaining.length === 0) {
        this.state.updateTask(task.id, {
          status: TaskStatus.Pending,
          blockedBy: [],
        });
      } else {
        this.state.updateTask(task.id, { blockedBy: remaining });
      }
    }
  }
}
