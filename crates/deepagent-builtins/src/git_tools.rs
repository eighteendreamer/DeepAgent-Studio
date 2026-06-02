//! Git built-in tools (gap-closure spec, Phase 2A).
//!
//! Read-only inspection tools (`git_status`, `git_diff`, `git_log`) and one
//! workspace-mutating tool (`git_commit`). They shell out via the same
//! [`CommandExecutor`] abstraction as the `bash` tool, so tests run offline and
//! the runtime can sandbox later.
//!
//! Safety model:
//! - `git_status` / `git_diff` / `git_log` are [`RiskLevel::Safe`], read-only.
//! - `git_commit` is [`RiskLevel::Low`] and requires [`Permission::WorkspaceWrite`]
//!   (a local, reversible operation).
//! - `git push` is deliberately NOT offered as a built-in: pushing to a remote
//!   needs [`Permission::GitPush`], which the default developer permission set
//!   does not grant. The model can still attempt it through `bash`, where the
//!   danger classifier forces explicit approval.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

use crate::bash_tool::CommandExecutor;

/// Cap on captured output bytes per git tool (keeps diffs/logs bounded).
const MAX_OUTPUT_BYTES: usize = 16_000;

/// Truncate `s` to [`MAX_OUTPUT_BYTES`], appending a notice when cut.
fn cap(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    let mut out = s;
    // Truncate on a char boundary.
    let mut end = MAX_OUTPUT_BYTES;
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.push_str("\n... (output truncated; refine the query or use bash for full output)");
    out
}

/// Shared runner for the read-only git tools.
async fn run_git<E: CommandExecutor>(executor: &E, cwd: &str, command: &str) -> Result<ToolOutput> {
    match executor.run(command, cwd).await {
        Ok(out) => {
            let ok = out.exit_code == Some(0);
            let value = serde_json::json!({
                "command": command,
                "exit_code": out.exit_code,
                "stdout": cap(out.stdout),
                "stderr": cap(out.stderr),
            });
            Ok(ToolOutput {
                ok,
                value,
                truncated: false,
            })
        }
        Err(e) => Ok(ToolOutput::failure(e.to_string())),
    }
}

/// `git_status`: porcelain working-tree status (read-only).
pub struct GitStatusTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
}

impl<E: CommandExecutor> GitStatusTool<E> {
    /// Build rooted at `cwd` (the workspace/repo directory).
    pub fn new(executor: E, cwd: impl Into<String>) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for GitStatusTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_status".into(),
            description: "Show the working-tree status (branch + changed files). No args.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly]),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        run_git(&self.executor, &self.cwd, "git status").await
    }
}

/// `git_diff`: show changes. Optional `path` narrows to a file; `staged` shows
/// the staged diff.
pub struct GitDiffTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
}

impl<E: CommandExecutor> GitDiffTool<E> {
    /// Build rooted at `cwd`.
    pub fn new(executor: E, cwd: impl Into<String>) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for GitDiffTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_diff".into(),
            description: "Show uncommitted changes. Args: { path?: string, staged?: bool }.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Limit the diff to this file/dir." },
                    "staged": { "type": "boolean", "description": "Show the staged (cached) diff." }
                }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let staged = args
            .get("staged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let mut command = String::from("git diff");
        if staged {
            command.push_str(" --staged");
        }
        if !path.trim().is_empty() {
            // Quote to tolerate spaces; reject embedded quotes to avoid escaping.
            if path.contains('"') {
                return Ok(ToolOutput::failure("invalid path: contains a quote"));
            }
            command.push_str(&format!(" -- \"{}\"", path.trim()));
        }
        run_git(&self.executor, &self.cwd, &command).await
    }
}

/// `git_log`: recent commit history (read-only). Optional `limit` (default 10).
pub struct GitLogTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
}

impl<E: CommandExecutor> GitLogTool<E> {
    /// Build rooted at `cwd`.
    pub fn new(executor: E, cwd: impl Into<String>) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for GitLogTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_log".into(),
            description: "Show recent commit history (one line each). Args: { limit?: number }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "number", "description": "How many commits (default 10, max 50)." }
                }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50);
        let command = format!("git log -{limit} --oneline --decorate");
        run_git(&self.executor, &self.cwd, &command).await
    }
}

/// `git_commit`: stage all changes and commit with a message (workspace write).
pub struct GitCommitTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
}

impl<E: CommandExecutor> GitCommitTool<E> {
    /// Build rooted at `cwd`.
    pub fn new(executor: E, cwd: impl Into<String>) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for GitCommitTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_commit".into(),
            description:
                "Stage all changes and create a commit. Args: { message: string, all?: bool }."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Commit message." },
                    "all": { "type": "boolean", "description": "Stage all tracked+untracked changes first (default true)." }
                },
                "required": ["message"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(message) = args.get("message").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'message'"));
        };
        let message = message.trim();
        if message.is_empty() {
            return Ok(ToolOutput::failure("commit message must not be empty"));
        }
        // Reject embedded double-quotes to avoid shell-escaping pitfalls; the
        // model can rephrase. Newlines are fine inside the quoted argument.
        if message.contains('"') {
            return Ok(ToolOutput::failure(
                "commit message must not contain a double-quote character",
            ));
        }
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(true);

        if all {
            // Stage everything first (best-effort; report failure if it errors).
            let add = self.executor.run("git add -A", &self.cwd).await;
            if let Ok(o) = &add {
                if o.exit_code != Some(0) {
                    return Ok(ToolOutput {
                        ok: false,
                        value: serde_json::json!({
                            "stage_exit_code": o.exit_code,
                            "stderr": cap(o.stderr.clone()),
                            "error": "git add -A failed",
                        }),
                        truncated: false,
                    });
                }
            } else if let Err(e) = add {
                return Ok(ToolOutput::failure(e.to_string()));
            }
        }

        let command = format!("git commit -m \"{message}\"");
        run_git(&self.executor, &self.cwd, &command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash_tool::CommandOutcome;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingExecutor {
        ran: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn run(&self, command: &str, _cwd: &str) -> Result<CommandOutcome> {
            self.ran.lock().unwrap().push(command.to_string());
            Ok(CommandOutcome {
                exit_code: Some(0),
                stdout: format!("ran: {command}"),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn status_runs_git_status() {
        let tool = GitStatusTool::new(RecordingExecutor::default(), "/work");
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["command"], "git status");
    }

    #[tokio::test]
    async fn diff_with_path_and_staged() {
        let exec = RecordingExecutor::default();
        let tool = GitDiffTool::new(exec, "/work");
        let out = tool
            .invoke(serde_json::json!({ "path": "src/main.rs", "staged": true }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["command"], "git diff --staged -- \"src/main.rs\"");
    }

    #[tokio::test]
    async fn diff_rejects_quote_in_path() {
        let tool = GitDiffTool::new(RecordingExecutor::default(), "/work");
        let out = tool
            .invoke(serde_json::json!({ "path": "a\"b" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn log_clamps_limit() {
        let tool = GitLogTool::new(RecordingExecutor::default(), "/work");
        let out = tool
            .invoke(serde_json::json!({ "limit": 999 }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["command"], "git log -50 --oneline --decorate");
    }

    #[tokio::test]
    async fn commit_stages_then_commits() {
        let tool = GitCommitTool::new(RecordingExecutor::default(), "/work");
        let out = tool
            .invoke(serde_json::json!({ "message": "do a thing" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["command"], "git commit -m \"do a thing\"");
    }

    #[tokio::test]
    async fn commit_requires_message() {
        let tool = GitCommitTool::new(RecordingExecutor::default(), "/work");
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn commit_rejects_quote_in_message() {
        let tool = GitCommitTool::new(RecordingExecutor::default(), "/work");
        let out = tool
            .invoke(serde_json::json!({ "message": "bad \"quote\"" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn descriptors_have_expected_risk_and_perms() {
        let status = GitStatusTool::new(RecordingExecutor::default(), "/w");
        assert_eq!(status.descriptor().risk, RiskLevel::Safe);
        let commit = GitCommitTool::new(RecordingExecutor::default(), "/w");
        assert_eq!(commit.descriptor().risk, RiskLevel::Low);
        assert!(commit
            .descriptor()
            .required_permissions
            .contains(Permission::WorkspaceWrite));
    }
}
