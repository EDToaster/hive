import * as fs from "node:fs";
import * as path from "node:path";
import type { AgentMessage } from "@mariozechner/pi-agent-core";
import type { RunState, Task, AgentInfo, HiveMessage, MergeQueueEntry } from "./types.js";

export class StateManager {
  private hiveDir: string;
  private state!: RunState;

  constructor(hiveDir: string) {
    this.hiveDir = hiveDir;
  }

  get statePath(): string {
    return path.join(this.hiveDir, "state.json");
  }

  initRun(runId: string, specFile: string): RunState {
    this.state = {
      runId,
      specFile,
      tasks: {},
      agents: {},
      messages: [],
      mergeQueue: [],
      createdAt: Date.now(),
      status: "running",
    };
    fs.mkdirSync(this.hiveDir, { recursive: true });
    this.save();
    return this.state;
  }

  getState(): RunState {
    return this.state;
  }

  save(): void {
    const tmp = this.statePath + ".tmp";
    fs.writeFileSync(tmp, JSON.stringify(this.state, null, 2));
    fs.renameSync(tmp, this.statePath);
  }

  load(): void {
    const data = fs.readFileSync(this.statePath, "utf-8");
    this.state = JSON.parse(data);
  }

  addTask(task: Task): void {
    this.state.tasks[task.id] = task;
    this.save();
  }

  updateTask(taskId: string, updates: Partial<Task>): Task {
    const task = this.state.tasks[taskId];
    if (!task) throw new Error(`Task not found: ${taskId}`);
    Object.assign(task, updates, { updatedAt: Date.now() });
    this.save();
    return task;
  }

  addAgent(agent: AgentInfo): void {
    this.state.agents[agent.id] = agent;
    this.save();
  }

  updateAgent(agentId: string, updates: Partial<AgentInfo>): void {
    const agent = this.state.agents[agentId];
    if (!agent) throw new Error(`Agent not found: ${agentId}`);
    Object.assign(agent, updates);
    this.save();
  }

  addMessage(msg: HiveMessage): void {
    this.state.messages.push(msg);
    this.save();
  }

  addMergeQueueEntry(entry: MergeQueueEntry): void {
    this.state.mergeQueue.push(entry);
    this.save();
  }

  updateMergeQueueEntry(taskId: string, updates: Partial<MergeQueueEntry>): void {
    const entry = this.state.mergeQueue.find((e) => e.taskId === taskId);
    if (!entry) throw new Error(`Merge queue entry not found: ${taskId}`);
    Object.assign(entry, updates);
    this.save();
  }

  // Agent conversation persistence (append-only JSONL)
  private agentDir(agentId: string): string {
    const dir = path.join(this.hiveDir, "agents", agentId);
    fs.mkdirSync(dir, { recursive: true });
    return dir;
  }

  appendAgentMessage(agentId: string, message: AgentMessage): void {
    const filePath = path.join(this.agentDir(agentId), "messages.jsonl");
    fs.appendFileSync(filePath, JSON.stringify(message) + "\n");
  }

  loadAgentMessages(agentId: string): AgentMessage[] {
    const filePath = path.join(this.agentDir(agentId), "messages.jsonl");
    if (!fs.existsSync(filePath)) return [];
    return fs
      .readFileSync(filePath, "utf-8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  }

  saveAgentMeta(agentId: string, meta: Record<string, unknown>): void {
    const filePath = path.join(this.agentDir(agentId), "meta.json");
    fs.writeFileSync(filePath, JSON.stringify(meta, null, 2));
  }

  loadAgentMeta(agentId: string): Record<string, unknown> | null {
    const filePath = path.join(this.agentDir(agentId), "meta.json");
    if (!fs.existsSync(filePath)) return null;
    return JSON.parse(fs.readFileSync(filePath, "utf-8"));
  }
}
