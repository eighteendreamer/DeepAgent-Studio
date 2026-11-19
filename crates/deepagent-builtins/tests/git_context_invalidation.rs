//! Exercise real mutations rather than a synthetic cache invalidation hook.

use std::path::Path;
use std::process::Command;

use deepagent_builtins::bash_tool::{BashTool, SystemExecutor};
use deepagent_builtins::file_tools::{DeletePathTool, MovePathTool, WriteFileTool};
use deepagent_builtins::git_tools::GitCommitTool;
use deepagent_builtins::WorkspaceRoot;
use deepagent_tools::Tool;
use deepagent_workspace::cached_git_context;
use serde_json::json;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q", "-b", "main"]);
    git(root.path(), &["config", "user.name", "CacheFixture"]);
    git(
        root.path(),
        &["config", "user.email", "cache@example.invalid"],
    );
    git(root.path(), &["config", "core.autocrlf", "false"]);
    git(
        root.path(),
        &["commit", "--allow-empty", "-q", "-m", "initial"],
    );
    root
}

#[tokio::test]
async fn native_file_write_move_and_delete_refresh_the_next_snapshot() {
    let dir = repository();
    let root = dir.path();
    let workspace = WorkspaceRoot::new(root);
    assert!(!cached_git_context(root).unwrap().has_uncommitted);
    let write = WriteFileTool::new(workspace.clone())
        .invoke(json!({
            "path":"nested/new.txt", "content":"cache fixture"
        }))
        .await
        .unwrap();
    assert!(write.ok, "{write:?}");
    assert_eq!(
        cached_git_context(root).unwrap().changed_files,
        ["nested/new.txt"]
    );

    let moved = MovePathTool::new(workspace.clone())
        .invoke(json!({
            "source":"nested/new.txt", "destination":"moved.txt"
        }))
        .await
        .unwrap();
    assert!(moved.ok, "{moved:?}");
    assert_eq!(
        cached_git_context(root).unwrap().changed_files,
        ["moved.txt"]
    );

    let deleted = DeletePathTool::new(workspace)
        .invoke(json!({"path":"moved.txt"}))
        .await
        .unwrap();
    assert!(deleted.ok, "{deleted:?}");
    assert!(!cached_git_context(root).unwrap().has_uncommitted);
}

#[tokio::test]
async fn shell_failure_after_a_git_mutation_still_refreshes_the_snapshot() {
    let dir = repository();
    let root = dir.path();
    assert_eq!(cached_git_context(root).unwrap().branch, "main");
    let shell = if cfg!(windows) { "powershell" } else { "sh" };
    let tool = BashTool::new(SystemExecutor, root.display().to_string(), Vec::new())
        .with_external_safety_gate(true);
    let output = tool
        .invoke(json!({
            "command":"git switch -c changed; exit 7", "shell": shell
        }))
        .await
        .unwrap();
    assert!(!output.ok);
    assert_eq!(cached_git_context(root).unwrap().branch, "changed");
}

#[tokio::test]
async fn git_commit_tool_refreshes_status_and_recent_commits() {
    let dir = repository();
    let root = dir.path();
    std::fs::write(root.join("new.txt"), "fixture").unwrap();
    assert!(cached_git_context(root).unwrap().has_uncommitted);
    let output = GitCommitTool::new(SystemExecutor, root.display().to_string())
        .invoke(json!({"message":"cache commit"}))
        .await
        .unwrap();
    assert!(output.ok, "{output:?}");
    let snapshot = cached_git_context(root).unwrap();
    assert!(!snapshot.has_uncommitted);
    assert_eq!(snapshot.recent_commits[0].subject, "cache commit");
}
