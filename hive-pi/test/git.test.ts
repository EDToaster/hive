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
