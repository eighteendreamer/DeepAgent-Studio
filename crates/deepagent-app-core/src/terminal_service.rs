//! Interactive terminal for the desktop Terminal panel (Phase C).
//!
//! Runs one-shot shell commands in the **active project** directory and returns
//! captured output. Two safety properties mirror the agent's `bash` tool:
//! - **Dangerous commands are refused** (`rm -rf`, `curl | sh`, `sudo`, …) and
//!   reported as `blocked` rather than executed — these need explicit approval.
//! - Commands run with the active project folder as the working directory, so
//!   the panel operates within the same project the agent does.
//!
//! Unlike the agent's `bash` tool this panel is **not** allow-list gated: it is
//! a user-driven terminal, so any non-dangerous command the user types runs.
//! The dangerous-command refusal remains as a guardrail.

use std::sync::Arc;

use deepagent_builtins::{is_dangerous, CommandExecutor, SystemExecutor};
use deepagent_core::error::Result;

use crate::dto::TerminalResultDto;
use crate::project_service::ProjectService;

/// Runs interactive terminal commands in the active project directory.
pub struct TerminalService {
    projects: Arc<ProjectService>,
    /// Fallback working directory when no project is active.
    default_cwd: String,
    executor: Arc<dyn CommandExecutor>,
}

impl TerminalService {
    /// Build over the project registry, with a default cwd used when no project
    /// is active. Uses the real [`SystemExecutor`].
    pub fn new(projects: Arc<ProjectService>, default_cwd: impl Into<String>) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor: Arc::new(SystemExecutor),
        }
    }

    /// Build with a custom executor (for tests).
    pub fn with_executor(
        projects: Arc<ProjectService>,
        default_cwd: impl Into<String>,
        executor: Arc<dyn CommandExecutor>,
    ) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor,
        }
    }

    /// The working directory for terminal commands: the active project, else
    /// the default cwd.
    fn cwd(&self) -> String {
        self.projects
            .active()
            .ok()
            .flatten()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| self.default_cwd.clone())
    }

    /// Run `command` in the active project directory. Dangerous commands are
    /// refused (reported as `blocked`) instead of being executed.
    pub async fn run(&self, command: &str) -> Result<TerminalResultDto> {
        let command = command.trim();
        let cwd = self.cwd();

        if command.is_empty() {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                blocked: false,
            });
        }

        if is_dangerous(command) {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("blocked: '{command}' is high-risk and requires explicit approval"),
                blocked: true,
            });
        }

        let outcome = self.executor.run(command, &cwd).await?;
        Ok(TerminalResultDto {
            command: command.to_string(),
            cwd,
            exit_code: outcome.exit_code,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            blocked: false,
        })
    }

    /// The current working directory (for the prompt display).
    pub fn current_dir(&self) -> String {
        self.cwd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_builtins::CommandOutcome;
    use deepagent_persistence::Database;
    use std::sync::Mutex;

    struct RecordingExecutor {
        ran: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
            self.ran
                .lock()
                .unwrap()
                .push((command.to_string(), cwd.to_string()));
            Ok(CommandOutcome {
                exit_code: Some(0),
                stdout: format!("ran: {command}"),
                stderr: String::new(),
            })
        }
    }

    fn service() -> (TerminalService, Arc<RecordingExecutor>, Arc<ProjectService>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let projects = Arc::new(ProjectService::new(db));
        let exec = Arc::new(RecordingExecutor {
            ran: Mutex::new(Vec::new()),
        });
        let svc = TerminalService::with_executor(projects.clone(), "/default", exec.clone());
        (svc, exec, projects)
    }

    #[tokio::test]
    async fn runs_in_active_project_dir() {
        let (svc, exec, projects) = service();
        projects.add_project("/work/proj").unwrap();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/work/proj");
        assert_eq!(res.exit_code, Some(0));
        assert!(!res.blocked);
        assert_eq!(exec.ran.lock().unwrap()[0].1, "/work/proj");
    }

    #[tokio::test]
    async fn falls_back_to_default_cwd() {
        let (svc, _exec, _projects) = service();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/default");
    }

    #[tokio::test]
    async fn dangerous_command_is_blocked_not_run() {
        let (svc, exec, _projects) = service();
        let res = svc.run("rm -rf /").await.unwrap();
        assert!(res.blocked);
        assert!(res.stderr.contains("high-risk"));
        // The executor was never invoked.
        assert!(exec.ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_command_is_noop() {
        let (svc, exec, _projects) = service();
        let res = svc.run("   ").await.unwrap();
        assert!(!res.blocked);
        assert!(exec.ran.lock().unwrap().is_empty());
    }
}
