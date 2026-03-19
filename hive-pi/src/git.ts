import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs";

const execFileAsync = promisify(execFile);

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

  private async exec(args: string[], cwd?: string): Promise<string> {
    const { stdout } = await execFileAsync("git", args, {
      cwd: cwd ?? this.repoRoot,
    });
    return stdout.trim();
  }

  async getMainBranch(): Promise<string> {
    try {
      const ref = await this.exec(["symbolic-ref", "refs/remotes/origin/HEAD"]);
      return ref.replace("refs/remotes/origin/", "");
    } catch {
      // Fallback: check if main or master exists
      try {
        await this.exec(["rev-parse", "--verify", "main"]);
        return "main";
      } catch {
        return "master";
      }
    }
  }

  async createWorktree(agentId: string, runId: string, options?: WorktreeOptions): Promise<WorktreeResult> {
    const branch = `hive/${runId}/${agentId}`;
    const worktreePath = path.join(this.hiveDir, "worktrees", agentId);

    const base = options?.baseBranch ?? "HEAD";
    await this.exec(["worktree", "add", worktreePath, "-b", branch, base]);

    if (options?.sparsePaths && options.sparsePaths.length > 0) {
      await this.exec(["sparse-checkout", "init", "--cone"], worktreePath);
      await this.exec(
        ["sparse-checkout", "set", ...options.sparsePaths],
        worktreePath
      );
    }

    return { worktreePath, branch };
  }

  async removeWorktree(agentId: string): Promise<void> {
    const worktreePath = path.join(this.hiveDir, "worktrees", agentId);
    if (fs.existsSync(worktreePath)) {
      await this.exec(["worktree", "remove", worktreePath, "--force"]);
    }
    // Clean up the branch too
    const branchList = await this.exec(["branch"]);
    const branches = branchList.split("\n").map((b) => b.trim().replace("* ", ""));
    for (const branch of branches) {
      if (branch.includes(`/${agentId}`)) {
        try {
          await this.exec(["branch", "-D", branch]);
        } catch {}
      }
    }
  }

  async cleanupAllWorktrees(): Promise<void> {
    const worktreeDir = path.join(this.hiveDir, "worktrees");
    if (!fs.existsSync(worktreeDir)) return;

    for (const entry of fs.readdirSync(worktreeDir)) {
      await this.removeWorktree(entry);
    }
  }

  async mergeBranch(branch: string, targetBranch: string): Promise<MergeResult> {
    try {
      // Save current branch
      const currentBranch = await this.exec(["rev-parse", "--abbrev-ref", "HEAD"]);

      // Checkout target, merge, return to original
      await this.exec(["checkout", targetBranch]);
      try {
        await this.exec(["merge", branch, "--no-ff", "-m", `merge: ${branch}`]);
        return { success: true };
      } catch (err: any) {
        // Abort failed merge
        try { await this.exec(["merge", "--abort"]); } catch {}
        return { success: false, error: err.stderr ?? err.message };
      } finally {
        // Return to original branch
        if (currentBranch !== targetBranch) {
          try { await this.exec(["checkout", currentBranch]); } catch {}
        }
      }
    } catch (err: any) {
      return { success: false, error: err.stderr ?? err.message };
    }
  }

  async rebaseBranch(branch: string, ontoBranch: string): Promise<MergeResult> {
    try {
      await this.exec(["checkout", branch]);
      try {
        await this.exec(["rebase", ontoBranch]);
        return { success: true };
      } catch (err: any) {
        try { await this.exec(["rebase", "--abort"]); } catch {}
        return { success: false, error: err.stderr ?? err.message };
      } finally {
        try { await this.exec(["checkout", ontoBranch]); } catch {}
      }
    } catch (err: any) {
      return { success: false, error: err.stderr ?? err.message };
    }
  }

  /**
   * Validate that a domain path exists in the git tree.
   */
  async validateDomainPath(domainPath: string): Promise<boolean> {
    try {
      await this.exec(["ls-tree", "HEAD", domainPath]);
      return true;
    } catch {
      return false;
    }
  }
}
