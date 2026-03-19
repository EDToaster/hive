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
