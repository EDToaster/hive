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

    /// Create a new worktree with a new branch (full checkout)
    pub fn worktree_add(
        repo_root: &Path,
        worktree_path: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<(), String> {
        let wt_str = worktree_path.to_string_lossy();
        let mut args = vec!["worktree", "add", &wt_str, "-b", branch];
        if let Some(sp) = start_point {
            args.push(sp);
        }
        Self::run(&args, repo_root)?;
        Ok(())
    }

    /// Create a new worktree with a new branch, skipping file checkout.
    /// Use this as the first step for sparse or no-checkout worktrees.
    pub fn worktree_add_no_checkout(
        repo_root: &Path,
        worktree_path: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<(), String> {
        let wt_str = worktree_path.to_string_lossy();
        let mut args = vec!["worktree", "add", "--no-checkout", &wt_str, "-b", branch];
        if let Some(sp) = start_point {
            args.push(sp);
        }
        Self::run(&args, repo_root)?;
        Ok(())
    }

    /// Initialize cone-mode sparse checkout in a worktree.
    /// Must be called after `worktree_add_no_checkout`.
    pub fn sparse_checkout_init(worktree_path: &Path) -> Result<(), String> {
        Self::run(&["sparse-checkout", "init", "--cone"], worktree_path)?;
        Ok(())
    }

    /// Set the sparse checkout paths for a worktree.
    /// Paths are directory prefixes (cone mode). This triggers file population.
    pub fn sparse_checkout_set(worktree_path: &Path, paths: &[&str]) -> Result<(), String> {
        let mut args = vec!["sparse-checkout", "set"];
        args.extend_from_slice(paths);
        Self::run(&args, worktree_path)?;
        Ok(())
    }

    /// List current sparse checkout paths in a worktree.
    pub fn sparse_checkout_list(worktree_path: &Path) -> Result<Vec<String>, String> {
        let out = Self::run(&["sparse-checkout", "list"], worktree_path)?;
        Ok(out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// Populate files in a `--no-checkout` worktree by running `git checkout`.
    /// This respects any active sparse-checkout configuration.
    /// Must be called after `sparse_checkout_set` to actually write files to disk.
    pub fn checkout_populate(worktree_path: &Path) -> Result<(), String> {
        Self::run(&["checkout"], worktree_path)?;
        Ok(())
    }

    /// Check which paths do NOT exist as directories in the repo at HEAD.
    /// Returns the subset of `paths` that are invalid (empty means all valid).
    pub fn validate_sparse_paths(repo_root: &Path, paths: &[&str]) -> Result<Vec<String>, String> {
        let mut invalid = Vec::new();
        for path in paths {
            let output = Self::run(&["ls-tree", "-d", "HEAD", path], repo_root)?;
            if output.is_empty() {
                invalid.push(path.to_string());
            }
        }
        Ok(invalid)
    }

    /// Count the number of tracked files in the repo.
    pub fn file_count(repo_root: &Path) -> Result<usize, String> {
        let output = Self::run(&["ls-files"], repo_root)?;
        if output.is_empty() {
            return Ok(0);
        }
        Ok(output.lines().count())
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

    /// Squash-merge a branch into the current branch as a single commit.
    /// Runs `git merge --squash` then `git commit -m <message>`.
    pub fn merge_squash(repo_root: &Path, branch: &str, message: &str) -> Result<(), String> {
        Self::run(&["merge", "--squash", branch], repo_root)?;
        Self::run(&["commit", "-m", message], repo_root)?;
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

    /// Get porcelain status (machine-readable) for a worktree
    pub fn status_porcelain(cwd: &Path) -> Result<String, String> {
        Self::run(&["status", "--porcelain"], cwd)
    }

    /// Stage all changes (tracked and untracked)
    pub fn add_all(cwd: &Path) -> Result<(), String> {
        Self::run(&["add", "-A"], cwd)?;
        Ok(())
    }

    /// Create a commit with the given message
    pub fn commit(cwd: &Path, message: &str) -> Result<(), String> {
        Self::run(&["commit", "-m", message], cwd)?;
        Ok(())
    }

    /// Get one-line log of commits since a base branch
    pub fn log_oneline_since(cwd: &Path, base: &str) -> Result<String, String> {
        Self::run(&["log", "--oneline", &format!("{base}..HEAD")], cwd)
    }

    /// Get diff --stat since a base branch
    pub fn diff_stat_since(cwd: &Path, base: &str) -> Result<String, String> {
        Self::run(&["diff", "--stat", &format!("{base}..HEAD")], cwd)
    }

    /// Run a shell command in the given directory. Returns Ok(stdout) or Err(stderr).
    pub fn run_shell_command(cwd: &Path, command: &str) -> Result<String, String> {
        let output = std::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run command: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }

    /// Reset the current branch hard to a given ref (e.g. "HEAD~1")
    pub fn reset_hard(repo_root: &Path, to_ref: &str) -> Result<(), String> {
        Self::run(&["reset", "--hard", to_ref], repo_root)?;
        Ok(())
    }

    /// Rebase a branch onto a target branch
    pub fn rebase(repo_root: &Path, branch: &str, onto: &str) -> Result<(), String> {
        Self::run(&["rebase", onto, branch], repo_root)?;
        Ok(())
    }

    /// Abort a rebase in progress
    pub fn rebase_abort(repo_root: &Path) -> Result<(), String> {
        Self::run(&["rebase", "--abort"], repo_root)?;
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
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
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

        Git::worktree_add(dir.path(), &wt_path, "test-branch", None).unwrap();

        assert!(wt_path.exists());
        assert!(wt_path.is_dir());
        assert_eq!(Git::current_branch(&wt_path).unwrap(), "test-branch");
    }

    #[test]
    fn worktree_add_fails_on_duplicate_branch() {
        let dir = init_test_repo();
        let wt1 = dir.path().join("wt1");
        let wt2 = dir.path().join("wt2");

        Git::worktree_add(dir.path(), &wt1, "same-branch", None).unwrap();
        assert!(Git::worktree_add(dir.path(), &wt2, "same-branch", None).is_err());
    }

    #[test]
    fn worktree_remove_cleans_up() {
        let dir = init_test_repo();
        let wt_path = dir.path().join("wt-remove");

        Git::worktree_add(dir.path(), &wt_path, "remove-branch", None).unwrap();
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
    fn merge_squash_creates_single_commit() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        Git::run(&["checkout", "-b", "feature-squash"], dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "first").unwrap();
        Git::run(&["add", "a.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "first commit"], dir.path()).unwrap();
        fs::write(dir.path().join("b.txt"), "second").unwrap();
        Git::run(&["add", "b.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "second commit"], dir.path()).unwrap();

        Git::checkout(dir.path(), &main_branch).unwrap();
        Git::merge_squash(dir.path(), "feature-squash", "squashed: add a and b").unwrap();

        let log = Git::run(&["log", "--oneline", "-1"], dir.path()).unwrap();
        assert!(log.contains("squashed: add a and b"));
        assert!(!log.contains("Merge branch"));
        assert!(dir.path().join("a.txt").exists());
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn merge_squash_conflict_fails() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        fs::write(dir.path().join("conflict.txt"), "main content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "main version").unwrap();

        Git::run(&["checkout", "-b", "squash-conflict", "HEAD~1"], dir.path()).unwrap();
        fs::write(dir.path().join("conflict.txt"), "branch content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "branch version").unwrap();

        Git::checkout(dir.path(), &main_branch).unwrap();
        assert!(Git::merge_squash(dir.path(), "squash-conflict", "should fail").is_err());
    }

    #[test]
    fn has_conflicts_returns_false_on_clean_repo() {
        let dir = init_test_repo();
        assert!(!Git::has_conflicts(dir.path()).unwrap());
    }

    #[test]
    fn status_porcelain_empty_on_clean_repo() {
        let dir = init_test_repo();
        let status = Git::status_porcelain(dir.path()).unwrap();
        assert!(status.is_empty());
    }

    #[test]
    fn status_porcelain_shows_changes() {
        let dir = init_test_repo();
        fs::write(dir.path().join("new.txt"), "hello").unwrap();
        let status = Git::status_porcelain(dir.path()).unwrap();
        assert!(status.contains("new.txt"));
    }

    #[test]
    fn add_all_stages_files() {
        let dir = init_test_repo();
        fs::write(dir.path().join("new.txt"), "hello").unwrap();
        Git::add_all(dir.path()).unwrap();
        let status = Git::run(&["status", "--porcelain"], dir.path()).unwrap();
        assert!(status.contains("A  new.txt") || status.contains("A new.txt"));
    }

    #[test]
    fn commit_with_message_creates_commit() {
        let dir = init_test_repo();
        fs::write(dir.path().join("file.txt"), "content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "test commit").unwrap();
        let log = Git::run(&["log", "--oneline", "-1"], dir.path()).unwrap();
        assert!(log.contains("test commit"));
    }

    #[test]
    fn commit_empty_repo_fails() {
        let dir = init_test_repo();
        assert!(Git::commit(dir.path(), "empty").is_err());
    }

    #[test]
    fn log_oneline_since_returns_commits() {
        let dir = init_test_repo();
        let main = Git::current_branch(dir.path()).unwrap();
        Git::run(&["checkout", "-b", "feature"], dir.path()).unwrap();
        fs::write(dir.path().join("f.txt"), "x").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "feature work").unwrap();
        let log = Git::log_oneline_since(dir.path(), &main).unwrap();
        assert!(log.contains("feature work"));
    }

    #[test]
    fn diff_stat_since_returns_stats() {
        let dir = init_test_repo();
        let main = Git::current_branch(dir.path()).unwrap();
        Git::run(&["checkout", "-b", "feature2"], dir.path()).unwrap();
        fs::write(dir.path().join("g.txt"), "y").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "feature2 work").unwrap();
        let stat = Git::diff_stat_since(dir.path(), &main).unwrap();
        assert!(stat.contains("g.txt"));
    }

    #[test]
    fn run_shell_command_success() {
        let dir = TempDir::new().unwrap();
        let result = Git::run_shell_command(dir.path(), "echo hello").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn run_shell_command_failure() {
        let dir = TempDir::new().unwrap();
        let result = Git::run_shell_command(dir.path(), "sh -c 'echo fail >&2 && exit 1'");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("fail"));
    }

    #[test]
    fn rebase_onto_main_succeeds() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        // Create a commit on main after branching point
        fs::write(dir.path().join("main.txt"), "main content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "main work").unwrap();

        // Create feature branch from before that commit
        Git::run(&["checkout", "-b", "feature-rebase", "HEAD~1"], dir.path()).unwrap();
        fs::write(dir.path().join("feature.txt"), "feature content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "feature work").unwrap();

        // Rebase feature onto main (no conflicts since different files)
        Git::rebase(dir.path(), "feature-rebase", &main_branch).unwrap();
    }

    #[test]
    fn reset_hard_undoes_commit() {
        let dir = init_test_repo();
        let before = Git::run(&["rev-parse", "HEAD"], dir.path()).unwrap();

        fs::write(dir.path().join("reset.txt"), "content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "to be undone").unwrap();

        let after = Git::run(&["rev-parse", "HEAD"], dir.path()).unwrap();
        assert_ne!(before, after);

        Git::reset_hard(dir.path(), "HEAD~1").unwrap();

        let restored = Git::run(&["rev-parse", "HEAD"], dir.path()).unwrap();
        assert_eq!(before, restored);
        assert!(!dir.path().join("reset.txt").exists());
    }

    #[test]
    fn reset_hard_invalid_ref_fails() {
        let dir = init_test_repo();
        assert!(Git::reset_hard(dir.path(), "nonexistent-ref-abc123").is_err());
    }

    #[test]
    fn worktree_add_with_start_point() {
        let dir = init_test_repo();

        // Create a branch with a unique commit
        Git::run(&["checkout", "-b", "parent-branch"], dir.path()).unwrap();
        fs::write(dir.path().join("parent.txt"), "parent content").unwrap();
        Git::run(&["add", "parent.txt"], dir.path()).unwrap();
        Git::run(&["commit", "-m", "parent commit"], dir.path()).unwrap();

        // Go back to main
        Git::checkout(dir.path(), "main").unwrap();

        // Create worktree branching from parent-branch
        let wt_path = dir.path().join("child-wt");
        Git::worktree_add(dir.path(), &wt_path, "child-branch", Some("parent-branch")).unwrap();

        // Verify the worktree exists and has the parent's commit
        assert!(wt_path.exists());
        assert_eq!(Git::current_branch(&wt_path).unwrap(), "child-branch");
        assert!(wt_path.join("parent.txt").exists());
        let content = fs::read_to_string(wt_path.join("parent.txt")).unwrap();
        assert_eq!(content, "parent content");
    }

    #[test]
    fn rebase_conflicting_branch_fails() {
        let dir = init_test_repo();
        let main_branch = Git::current_branch(dir.path()).unwrap();

        // Create conflicting changes
        fs::write(dir.path().join("conflict.txt"), "main content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "main version").unwrap();

        Git::run(&["checkout", "-b", "conflict-rebase", "HEAD~1"], dir.path()).unwrap();
        fs::write(dir.path().join("conflict.txt"), "branch content").unwrap();
        Git::add_all(dir.path()).unwrap();
        Git::commit(dir.path(), "branch version").unwrap();

        // Rebase should fail due to conflicts
        let result = Git::rebase(dir.path(), "conflict-rebase", &main_branch);
        assert!(result.is_err());

        // Clean up
        Git::rebase_abort(dir.path()).unwrap();
    }

    /// Helper: create a test repo with some files in nested directories.
    fn init_test_repo_with_files() -> TempDir {
        let dir = init_test_repo();
        // Create some dirs and files to test sparse checkout
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("docs/readme.md"), "# readme").unwrap();
        fs::write(dir.path().join("root.txt"), "root file").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add files"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn worktree_add_no_checkout_creates_empty_worktree() {
        let dir = init_test_repo_with_files();
        let wt_path = dir.path().join("wt-no-checkout");

        Git::worktree_add_no_checkout(dir.path(), &wt_path, "no-checkout-branch", None).unwrap();

        assert!(wt_path.exists(), "worktree dir should exist");
        assert!(
            !wt_path.join("src/main.rs").exists(),
            "files should NOT be checked out"
        );
        assert!(
            !wt_path.join("root.txt").exists(),
            "root file should NOT be checked out"
        );
        assert_eq!(Git::current_branch(&wt_path).unwrap(), "no-checkout-branch");
    }

    #[test]
    fn sparse_checkout_init_and_set_populates_paths() {
        let dir = init_test_repo_with_files();
        let wt_path = dir.path().join("wt-sparse");

        Git::worktree_add_no_checkout(dir.path(), &wt_path, "sparse-branch", None).unwrap();
        Git::sparse_checkout_init(&wt_path).unwrap();
        Git::sparse_checkout_set(&wt_path, &["src"]).unwrap();
        Git::checkout_populate(&wt_path).unwrap();

        assert!(
            wt_path.join("src/main.rs").exists(),
            "src/ should be checked out"
        );
        assert!(
            !wt_path.join("docs/readme.md").exists(),
            "docs/ should NOT be checked out"
        );
    }

    #[test]
    fn sparse_checkout_list_returns_paths() {
        let dir = init_test_repo_with_files();
        let wt_path = dir.path().join("wt-sparse-list");

        Git::worktree_add_no_checkout(dir.path(), &wt_path, "sparse-list-branch", None).unwrap();
        Git::sparse_checkout_init(&wt_path).unwrap();
        Git::sparse_checkout_set(&wt_path, &["src"]).unwrap();
        Git::checkout_populate(&wt_path).unwrap();

        let paths = Git::sparse_checkout_list(&wt_path).unwrap();
        assert!(
            paths.iter().any(|p| p.contains("src")),
            "sparse list should include 'src'"
        );
    }

    #[test]
    fn sparse_checkout_multiple_paths() {
        let dir = init_test_repo_with_files();
        let wt_path = dir.path().join("wt-multi-sparse");

        Git::worktree_add_no_checkout(dir.path(), &wt_path, "multi-sparse-branch", None).unwrap();
        Git::sparse_checkout_init(&wt_path).unwrap();
        Git::sparse_checkout_set(&wt_path, &["src", "docs"]).unwrap();
        Git::checkout_populate(&wt_path).unwrap();

        assert!(
            wt_path.join("src/main.rs").exists(),
            "src/ should be checked out"
        );
        assert!(
            wt_path.join("docs/readme.md").exists(),
            "docs/ should be checked out"
        );
    }

    #[test]
    fn file_count_returns_tracked_files() {
        let dir = init_test_repo_with_files();
        // init_test_repo_with_files creates: src/main.rs, docs/readme.md, root.txt
        let count = Git::file_count(dir.path()).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn validate_sparse_paths_valid() {
        let dir = init_test_repo_with_files();
        let invalid = Git::validate_sparse_paths(dir.path(), &["src", "docs"]).unwrap();
        assert!(invalid.is_empty());
    }

    #[test]
    fn validate_sparse_paths_invalid() {
        let dir = init_test_repo_with_files();
        let invalid = Git::validate_sparse_paths(dir.path(), &["src", "nonexistent"]).unwrap();
        assert_eq!(invalid, vec!["nonexistent"]);
    }

    #[test]
    fn validate_sparse_paths_all_invalid() {
        let dir = init_test_repo_with_files();
        let invalid = Git::validate_sparse_paths(dir.path(), &["nope", "also-nope"]).unwrap();
        assert_eq!(invalid, vec!["nope", "also-nope"]);
    }
}
