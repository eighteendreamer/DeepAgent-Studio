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

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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

/// A single entry from `git status --porcelain=v1 -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusEntry {
    /// The two-column porcelain status (for example, ` M` or `??`).
    pub status: String,
    /// The current path, relative to the repository root.
    pub path: String,
    /// The previous path for a rename or copy entry, when Git reports one.
    pub old_path: Option<String>,
}

/// Parsed Git status facts shared by UI and model-context projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusSnapshot {
    /// Whether Git reported any status record, independent of display limits.
    pub has_uncommitted: bool,
    /// The status records parsed from the NUL-delimited output.
    pub entries: Vec<GitStatusEntry>,
}

/// Max changed files / recent commits surfaced (keeps the prompt block small).
const MAX_CHANGED_FILES: usize = 10;
const MAX_RECENT_COMMITS: usize = 3;

/// Run `git <args>` in `root`, returning trimmed stdout on a clean exit, else
/// `None`. Best-effort: any spawn/exec failure is swallowed.
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    git_stdout(root, args).map(|stdout| stdout.trim().to_string())
}

/// Run `git <args>` in `root`, preserving stdout byte/record boundaries.
///
/// Porcelain status output is a machine-readable two-column format. It must
/// not share the trimmed single-value helper above because the leading space
/// in an unstaged record is meaningful.
fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(root);
    configure_hidden_process(&mut cmd);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Detect the git context at `root`. Returns `None` when `git` is unavailable
/// or `root` is not a repository.
pub fn detect_git_context(root: &Path) -> Option<GitContext> {
    // One query supplies repository detection, branch and worktree changes.
    // v2's branch header also works before the first commit. A failed status
    // query remains unknown rather than being projected as a clean worktree.
    let status = git_stdout(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    let mut context = parse_context_status(&status)?;

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
    context.recent_commits = log
        .lines()
        .filter_map(|line| {
            let (hash, subject) = line.split_once('\x1f')?;
            Some(CommitSummary {
                hash: hash.trim().to_string(),
                subject: subject.trim().to_string(),
            })
        })
        .collect();

    if context.branch == "(detached)" {
        context.branch = context
            .recent_commits
            .first()
            .map(|commit| commit.hash.clone())
            .unwrap_or_else(|| "(unknown)".into());
    }
    Some(context)
}

/// Parse Git's porcelain v2 records; paths can contain spaces, tabs or newlines.
fn parse_context_status(output: &str) -> Option<GitContext> {
    let mut branch = None;
    let mut changed_files = Vec::new();
    let mut has_uncommitted = false;
    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix("# branch.head ") {
            branch = Some(head.to_string());
            continue;
        }
        let path = match record.as_bytes().first() {
            Some(b'1') => record.splitn(9, ' ').nth(8),
            Some(b'2') => {
                let path = record.splitn(10, ' ').nth(9);
                records.next(); // The rename/copy source is a separate NUL record.
                path
            }
            Some(b'u') => record.splitn(11, ' ').nth(10),
            Some(b'?') => record.strip_prefix("? "),
            _ => continue,
        }?;
        has_uncommitted = true;
        if changed_files.len() < MAX_CHANGED_FILES {
            changed_files.push(path.to_string());
        }
    }
    Some(GitContext {
        branch: branch?,
        has_uncommitted,
        changed_files,
        recent_commits: Vec::new(),
    })
}

/// Parse NUL-delimited porcelain v1 status without trimming the status column.
pub fn parse_git_status_porcelain_z(output: &str) -> GitStatusSnapshot {
    let has_uncommitted = !output.is_empty();
    let mut entries = output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .peekable();

    let mut parsed = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry.as_bytes()[2] != b' ' {
            continue;
        }

        let status = entry[..2].to_string();
        let path = entry[3..].to_string();
        let old_path = if status.contains('R') || status.contains('C') {
            entries.next().map(str::to_string)
        } else {
            None
        };
        parsed.push(GitStatusEntry {
            status,
            path,
            old_path,
        });
    }

    GitStatusSnapshot {
        has_uncommitted,
        entries: parsed,
    }
}

fn configure_hidden_process(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
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

    #[test]
    fn context_includes_the_branch_before_the_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let context = detect_git_context(dir.path()).unwrap();
        assert_eq!(context.branch, "main");
        assert!(context.recent_commits.is_empty());
        assert!(!context.has_uncommitted);
    }

    #[test]
    fn detached_context_and_renamed_paths_are_preserved_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("old.txt"), "fixture").unwrap();
        git(root, &["add", "old.txt"]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["checkout", "-q", "--detach"]);
        git(root, &["mv", "old.txt", "中文 renamed.txt"]);
        std::fs::write(root.join("nested/untracked file.txt"), "fixture").unwrap();
        let context = detect_git_context(&root.join("nested")).unwrap();
        assert_eq!(
            context.branch,
            git_output(root, &["rev-parse", "--short", "HEAD"]).unwrap()
        );
        assert!(context.has_uncommitted);
        assert_eq!(
            context.changed_files,
            ["中文 renamed.txt", "nested/untracked file.txt"]
        );
        assert_eq!(context.recent_commits[0].subject, "baseline");
    }

    #[test]
    fn parses_porcelain_status_without_losing_leading_state_or_paths() {
        let snapshot = parse_git_status_porcelain_z(
            " M a\0 D src/main.rs\0?? 中文 空格.txt\0R  new name.rs\0old name.rs\0",
        );

        assert!(snapshot.has_uncommitted);
        assert_eq!(
            snapshot.entries,
            vec![
                GitStatusEntry {
                    status: " M".to_string(),
                    path: "a".to_string(),
                    old_path: None,
                },
                GitStatusEntry {
                    status: " D".to_string(),
                    path: "src/main.rs".to_string(),
                    old_path: None,
                },
                GitStatusEntry {
                    status: "??".to_string(),
                    path: "中文 空格.txt".to_string(),
                    old_path: None,
                },
                GitStatusEntry {
                    status: "R ".to_string(),
                    path: "new name.rs".to_string(),
                    old_path: Some("old name.rs".to_string()),
                },
            ]
        );
    }

    #[test]
    fn porcelain_status_empty_is_clean() {
        let snapshot = parse_git_status_porcelain_z("");
        assert!(!snapshot.has_uncommitted);
        assert!(snapshot.entries.is_empty());
    }
}
