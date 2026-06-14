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

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
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
            description: "Run an allow-listed shell command in the workspace. Args: { command }.\n\
                \n\
                ## Tool priority — prefer dedicated tools when one fits\n\
                - read a file → use `read_file` (NOT `cat` / `head` / `tail`)\n\
                - edit a file → use `edit_file` / `multi_edit` (NOT `sed` / `awk`)\n\
                - create a file → use `write_file` (NOT echo redirection / heredoc)\n\
                - find files → use `glob` (NOT `find` / `ls`)\n\
                - search file contents → use `grep` (NOT `grep` / `rg` on the shell)\n\
                - explore project structure → use `code_map_*` tools (NOT recursive shell walks)\n\
                Reserve `bash` for build / test / install / git / system inspection that no dedicated tool covers.\n\
                \n\
                ## Multi-command guidance\n\
                - Independent commands → emit them as PARALLEL tool_calls in one assistant message; each becomes its own `bash` invocation. Do NOT pack independent commands into one shell line.\n\
                - Genuinely sequential dependent commands → join with `&&` so a failure short-circuits (e.g. `cd repo && git pull && cargo build`). Do NOT use newline-separated multi-line commands; they're harder to parse and less portable.\n\
                - Avoid sleep / poll loops (`while true; sleep`) — write a single targeted check and let the agent loop drive retries.\n\
                \n\
                ## Git safety protocol\n\
                - Never modify `git config` settings.\n\
                - Never `--no-verify` or `--no-gpg-sign` to skip hooks; fix the actual issue instead.\n\
                - Never force-push to `main` / `master` (force-push to a feature branch is OK after explicit approval).\n\
                - Prefer NEW commits over `git commit --amend` (amend rewrites history; new commits are reversible).\n\
                - Stage with `git add <specific files>` rather than `git add -A` so you commit only the intended changes.\n\
                \n\
                ## Path quoting & navigation\n\
                - Quote paths containing spaces or non-ASCII characters: `\"C:\\\\Program Files\\\\...\"`, `\"a path/with spaces\"`.\n\
                - Prefer absolute paths over `cd <dir> && ...` when the dedicated tools won't do; the working directory is reset every invocation.\n\
                - Before creating a new directory with `mkdir`, run `ls <parent>` first so you don't overwrite an existing non-directory or write outside the workspace.\n\
                \n\
                ## Silent success\n\
                - A command that exits 0 with empty stdout/stderr is success. The runtime substitutes a stub payload so the model doesn't mistake an empty body for a stop signal — do NOT retry just because the output was empty.".into(),
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
                Ok(ToolOutput {
                    ok,
                    value,
                    truncated: false,
                })
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

    #[test]
    fn descriptor_carries_phase_5a_guidance() {
        // Phase 5A: the bash tool description must surface tool-priority,
        // multi-command guidance, git safety protocol, path quoting, and the
        // silent-success contract so the model self-routes correctly.
        let tool = BashTool::new(RecordingExecutor::default(), "/work", ["git".to_string()]);
        let d = tool.descriptor();
        // Tool-priority routing.
        assert!(d.description.contains("read_file"));
        assert!(d.description.contains("edit_file"));
        assert!(d.description.contains("code_map_"));
        // Multi-command guidance.
        assert!(d.description.contains("PARALLEL tool_calls"));
        assert!(d.description.contains("&&"));
        // Git safety.
        assert!(d.description.contains("Never modify `git config`"));
        assert!(d.description.contains("force-push"));
        assert!(d.description.contains("--no-verify"));
        // Path quoting + parent ls-before-mkdir.
        assert!(d.description.contains("Quote paths"));
        assert!(d.description.contains("Before creating a new directory"));
        // Silent success.
        assert!(d.description.contains("Silent success"));
    }
}
