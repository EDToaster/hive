import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { Agent } from "@mariozechner/pi-agent-core";
import type { AgentEvent, AgentMessage } from "@mariozechner/pi-agent-core";
import { getModel } from "@mariozechner/pi-ai";
import { StateManager } from "./state.js";
import { TaskManager } from "./task-manager.js";
import { GitManager } from "./git.js";
import { getToolsForRole } from "./tools/index.js";
import { coordinatorPrompt, leadPrompt, workerPrompt } from "./prompts/index.js";
import { AgentRole, AgentStatus, TaskStatus } from "./types.js";
import type { AgentInfo, HiveMessage } from "./types.js";

/** Character threshold above which context pruning is triggered. */
export const CONTEXT_CHAR_THRESHOLD = 100_000;

/** Number of most-recent messages to preserve when pruning. */
export const CONTEXT_KEEP_RECENT = 20;

/**
 * Compute approximate character count for a single AgentMessage's content.
 * Handles UserMessage (string or block array), AssistantMessage (block array),
 * and ToolResultMessage (block array).
 */
function messageContentLength(msg: AgentMessage): number {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const m = msg as any;
  if (!m.content) return 0;
  if (typeof m.content === "string") return m.content.length;
  if (Array.isArray(m.content)) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return (m.content as any[]).reduce((sum: number, block: any) => {
      if (typeof block.text === "string") return sum + block.text.length;
      // tool call input JSON
      if (block.input && typeof block.input === "object") {
        return sum + JSON.stringify(block.input).length;
      }
      return sum;
    }, 0);
  }
  return 0;
}

/**
 * Context pruning callback for long-running agents.
 *
 * When the total character count of all messages exceeds CONTEXT_CHAR_THRESHOLD,
 * this function truncates older messages while preserving:
 * - The first message (initial task instruction)
 * - The most recent CONTEXT_KEEP_RECENT messages
 *
 * A "[context truncated — N earlier messages removed]" user message is injected
 * between the preserved head and tail so the LLM knows history was dropped.
 *
 * Exported for unit testing.
 */
export async function transformContext(messages: AgentMessage[]): Promise<AgentMessage[]> {
  const totalLength = messages.reduce((sum, msg) => sum + messageContentLength(msg), 0);
  if (totalLength <= CONTEXT_CHAR_THRESHOLD) return messages;

  // Not enough messages to meaningfully truncate — keep all
  if (messages.length <= CONTEXT_KEEP_RECENT + 1) return messages;

  const first = messages[0];
  const recent = messages.slice(-CONTEXT_KEEP_RECENT);
  const removedCount = messages.length - 1 - CONTEXT_KEEP_RECENT;

  const marker: AgentMessage = {
    role: "user" as const,
    content: `[context truncated — ${removedCount} earlier messages removed]`,
    timestamp: Date.now(),
  };

  return [first, marker, ...recent];
}

export interface SpawnAgentParams {
  agentId: string;
  role: AgentRole;
  taskId: string;
  parentAgent?: string;
  model?: string;
}

export interface DiscoveryParams {
  content: string;
  confidence: "low" | "medium" | "high";
  tags: string[];
}

export interface HiveOptions {
  repoRoot: string;
  defaultModel?: string; // "provider/model-id" format
}

/**
 * Core Hive orchestrator class. Manages agent lifecycle, messaging, and merge queue.
 * Implements the Hive interface used by all tool definitions.
 */
export class Hive {
  readonly stateManager: StateManager;
  readonly taskManager: TaskManager;
  readonly gitManager: GitManager;
  private liveAgents: Map<string, Agent> = new Map();
  private repoRoot: string;
  private hiveDir: string;
  private specContent: string = "";
  private defaultModel: string;

  constructor(options: HiveOptions) {
    this.repoRoot = options.repoRoot;
    this.hiveDir = path.join(this.repoRoot, ".hive");
    this.defaultModel = options.defaultModel ?? "anthropic/claude-sonnet-4-6";
    this.stateManager = new StateManager(this.hiveDir);
    this.taskManager = new TaskManager(this.stateManager);
    this.gitManager = new GitManager(this.repoRoot, this.hiveDir);
  }

  /** Initialize a new run from an inline spec string. Returns the run ID. */
  initRun(specContent: string): string {
    const runId = crypto.randomUUID().slice(0, 8);
    this.specContent = specContent;
    this.stateManager.initRun(runId, "inline");
    return runId;
  }

  /** Initialize a new run from a spec file. Returns the run ID. */
  initRunFromFile(specFile: string): string {
    this.specContent = fs.readFileSync(specFile, "utf-8");
    const runId = crypto.randomUUID().slice(0, 8);
    this.stateManager.initRun(runId, specFile);
    return runId;
  }

  /** Spawn a new agent with a pi-mono Agent instance. Creates a worktree for leads/workers. */
  async spawnAgent(params: SpawnAgentParams): Promise<AgentInfo> {
    const state = this.stateManager.getState();
    const runId = state.runId;

    // Parse model string "provider/model-id"
    const modelStr = params.model ?? this.defaultModel;
    const slashIndex = modelStr.indexOf("/");
    const provider = slashIndex >= 0 ? modelStr.slice(0, slashIndex) : "anthropic";
    const modelId = slashIndex >= 0 ? modelStr.slice(slashIndex + 1) : modelStr;

    let worktreePath: string | undefined;
    let branch: string | undefined;

    // Create git worktree for leads and workers (not coordinator)
    if (params.role !== AgentRole.Coordinator) {
      const task = this.taskManager.get(params.taskId);
      // Workers share the lead's worktree — look up parent agent's worktree
      if (params.role === AgentRole.Worker) {
        if (!params.parentAgent) {
          throw new Error(`Worker agent "${params.agentId}" must specify a parentAgent`);
        }
        const parentInfo = state.agents[params.parentAgent];
        if (!parentInfo) {
          throw new Error(
            `Worker agent "${params.agentId}": parent agent "${params.parentAgent}" not found in state`
          );
        }
        if (parentInfo.worktree) {
          worktreePath = parentInfo.worktree;
          branch = parentInfo.branch;
        }
      }
      // Lead gets its own worktree
      if (!worktreePath) {
        const wt = await this.gitManager.createWorktree(params.agentId, runId, {
          sparsePaths: task.domain ? [task.domain] : undefined,
        });
        worktreePath = wt.worktreePath;
        branch = wt.branch;
      }
    }

    // Register agent in state before spawning
    const agentInfo: AgentInfo = {
      id: params.agentId,
      role: params.role,
      status: AgentStatus.Running,
      parentAgent: params.parentAgent,
      taskId: params.taskId,
      model: modelStr,
      worktree: worktreePath,
      branch,
      createdAt: Date.now(),
    };
    this.stateManager.addAgent(agentInfo);

    // Assign task to agent and activate
    if (params.taskId !== "__coordinator__") {
      this.taskManager.assign(params.taskId, params.agentId);
      const task = this.taskManager.get(params.taskId);
      if (task.status === TaskStatus.Pending) {
        this.taskManager.updateStatus(params.taskId, TaskStatus.Active);
      }
    }

    // Build system prompt based on role
    const systemPrompt = this.buildSystemPrompt(
      params.role,
      params.agentId,
      params.taskId,
      worktreePath
    );

    // Build tool set based on role
    const tools = getToolsForRole(params.role, params.agentId, this);

    // Resolve pi-ai model — cast needed since model ID type depends on provider type
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const model = (getModel as any)(provider, modelId);

    // Create pi-mono Agent instance with all refinements:
    // - steeringMode: "all" for responsive inter-agent messaging
    // - transformContext for context window management on long-running agents
    // - beforeToolCall/afterToolCall hooks for observability
    const toolLogPath = path.join(this.hiveDir, "tool-calls.jsonl");
    const agentId = params.agentId;
    const agentRole = params.role;
    const agent = new Agent({
      steeringMode: "all",
      initialState: {
        model,
        systemPrompt,
        tools,
      },
      transformContext,
      beforeToolCall: async (context, _signal) => {
        const entry = {
          timestamp: new Date().toISOString(),
          agentId,
          role: agentRole,
          toolName: context.toolCall.name,
          toolCallId: context.toolCall.id,
          args: context.args,
          phase: "before",
        };
        fs.appendFileSync(toolLogPath, JSON.stringify(entry) + "\n");
        return undefined; // allow all tool calls (observability only)
      },
      afterToolCall: async (context, _signal) => {
        const entry = {
          timestamp: new Date().toISOString(),
          agentId,
          role: agentRole,
          toolName: context.toolCall.name,
          toolCallId: context.toolCall.id,
          isError: context.isError,
          phase: "after",
        };
        fs.appendFileSync(toolLogPath, JSON.stringify(entry) + "\n");
        return undefined; // no result override
      },
    });

    // Subscribe to events for persistence and status updates
    agent.subscribe((event: AgentEvent) => {
      if (event.type === "message_end") {
        this.stateManager.appendAgentMessage(params.agentId, event.message);
      }
      if (event.type === "agent_end") {
        // agent_end has `messages` (plural), not `message`
        this.stateManager.updateAgent(params.agentId, { status: AgentStatus.Done });
      }
    });

    this.liveAgents.set(params.agentId, agent);

    // Save agent metadata for observability / crash recovery
    this.stateManager.saveAgentMeta(params.agentId, {
      role: params.role,
      model: modelStr,
      systemPrompt,
      toolNames: tools.map((t) => t.name),
      worktree: worktreePath,
      branch,
    });

    return agentInfo;
  }

  /** Send a message from one agent to another. Persists to state and delivers via steer(). */
  sendMessage(from: string, to: string, body: string): void {
    const msg: HiveMessage = {
      id: crypto.randomUUID().slice(0, 8),
      from,
      to,
      body,
      timestamp: Date.now(),
    };
    this.stateManager.addMessage(msg);

    // Deliver in real-time via steer if target agent is live
    const targetAgent = this.liveAgents.get(to);
    if (targetAgent) {
      targetAgent.steer({
        role: "user" as const,
        content: `[Message from ${from}]: ${body}`,
        timestamp: Date.now(),
      });
    }
  }

  /** Get messages addressed to an agent, optionally since a timestamp. */
  getMessages(agentId: string, since?: number): HiveMessage[] {
    const allMessages = this.stateManager.getState().messages;
    return allMessages.filter(
      (m) => m.to === agentId && (since === undefined || m.timestamp >= since)
    );
  }

  /** Submit a completed branch to the merge queue. */
  submitToMergeQueue(taskId: string, branch: string, submittedBy: string): void {
    this.stateManager.addMergeQueueEntry({
      taskId,
      branch,
      submittedBy,
      submittedAt: Date.now(),
      status: "pending",
    });
  }

  /** Process the next pending entry in the merge queue. Returns a status message. */
  async processMergeQueue(): Promise<string> {
    const state = this.stateManager.getState();
    const pending = state.mergeQueue.find((e) => e.status === "pending");
    if (!pending) return "No pending entries in merge queue.";

    this.stateManager.updateMergeQueueEntry(pending.taskId, { status: "merging" });
    const mainBranch = await this.gitManager.getMainBranch();

    // Try direct merge first
    let result = await this.gitManager.mergeBranch(pending.branch, mainBranch);

    // If merge fails, try rebase then merge
    if (!result.success) {
      const rebaseResult = await this.gitManager.rebaseBranch(pending.branch, mainBranch);
      if (rebaseResult.success) {
        result = await this.gitManager.mergeBranch(pending.branch, mainBranch);
      }
    }

    if (result.success) {
      this.stateManager.updateMergeQueueEntry(pending.taskId, { status: "merged" });
      this.taskManager.updateStatus(pending.taskId, TaskStatus.Merged);
      return `Merged branch "${pending.branch}" for task ${pending.taskId}.`;
    } else {
      this.stateManager.updateMergeQueueEntry(pending.taskId, { status: "failed" });
      this.taskManager.updateStatus(pending.taskId, TaskStatus.Failed);
      return `Failed to merge "${pending.branch}": ${result.error}`;
    }
  }

  /** Record a discovery to the run's discoveries log. */
  recordDiscovery(agentId: string, discovery: DiscoveryParams): void {
    const entry = {
      id: crypto.randomUUID().slice(0, 8),
      agentId,
      ...discovery,
      timestamp: Date.now(),
    };
    const filePath = path.join(this.hiveDir, "discoveries.jsonl");
    fs.appendFileSync(filePath, JSON.stringify(entry) + "\n");
  }

  /** Get a live Agent instance by ID (for direct control). */
  getAgent(agentId: string): Agent | undefined {
    return this.liveAgents.get(agentId);
  }

  /** Stop all agents and clean up worktrees. */
  async stop(): Promise<void> {
    for (const agent of this.liveAgents.values()) {
      agent.abort();
    }
    this.liveAgents.clear();
    await this.gitManager.cleanupAllWorktrees();
    const state = this.stateManager.getState();
    state.status = "stopped";
    this.stateManager.save();
  }

  private buildSystemPrompt(
    role: AgentRole,
    agentId: string,
    taskId: string,
    worktreePath?: string
  ): string {
    const state = this.stateManager.getState();
    switch (role) {
      case AgentRole.Coordinator:
        return coordinatorPrompt(state.runId, this.specContent, this.taskManager.listAll());
      case AgentRole.Lead: {
        const task = this.taskManager.get(taskId);
        return leadPrompt(agentId, task, worktreePath ?? ".");
      }
      case AgentRole.Worker: {
        const task = this.taskManager.get(taskId);
        return workerPrompt(agentId, task, worktreePath ?? ".");
      }
    }
  }
}
