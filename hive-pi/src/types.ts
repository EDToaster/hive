// Task statuses — simplified from Rust Hive's 10 to 7 for MVP
export enum TaskStatus {
  Pending = "pending",
  Active = "active",
  Blocked = "blocked",
  Review = "review",
  Merged = "merged",
  Failed = "failed",
  Cancelled = "cancelled",
}

export function isTerminalTaskStatus(s: TaskStatus): boolean {
  return s === TaskStatus.Merged || s === TaskStatus.Failed || s === TaskStatus.Cancelled;
}

export function isSuccessTaskStatus(s: TaskStatus): boolean {
  return s === TaskStatus.Merged || s === TaskStatus.Cancelled;
}

// Agent roles — 3 for MVP (coordinator, lead, worker)
export enum AgentRole {
  Coordinator = "coordinator",
  Lead = "lead",
  Worker = "worker",
}

export enum AgentStatus {
  Running = "running",
  Idle = "idle",
  Done = "done",
  Failed = "failed",
}

export interface Task {
  id: string;
  title: string;
  description: string;
  status: TaskStatus;
  assignedTo?: string; // agent ID
  parentTask?: string; // task ID
  blockedBy: string[]; // task IDs
  domain?: string; // filesystem path for sparse checkout
  branch?: string; // git branch name
  createdAt: number; // unix ms
  updatedAt: number; // unix ms
}

export interface AgentInfo {
  id: string;
  role: AgentRole;
  status: AgentStatus;
  parentAgent?: string; // agent ID
  taskId: string; // bound task
  model: string; // e.g. "anthropic/claude-sonnet-4-6"
  worktree?: string; // path to worktree
  branch?: string; // git branch
  createdAt: number;
}

export interface HiveMessage {
  id: string;
  from: string; // agent ID
  to: string; // agent ID
  body: string;
  timestamp: number;
}

export interface MergeQueueEntry {
  taskId: string;
  branch: string;
  submittedBy: string; // agent ID
  submittedAt: number;
  status: "pending" | "merging" | "merged" | "failed";
}

export interface RunState {
  runId: string;
  specFile: string;
  tasks: Record<string, Task>;
  agents: Record<string, AgentInfo>;
  messages: HiveMessage[];
  mergeQueue: MergeQueueEntry[];
  createdAt: number;
  status: "running" | "stopped" | "completed";
}
