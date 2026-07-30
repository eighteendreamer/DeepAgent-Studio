//! Worktree model tools (`enter_worktree` / `exit_worktree`).
//!
//! Aligned with Claude Code's `EnterWorktreeTool` / `ExitWorktreeTool`
//! (restored-src/src/tools/EnterWorktreeTool, ExitWorktreeTool):
//! - `enter_worktree` creates an isolated git worktree on a fresh branch based
//!   on HEAD, and records it as the session's active worktree.
//! - `exit_worktree` ends that session with `action: keep | remove`; `remove`
//!   refuses when uncommitted files or unmerged commits exist unless the model
//!   explicitly passes `discard_changes: true` (CC's sole destructive gate).
//! - Only a worktree created by `enter_worktree` *in this run* can be exited
//!   (CC's `getCurrentWorktreeSession` entry gate).
//!
//! Divergences from CC (documented, architecture-driven):
//! - CC switches the whole session cwd (`process.chdir`) into the worktree.
//!   This runtime roots every tool at the fixed [`WorkspaceRoot`], so instead
//!   the worktree lives *inside* the workspace at `.deepagent/worktrees/<name>`
//!   (CC uses the in-repo `.claude/worktrees/` for the same reason) and the
//!   model works in it via relative paths. The superpowers
//!   `using-git-worktrees` skill mandates the ignore check before creating an
//!   in-repo worktree; we append to `.gitignore` when missing.
//! - Worktree session state is per-run (not persisted across resume). CC
//!   persists it to the transcript; registered as a follow-up.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

use crate::bash_tool::CommandExecutor;

/// Canonical name of the enter tool.
pub const ENTER_WORKTREE_TOOL_NAME: &str = "enter_worktree";
/// Canonical name of the exit tool.
pub const EXIT_WORKTREE_TOOL_NAME: &str = "exit_worktree";

/// Where managed worktrees live, relative to the workspace root (in-repo,
/// mirroring CC's `.claude/worktrees/`).
const WORKTREE_DIR: &str = ".deepagent/worktrees";
/// Branch prefix for managed worktree branches.
const BRANCH_PREFIX: &str = "wt/";
/// Cap on the uncommitted-file listing echoed back on a refused remove.
const MAX_LISTED_FILES: usize = 20;

/// The active worktree created by `enter_worktree` in this run.
#[derive(Debug, Clone)]
pub struct ActiveWorktree {
    /// Validated worktree name (directory + branch suffix).
    pub name: String,
    /// Absolute worktree path.
    pub path: String,
    /// Branch created for the worktree (`wt/<name>`).
    pub branch: String,
    /// HEAD commit the worktree was based on (for unmerged detection).
    pub base_commit: String,
}

/// Shared per-run worktree session state (CC `getCurrentWorktreeSession`):
/// `exit_worktree` only operates on a worktree recorded here.
pub type WorktreeSessionState = Arc<StdMutex<Option<ActiveWorktree>>>;

/// Create a fresh empty session state shared by the enter/exit tool pair.
pub fn worktree_session_state() -> WorktreeSessionState {
    Arc::new(StdMutex::new(None))
}

/// CC `validateWorktreeSlug` equivalent: letters, digits, dots, underscores
/// and dashes per segment; we keep a single flat segment and 64-char cap.
fn validate_worktree_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("worktree name must be 1-64 characters".to_string());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(
            "worktree name may contain only letters, digits, dots, underscores and dashes"
                .to_string(),
        );
    }
    if name.starts_with('.') {
        return Err("worktree name must not start with a dot".to_string());
    }
    Ok(())
}

/// Random fallback name when the model omits one (CC generates one too).
fn generated_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() << 20))
        .unwrap_or(0);
    format!("wt-{nanos:08x}")
}

/// Run a git command via the executor, mapping transport errors to failures.
async fn git<E: CommandExecutor>(
    executor: &E,
    cwd: &str,
    command: &str,
) -> std::result::Result<(i32, String, String), String> {
    match executor.run(command, cwd).await {
        Ok(out) => Ok((
            out.exit_code.unwrap_or(-1),
            out.stdout.trim().to_string(),
            out.stderr.trim().to_string(),
        )),
        Err(e) => Err(format!("failed to run `{command}`: {e}")),
    }
}

/// `enter_worktree`: create an isolated git worktree and record it as the
/// run's active worktree session.
pub struct EnterWorktreeTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
    state: WorktreeSessionState,
}

impl<E: CommandExecutor> EnterWorktreeTool<E> {
    /// Build rooted at `cwd` (the workspace/repo directory) with the shared
    /// session `state`.
    pub fn new(executor: E, cwd: impl Into<String>, state: WorktreeSessionState) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
            state,
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for EnterWorktreeTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: ENTER_WORKTREE_TOOL_NAME.into(),
            description: "Create an isolated git worktree (new branch based on HEAD) under \
                          .deepagent/worktrees/ and make it this session's active worktree. Use \
                          ONLY when the user explicitly asks to work in a worktree. Work inside \
                          it via its returned path; finish with exit_worktree. Args: { name?: \
                          string }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Optional worktree name (letters, digits, dots, underscores, dashes; max 64). Generated if omitted."
                    }
                }
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        if let Some(active) = self.state.lock().ok().and_then(|s| s.clone()) {
            return Ok(ToolOutput::failure(format!(
                "already in a worktree session at {}; call exit_worktree first",
                active.path
            )));
        }

        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(raw) => {
                let trimmed = raw.trim().to_string();
                if let Err(reason) = validate_worktree_name(&trimmed) {
                    return Ok(ToolOutput::failure(reason));
                }
                trimmed
            }
            None => generated_name(),
        };

        // Must be a git repository with at least one commit (branch base).
        match git(
            &self.executor,
            &self.cwd,
            "git rev-parse --is-inside-work-tree",
        )
        .await
        {
            Ok((0, _, _)) => {}
            Ok((_, _, stderr)) => {
                return Ok(ToolOutput::failure(format!(
                    "not a git repository: {stderr}"
                )))
            }
            Err(e) => return Ok(ToolOutput::failure(e)),
        }
        let base_commit = match git(&self.executor, &self.cwd, "git rev-parse HEAD").await {
            Ok((0, stdout, _)) => stdout,
            Ok((_, _, stderr)) => {
                return Ok(ToolOutput::failure(format!(
                    "repository has no commits yet (worktrees need a HEAD): {stderr}"
                )))
            }
            Err(e) => return Ok(ToolOutput::failure(e)),
        };

        // Ignore check (superpowers using-git-worktrees skill: MUST verify the
        // in-repo worktree dir is ignored; append to .gitignore when missing).
        let mut gitignore_note = String::new();
        match git(
            &self.executor,
            &self.cwd,
            &format!("git check-ignore -q {WORKTREE_DIR}"),
        )
        .await
        {
            Ok((0, _, _)) => {}
            Ok(_) => {
                let gitignore = std::path::Path::new(&self.cwd).join(".gitignore");
                let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
                let mut updated = existing.clone();
                if !updated.is_empty() && !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str(WORKTREE_DIR);
                updated.push_str("/\n");
                match std::fs::write(&gitignore, updated) {
                    Ok(()) => {
                        gitignore_note =
                            format!(" Added `{WORKTREE_DIR}/` to .gitignore (was not ignored).")
                    }
                    Err(e) => {
                        gitignore_note = format!(
                            " WARNING: `{WORKTREE_DIR}/` is not git-ignored and .gitignore could \
                             not be updated ({e}); add it manually."
                        )
                    }
                }
            }
            Err(e) => return Ok(ToolOutput::failure(e)),
        }

        let rel_path = format!("{WORKTREE_DIR}/{name}");
        let branch = format!("{BRANCH_PREFIX}{name}");
        let command = format!("git worktree add \"{rel_path}\" -b \"{branch}\"");
        match git(&self.executor, &self.cwd, &command).await {
            Ok((0, _, _)) => {}
            Ok((code, _, stderr)) => {
                return Ok(ToolOutput::failure(format!(
                    "git worktree add failed (exit {code}): {stderr}"
                )))
            }
            Err(e) => return Ok(ToolOutput::failure(e)),
        }

        let abs_path = std::path::Path::new(&self.cwd)
            .join(WORKTREE_DIR)
            .join(&name)
            .to_string_lossy()
            .to_string();
        let active = ActiveWorktree {
            name: name.clone(),
            path: abs_path.clone(),
            branch: branch.clone(),
            base_commit,
        };
        if let Ok(mut slot) = self.state.lock() {
            *slot = Some(active);
        }

        Ok(ToolOutput::success(serde_json::json!({
            "worktree_path": abs_path,
            "relative_path": rel_path,
            "branch": branch,
            "message": format!(
                "Created worktree at {rel_path} on branch {branch}. Work inside it using that \
                 path (e.g. `cd {rel_path} && ...` in bash, or path-prefixed file tools). Finish \
                 with exit_worktree (keep or remove).{gitignore_note}"
            ),
        })))
    }
}

/// `exit_worktree`: end the active worktree session (`keep` leaves it on
/// disk; `remove` deletes worktree + branch, gated by `discard_changes`).
pub struct ExitWorktreeTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
    state: WorktreeSessionState,
}

impl<E: CommandExecutor> ExitWorktreeTool<E> {
    /// Build rooted at `cwd` with the shared session `state`.
    pub fn new(executor: E, cwd: impl Into<String>, state: WorktreeSessionState) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
            state,
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for ExitWorktreeTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: EXIT_WORKTREE_TOOL_NAME.into(),
            description: "End the worktree session created by enter_worktree. Args: { action: \
                          \"keep\" | \"remove\", discard_changes?: bool }. \"keep\" leaves the \
                          worktree and branch on disk; \"remove\" deletes both and refuses when \
                          uncommitted files or unmerged commits exist unless discard_changes is \
                          true."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["keep", "remove"],
                        "description": "\"keep\" leaves the worktree and branch on disk; \"remove\" deletes both."
                    },
                    "discard_changes": {
                        "type": "boolean",
                        "description": "Required true when action is \"remove\" and the worktree has uncommitted files or unmerged commits."
                    }
                },
                "required": ["action"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(active) = self.state.lock().ok().and_then(|s| s.clone()) else {
            return Ok(ToolOutput::failure(
                "no active worktree session; only a worktree created by enter_worktree in this \
                 run can be exited",
            ));
        };
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match action {
            "keep" => {
                if let Ok(mut slot) = self.state.lock() {
                    *slot = None;
                }
                Ok(ToolOutput::success(serde_json::json!({
                    "action": "keep",
                    "worktree_path": active.path,
                    "branch": active.branch,
                    "message": format!(
                        "Left the worktree session; {} and branch {} remain on disk.",
                        active.path, active.branch
                    ),
                })))
            }
            "remove" => self.remove(active, &args).await,
            other => Ok(ToolOutput::failure(format!(
                "invalid action '{other}': expected \"keep\" or \"remove\""
            ))),
        }
    }
}

impl<E: CommandExecutor> ExitWorktreeTool<E> {
    async fn remove(&self, active: ActiveWorktree, args: &serde_json::Value) -> Result<ToolOutput> {
        let discard = args
            .get("discard_changes")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // CC gate: refuse `remove` while uncommitted files or unmerged
        // commits exist, unless discard_changes is explicitly true.
        let (_, porcelain, _) =
            match git(&self.executor, &active.path, "git status --porcelain").await {
                Ok(out) => out,
                Err(e) => return Ok(ToolOutput::failure(e)),
            };
        let uncommitted: Vec<&str> = porcelain.lines().filter(|l| !l.trim().is_empty()).collect();

        let unmerged_commits = match git(
            &self.executor,
            &self.cwd,
            &format!(
                "git rev-list --count {}..{}",
                active.base_commit, active.branch
            ),
        )
        .await
        {
            Ok((0, stdout, _)) => {
                let ahead: u64 = stdout.parse().unwrap_or(0);
                if ahead == 0 {
                    0
                } else {
                    // Ahead of base: unmerged unless another branch already
                    // contains the worktree branch tip.
                    let (_, containing, _) = git(
                        &self.executor,
                        &self.cwd,
                        &format!(
                            "git branch --format=%(refname:short) --contains {}",
                            active.branch
                        ),
                    )
                    .await
                    .unwrap_or((0, String::new(), String::new()));
                    let merged_elsewhere = containing
                        .lines()
                        .map(str::trim)
                        .any(|b| !b.is_empty() && b != active.branch);
                    if merged_elsewhere {
                        0
                    } else {
                        ahead
                    }
                }
            }
            _ => 0,
        };

        if (!uncommitted.is_empty() || unmerged_commits > 0) && !discard {
            let listed: Vec<&str> = uncommitted.iter().take(MAX_LISTED_FILES).copied().collect();
            return Ok(ToolOutput {
                ok: false,
                value: serde_json::json!({
                    "error": "worktree has uncommitted files or unmerged commits; pass discard_changes: true to remove anyway",
                    "uncommitted_files": listed,
                    "uncommitted_count": uncommitted.len(),
                    "unmerged_commits": unmerged_commits,
                }),
                truncated: uncommitted.len() > MAX_LISTED_FILES,
            });
        }

        let remove_cmd = format!("git worktree remove --force \"{}\"", active.path);
        match git(&self.executor, &self.cwd, &remove_cmd).await {
            Ok((0, _, _)) => {}
            Ok((code, _, stderr)) => {
                return Ok(ToolOutput::failure(format!(
                    "git worktree remove failed (exit {code}): {stderr}"
                )))
            }
            Err(e) => return Ok(ToolOutput::failure(e)),
        }
        // Branch + admin cleanup are best-effort once the worktree is gone.
        let _ = git(
            &self.executor,
            &self.cwd,
            &format!("git branch -D \"{}\"", active.branch),
        )
        .await;
        let _ = git(&self.executor, &self.cwd, "git worktree prune").await;

        if let Ok(mut slot) = self.state.lock() {
            *slot = None;
        }
        Ok(ToolOutput::success(serde_json::json!({
            "action": "remove",
            "worktree_path": active.path,
            "branch": active.branch,
            "discarded_files": uncommitted.len(),
            "discarded_commits": unmerged_commits,
            "message": format!(
                "Removed worktree {} and branch {}.",
                active.path, active.branch
            ),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash_tool::SystemExecutor;

    /// Init a git repo with one commit in a tempdir; returns (dir, cwd string).
    fn git_repo() -> Option<(tempfile::TempDir, String)> {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(args)
                .output()
                .ok()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !run(&["init", "-q"]) {
            return None; // no git available — skip
        }
        assert!(run(&["config", "user.email", "test@example.com"]));
        assert!(run(&["config", "user.name", "test"]));
        assert!(run(&["commit", "--allow-empty", "-q", "-m", "init"]));
        Some((dir, cwd))
    }

    fn tools(
        cwd: &str,
    ) -> (
        EnterWorktreeTool<SystemExecutor>,
        ExitWorktreeTool<SystemExecutor>,
    ) {
        let state = worktree_session_state();
        (
            EnterWorktreeTool::new(SystemExecutor, cwd, state.clone()),
            ExitWorktreeTool::new(SystemExecutor, cwd, state),
        )
    }

    #[test]
    fn name_validation() {
        assert!(validate_worktree_name("feature-1_x.2").is_ok());
        assert!(validate_worktree_name("").is_err());
        assert!(validate_worktree_name(".hidden").is_err());
        assert!(validate_worktree_name("bad/slash").is_err());
        assert!(validate_worktree_name(&"a".repeat(65)).is_err());
    }

    #[tokio::test]
    async fn enter_creates_worktree_and_rejects_reentry() {
        let Some((_dir, cwd)) = git_repo() else {
            return;
        };
        let (enter, _exit) = tools(&cwd);

        let out = enter
            .invoke(serde_json::json!({ "name": "feat-a" }))
            .await
            .unwrap();
        assert!(out.ok, "enter failed: {}", out.value);
        assert_eq!(out.value["branch"], "wt/feat-a");
        let wt_path = out.value["worktree_path"].as_str().unwrap().to_string();
        assert!(std::path::Path::new(&wt_path).is_dir());
        // The in-repo worktree dir must be git-ignored after enter.
        let ignored = std::process::Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args(["check-ignore", "-q", WORKTREE_DIR])
            .status()
            .unwrap()
            .success();
        assert!(ignored, "worktree dir must be ignored after enter");

        // Second enter while a session is active must fail (CC gate).
        let again = enter.invoke(serde_json::json!({})).await.unwrap();
        assert!(!again.ok);
    }

    #[tokio::test]
    async fn exit_keep_clears_session_and_leaves_worktree() {
        let Some((_dir, cwd)) = git_repo() else {
            return;
        };
        let (enter, exit) = tools(&cwd);
        let out = enter
            .invoke(serde_json::json!({ "name": "keepme" }))
            .await
            .unwrap();
        assert!(out.ok);
        let wt_path = out.value["worktree_path"].as_str().unwrap().to_string();

        let kept = exit
            .invoke(serde_json::json!({ "action": "keep" }))
            .await
            .unwrap();
        assert!(kept.ok, "{}", kept.value);
        assert!(
            std::path::Path::new(&wt_path).is_dir(),
            "keep must not delete"
        );

        // Session is closed: exiting again fails.
        let again = exit
            .invoke(serde_json::json!({ "action": "keep" }))
            .await
            .unwrap();
        assert!(!again.ok);
    }

    #[tokio::test]
    async fn remove_refuses_dirty_worktree_then_discards_explicitly() {
        let Some((_dir, cwd)) = git_repo() else {
            return;
        };
        let (enter, exit) = tools(&cwd);
        let out = enter
            .invoke(serde_json::json!({ "name": "dirty" }))
            .await
            .unwrap();
        assert!(out.ok, "{}", out.value);
        let wt_path = out.value["worktree_path"].as_str().unwrap().to_string();
        std::fs::write(std::path::Path::new(&wt_path).join("draft.txt"), "wip").unwrap();

        // Refused without discard_changes (CC contract), listing the file.
        let refused = exit
            .invoke(serde_json::json!({ "action": "remove" }))
            .await
            .unwrap();
        assert!(!refused.ok);
        assert_eq!(refused.value["uncommitted_count"], 1);

        // Explicit discard removes worktree + branch.
        let removed = exit
            .invoke(serde_json::json!({ "action": "remove", "discard_changes": true }))
            .await
            .unwrap();
        assert!(removed.ok, "{}", removed.value);
        assert!(!std::path::Path::new(&wt_path).exists());
        let branch_gone = !std::process::Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args(["rev-parse", "--verify", "-q", "wt/dirty"])
            .status()
            .unwrap()
            .success();
        assert!(branch_gone, "branch must be deleted on remove");
    }

    #[tokio::test]
    async fn remove_clean_worktree_needs_no_discard() {
        let Some((_dir, cwd)) = git_repo() else {
            return;
        };
        let (enter, exit) = tools(&cwd);
        assert!(
            enter
                .invoke(serde_json::json!({ "name": "clean" }))
                .await
                .unwrap()
                .ok
        );
        let removed = exit
            .invoke(serde_json::json!({ "action": "remove" }))
            .await
            .unwrap();
        assert!(removed.ok, "{}", removed.value);
        assert_eq!(removed.value["discarded_files"], 0);
    }

    #[tokio::test]
    async fn enter_fails_outside_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let (enter, _exit) = tools(&cwd);
        let out = enter.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok, "must fail outside a git repository");
    }

    #[tokio::test]
    async fn exit_without_session_fails() {
        let Some((_dir, cwd)) = git_repo() else {
            return;
        };
        let (_enter, exit) = tools(&cwd);
        let out = exit
            .invoke(serde_json::json!({ "action": "remove" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
