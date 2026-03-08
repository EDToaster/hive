use std::path::Path;
use std::process::Command;

pub struct Git;

impl Git {
    fn run(args: &[&str], cwd: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(format!("git {} failed: {stderr}", args.join(" ")))
        }
    }

    /// Create a new worktree with a new branch
    pub fn worktree_add(
        repo_root: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                "-b",
                branch,
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Remove a worktree
    pub fn worktree_remove(repo_root: &Path, worktree_path: &Path) -> Result<(), String> {
        Self::run(
            &[
                "worktree",
                "remove",
                &worktree_path.to_string_lossy(),
                "--force",
            ],
            repo_root,
        )?;
        Ok(())
    }

    /// Prune stale worktree metadata
    pub fn worktree_prune(repo_root: &Path) -> Result<(), String> {
        Self::run(&["worktree", "prune"], repo_root)?;
        Ok(())
    }

    /// Merge a branch into the current branch
    pub fn merge(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["merge", branch, "--no-ff"], repo_root)?;
        Ok(())
    }

    /// Abort a merge in progress
    pub fn merge_abort(repo_root: &Path) -> Result<(), String> {
        Self::run(&["merge", "--abort"], repo_root)?;
        Ok(())
    }

    /// Check if there are merge conflicts
    pub fn has_conflicts(repo_root: &Path) -> Result<bool, String> {
        let output = Self::run(&["diff", "--name-only", "--diff-filter=U"], repo_root)?;
        Ok(!output.is_empty())
    }

    /// Get the current branch name
    pub fn current_branch(repo_root: &Path) -> Result<String, String> {
        Self::run(&["branch", "--show-current"], repo_root)
    }

    /// Checkout a branch
    pub fn checkout(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["checkout", branch], repo_root)?;
        Ok(())
    }

    /// Delete a branch
    pub fn branch_delete(repo_root: &Path, branch: &str) -> Result<(), String> {
        Self::run(&["branch", "-D", branch], repo_root)?;
        Ok(())
    }
}
