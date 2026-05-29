//! Command execution abstraction for verification.
//!
//! Verification runs shell-level checks (`cargo test`, `npm run build`, lint…).
//! To keep the verification/reflection logic fully testable offline and to let
//! the runtime sandbox or mock execution, all command execution goes through
//! the [`CommandRunner`] trait. The real process runner lives behind
//! [`SystemCommandRunner`]; tests use [`MockRunner`].

use std::collections::HashMap;

use async_trait::async_trait;

use deepagent_core::error::Result;

/// A command to execute as part of verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The program to run (e.g. `"cargo"`).
    pub program: String,
    /// Arguments (e.g. `["test", "--workspace"]`).
    pub args: Vec<String>,
    /// Optional working directory.
    pub cwd: Option<String>,
}

impl Command {
    /// Build a command from a program and args.
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            cwd: None,
        }
    }

    /// Parse a simple whitespace-separated command line (no quoting). Intended
    /// for ergonomic test/config use, not untrusted input.
    pub fn parse(line: &str) -> Self {
        let mut parts = line.split_whitespace().map(str::to_string);
        let program = parts.next().unwrap_or_default();
        Self {
            program,
            args: parts.collect(),
            cwd: None,
        }
    }

    /// Set the working directory (builder style).
    pub fn in_dir(mut self, dir: impl Into<String>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Render the command for display / keys.
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// The captured result of running a [`Command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Process exit code (`None` if terminated by signal).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl CommandOutput {
    /// Whether the command succeeded (exit code 0).
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Combined stdout+stderr (used for failure analysis).
    pub fn combined(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// Runs commands.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// Execute `command`, capturing output.
    async fn run(&self, command: &Command) -> Result<CommandOutput>;
}

/// Blanket impl so an `Arc<dyn CommandRunner>` can be used wherever a
/// `CommandRunner` is expected (lets callers hold a trait object without making
/// every consumer generic).
#[async_trait]
impl CommandRunner for std::sync::Arc<dyn CommandRunner> {
    async fn run(&self, command: &Command) -> Result<CommandOutput> {
        (**self).run(command).await
    }
}

/// Executes commands as real OS processes.
#[derive(Debug, Clone, Default)]
pub struct SystemCommandRunner;

#[async_trait]
impl CommandRunner for SystemCommandRunner {
    async fn run(&self, command: &Command) -> Result<CommandOutput> {
        use std::process::Command as StdCommand;

        let program = command.program.clone();
        let args = command.args.clone();
        let cwd = command.cwd.clone();

        // Run on a blocking thread; process spawning is blocking IO.
        let output = tokio::task::spawn_blocking(move || {
            let mut cmd = StdCommand::new(&program);
            cmd.args(&args);
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }
            cmd.output()
        })
        .await
        .map_err(|e| deepagent_core::error::CoreError::other(format!("join error: {e}")))?
        .map_err(|e| {
            deepagent_core::error::CoreError::other(format!(
                "failed to spawn '{}': {e}",
                command.program
            ))
        })?;

        Ok(CommandOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A deterministic runner that returns canned outputs keyed by command display
/// string. Unknown commands default to success with empty output.
#[derive(Debug, Clone, Default)]
pub struct MockRunner {
    responses: HashMap<String, CommandOutput>,
}

impl MockRunner {
    /// New empty mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a canned response for a command display string.
    pub fn with(mut self, command_display: impl Into<String>, output: CommandOutput) -> Self {
        self.responses.insert(command_display.into(), output);
        self
    }

    /// Register a failing response (exit 1) with the given stderr.
    pub fn with_failure(
        self,
        command_display: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Self {
        self.with(
            command_display,
            CommandOutput {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: stderr.into(),
            },
        )
    }

    /// Register a successful response.
    pub fn with_success(self, command_display: impl Into<String>) -> Self {
        self.with(
            command_display,
            CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            },
        )
    }
}

#[async_trait]
impl CommandRunner for MockRunner {
    async fn run(&self, command: &Command) -> Result<CommandOutput> {
        Ok(self
            .responses
            .get(&command.display())
            .cloned()
            .unwrap_or(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parse_and_display() {
        let c = Command::parse("cargo test --workspace");
        assert_eq!(c.program, "cargo");
        assert_eq!(c.args, vec!["test", "--workspace"]);
        assert_eq!(c.display(), "cargo test --workspace");
    }

    #[test]
    fn output_success_and_combined() {
        let ok = CommandOutput {
            exit_code: Some(0),
            stdout: "fine".into(),
            stderr: String::new(),
        };
        assert!(ok.success());
        assert_eq!(ok.combined(), "fine");

        let fail = CommandOutput {
            exit_code: Some(1),
            stdout: "out".into(),
            stderr: "err".into(),
        };
        assert!(!fail.success());
        assert_eq!(fail.combined(), "out\nerr");
    }

    #[tokio::test]
    async fn mock_runner_returns_canned_and_defaults() {
        let runner = MockRunner::new()
            .with_failure("cargo test", "test failed")
            .with_success("cargo build");

        let fail = runner.run(&Command::parse("cargo test")).await.unwrap();
        assert!(!fail.success());
        assert_eq!(fail.stderr, "test failed");

        let ok = runner.run(&Command::parse("cargo build")).await.unwrap();
        assert!(ok.success());

        // Unknown command defaults to success.
        let unknown = runner.run(&Command::parse("echo hi")).await.unwrap();
        assert!(unknown.success());
    }
}
