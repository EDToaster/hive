#!/usr/bin/env node

import { Command } from "commander";
import * as fs from "node:fs";
import * as path from "node:path";
import { Hive } from "./hive.js";
import { AgentRole } from "./types.js";

const program = new Command();

program
  .name("hive-pi")
  .description("Swarm orchestrator using pi-mono agents")
  .version("0.1.0");

// ─── start ──────────────────────────────────────────────────────────────────
program
  .command("start")
  .description("Start a new run from a spec file")
  .argument("<spec>", "Path to the spec file")
  .option("--model <model>", "Default model (provider/model-id)", "anthropic/claude-sonnet-4-6")
  .option("--repo <path>", "Repository root (default: cwd)", process.cwd())
  .action(async (specFile: string, opts: { model: string; repo: string }) => {
    const specPath = path.resolve(opts.repo, specFile);
    if (!fs.existsSync(specPath)) {
      console.error(`Spec file not found: ${specPath}`);
      process.exit(1);
    }

    const hive = new Hive({ repoRoot: opts.repo, defaultModel: opts.model });
    const runId = hive.initRunFromFile(specPath);
    console.log(`Started run ${runId}`);

    // Spawn coordinator
    const coordinator = await hive.spawnAgent({
      agentId: "coordinator",
      role: AgentRole.Coordinator,
      taskId: "__coordinator__",
    });
    console.log(`Coordinator spawned: ${coordinator.id} (model: ${coordinator.model})`);

    // Start coordinator with the spec
    const agent = hive.getAgent("coordinator");
    if (agent) {
      const specContent = fs.readFileSync(specPath, "utf-8");
      console.log("Coordinator running...");
      await agent.prompt(`Execute this spec:\n\n${specContent}`);
      console.log("Coordinator complete.");
    }
  });

// ─── status ─────────────────────────────────────────────────────────────────
program
  .command("status")
  .description("Show current run status")
  .option("--repo <path>", "Repository root (default: cwd)", process.cwd())
  .action((opts: { repo: string }) => {
    const stateFile = path.join(opts.repo, ".hive", "state.json");
    if (!fs.existsSync(stateFile)) {
      console.error("No active run. Use 'hive-pi start <spec>' to begin.");
      process.exit(1);
    }

    const state = JSON.parse(fs.readFileSync(stateFile, "utf-8"));
    const taskList = Object.values(state.tasks) as any[];
    const agentList = Object.values(state.agents) as any[];

    console.log(`Run: ${state.runId} [${state.status}]`);
    console.log(`Spec: ${state.specFile}`);
    console.log(`\nTasks (${taskList.length}):`);
    for (const task of taskList) {
      const blocked =
        task.blockedBy?.length > 0 ? ` blocked_by=[${task.blockedBy.join(",")}]` : "";
      console.log(
        `  ${task.id}: "${task.title}" [${task.status}] assigned=${task.assignedTo ?? "-"}${blocked}`
      );
    }
    console.log(`\nAgents (${agentList.length}):`);
    for (const agent of agentList) {
      console.log(
        `  ${agent.id}: [${agent.role}] ${agent.status} model=${agent.model} task=${agent.taskId}`
      );
    }
    console.log(`\nMerge Queue (${state.mergeQueue.length}):`);
    for (const entry of state.mergeQueue) {
      console.log(`  ${entry.taskId}: ${entry.branch} [${entry.status}]`);
    }
  });

// ─── stop ────────────────────────────────────────────────────────────────────
program
  .command("stop")
  .description("Stop the current run and clean up worktrees")
  .option("--repo <path>", "Repository root (default: cwd)", process.cwd())
  .action((opts: { repo: string }) => {
    const stateFile = path.join(opts.repo, ".hive", "state.json");
    if (!fs.existsSync(stateFile)) {
      console.error("No active run found.");
      process.exit(1);
    }
    const hive = new Hive({ repoRoot: opts.repo });
    try {
      hive.stateManager.load();
      hive.stop();
      console.log("Run stopped. Worktrees cleaned up.");
    } catch (err: any) {
      console.error(`Failed to stop: ${err.message}`);
      process.exit(1);
    }
  });

// ─── tasks ───────────────────────────────────────────────────────────────────
program
  .command("tasks")
  .description("List all tasks")
  .option("--repo <path>", "Repository root (default: cwd)", process.cwd())
  .option("--status <status>", "Filter by status")
  .action((opts: { repo: string; status?: string }) => {
    const stateFile = path.join(opts.repo, ".hive", "state.json");
    if (!fs.existsSync(stateFile)) {
      console.error("No active run found.");
      process.exit(1);
    }
    const state = JSON.parse(fs.readFileSync(stateFile, "utf-8"));
    let tasks = Object.values(state.tasks) as any[];
    if (opts.status) {
      tasks = tasks.filter((t) => t.status === opts.status);
    }
    if (tasks.length === 0) {
      console.log("No tasks found.");
      return;
    }
    for (const task of tasks) {
      const blocked =
        task.blockedBy?.length > 0 ? ` blocked_by=[${task.blockedBy.join(",")}]` : "";
      console.log(`${task.id}: "${task.title}" [${task.status}]${blocked}`);
    }
  });

// ─── agents ──────────────────────────────────────────────────────────────────
program
  .command("agents")
  .description("List all agents")
  .option("--repo <path>", "Repository root (default: cwd)", process.cwd())
  .action((opts: { repo: string }) => {
    const stateFile = path.join(opts.repo, ".hive", "state.json");
    if (!fs.existsSync(stateFile)) {
      console.error("No active run found.");
      process.exit(1);
    }
    const state = JSON.parse(fs.readFileSync(stateFile, "utf-8"));
    const agents = Object.values(state.agents) as any[];
    if (agents.length === 0) {
      console.log("No agents found.");
      return;
    }
    for (const agent of agents) {
      console.log(
        `${agent.id}: [${agent.role}] ${agent.status} task=${agent.taskId} model=${agent.model}`
      );
    }
  });

program.parse();
