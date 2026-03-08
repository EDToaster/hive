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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_test_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn worktree_add_creates_worktree() {
        let dir = init_test_repo();
        let wt_path = dir.path().join("worktree-1");

        Git::worktree_add(dir.path(), &wt_path, "test-branch").unwrap();

        assert!(wt_path.exists());
        assert!(wt_path.is_dir());
        assert_eq!(Git::current_branch(&wt_path).unwrap(), "test-branch");
    }

    #[test]
    fn worktree_add_fails_on_duplicate_branch() {
        let dir = init_test_repo();
        let wt1 = dir.path().join("wt1");
        let wt2 = dir.path().join("wt2");

        Git::worktree_add(dir.path(), &wt1, "same-branch").unwrap();
        assert!(Git::worktree_add(dir.path(), &wt2, "same-branch").is_err());
    }

    #[test]
    fn worktree_remove_cleans_up() {
        let dir = init_test_repo();
        let wt_path = dir.path().join("wt-remove");

        Git::worktree_add(dir.path(), &wt_path, "remove-branch").unwrap();
        assert!(wt_path.exists());

        Git::worktree_remove(dir.path(), &wt_path).unwrap();
        assert!(!wt_path.exists());
    }

    #[test]
    fn worktree_prune_succeeds() {
        let dir = init_test_repo();
        Git::worktree_prune(dir.path()).unwrap();
    }

    #[test]
    fn current_branch_returns_branch_name() {
        let dir = init_test_repo();
        let branch = Git::current_branch(dir.path()).unwrap();
        assert!(!branch.is_empty());
    }

    #[test]
    fn checkout_switches_branch() {
        let dir = init_test_repo();
        Git::run(&["branch", "feature-1"], dir.path()).unwrap();
        Git::checkout(dir.path(), "feature-1").unwrap();
        assert_eq!(Git::current_branch(dir.path()).unwrap(), "feature-1");
    }

    #[test]
    fn checkout_nonexistent_branch_fails() {
        let dir = init_test_repo();
        assert!(Git::checkout(dir.path(), "nonexistent-branch").is_err());
    }

    #[test]
    fn branch_delete_removes_branch() {
        let dir = init_test_repo();
        Git::run(&["branch", "to-delete"], dir.path()).unwrap();
        Git::branch_delete(dir.path(), "to-delete").unwrap();

        let branches = Git::run(&["branch"], dir.path()).unwrap();
        assert!(!branches.contains("to-delete"));
    }

    #[test]
    fn merge_no_ff_creates_merge_commit() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        Git::run(&["checkout", "-b", "feature-merge"], dir.path()).unwrap();
        fs::write(dir.path().join("feature.txt"), "feature content").unwrap();
        Git::run(&["add", "feature.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "add feature"], dir.path()).unwrap();

        Git::checkout(dir.path(), &main_branch).unwrap();
        Git::merge(dir.path(), "feature-merge").unwrap();

        let log = Git::run(&["log", "--oneline", "-1"], dir.path()).unwrap();
        assert!(log.contains("Merge branch"));
    }

    #[test]
    fn has_conflicts_returns_false_on_clean_repo() {
        let dir = init_test_repo();
        assert!(!Git::has_conflicts(dir.path()).unwrap());
    }

    #[test]
    fn merge_conflicting_branches_detected() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        // Create conflicting file on main
        fs::write(dir.path().join("conflict.txt"), "main content").unwrap();
        Git::run(&["add", "conflict.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "main version"], dir.path()).unwrap();

        // Create branch with different content for same file
        Git::run(&["checkout", "-b", "conflict-branch", "HEAD~1"], dir.path()).unwrap();
        fs::write(dir.path().join("conflict.txt"), "branch content").unwrap();
        Git::run(&["add", "conflict.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "branch version"], dir.path()).unwrap();

        // Try to merge — should fail
        Git::checkout(dir.path(), &main_branch).unwrap();
        assert!(Git::merge(dir.path(), "conflict-branch").is_err());

        // Check for conflicts
        assert!(Git::has_conflicts(dir.path()).unwrap());

        // Abort merge
        Git::merge_abort(dir.path()).unwrap();
    }
}
