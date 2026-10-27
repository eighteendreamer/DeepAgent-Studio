//! Automatic checks are host-generated bash calls, not extra execution authority.
//! Reuse the run's registry, hooks, approval gate, controls and action projection.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use deepagent_builtins::{CommandExecutor, CommandOutcome};
use deepagent_core::{
    error::{CoreError, Result},
    id::SessionId,
};
use deepagent_hooks::HookRegistry;
use deepagent_runtime::{
    tool_pipeline::ToolExecutionPipeline, ApprovalGate, CancellationTree, RuntimeConfig,
    RuntimeEventSink,
};
use deepagent_tools::{ToolInvocation, ToolRegistry};
use deepagent_verification::{Command, CommandOutput, CommandRunner};

pub(crate) struct VerificationCommandContext {
    pub registry: Arc<ToolRegistry>,
    pub session_id: SessionId,
    pub hooks: Arc<HookRegistry>,
    pub approvals: Arc<dyn ApprovalGate>,
    pub cancellation: CancellationTree,
}

pub(crate) struct VerificationCommands {
    context: VerificationCommandContext,
    root: PathBuf,
    config: RuntimeConfig,
    events: OnceLock<Arc<dyn RuntimeEventSink>>,
}

impl VerificationCommands {
    pub(crate) fn new(
        context: VerificationCommandContext,
        root: &Path,
        config: &RuntimeConfig,
    ) -> Self {
        let mut config = config.clone();
        // Internal bash results must not recursively invoke the post-edit chain.
        config.tool_result_decorator = None;
        Self {
            context,
            root: root.to_path_buf(),
            config,
            events: OnceLock::new(),
        }
    }

    pub(crate) fn bind_events(&self, events: Arc<dyn RuntimeEventSink>) -> Result<()> {
        self.events
            .set(events)
            .map_err(|_| CoreError::invalid("verification event sink already bound"))
    }
}

/// Quote one literal argument for the same platform-default shell as bash.
/// This is for generated argv/path values, never for quoting a whole script.
pub(crate) fn quote_argument(value: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b"_./:-".contains(&b) || (cfg!(windows) && b == b'\\')
        })
    {
        return value.to_string();
    }
    if cfg!(windows) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

impl VerificationCommands {
    async fn execute(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        // Bash is registered with the effective run root. Preserve a verifier's
        // nested cwd without inventing a second executor or extending tool DTOs.
        let root = self.root.canonicalize().map_err(|error| {
            CoreError::other(format!(
                "resolve verification root {}: {error}",
                self.root.display()
            ))
        })?;
        let cwd = Path::new(cwd).canonicalize().map_err(|error| {
            CoreError::other(format!("resolve verification cwd {cwd}: {error}"))
        })?;
        if !cwd.starts_with(&root) {
            return Ok(CommandOutcome {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: "automatic verification cwd is outside the run workspace".into(),
            });
        }
        let command = if cwd == root {
            command.to_string()
        } else if cfg!(windows) {
            format!(
                "Set-Location -LiteralPath {} -ErrorAction Stop; {command}",
                quote_argument(&cwd.to_string_lossy())
            )
        } else {
            format!("cd {} && {command}", quote_argument(&cwd.to_string_lossy()))
        };
        let config = &self.config;
        let context = &self.context;
        let events = self
            .events
            .get()
            .ok_or_else(|| CoreError::invalid("verification requires the kernel's event sink"))?;
        let result = ToolExecutionPipeline::new(
            &context.registry,
            context.session_id,
            config.permissions.clone(),
        )
        .with_hooks(Some(&context.hooks))
        .with_approvals(context.approvals.clone())
        .with_events(events.clone())
        .with_auto_approve(config.auto_approve)
        .with_execution_controls(context.cancellation.legacy_flag(), config.tool_timeout)
        .with_execution_token(Some(context.cancellation.token()))
        .with_result_policy(config.tool_result_budget.clone(), None, Arc::default())
        .with_artifact_persistence(config.artifact_persistence.clone())
        .with_action_persistence(config.action_persistence.clone())
        .with_raw_output(true)
        .execute_registered(ToolInvocation::new(
            "bash",
            serde_json::json!({"command": command}),
        ))
        .await?;
        let output = result.output;
        let value = result.raw_output.as_ref().unwrap_or(&output.value);
        let exit_code = value["exit_code"].as_i64().map(|n| n as i32);
        Ok(CommandOutcome {
            exit_code: if !output.ok && (exit_code.is_none() || exit_code == Some(0)) {
                Some(1)
            } else {
                exit_code
            },
            stdout: value["stdout"].as_str().unwrap_or_default().to_string(),
            stderr: value["stderr"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }
}

#[async_trait]
impl CommandExecutor for VerificationCommands {
    async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        // A policy/persistence/path error is a failed check, not a missing
        // optional toolchain. Never let the dispatcher silently skip it.
        Ok(self
            .execute(command, cwd)
            .await
            .unwrap_or_else(|error| CommandOutcome {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: error.to_string(),
            }))
    }
}

#[async_trait]
impl CommandRunner for VerificationCommands {
    async fn run(&self, command: &Command) -> Result<CommandOutput> {
        let program = quote_argument(&command.program);
        let program = if cfg!(windows) && program.starts_with('\'') {
            format!("& {program}")
        } else {
            program
        };
        let line = std::iter::once(program)
            .chain(command.args.iter().map(|arg| quote_argument(arg)))
            .collect::<Vec<_>>()
            .join(" ");
        let root = self.root.to_string_lossy();
        let out =
            CommandExecutor::run(self, &line, command.cwd.as_deref().unwrap_or(&root)).await?;
        Ok(CommandOutput {
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
mod lifecycle_tests;
