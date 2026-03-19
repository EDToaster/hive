# Hive-Pi Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a minimal viable swarm orchestrator in TypeScript using pi-mono's Agent class for in-process LLM agents with git worktree isolation.

**Architecture:** Single Node.js process creates pi-mono `Agent` instances per role (coordinator/lead/worker). Agents communicate via `agent.steer()` push messaging. State lives in-memory with `.hive/` disk persistence for crash recovery. Git worktrees with sparse checkout provide filesystem isolation.

**Tech Stack:** TypeScript, `@mariozechner/pi-agent-core`, `@mariozechner/pi-ai`, `@sinclair/typebox`, `commander`, `vitest`

**Reference design:** `docs/plans/2026-03-19-hive-pi-rewrite-design.md`

---

### Task 1: Project Scaffolding

**Files:**
- Create: `hive-pi/package.json`
- Create: `hive-pi/tsconfig.json`
- Create: `hive-pi/vitest.config.ts`
- Create: `hive-pi/src/index.ts`

**Step 1: Create package.json**

```json
{
  "name": "@hive/cli",
  "version": "0.1.0",
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "bin": {
    "hive-pi": "./dist/main.js"
  },
  "scripts": {
    "build": "tsc",
    "dev": "tsc --watch",
    "test": "vitest --run",
    "test:watch": "vitest",
    "check": "tsc --noEmit"
  },
  "dependencies": {
    "@mariozechner/pi-ai": "^0.60.0",
    "@mariozechner/pi-agent-core": "^0.60.0",
    "@sinclair/typebox": "^0.34.0",
    "commander": "^13.0.0"
  },
  "devDependencies": {
    "typescript": "^5.8.0",
    "vitest": "^3.0.0"
  }
}
```

**Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "outDir": "./dist",
    "rootDir": "./src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "declaration": true,
    "sourceMap": true
  },
  "include": ["src"],
  "exclude": ["node_modules", "dist", "test"]
}
```

**Step 3: Create vitest.config.ts**

```typescript
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    globals: true,
    include: ["test/**/*.test.ts"],
  },
});
```

**Step 4: Create src/index.ts**

```typescript
export { Hive } from "./hive.js";
export type * from "./types.js";
```

**Step 5: Install dependencies**

Run: `cd hive-pi && npm install`
Expected: `node_modules/` created, lock file generated

**Step 6: Verify TypeScript compiles (will fail — no hive.ts/types.ts yet, that's fine)**

Run: `cd hive-pi && npx tsc --noEmit`
Expected: Errors about missing modules (expected at this stage)

**Step 7: Commit**

```bash
git add hive-pi/
git commit -m "scaffold: hive-pi project with pi-mono dependencies"
```

---

### Task 2: Core Types

**Files:**
- Create: `hive-pi/src/types.ts`
- Create: `hive-pi/test/types.test.ts`

**Step 1: Write the types test**

```typescript
import { describe, it, expect } from "vitest";
import {
  type Task,
  type AgentInfo,
  type HiveMessage,
  type RunState,
  TaskStatus,
  AgentRole,
  AgentStatus,
  isTerminalTaskStatus,
  isSuccessTaskStatus,
} from "../src/types.js";

describe("TaskStatus", () => {
  it("terminal statuses are correct", () => {
    expect(isTerminalTaskStatus(TaskStatus.Merged)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Failed)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Cancelled)).toBe(true);
    expect(isTerminalTaskStatus(TaskStatus.Active)).toBe(false);
    expect(isTerminalTaskStatus(TaskStatus.Pending)).toBe(false);
  });

  it("success statuses are correct", () => {
    expect(isSuccessTaskStatus(TaskStatus.Merged)).toBe(true);
    expect(isSuccessTaskStatus(TaskStatus.Cancelled)).toBe(true);
    expect(isSuccessTaskStatus(TaskStatus.Failed)).toBe(false);
  });
});

describe("AgentRole", () => {
  it("has three roles for MVP", () => {
    expect(Object.values(AgentRole)).toEqual(["coordinator", "lead", "worker"]);
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/types.test.ts`
Expected: FAIL — module not found

**Step 3: Write types.ts**

```typescript
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
```

**Step 4: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/types.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add hive-pi/src/types.ts hive-pi/test/types.test.ts
git commit -m "feat: core types for tasks, agents, messages, and run state"
```

---

### Task 3: State Persistence

**Files:**
- Create: `hive-pi/src/state.ts`
- Create: `hive-pi/test/state.test.ts`

**Step 1: Write the state persistence test**

```typescript
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { StateManager } from "../src/state.js";
import { TaskStatus, AgentRole, AgentStatus } from "../src/types.js";
import type { RunState, Task } from "../src/types.js";

describe("StateManager", () => {
  let tmpDir: string;
  let stateManager: StateManager;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    stateManager = new StateManager(tmpDir);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("initializes a new run", () => {
    const state = stateManager.initRun("test-run", "spec.md");
    expect(state.runId).toBe("test-run");
    expect(state.specFile).toBe("spec.md");
    expect(state.status).toBe("running");
    expect(Object.keys(state.tasks)).toHaveLength(0);
  });

  it("persists and loads state", () => {
    stateManager.initRun("test-run", "spec.md");
    stateManager.addTask({
      id: "t1",
      title: "Test task",
      description: "Do something",
      status: TaskStatus.Pending,
      blockedBy: [],
      createdAt: Date.now(),
      updatedAt: Date.now(),
    });
    stateManager.save();

    // Create a new StateManager pointing at same directory
    const loaded = new StateManager(tmpDir);
    loaded.load();
    const state = loaded.getState();
    expect(state.tasks["t1"].title).toBe("Test task");
  });

  it("appends agent messages to JSONL", () => {
    stateManager.initRun("test-run", "spec.md");
    stateManager.appendAgentMessage("agent-1", {
      role: "user",
      content: "Hello",
      timestamp: Date.now(),
    });
    stateManager.appendAgentMessage("agent-1", {
      role: "assistant",
      content: [{ type: "text", text: "Hi" }],
      api: "anthropic-messages",
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      stopReason: "stop",
      timestamp: Date.now(),
    });

    const messages = stateManager.loadAgentMessages("agent-1");
    expect(messages).toHaveLength(2);
    expect(messages[0].role).toBe("user");
    expect(messages[1].role).toBe("assistant");
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/state.test.ts`
Expected: FAIL — module not found

**Step 3: Write state.ts**

```typescript
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
```

**Step 4: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/state.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add hive-pi/src/state.ts hive-pi/test/state.test.ts
git commit -m "feat: state persistence with atomic writes and JSONL message log"
```

---

### Task 4: Task Manager

**Files:**
- Create: `hive-pi/src/task-manager.ts`
- Create: `hive-pi/test/task-manager.test.ts`

**Step 1: Write the task manager test**

```typescript
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { TaskManager } from "../src/task-manager.js";
import { StateManager } from "../src/state.js";
import { TaskStatus } from "../src/types.js";

describe("TaskManager", () => {
  let tmpDir: string;
  let stateManager: StateManager;
  let taskManager: TaskManager;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    stateManager = new StateManager(tmpDir);
    stateManager.initRun("test-run", "spec.md");
    taskManager = new TaskManager(stateManager);
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("creates a task", () => {
    const task = taskManager.create({
      title: "Build feature",
      description: "Implement X",
    });
    expect(task.id).toBeDefined();
    expect(task.status).toBe(TaskStatus.Pending);
    expect(task.title).toBe("Build feature");
  });

  it("creates a task with blocked_by", () => {
    const t1 = taskManager.create({ title: "First", description: "Do first" });
    const t2 = taskManager.create({
      title: "Second",
      description: "Do second",
      blockedBy: [t1.id],
    });
    expect(t2.status).toBe(TaskStatus.Blocked);
    expect(t2.blockedBy).toEqual([t1.id]);
  });

  it("unblocks tasks when dependencies complete", () => {
    const t1 = taskManager.create({ title: "First", description: "Do first" });
    const t2 = taskManager.create({
      title: "Second",
      description: "Do second",
      blockedBy: [t1.id],
    });
    expect(t2.status).toBe(TaskStatus.Blocked);

    taskManager.updateStatus(t1.id, TaskStatus.Merged);
    const updated = taskManager.get(t2.id);
    expect(updated.status).toBe(TaskStatus.Pending);
  });

  it("lists tasks by status", () => {
    taskManager.create({ title: "A", description: "a" });
    taskManager.create({ title: "B", description: "b" });
    const t3 = taskManager.create({ title: "C", description: "c" });
    taskManager.updateStatus(t3.id, TaskStatus.Active);

    expect(taskManager.listByStatus(TaskStatus.Pending)).toHaveLength(2);
    expect(taskManager.listByStatus(TaskStatus.Active)).toHaveLength(1);
  });

  it("rejects invalid status transitions", () => {
    const t = taskManager.create({ title: "Task", description: "d" });
    // Can't go from Pending directly to Merged
    expect(() => taskManager.updateStatus(t.id, TaskStatus.Merged)).toThrow();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/task-manager.test.ts`
Expected: FAIL — module not found

**Step 3: Write task-manager.ts**

```typescript
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
```

**Step 4: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/task-manager.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add hive-pi/src/task-manager.ts hive-pi/test/task-manager.test.ts
git commit -m "feat: task manager with dependency tracking and status transitions"
```

---

### Task 5: Git Operations

**Files:**
- Create: `hive-pi/src/git.ts`
- Create: `hive-pi/test/git.test.ts`

**Step 1: Write the git operations test**

Note: These tests create real git repos in temp dirs. They test actual git CLI behavior.

```typescript
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";
import { GitManager } from "../src/git.js";

describe("GitManager", () => {
  let repoDir: string;
  let hiveDir: string;
  let git: GitManager;

  beforeEach(() => {
    repoDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-git-test-"));
    hiveDir = path.join(repoDir, ".hive");
    fs.mkdirSync(hiveDir, { recursive: true });

    // Initialize a git repo with an initial commit
    execSync("git init", { cwd: repoDir });
    execSync("git config user.email 'test@test.com'", { cwd: repoDir });
    execSync("git config user.name 'Test'", { cwd: repoDir });
    fs.writeFileSync(path.join(repoDir, "README.md"), "# Test");
    fs.mkdirSync(path.join(repoDir, "src"), { recursive: true });
    fs.writeFileSync(path.join(repoDir, "src", "main.ts"), "console.log('hello');");
    execSync("git add -A && git commit -m 'init'", { cwd: repoDir });

    git = new GitManager(repoDir, hiveDir);
  });

  afterEach(() => {
    // Clean up worktrees before removing dir
    try {
      git.cleanupAllWorktrees();
    } catch {}
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("creates a worktree with a branch", () => {
    const result = git.createWorktree("agent-1", "run-1");
    expect(fs.existsSync(result.worktreePath)).toBe(true);
    expect(result.branch).toBe("hive/run-1/agent-1");
  });

  it("creates a worktree with sparse checkout", () => {
    const result = git.createWorktree("agent-1", "run-1", { sparsePaths: ["src"] });
    expect(fs.existsSync(result.worktreePath)).toBe(true);
    // src/main.ts should exist in worktree
    expect(fs.existsSync(path.join(result.worktreePath, "src", "main.ts"))).toBe(true);
  });

  it("removes a worktree", () => {
    const result = git.createWorktree("agent-1", "run-1");
    git.removeWorktree("agent-1");
    expect(fs.existsSync(result.worktreePath)).toBe(false);
  });

  it("merges a branch into main", () => {
    const result = git.createWorktree("agent-1", "run-1");
    // Make a change in the worktree
    fs.writeFileSync(path.join(result.worktreePath, "new-file.txt"), "content");
    execSync("git add -A && git commit -m 'add file'", { cwd: result.worktreePath });

    const mainBranch = git.getMainBranch();
    const mergeResult = git.mergeBranch(result.branch, mainBranch);
    expect(mergeResult.success).toBe(true);
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/git.test.ts`
Expected: FAIL — module not found

**Step 3: Write git.ts**

```typescript
import { execSync } from "node:child_process";
import * as path from "node:path";
import * as fs from "node:fs";

export interface WorktreeResult {
  worktreePath: string;
  branch: string;
}

export interface MergeResult {
  success: boolean;
  error?: string;
}

export interface WorktreeOptions {
  sparsePaths?: string[];
  baseBranch?: string; // branch to create worktree from (default: HEAD)
}

export class GitManager {
  constructor(
    private repoRoot: string,
    private hiveDir: string
  ) {}

  private exec(cmd: string, cwd?: string): string {
    return execSync(cmd, {
      cwd: cwd ?? this.repoRoot,
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  }

  getMainBranch(): string {
    try {
      const ref = this.exec("git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null");
      return ref.replace("refs/remotes/origin/", "");
    } catch {
      // Fallback: check if main or master exists
      try {
        this.exec("git rev-parse --verify main");
        return "main";
      } catch {
        return "master";
      }
    }
  }

  createWorktree(agentId: string, runId: string, options?: WorktreeOptions): WorktreeResult {
    const branch = `hive/${runId}/${agentId}`;
    const worktreePath = path.join(this.hiveDir, "worktrees", agentId);

    const base = options?.baseBranch ?? "HEAD";
    this.exec(`git worktree add "${worktreePath}" -b "${branch}" ${base}`);

    if (options?.sparsePaths && options.sparsePaths.length > 0) {
      this.exec("git sparse-checkout init --cone", worktreePath);
      this.exec(
        `git sparse-checkout set ${options.sparsePaths.join(" ")}`,
        worktreePath
      );
    }

    return { worktreePath, branch };
  }

  removeWorktree(agentId: string): void {
    const worktreePath = path.join(this.hiveDir, "worktrees", agentId);
    if (fs.existsSync(worktreePath)) {
      this.exec(`git worktree remove "${worktreePath}" --force`);
    }
    // Clean up the branch too
    const branches = this.exec("git branch").split("\n").map((b) => b.trim().replace("* ", ""));
    for (const branch of branches) {
      if (branch.includes(`/${agentId}`)) {
        try {
          this.exec(`git branch -D "${branch}"`);
        } catch {}
      }
    }
  }

  cleanupAllWorktrees(): void {
    const worktreeDir = path.join(this.hiveDir, "worktrees");
    if (!fs.existsSync(worktreeDir)) return;

    for (const entry of fs.readdirSync(worktreeDir)) {
      this.removeWorktree(entry);
    }
  }

  mergeBranch(branch: string, targetBranch: string): MergeResult {
    try {
      // Save current branch
      const currentBranch = this.exec("git rev-parse --abbrev-ref HEAD");

      // Checkout target, merge, return to original
      this.exec(`git checkout "${targetBranch}"`);
      try {
        this.exec(`git merge "${branch}" --no-ff -m "merge: ${branch}"`);
        return { success: true };
      } catch (err: any) {
        // Abort failed merge
        try { this.exec("git merge --abort"); } catch {}
        return { success: false, error: err.message };
      } finally {
        // Return to original branch
        if (currentBranch !== targetBranch) {
          try { this.exec(`git checkout "${currentBranch}"`); } catch {}
        }
      }
    } catch (err: any) {
      return { success: false, error: err.message };
    }
  }

  rebaseBranch(branch: string, ontoBranch: string): MergeResult {
    try {
      this.exec(`git checkout "${branch}"`);
      try {
        this.exec(`git rebase "${ontoBranch}"`);
        return { success: true };
      } catch (err: any) {
        try { this.exec("git rebase --abort"); } catch {}
        return { success: false, error: err.message };
      } finally {
        try { this.exec(`git checkout "${ontoBranch}"`); } catch {}
      }
    } catch (err: any) {
      return { success: false, error: err.message };
    }
  }

  /**
   * Validate that a domain path exists in the git tree.
   */
  validateDomainPath(domainPath: string): boolean {
    try {
      this.exec(`git ls-tree HEAD "${domainPath}"`);
      return true;
    } catch {
      return false;
    }
  }
}
```

**Step 4: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/git.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add hive-pi/src/git.ts hive-pi/test/git.test.ts
git commit -m "feat: git manager with worktree, sparse checkout, and merge operations"
```

---

### Task 6: Hive Tools (AgentTool Definitions)

**Files:**
- Create: `hive-pi/src/tools/schemas.ts`
- Create: `hive-pi/src/tools/task-tools.ts`
- Create: `hive-pi/src/tools/message-tools.ts`
- Create: `hive-pi/src/tools/spawn-tools.ts`
- Create: `hive-pi/src/tools/merge-tools.ts`
- Create: `hive-pi/src/tools/index.ts`
- Create: `hive-pi/test/tools.test.ts`

**Step 1: Write the tools test**

```typescript
import { describe, it, expect } from "vitest";
import { getToolsForRole } from "../src/tools/index.js";
import { AgentRole } from "../src/types.js";

describe("getToolsForRole", () => {
  // Use a mock hive context — we just need to test tool set composition
  const mockHive = {} as any;

  it("coordinator gets spawn, task, message, merge tools", () => {
    const tools = getToolsForRole(AgentRole.Coordinator, "coord-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("create_task");
    expect(names).toContain("update_task");
    expect(names).toContain("list_tasks");
    expect(names).toContain("spawn_agent");
    expect(names).toContain("send_message");
    expect(names).toContain("process_merge_queue");
  });

  it("lead gets task, spawn, message, submit tools but not process_merge_queue", () => {
    const tools = getToolsForRole(AgentRole.Lead, "lead-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("create_task");
    expect(names).toContain("spawn_agent");
    expect(names).toContain("send_message");
    expect(names).toContain("submit_to_queue");
    expect(names).not.toContain("process_merge_queue");
  });

  it("worker gets update_task and send_message only", () => {
    const tools = getToolsForRole(AgentRole.Worker, "worker-1", mockHive);
    const names = tools.map((t) => t.name);
    expect(names).toContain("update_task");
    expect(names).toContain("send_message");
    expect(names).toContain("list_tasks");
    expect(names).not.toContain("create_task");
    expect(names).not.toContain("spawn_agent");
    expect(names).not.toContain("submit_to_queue");
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/tools.test.ts`
Expected: FAIL — module not found

**Step 3: Write schemas.ts (TypeBox schemas for tool parameters)**

```typescript
import { Type } from "@sinclair/typebox";

export const CreateTaskSchema = Type.Object({
  title: Type.String({ description: "Short title for the task" }),
  description: Type.String({ description: "Detailed description of the work" }),
  blockedBy: Type.Optional(Type.Array(Type.String(), { description: "Task IDs this is blocked by" })),
  domain: Type.Optional(Type.String({ description: "Filesystem path for sparse checkout scope" })),
});

export const UpdateTaskSchema = Type.Object({
  taskId: Type.String({ description: "Task ID to update" }),
  status: Type.Optional(Type.String({ description: "New status: active, review, failed, cancelled" })),
  notes: Type.Optional(Type.String({ description: "Notes to append to task description" })),
});

export const ListTasksSchema = Type.Object({
  status: Type.Optional(Type.String({ description: "Filter by status" })),
  assignee: Type.Optional(Type.String({ description: "Filter by assigned agent" })),
});

export const SpawnAgentSchema = Type.Object({
  agentId: Type.String({ description: "Unique agent ID (e.g., 'lead-backend', 'worker-1')" }),
  role: Type.String({ description: "Role: 'lead' or 'worker'" }),
  taskId: Type.String({ description: "Task ID to bind this agent to" }),
  model: Type.Optional(Type.String({ description: "Model override (e.g., 'anthropic/claude-sonnet-4-6')" })),
});

export const SendMessageSchema = Type.Object({
  to: Type.String({ description: "Recipient agent ID" }),
  body: Type.String({ description: "Message body" }),
});

export const SubmitToQueueSchema = Type.Object({
  taskId: Type.String({ description: "Task ID of the completed work" }),
  branch: Type.String({ description: "Branch name to merge" }),
});
```

**Step 4: Write individual tool files**

Create `hive-pi/src/tools/task-tools.ts`:

```typescript
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { CreateTaskSchema, UpdateTaskSchema, ListTasksSchema } from "./schemas.js";
import { TaskStatus } from "../types.js";
import type { Hive } from "../hive.js";

export function createTaskTool(agentId: string, hive: Hive): AgentTool {
  return {
    name: "create_task",
    label: "Create Task",
    description: "Create a new task with title, description, and optional dependencies.",
    parameters: CreateTaskSchema,
    execute: async (_toolCallId, params) => {
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

export function updateTaskTool(agentId: string, hive: Hive): AgentTool {
  return {
    name: "update_task",
    label: "Update Task",
    description: "Update a task's status or append notes.",
    parameters: UpdateTaskSchema,
    execute: async (_toolCallId, params) => {
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

export function listTasksTool(agentId: string, hive: Hive): AgentTool {
  return {
    name: "list_tasks",
    label: "List Tasks",
    description: "List tasks, optionally filtered by status or assignee.",
    parameters: ListTasksSchema,
    execute: async (_toolCallId, params) => {
      let tasks = hive.taskManager.listAll();
      if (params.status) {
        tasks = tasks.filter((t) => t.status === params.status);
      }
      if (params.assignee) {
        tasks = tasks.filter((t) => t.assignedTo === params.assignee);
      }
      const summary = tasks.map((t) =>
        `${t.id}: "${t.title}" [${t.status}] assigned=${t.assignedTo ?? "none"}`
      ).join("\n");
      return { content: [{ type: "text", text: summary || "No tasks found." }], details: tasks };
    },
  };
}
```

Create `hive-pi/src/tools/message-tools.ts`:

```typescript
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { SendMessageSchema } from "./schemas.js";
import type { Hive } from "../hive.js";

export function sendMessageTool(agentId: string, hive: Hive): AgentTool {
  return {
    name: "send_message",
    label: "Send Message",
    description: "Send a message to another agent. Delivered in real-time via steering.",
    parameters: SendMessageSchema,
    execute: async (_toolCallId, params) => {
      hive.sendMessage(agentId, params.to, params.body);
      return {
        content: [{ type: "text", text: `Message sent to ${params.to}.` }],
        details: { from: agentId, to: params.to },
      };
    },
  };
}
```

Create `hive-pi/src/tools/spawn-tools.ts`:

```typescript
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { SpawnAgentSchema } from "./schemas.js";
import { AgentRole } from "../types.js";
import type { Hive } from "../hive.js";

export function spawnAgentTool(agentId: string, callerRole: AgentRole, hive: Hive): AgentTool {
  return {
    name: "spawn_agent",
    label: "Spawn Agent",
    description: "Spawn a new agent (lead or worker) bound to a task.",
    parameters: SpawnAgentSchema,
    execute: async (_toolCallId, params) => {
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
        content: [{ type: "text", text: `Spawned ${role} agent "${agent.id}" for task ${params.taskId}.` }],
        details: agent,
      };
    },
  };
}
```

Create `hive-pi/src/tools/merge-tools.ts`:

```typescript
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { SubmitToQueueSchema } from "./schemas.js";
import type { Hive } from "../hive.js";

export function submitToQueueTool(agentId: string, hive: Hive): AgentTool {
  return {
    name: "submit_to_queue",
    label: "Submit to Merge Queue",
    description: "Submit a completed branch to the merge queue.",
    parameters: SubmitToQueueSchema,
    execute: async (_toolCallId, params) => {
      hive.submitToMergeQueue(params.taskId, params.branch, agentId);
      return {
        content: [{ type: "text", text: `Branch "${params.branch}" submitted to merge queue for task ${params.taskId}.` }],
        details: { taskId: params.taskId, branch: params.branch },
      };
    },
  };
}

export function processMergeQueueTool(hive: Hive): AgentTool {
  return {
    name: "process_merge_queue",
    label: "Process Merge Queue",
    description: "Process the next entry in the merge queue. Coordinator only.",
    parameters: { type: "object" as const, properties: {} },
    execute: async () => {
      const result = await hive.processMergeQueue();
      return {
        content: [{ type: "text", text: result }],
        details: {},
      };
    },
  };
}
```

Create `hive-pi/src/tools/index.ts`:

```typescript
import type { AgentTool } from "@mariozechner/pi-agent-core";
import { AgentRole } from "../types.js";
import type { Hive } from "../hive.js";
import { createTaskTool, updateTaskTool, listTasksTool } from "./task-tools.js";
import { sendMessageTool } from "./message-tools.js";
import { spawnAgentTool } from "./spawn-tools.js";
import { submitToQueueTool, processMergeQueueTool } from "./merge-tools.js";

export function getToolsForRole(role: AgentRole, agentId: string, hive: Hive): AgentTool[] {
  const tools: AgentTool[] = [];

  switch (role) {
    case AgentRole.Coordinator:
      tools.push(createTaskTool(agentId, hive));
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(spawnAgentTool(agentId, role, hive));
      tools.push(sendMessageTool(agentId, hive));
      tools.push(processMergeQueueTool(hive));
      break;

    case AgentRole.Lead:
      tools.push(createTaskTool(agentId, hive));
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(spawnAgentTool(agentId, role, hive));
      tools.push(sendMessageTool(agentId, hive));
      tools.push(submitToQueueTool(agentId, hive));
      break;

    case AgentRole.Worker:
      tools.push(updateTaskTool(agentId, hive));
      tools.push(listTasksTool(agentId, hive));
      tools.push(sendMessageTool(agentId, hive));
      break;
  }

  return tools;
}
```

**Step 5: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/tools.test.ts`
Expected: PASS

**Step 6: Commit**

```bash
git add hive-pi/src/tools/
git add hive-pi/test/tools.test.ts
git commit -m "feat: role-scoped AgentTool definitions for tasks, messages, spawning, merging"
```

---

### Task 7: System Prompts

**Files:**
- Create: `hive-pi/src/prompts/coordinator.ts`
- Create: `hive-pi/src/prompts/lead.ts`
- Create: `hive-pi/src/prompts/worker.ts`
- Create: `hive-pi/src/prompts/index.ts`

**Step 1: Write prompt templates**

Create `hive-pi/src/prompts/coordinator.ts`:

```typescript
import type { Task } from "../types.js";

export function coordinatorPrompt(runId: string, specContent: string, tasks: Task[]): string {
  const taskList = tasks.map((t) =>
    `- ${t.id}: "${t.title}" [${t.status}] ${t.blockedBy.length > 0 ? `(blocked by: ${t.blockedBy.join(", ")})` : ""}`
  ).join("\n");

  return `You are the coordinator agent for Hive run ${runId}.

## Your Role
You orchestrate a swarm of coding agents. You do NOT write code directly.
You create tasks, spawn lead agents, monitor progress, and process the merge queue.

## Spec
${specContent}

## Current Tasks
${taskList || "No tasks yet. Create tasks based on the spec above."}

## Workflow
1. Analyze the spec and create tasks with dependencies using create_task
2. Spawn lead agents for each domain using spawn_agent
3. Monitor progress via list_tasks
4. When leads submit completed branches, process the merge queue with process_merge_queue
5. Send messages to leads if they need guidance

## Rules
- You can only spawn agents with role "lead"
- Create all tasks with blockedBy relationships FIRST, then spawn leads
- Process the merge queue when tasks reach "review" status
- Group tasks by domain to minimize merge conflicts
`;
}
```

Create `hive-pi/src/prompts/lead.ts`:

```typescript
import type { Task } from "../types.js";

export function leadPrompt(agentId: string, task: Task, worktreePath: string): string {
  return `You are lead agent "${agentId}" in a Hive swarm.

## Your Task
${task.title}: ${task.description}

## Your Role
You decompose your task into subtasks for workers, spawn workers, review their work,
and submit your branch for merging when complete.

## Working Directory
Your worktree is at: ${worktreePath}
Your branch: ${task.branch ?? "not yet assigned"}
Domain: ${task.domain ?? "full repo"}

## Workflow
1. Analyze your task and create subtasks using create_task
2. Spawn worker agents for each subtask using spawn_agent
3. Monitor workers via list_tasks
4. When all subtasks are done, review the combined work
5. Submit your branch using submit_to_queue

## Rules
- You can only spawn agents with role "worker"
- Workers will work in your worktree directory
- Send messages to workers for guidance
- Send messages to coordinator if you're blocked
- Update your task status to "review" when submitting to queue
`;
}
```

Create `hive-pi/src/prompts/worker.ts`:

```typescript
import type { Task } from "../types.js";

export function workerPrompt(agentId: string, task: Task, worktreePath: string): string {
  return `You are worker agent "${agentId}" in a Hive swarm.

## Your Task
${task.title}: ${task.description}

## Working Directory
Your worktree is at: ${worktreePath}

## Workflow
1. Read the task carefully
2. Implement the changes described
3. Write tests for your changes
4. Make sure existing tests still pass
5. Commit your work with a clear message
6. Update your task status to "review" when done

## Rules
- Work only on files related to your task
- Do not modify files outside your domain
- Send messages to your lead if you're blocked or need clarification
- Do not spawn other agents
`;
}
```

Create `hive-pi/src/prompts/index.ts`:

```typescript
export { coordinatorPrompt } from "./coordinator.js";
export { leadPrompt } from "./lead.js";
export { workerPrompt } from "./worker.js";
```

**Step 2: Commit**

```bash
git add hive-pi/src/prompts/
git commit -m "feat: system prompt templates for coordinator, lead, and worker agents"
```

---

### Task 8: Core Hive Class (Agent Manager + Orchestration)

**Files:**
- Create: `hive-pi/src/hive.ts`
- Create: `hive-pi/test/hive.test.ts`

This is the central class that wires everything together: state, tasks, git, agent spawning, message passing, and merge queue.

**Step 1: Write the hive integration test**

```typescript
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";
import { Hive } from "../src/hive.js";
import { TaskStatus, AgentRole, AgentStatus } from "../src/types.js";

// Mock the pi-mono Agent class to avoid real LLM calls
vi.mock("@mariozechner/pi-agent-core", () => {
  return {
    Agent: class MockAgent {
      _state = {
        systemPrompt: "",
        model: { provider: "anthropic", id: "claude-sonnet-4-6", api: "anthropic-messages", cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } },
        thinkingLevel: "off",
        tools: [],
        messages: [],
        isStreaming: false,
        streamMessage: null,
        pendingToolCalls: new Set(),
      };
      listeners = new Set();
      constructor(opts: any) {
        if (opts?.initialState) Object.assign(this._state, opts.initialState);
      }
      get state() { return this._state; }
      setSystemPrompt(v: string) { this._state.systemPrompt = v; }
      setModel(m: any) { this._state.model = m; }
      setTools(t: any[]) { this._state.tools = t; }
      subscribe(fn: any) { this.listeners.add(fn); return () => this.listeners.delete(fn); }
      replaceMessages(ms: any[]) { this._state.messages = ms; }
      steer(m: any) {}
      abort() {}
      async prompt(input: any) {}
      async waitForIdle() {}
    },
  };
});

describe("Hive", () => {
  let repoDir: string;
  let hive: Hive;

  beforeEach(() => {
    repoDir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-test-"));
    execSync("git init", { cwd: repoDir });
    execSync("git config user.email 'test@test.com'", { cwd: repoDir });
    execSync("git config user.name 'Test'", { cwd: repoDir });
    fs.writeFileSync(path.join(repoDir, "README.md"), "# Test");
    fs.mkdirSync(path.join(repoDir, "src"), { recursive: true });
    fs.writeFileSync(path.join(repoDir, "src", "main.ts"), "// main");
    execSync("git add -A && git commit -m 'init'", { cwd: repoDir });

    hive = new Hive({ repoRoot: repoDir });
  });

  afterEach(() => {
    try { hive.stop(); } catch {}
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("initializes a run", () => {
    hive.initRun("# Spec\n- Task 1: Do something");
    const state = hive.stateManager.getState();
    expect(state.status).toBe("running");
    expect(state.runId).toBeDefined();
  });

  it("spawns a coordinator agent", async () => {
    hive.initRun("# Spec\n- Task 1: Do something");
    const agent = await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    expect(agent.role).toBe(AgentRole.Coordinator);
    expect(agent.status).toBe(AgentStatus.Running);
  });

  it("spawns a lead with a worktree", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Backend", description: "Build API" });
    const agent = await hive.spawnAgent({
      agentId: "lead-backend",
      role: AgentRole.Lead,
      taskId: task.id,
      parentAgent: "coordinator",
    });
    expect(agent.worktree).toBeDefined();
    expect(fs.existsSync(agent.worktree!)).toBe(true);
  });

  it("sends a message between agents via steer", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Work", description: "Do it" });
    await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
      parentAgent: "coordinator",
    });
    // Should not throw
    hive.sendMessage("coordinator", "lead-1", "Start working");
    const messages = hive.stateManager.getState().messages;
    expect(messages).toHaveLength(1);
    expect(messages[0].from).toBe("coordinator");
    expect(messages[0].to).toBe("lead-1");
  });

  it("submits to merge queue and processes", async () => {
    hive.initRun("# Spec");
    const task = hive.taskManager.create({ title: "Feature", description: "Build it" });
    hive.taskManager.updateStatus(task.id, TaskStatus.Active);
    hive.taskManager.updateStatus(task.id, TaskStatus.Review);

    const lead = await hive.spawnAgent({
      agentId: "lead-1",
      role: AgentRole.Lead,
      taskId: task.id,
      parentAgent: "coordinator",
    });

    // Make a change in the lead's worktree
    fs.writeFileSync(path.join(lead.worktree!, "new-file.txt"), "content");
    execSync("git add -A && git commit -m 'add feature'", { cwd: lead.worktree! });

    hive.submitToMergeQueue(task.id, lead.branch!, "lead-1");
    const result = await hive.processMergeQueue();
    expect(result).toContain("Merged");
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd hive-pi && npx vitest --run test/hive.test.ts`
Expected: FAIL — module not found

**Step 3: Write hive.ts**

```typescript
import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { Agent } from "@mariozechner/pi-agent-core";
import { getModel } from "@mariozechner/pi-ai";
import { StateManager } from "./state.js";
import { TaskManager } from "./task-manager.js";
import { GitManager } from "./git.js";
import { getToolsForRole } from "./tools/index.js";
import { coordinatorPrompt, leadPrompt, workerPrompt } from "./prompts/index.js";
import { AgentRole, AgentStatus, TaskStatus } from "./types.js";
import type { AgentInfo, HiveMessage } from "./types.js";

export interface HiveOptions {
  repoRoot: string;
  defaultModel?: string; // "provider/model-id" format
}

export interface SpawnAgentParams {
  agentId: string;
  role: AgentRole;
  taskId: string;
  parentAgent?: string;
  model?: string;
}

export class Hive {
  readonly stateManager: StateManager;
  readonly taskManager: TaskManager;
  readonly gitManager: GitManager;
  private agents: Map<string, Agent> = new Map();
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

  initRun(specContent: string): string {
    const runId = crypto.randomUUID().slice(0, 8);
    this.specContent = specContent;
    this.stateManager.initRun(runId, "inline");
    return runId;
  }

  initRunFromFile(specFile: string): string {
    this.specContent = fs.readFileSync(specFile, "utf-8");
    const runId = crypto.randomUUID().slice(0, 8);
    this.stateManager.initRun(runId, specFile);
    return runId;
  }

  async spawnAgent(params: SpawnAgentParams): Promise<AgentInfo> {
    const state = this.stateManager.getState();
    const runId = state.runId;

    // Parse model string "provider/model-id"
    const modelStr = params.model ?? this.defaultModel;
    const [provider, ...modelParts] = modelStr.split("/");
    const modelId = modelParts.join("/");

    let worktreePath: string | undefined;
    let branch: string | undefined;

    // Create worktree for leads and workers
    if (params.role !== AgentRole.Coordinator) {
      const task = this.taskManager.get(params.taskId);
      const wt = this.gitManager.createWorktree(params.agentId, runId, {
        sparsePaths: task.domain ? [task.domain] : undefined,
        baseBranch: params.parentAgent
          ? state.agents[params.parentAgent]?.branch
          : undefined,
      });
      worktreePath = wt.worktreePath;
      branch = wt.branch;
    }

    // Register agent in state
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

    // Assign task
    if (params.taskId !== "__coordinator__") {
      this.taskManager.assign(params.taskId, params.agentId);
      if (this.taskManager.get(params.taskId).status === TaskStatus.Pending) {
        this.taskManager.updateStatus(params.taskId, TaskStatus.Active);
      }
    }

    // Create pi-mono Agent instance
    const model = getModel(provider as any, modelId as any);
    const tools = getToolsForRole(params.role, params.agentId, this);
    const systemPrompt = this.buildSystemPrompt(params.role, params.agentId, params.taskId, worktreePath);

    const agent = new Agent({
      initialState: {
        model,
        systemPrompt,
        tools,
      },
    });

    // Subscribe to events for persistence
    agent.subscribe((event: any) => {
      if (event.type === "message_end") {
        this.stateManager.appendAgentMessage(params.agentId, event.message);
      }
      if (event.type === "agent_end") {
        this.stateManager.updateAgent(params.agentId, { status: AgentStatus.Done });
      }
    });

    this.agents.set(params.agentId, agent);

    // Save agent metadata for crash recovery
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

  private buildSystemPrompt(role: AgentRole, agentId: string, taskId: string, worktreePath?: string): string {
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

  sendMessage(from: string, to: string, body: string): void {
    const msg: HiveMessage = {
      id: crypto.randomUUID().slice(0, 8),
      from,
      to,
      body,
      timestamp: Date.now(),
    };
    this.stateManager.addMessage(msg);

    // Deliver via steer if agent exists
    const targetAgent = this.agents.get(to);
    if (targetAgent) {
      targetAgent.steer({
        role: "user" as const,
        content: `[Message from ${from}]: ${body}`,
        timestamp: Date.now(),
      });
    }
  }

  submitToMergeQueue(taskId: string, branch: string, submittedBy: string): void {
    this.stateManager.addMergeQueueEntry({
      taskId,
      branch,
      submittedBy,
      submittedAt: Date.now(),
      status: "pending",
    });
  }

  async processMergeQueue(): Promise<string> {
    const state = this.stateManager.getState();
    const pending = state.mergeQueue.find((e) => e.status === "pending");
    if (!pending) return "No pending entries in merge queue.";

    this.stateManager.updateMergeQueueEntry(pending.taskId, { status: "merging" });
    const mainBranch = this.gitManager.getMainBranch();

    let result = this.gitManager.mergeBranch(pending.branch, mainBranch);
    if (!result.success) {
      // Try rebase
      const rebaseResult = this.gitManager.rebaseBranch(pending.branch, mainBranch);
      if (rebaseResult.success) {
        result = this.gitManager.mergeBranch(pending.branch, mainBranch);
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

  getAgent(agentId: string): Agent | undefined {
    return this.agents.get(agentId);
  }

  stop(): void {
    // Abort all running agents
    for (const [id, agent] of this.agents) {
      agent.abort();
    }
    // Clean up worktrees
    this.gitManager.cleanupAllWorktrees();
    // Update state
    const state = this.stateManager.getState();
    state.status = "stopped";
    this.stateManager.save();
  }
}
```

**Step 4: Update src/index.ts**

```typescript
export { Hive } from "./hive.js";
export { StateManager } from "./state.js";
export { TaskManager } from "./task-manager.js";
export { GitManager } from "./git.js";
export type * from "./types.js";
export * from "./types.js";
```

**Step 5: Run test to verify it passes**

Run: `cd hive-pi && npx vitest --run test/hive.test.ts`
Expected: PASS

**Step 6: Commit**

```bash
git add hive-pi/src/hive.ts hive-pi/src/index.ts hive-pi/test/hive.test.ts
git commit -m "feat: core Hive class with agent spawning, messaging, and merge queue"
```

---

### Task 9: CLI Entry Point

**Files:**
- Create: `hive-pi/src/main.ts`

**Step 1: Write the CLI**

```typescript
#!/usr/bin/env node

import { Command } from "commander";
import * as fs from "node:fs";
import { Hive } from "./hive.js";

const program = new Command();

program
  .name("hive-pi")
  .description("Swarm orchestrator using pi-mono agents")
  .version("0.1.0");

program
  .command("start")
  .description("Start a new run from a spec file")
  .argument("<spec>", "Path to the spec file")
  .option("--model <model>", "Default model (provider/model-id)", "anthropic/claude-sonnet-4-6")
  .action(async (specFile: string, opts: { model: string }) => {
    if (!fs.existsSync(specFile)) {
      console.error(`Spec file not found: ${specFile}`);
      process.exit(1);
    }

    const hive = new Hive({
      repoRoot: process.cwd(),
      defaultModel: opts.model,
    });

    const runId = hive.initRunFromFile(specFile);
    console.log(`Started run ${runId}`);

    // Spawn coordinator and give it the spec
    const specContent = fs.readFileSync(specFile, "utf-8");
    const coordinator = await hive.spawnAgent({
      agentId: "coordinator",
      role: "coordinator" as any,
      taskId: "__coordinator__",
    });

    console.log(`Coordinator spawned: ${coordinator.id}`);
    console.log("Run is active. Use 'hive-pi status' to monitor.");

    // Start the coordinator with the spec
    const agent = hive.getAgent("coordinator");
    if (agent) {
      await agent.prompt(`Execute this spec:\n\n${specContent}`);
    }
  });

program
  .command("status")
  .description("Show current run status")
  .action(() => {
    const hiveDir = ".hive";
    if (!fs.existsSync(`${hiveDir}/state.json`)) {
      console.error("No active run. Use 'hive-pi start <spec>' to begin.");
      process.exit(1);
    }

    const state = JSON.parse(fs.readFileSync(`${hiveDir}/state.json`, "utf-8"));
    console.log(`Run: ${state.runId} [${state.status}]`);
    console.log(`\nTasks (${Object.keys(state.tasks).length}):`);
    for (const task of Object.values(state.tasks) as any[]) {
      console.log(`  ${task.id}: "${task.title}" [${task.status}] assigned=${task.assignedTo ?? "-"}`);
    }
    console.log(`\nAgents (${Object.keys(state.agents).length}):`);
    for (const agent of Object.values(state.agents) as any[]) {
      console.log(`  ${agent.id}: [${agent.role}] ${agent.status} model=${agent.model}`);
    }
    console.log(`\nMerge Queue (${state.mergeQueue.length}):`);
    for (const entry of state.mergeQueue) {
      console.log(`  ${entry.taskId}: ${entry.branch} [${entry.status}]`);
    }
  });

program
  .command("stop")
  .description("Stop the current run")
  .action(() => {
    const hive = new Hive({ repoRoot: process.cwd() });
    try {
      hive.stateManager.load();
      hive.stop();
      console.log("Run stopped. Worktrees cleaned up.");
    } catch (err: any) {
      console.error(`Failed to stop: ${err.message}`);
    }
  });

program
  .command("tasks")
  .description("List all tasks")
  .action(() => {
    const state = JSON.parse(fs.readFileSync(".hive/state.json", "utf-8"));
    for (const task of Object.values(state.tasks) as any[]) {
      const blocked = task.blockedBy?.length > 0 ? ` blocked_by=[${task.blockedBy.join(",")}]` : "";
      console.log(`${task.id}: "${task.title}" [${task.status}]${blocked}`);
    }
  });

program
  .command("agents")
  .description("List all agents")
  .action(() => {
    const state = JSON.parse(fs.readFileSync(".hive/state.json", "utf-8"));
    for (const agent of Object.values(state.agents) as any[]) {
      console.log(`${agent.id}: [${agent.role}] ${agent.status} task=${agent.taskId} model=${agent.model}`);
    }
  });

program
  .command("cost")
  .description("Show cost summary")
  .action(() => {
    // TODO: Aggregate from agent messages
    console.log("Cost tracking not yet implemented.");
  });

program.parse();
```

**Step 2: Verify it builds**

Run: `cd hive-pi && npx tsc`
Expected: Compiles (may have some errors to fix — iterate until clean)

**Step 3: Commit**

```bash
git add hive-pi/src/main.ts
git commit -m "feat: CLI entry point with start, status, stop, tasks, agents commands"
```

---

### Task 10: Integration Test — Full Run Simulation

**Files:**
- Create: `hive-pi/test/integration.test.ts`

**Step 1: Write a full integration test (with mocked LLM)**

```typescript
import { describe, it, expect, afterEach, vi } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { execSync } from "node:child_process";
import { Hive } from "../src/hive.js";
import { TaskStatus, AgentRole } from "../src/types.js";

// Same mock as hive.test.ts
vi.mock("@mariozechner/pi-agent-core", () => ({
  Agent: class MockAgent {
    _state = {
      systemPrompt: "",
      model: { provider: "anthropic", id: "claude-sonnet-4-6", api: "anthropic-messages", cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } },
      thinkingLevel: "off",
      tools: [],
      messages: [],
      isStreaming: false,
      streamMessage: null,
      pendingToolCalls: new Set(),
    };
    listeners = new Set();
    constructor(opts: any) {
      if (opts?.initialState) Object.assign(this._state, opts.initialState);
    }
    get state() { return this._state; }
    setSystemPrompt(v: string) { this._state.systemPrompt = v; }
    setModel(m: any) { this._state.model = m; }
    setTools(t: any[]) { this._state.tools = t; }
    subscribe(fn: any) { this.listeners.add(fn); return () => this.listeners.delete(fn); }
    replaceMessages(ms: any[]) { this._state.messages = ms; }
    steer(_m: any) {}
    abort() {}
    async prompt(_input: any) {}
    async waitForIdle() {}
  },
}));

describe("Integration: full run lifecycle", () => {
  let repoDir: string;
  let hive: Hive;

  function setupRepo(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hive-integration-"));
    execSync("git init", { cwd: dir });
    execSync("git config user.email 'test@test.com'", { cwd: dir });
    execSync("git config user.name 'Test'", { cwd: dir });
    fs.mkdirSync(path.join(dir, "src"), { recursive: true });
    fs.writeFileSync(path.join(dir, "README.md"), "# Project");
    fs.writeFileSync(path.join(dir, "src", "app.ts"), "// app");
    execSync("git add -A && git commit -m 'init'", { cwd: dir });
    return dir;
  }

  afterEach(() => {
    try { hive?.stop(); } catch {}
    if (repoDir) fs.rmSync(repoDir, { recursive: true, force: true });
  });

  it("simulates a complete run: init → tasks → leads → workers → merge", async () => {
    repoDir = setupRepo();
    hive = new Hive({ repoRoot: repoDir });

    // 1. Init run
    const runId = hive.initRun("# Build a REST API\n- Backend service\n- Database layer");
    expect(runId).toBeDefined();

    // 2. Create tasks (what coordinator would do)
    const t1 = hive.taskManager.create({ title: "Backend API", description: "Build REST endpoints", domain: "src" });
    const t2 = hive.taskManager.create({ title: "Database", description: "Add DB layer", domain: "src", blockedBy: [t1.id] });

    // 3. Spawn coordinator
    await hive.spawnAgent({ agentId: "coordinator", role: AgentRole.Coordinator, taskId: "__coordinator__" });

    // 4. Spawn lead for t1
    const lead1 = await hive.spawnAgent({
      agentId: "lead-backend", role: AgentRole.Lead, taskId: t1.id, parentAgent: "coordinator",
    });
    expect(lead1.worktree).toBeDefined();

    // 5. Spawn worker under lead
    const workerTask = hive.taskManager.create({ title: "Implement GET /users", description: "Handler", parentTask: t1.id });
    hive.taskManager.updateStatus(workerTask.id, TaskStatus.Active);
    const worker = await hive.spawnAgent({
      agentId: "worker-1", role: AgentRole.Worker, taskId: workerTask.id, parentAgent: "lead-backend",
    });
    expect(worker.worktree).toBeDefined();

    // 6. Worker "does work" — writes a file and commits
    fs.writeFileSync(path.join(worker.worktree!, "src", "users.ts"), "export function getUsers() {}");
    execSync("git add -A && git commit -m 'feat: add users endpoint'", { cwd: worker.worktree! });
    hive.taskManager.updateStatus(workerTask.id, TaskStatus.Review);

    // 7. Lead reviews and submits to merge queue
    // Merge worker's changes into lead's branch first
    execSync(`git merge ${worker.branch}`, { cwd: lead1.worktree! });
    hive.taskManager.updateStatus(t1.id, TaskStatus.Review);
    hive.submitToMergeQueue(t1.id, lead1.branch!, "lead-backend");

    // 8. Coordinator processes merge queue
    const mergeResult = await hive.processMergeQueue();
    expect(mergeResult).toContain("Merged");

    // 9. t2 should be unblocked
    const t2Updated = hive.taskManager.get(t2.id);
    expect(t2Updated.status).toBe(TaskStatus.Pending);

    // 10. Verify file exists on main
    const mainContent = execSync("git show HEAD:src/users.ts", { cwd: repoDir, encoding: "utf-8" });
    expect(mainContent).toContain("getUsers");
  });
});
```

**Step 2: Run integration test**

Run: `cd hive-pi && npx vitest --run test/integration.test.ts`
Expected: PASS

**Step 3: Run all tests**

Run: `cd hive-pi && npx vitest --run`
Expected: All tests pass

**Step 4: Commit**

```bash
git add hive-pi/test/integration.test.ts
git commit -m "test: integration test for full run lifecycle"
```

---

### Task 11: Type Check & Final Cleanup

**Step 1: Run type check**

Run: `cd hive-pi && npx tsc --noEmit`
Expected: Clean (fix any errors)

**Step 2: Run all tests**

Run: `cd hive-pi && npx vitest --run`
Expected: All pass

**Step 3: Final commit**

```bash
git add -A hive-pi/
git commit -m "chore: type check clean, all tests passing"
```

---

## Summary

| Task | Description | Est. Lines |
|------|-------------|------------|
| 1 | Project scaffolding | ~50 |
| 2 | Core types | ~100 |
| 3 | State persistence | ~120 |
| 4 | Task manager | ~100 |
| 5 | Git operations | ~120 |
| 6 | Hive tools | ~250 |
| 7 | System prompts | ~80 |
| 8 | Core Hive class | ~200 |
| 9 | CLI entry point | ~120 |
| 10 | Integration test | ~100 |
| 11 | Cleanup | ~0 |
| **Total** | | **~1,240** |
