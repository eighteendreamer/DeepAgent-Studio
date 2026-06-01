//! Git context detection (gap-closure spec, Phase 2A).
//!
//! Shells out to the `git` CLI (no `libgit2` dependency, so the crate stays
//! light and offline-friendly) to read a compact snapshot of the repository
//! state at `root`: current branch, whether there are uncommitted changes, the
//! changed file paths, and the most recent commits. This is injected into the
//! system prompt's DYNAMIC section so the agent reasons with VCS state in view,
//! mirroring Claude Code's git awareness.
//!
//! Every call is best-effort: a missing `git`, a non-repository directory, or
//! any command failure yields `None`/empty rather than an error, so a project
//! without git behaves exactly as before (Property 7: backward compatible).

use std::path::Path;
use std::process::Command;

/// A one-line summary of a commit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitSummary {
    /// Abbreviated commit hash.
    pub hash: String,
    /// Commit subject (first line of the message).
    pub subject: String,
}

/// A compact snapshot of the repository state at a workspace root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitContext {
    /// Current branch name (or a detached-HEAD short hash).
    pub branch: String,
    /// Whether the working tree has uncommitted changes (staged or unstaged).
    pub has_uncommitted: bool,
    /// Paths (relative to the repo root) with uncommitted changes, capped.
    pub changed_files: Vec<String>,
    /// The most recent commits (newest first), capped.
    pub recent_commits: Vec<CommitSummary>,
}

/// Max changed files / recent commits surfaced (keeps the prompt block small).
const MAX_CHANGED_FILES: usize = 10;
const MAX_RECENT_COMMITS: usize = 3;

/// Run `git <args>` in `root`, returning trimmed stdout on a clean exit, else
/// `None`. Best-effort: any spawn/exec failure is swallowed.
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether `root` is inside a git work tree.
fn is_git_repo(root: &Path) -> bool {
    git_output(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s == "true")
        .unwrap_or(false)
}

/// Detect the git context at `root`. Returns `None` when `git` is unavailable
/// or `root` is not a repository.
pub fn detect_git_context(root: &Path) -> Option<GitContext> {
    if !is_git_repo(root) {
        return None;
    }

    // Branch name (detached HEAD prints "HEAD"; fall back to a short hash).
    let branch = match git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(b) if b != "HEAD" && !b.is_empty() => b,
        _ => git_output(root, &["rev-parse", "--short", "HEAD"])
            .unwrap_or_else(|| "(unknown)".to_string()),
    };

    // Porcelain status → changed files (each line: "XY <path>").
    let status = git_output(root, &["status", "--porcelain"]).unwrap_or_default();
    let mut changed_files: Vec<String> = status
        .lines()
        .filter_map(|line| {
            let path = line.get(3..).unwrap_or("").trim();
            if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            }
        })
        .collect();
    let has_uncommitted = !changed_files.is_empty();
    changed_files.truncate(MAX_CHANGED_FILES);

    // Recent commits: "<short-hash>\x1f<subject>" per line.
    let log = git_output(
        root,
        &[
            "log",
            &format!("-{MAX_RECENT_COMMITS}"),
            "--pretty=format:%h\x1f%s",
        ],
    )
    .unwrap_or_default();
    let recent_commits: Vec<CommitSummary> = log
        .lines()
        .filter_map(|line| {
            let (hash, subject) = line.split_once('\x1f')?;
            Some(CommitSummary {
                hash: hash.trim().to_string(),
                subject: subject.trim().to_string(),
            })
        })
        .collect();

    Some(GitContext {
        branch,
        has_uncommitted,
        changed_files,
        recent_commits,
    })
}

impl GitContext {
    /// Render a compact prompt block describing the git state, suitable for
    /// appending to the system prompt's DYNAMIC section. Returns an empty
    /// string only when there is genuinely nothing to say (never the case once
    /// a branch is known).
    pub fn to_prompt_block(&self) -> String {
        let mut s = String::from("# Git\n");
        s.push_str(&format!("- Branch: {}\n", self.branch));
        s.push_str(&format!(
            "- Working tree: {}\n",
            if self.has_uncommitted {
                "uncommitted changes present"
            } else {
                "clean"
            }
        ));
        if !self.changed_files.is_empty() {
            s.push_str("- Changed files:\n");
            for f in &self.changed_files {
                s.push_str(&format!("  - {f}\n"));
            }
        }
        if !self.recent_commits.is_empty() {
            s.push_str("- Recent commits:\n");
            for c in &self.recent_commits {
                s.push_str(&format!("  - {} {}\n", c.hash, c.subject));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Run a git command in `dir`, asserting success (test setup only).
    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed");
    }

    /// Whether `git` is available on PATH (CI/dev boxes have it; skip if not).
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "t@t.dev"]);
        git(dir, &["config", "user.name", "Tester"]);
        // Ensure a deterministic default branch name across git versions.
        git(dir, &["checkout", "-q", "-b", "main"]);
    }

    #[test]
    fn non_repo_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        // A fresh temp dir is not a git repo (unless the temp root itself is one,
        // which is not the case on CI). Skip if git is missing.
        if !git_available() {
            return;
        }
        assert!(detect_git_context(dir.path()).is_none());
    }

    #[test]
    fn detects_branch_changes_and_commits() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);

        std::fs::write(root.join("a.txt"), "hello").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "first commit"]);

        // Now make an uncommitted change.
        std::fs::write(root.join("b.txt"), "world").unwrap();

        let ctx = detect_git_context(root).expect("should detect repo");
        assert_eq!(ctx.branch, "main");
        assert!(ctx.has_uncommitted);
        assert!(ctx.changed_files.iter().any(|f| f.contains("b.txt")));
        assert_eq!(ctx.recent_commits.len(), 1);
        assert_eq!(ctx.recent_commits[0].subject, "first commit");

        let block = ctx.to_prompt_block();
        assert!(block.contains("Branch: main"));
        assert!(block.contains("first commit"));
    }

    #[test]
    fn clean_tree_has_no_changes() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("a.txt"), "hello").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "only commit"]);

        let ctx = detect_git_context(root).expect("repo");
        assert!(!ctx.has_uncommitted);
        assert!(ctx.changed_files.is_empty());
    }
}
