//! Declarative external hooks (Phase B — `hooks.json`).
//!
//! Mirrors Claude Code's `hooks.json`: instead of compiling a Rust [`Hook`] per
//! policy, a user/plugin declares hooks as JSON and an external command (or, in
//! the future, an MCP tool) runs at a lifecycle point. The classic example is a
//! `PreToolUse` validator that inspects a `Bash` command and **blocks** it.
//!
//! ## Schema (Claude-Code-compatible)
//!
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       {
//!         "matcher": "Edit|Write|MultiEdit",
//!         "hooks": [
//!           { "type": "command", "command": "python3 validate.py", "timeout": 10 }
//!         ]
//!       }
//!     ],
//!     "UserPromptSubmit": [
//!       { "hooks": [ { "type": "command", "command": "./gate.sh" } ] }
//!     ]
//!   }
//! }
//! ```
//!
//! ## Events → lifecycle points
//!
//! | hooks.json event   | [`HookPoint`]                |
//! | ------------------ | ---------------------------- |
//! | `PreToolUse`       | [`HookPoint::BeforeToolUse`] |
//! | `PostToolUse`      | [`HookPoint::AfterToolUse`]  |
//! | `UserPromptSubmit` | [`HookPoint::UserPromptSubmit`] |
//! | `Stop`             | [`HookPoint::Stop`]          |
//! | `SubagentStop`     | [`HookPoint::SubagentStop`]  |
//! | `SessionEnd`       | [`HookPoint::SessionEnd`]    |
//! | … (every [`HookEvent`] variant maps 1:1 — see [`HookEvent::point`]) | |
//!
//! ## Command protocol (Claude-Code-compatible)
//!
//! The external command receives a JSON payload on **stdin**:
//! `{ "session_id", "hook_event_name", "tool_name?", "tool_input?",
//! "tool_response?", "prompt?" }`, and signals its decision via **exit code**:
//! - `0` — allow. stdout *may* carry a structured decision (see below).
//! - `2` — **block**: at a vetoable point this becomes [`HookOutcome::Deny`]
//!   with the command's stderr as the reason (fed back to the model).
//! - other non-zero — a non-blocking error: logged, treated as `Continue`.
//!
//! A `0`-exit command may also emit structured JSON on stdout to ask/deny:
//! `{ "decision": "block", "reason": "…" }` or
//! `{ "permissionDecision": "allow|deny|ask", "permissionDecisionReason": "…" }`
//! or `{ "continue": false, "stopReason": "…" }`.
//!
//! ## Matcher
//!
//! `matcher` filters by **tool name** for `PreToolUse`/`PostToolUse` (ignored
//! for prompt/stop events). To keep the kernel dependency-free and offline, the
//! matcher is a *simplified* regex: alternation (`A|B|C`), a `*` / `.*` / empty
//! wildcard (match all), and case-insensitive tool-name equality with the same
//! Claude-Code aliases as [`crate::permission_rules`] (`Bash`↔`shell`,
//! `Read`↔`read_file`, …). Full PCRE is intentionally out of scope.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

use crate::hook::{DecisionSource, Hook, HookOutcome};
use crate::lifecycle::{HookContext, HookData, HookPoint};
use crate::registry::HookRegistry;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Default command timeout when a hook omits `timeout` (seconds).
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 60;

/// A `hooks.json` lifecycle event (Claude Code naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    InstructionsLoaded,
    /// Before a tool runs (vetoable).
    PreToolUse,
    /// After a tool ran (observational).
    PostToolUse,
    PostToolUseFailure,
    PostToolBatch,
    PermissionRequest,
    PermissionDenied,
    /// When a user prompt is submitted (vetoable).
    UserPromptSubmit,
    /// When the main agent stops.
    Stop,
    StopFailure,
    PreCompact,
    PostCompact,
    SubagentStart,
    /// When a sub-agent stops.
    SubagentStop,
    SessionEnd,
    TaskCreated,
    TaskCompleted,
    WorktreeCreate,
    WorktreeRemove,
    CwdChanged,
    FileChanged,
    Notification,
}

impl HookEvent {
    /// Parse from the `hooks.json` key (e.g. "PreToolUse").
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "SessionStart" => Some(Self::SessionStart),
            "InstructionsLoaded" => Some(Self::InstructionsLoaded),
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            "PostToolUseFailure" => Some(Self::PostToolUseFailure),
            "PostToolBatch" => Some(Self::PostToolBatch),
            "PermissionRequest" => Some(Self::PermissionRequest),
            "PermissionDenied" => Some(Self::PermissionDenied),
            "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "Stop" => Some(Self::Stop),
            "StopFailure" => Some(Self::StopFailure),
            "PreCompact" => Some(Self::PreCompact),
            "PostCompact" => Some(Self::PostCompact),
            "SubagentStart" => Some(Self::SubagentStart),
            "SubagentStop" => Some(Self::SubagentStop),
            "SessionEnd" => Some(Self::SessionEnd),
            "TaskCreated" => Some(Self::TaskCreated),
            "TaskCompleted" => Some(Self::TaskCompleted),
            "WorktreeCreate" => Some(Self::WorktreeCreate),
            "WorktreeRemove" => Some(Self::WorktreeRemove),
            "CwdChanged" => Some(Self::CwdChanged),
            "FileChanged" => Some(Self::FileChanged),
            "Notification" => Some(Self::Notification),
            _ => None,
        }
    }

    /// The `hooks.json` key string.
    pub fn label(&self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::InstructionsLoaded => "InstructionsLoaded",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PostToolBatch => "PostToolBatch",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionDenied => "PermissionDenied",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::SessionEnd => "SessionEnd",
            Self::TaskCreated => "TaskCreated",
            Self::TaskCompleted => "TaskCompleted",
            Self::WorktreeCreate => "WorktreeCreate",
            Self::WorktreeRemove => "WorktreeRemove",
            Self::CwdChanged => "CwdChanged",
            Self::FileChanged => "FileChanged",
            Self::Notification => "Notification",
        }
    }

    /// The runtime lifecycle point this event maps to.
    pub fn point(&self) -> HookPoint {
        match self {
            Self::SessionStart => HookPoint::SessionStart,
            Self::InstructionsLoaded => HookPoint::InstructionsLoaded,
            Self::PreToolUse => HookPoint::BeforeToolUse,
            Self::PostToolUse => HookPoint::AfterToolUse,
            Self::PostToolUseFailure => HookPoint::PostToolUseFailure,
            Self::PostToolBatch => HookPoint::PostToolBatch,
            Self::PermissionRequest => HookPoint::PermissionRequest,
            Self::PermissionDenied => HookPoint::PermissionDenied,
            Self::UserPromptSubmit => HookPoint::UserPromptSubmit,
            Self::Stop => HookPoint::Stop,
            Self::StopFailure => HookPoint::StopFailure,
            Self::PreCompact => HookPoint::BeforeCompact,
            Self::PostCompact => HookPoint::PostCompact,
            Self::SubagentStart => HookPoint::SubagentStart,
            Self::SubagentStop => HookPoint::SubagentStop,
            Self::SessionEnd => HookPoint::SessionEnd,
            Self::TaskCreated => HookPoint::TaskCreated,
            Self::TaskCompleted => HookPoint::TaskCompleted,
            Self::WorktreeCreate => HookPoint::WorktreeCreate,
            Self::WorktreeRemove => HookPoint::WorktreeRemove,
            Self::CwdChanged => HookPoint::CwdChanged,
            Self::FileChanged => HookPoint::FileChanged,
            Self::Notification => HookPoint::Notification,
        }
    }

    /// Whether `matcher` applies (only the tool events filter by tool name).
    pub fn uses_matcher(&self) -> bool {
        matches!(
            self,
            Self::PreToolUse
                | Self::PostToolUse
                | Self::PostToolUseFailure
                | Self::PermissionRequest
                | Self::PermissionDenied
        )
    }
}

/// The action kind a single hook performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookActionType {
    /// Run an external command (the only executable type today).
    #[default]
    Command,
    /// POST the lifecycle payload to an HTTP endpoint.
    Http,
    /// Invoke an MCP tool (declared but not yet wired — see module docs).
    McpTool,
    /// Ask a model prompt hook runner for a decision.
    Prompt,
    /// Delegate hook evaluation to an isolated agent.
    Agent,
}

/// The shell environment used to run a command hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookCommandShell {
    /// Pick a platform default.
    #[default]
    Auto,
    /// Windows Command Prompt (`cmd.exe`).
    Cmd,
    /// PowerShell (`pwsh` or Windows PowerShell).
    Powershell,
    /// Windows Subsystem for Linux (`wsl.exe -- bash -lc`).
    Wsl,
    /// POSIX bash login shell.
    Bash,
    /// POSIX sh.
    Sh,
    /// POSIX zsh login shell.
    Zsh,
}

impl HookCommandShell {
    /// Stable label for diagnostics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cmd => "cmd",
            Self::Powershell => "powershell",
            Self::Wsl => "wsl",
            Self::Bash => "bash",
            Self::Sh => "sh",
            Self::Zsh => "zsh",
        }
    }

    /// Parse a user/model supplied shell label.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "default" => Some(Self::Auto),
            "cmd" | "command_prompt" | "commandprompt" => Some(Self::Cmd),
            "powershell" | "pwsh" | "ps" => Some(Self::Powershell),
            "wsl" | "wsl2" => Some(Self::Wsl),
            "bash" | "git_bash" | "git-bash" => Some(Self::Bash),
            "sh" => Some(Self::Sh),
            "zsh" => Some(Self::Zsh),
            _ => None,
        }
    }

    fn resolve(self) -> Self {
        match self {
            Self::Auto if cfg!(windows) => Self::Powershell,
            Self::Auto => Self::Sh,
            other => other,
        }
    }

    /// Stable label for the shell selected after resolving `auto`.
    pub fn resolved_label(self) -> &'static str {
        self.resolve().label()
    }
}

/// A single declared hook action (`{ "type", "command", "timeout" }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookAction {
    /// Action kind.
    #[serde(rename = "type", default)]
    pub action_type: HookActionType,
    /// The command line for `command` actions (or the MCP tool name).
    #[serde(default)]
    pub command: String,
    /// Prompt template for `prompt` and `agent` actions. The lifecycle payload
    /// replaces `$ARGUMENTS`, or is appended when the placeholder is absent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Optional model override for model-backed hook actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional isolated agent profile name for `agent` actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Static arguments merged with the lifecycle payload for `mcp_tool`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub arguments: serde_json::Value,
    /// Endpoint for `http` actions.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// HTTP method for `http` actions (defaults to POST).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Additional HTTP headers. Authorization values are redacted from logs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Per-hook timeout in seconds (defaults to [`DEFAULT_HOOK_TIMEOUT_SECS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Shell used to execute the command. Defaults to platform auto.
    #[serde(default, skip_serializing_if = "is_default_hook_shell")]
    pub shell: HookCommandShell,
    /// Environment variables injected when this command runs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl HookAction {
    /// The effective timeout duration.
    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS))
    }
}

impl Default for HookAction {
    fn default() -> Self {
        Self {
            action_type: HookActionType::Command,
            command: String::new(),
            prompt: String::new(),
            model: None,
            agent: None,
            arguments: serde_json::Value::Null,
            url: String::new(),
            method: None,
            headers: BTreeMap::new(),
            timeout: None,
            shell: HookCommandShell::Auto,
            env: BTreeMap::new(),
        }
    }
}

fn is_default_hook_shell(shell: &HookCommandShell) -> bool {
    *shell == HookCommandShell::Auto
}

/// A matcher group: an optional tool-name `matcher` + the actions to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMatcherGroup {
    /// Tool-name matcher (simplified regex). `None` matches all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// The hook actions in this group.
    #[serde(default)]
    pub hooks: Vec<HookAction>,
}

/// A parsed `hooks.json` document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookDefinitions {
    /// Events → matcher groups.
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<HookMatcherGroup>>,
}

impl HookDefinitions {
    /// Parse from a `hooks.json` string.
    pub fn parse(json: &str) -> Result<Self> {
        let definitions: Self = serde_json::from_str(json)
            .map_err(|e| CoreError::invalid(format!("invalid hooks.json: {e}")))?;
        definitions.validate()?;
        Ok(definitions)
    }

    pub fn validate(&self) -> Result<()> {
        for (event, groups) in &self.hooks {
            if HookEvent::parse(event).is_none() {
                return Err(CoreError::invalid(format!(
                    "unknown hooks.json event: {event}"
                )));
            }
            for group in groups {
                for action in &group.hooks {
                    match action.action_type {
                        HookActionType::Command => validate_command_action(action)?,
                        HookActionType::Http => validate_http_action(action)?,
                        HookActionType::McpTool => validate_mcp_action(action)?,
                        HookActionType::Prompt | HookActionType::Agent => {
                            validate_model_action(action)?
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether there are no hooks at all.
    pub fn is_empty(&self) -> bool {
        self.hooks.values().all(|g| g.is_empty())
    }

    /// Total number of declared hook actions across all events/groups.
    pub fn action_count(&self) -> usize {
        self.hooks
            .values()
            .flat_map(|groups| groups.iter())
            .map(|g| g.hooks.len())
            .sum()
    }

    /// Build [`ExternalCommandHook`]s and register them into `registry`, using
    /// `runner` to execute commands. Unknown event keys and non-command actions
    /// are skipped with a warning. Returns the number of hooks registered.
    pub fn register_into(
        &self,
        registry: &mut HookRegistry,
        runner: Arc<dyn HookCommandRunner>,
    ) -> usize {
        let mut registered = 0;
        for (event_key, groups) in &self.hooks {
            let Some(event) = HookEvent::parse(event_key) else {
                tracing::warn!(
                    event = event_key.as_str(),
                    "unknown hooks.json event; skipping"
                );
                continue;
            };
            for group in groups {
                for action in &group.hooks {
                    if action.action_type != HookActionType::Command {
                        if action.action_type == HookActionType::Http {
                            let hook = ExternalHttpHook {
                                event,
                                matcher: group.matcher.clone(),
                                action: action.clone(),
                                client: reqwest::Client::new(),
                            };
                            registry.register(event.point(), Arc::new(hook));
                            registered += 1;
                        } else {
                            tracing::warn!(
                                event = event.label(),
                                "hook action ('{:?}') requires a host executor; skipping",
                                action.action_type
                            );
                        }
                        continue;
                    }
                    let hook = ExternalCommandHook {
                        event,
                        matcher: group.matcher.clone(),
                        action: action.clone(),
                        runner: runner.clone(),
                    };
                    registry.register(event.point(), Arc::new(hook));
                    registered += 1;
                }
            }
        }
        registered
    }

    /// Register every supported action. Model-, agent-, and MCP-backed actions
    /// are delegated to the host so this crate remains provider-neutral.
    pub fn register_into_with_host(
        &self,
        registry: &mut HookRegistry,
        runner: Arc<dyn HookCommandRunner>,
        host: Arc<dyn HookActionExecutor>,
    ) -> usize {
        let mut registered = 0;
        for (event_key, groups) in &self.hooks {
            let Some(event) = HookEvent::parse(event_key) else {
                tracing::warn!(
                    event = event_key.as_str(),
                    "unknown hooks.json event; skipping"
                );
                continue;
            };
            for group in groups {
                for action in &group.hooks {
                    let hook: Arc<dyn Hook> = match action.action_type {
                        HookActionType::Command => Arc::new(ExternalCommandHook {
                            event,
                            matcher: group.matcher.clone(),
                            action: action.clone(),
                            runner: runner.clone(),
                        }),
                        HookActionType::Http => Arc::new(ExternalHttpHook {
                            event,
                            matcher: group.matcher.clone(),
                            action: action.clone(),
                            client: reqwest::Client::new(),
                        }),
                        HookActionType::McpTool
                        | HookActionType::Prompt
                        | HookActionType::Agent => Arc::new(ExternalHostHook {
                            event,
                            matcher: group.matcher.clone(),
                            action: action.clone(),
                            host: host.clone(),
                        }),
                    };
                    registry.register(event.point(), hook);
                    registered += 1;
                }
            }
        }
        registered
    }
}

fn validate_mcp_action(action: &HookAction) -> Result<()> {
    if action.command.trim().is_empty() {
        return Err(CoreError::invalid("MCP hook tool name cannot be empty"));
    }
    if !action.arguments.is_null() && !action.arguments.is_object() {
        return Err(CoreError::invalid(
            "MCP hook arguments must be a JSON object",
        ));
    }
    Ok(())
}

fn validate_model_action(action: &HookAction) -> Result<()> {
    if action.prompt.trim().is_empty() && action.command.trim().is_empty() {
        return Err(CoreError::invalid(
            "prompt/agent hook prompt cannot be empty",
        ));
    }
    Ok(())
}

fn validate_http_action(action: &HookAction) -> Result<()> {
    let url = reqwest::Url::parse(action.url.trim())
        .map_err(|error| CoreError::invalid(format!("invalid HTTP hook URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CoreError::invalid("HTTP hook URL must use http or https"));
    }
    let method = action
        .method
        .as_deref()
        .unwrap_or("POST")
        .to_ascii_uppercase();
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
        return Err(CoreError::invalid(
            "HTTP hook method must be POST, PUT, or PATCH",
        ));
    }
    Ok(())
}

fn validate_command_action(action: &HookAction) -> Result<()> {
    if action.command.trim().is_empty() {
        return Err(CoreError::invalid("hook command cannot be empty"));
    }
    let command = action.command.trim().to_ascii_lowercase();
    if action.shell.resolve() == HookCommandShell::Powershell
        && ["powershell ", "powershell.exe ", "pwsh ", "pwsh.exe "]
            .iter()
            .any(|prefix| command.starts_with(prefix))
    {
        return Err(CoreError::invalid(
            "PowerShell hook command must contain only the script body; choose shell='powershell' instead of nesting powershell -Command",
        ));
    }
    for key in action.env.keys() {
        let upper = key.trim().to_ascii_uppercase();
        if is_reserved_hook_env(&upper) {
            return Err(CoreError::invalid(format!(
                "hook environment cannot override reserved variable: {key}"
            )));
        }
    }
    Ok(())
}

fn is_reserved_hook_env(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "PATHEXT"
            | "SYSTEMROOT"
            | "WINDIR"
            | "COMSPEC"
            | "HOME"
            | "USERPROFILE"
            | "SHELL"
            | "TMP"
            | "TEMP"
            | "API_KEY"
            | "OPENAI_API_KEY"
            | "ANTHROPIC_API_KEY"
            | "DEEPSEEK_API_KEY"
    ) || key.starts_with("DEEPAGENT_")
}

/// The captured result of running a hook command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommandResult {
    /// Process exit code (or a synthetic non-zero on spawn/timeout failure).
    pub exit_code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Runs an external hook command with a JSON stdin payload and a timeout.
///
/// Pluggable so the runtime can sandbox later and tests run offline.
#[async_trait]
pub trait HookCommandRunner: Send + Sync {
    /// Run `command`, writing `stdin_json` to its stdin, killing it after
    /// `timeout`. Returns the captured result.
    async fn run(
        &self,
        command: &str,
        stdin_json: &str,
        shell: HookCommandShell,
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<HookCommandResult>;
}

/// Executes host-owned hook action types without coupling this crate to a
/// model provider, MCP transport, or agent runtime.
#[async_trait]
pub trait HookActionExecutor: Send + Sync {
    async fn execute(&self, action: &HookAction, payload: serde_json::Value)
        -> Result<HookOutcome>;
}

/// Real OS process runner (spawns via the platform shell, pipes JSON to stdin).
#[derive(Debug, Clone, Default)]
pub struct SystemHookRunner;

#[async_trait]
impl HookCommandRunner for SystemHookRunner {
    async fn run(
        &self,
        command: &str,
        stdin_json: &str,
        shell: HookCommandShell,
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<HookCommandResult> {
        run_hook_process(command, stdin_json, shell, env, timeout, None).await
    }
}

impl SystemHookRunner {
    /// Run a hook command in a specific working directory.
    pub async fn run_in_dir(
        &self,
        command: &str,
        stdin_json: &str,
        shell: HookCommandShell,
        env: &BTreeMap<String, String>,
        timeout: Duration,
        cwd: &Path,
    ) -> Result<HookCommandResult> {
        run_hook_process(command, stdin_json, shell, env, timeout, Some(cwd)).await
    }
}

async fn run_hook_process(
    command: &str,
    stdin_json: &str,
    shell: HookCommandShell,
    env: &BTreeMap<String, String>,
    timeout: Duration,
    cwd: Option<&Path>,
) -> Result<HookCommandResult> {
    use tokio::io::AsyncWriteExt;

    let (mut cmd, _script_guard) = prepare_hook_command(shell, command)?;
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.envs(env);
    cmd.env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("POWERSHELL_TELEMETRY_OPTOUT", "1");
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::other(format!("hook spawn failed: {e}")))?;
    let pid = child.id().unwrap_or_default();
    let mut process_tree = ProcessTreeGuard::new(pid);

    if let Some(mut stdin) = child.stdin.take() {
        let payload = stdin_json.as_bytes().to_vec();
        // Best-effort: ignore broken-pipe if the child closed stdin early.
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.shutdown().await;
    }

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(res) => {
            let output = res.map_err(|e| CoreError::other(format!("hook wait failed: {e}")))?;
            process_tree.disarm();
            output
        }
        Err(_) => {
            return Ok(HookCommandResult {
                exit_code: 124, // conventional timeout code
                stdout: String::new(),
                stderr: format!("hook timed out after {:?}", timeout),
            });
        }
    };

    Ok(HookCommandResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn prepare_hook_command(
    shell: HookCommandShell,
    command: &str,
) -> Result<(tokio::process::Command, Option<tempfile::TempPath>)> {
    let needs_script = match shell.resolve() {
        HookCommandShell::Powershell => encode_powershell_command(command).len() > 24_000,
        HookCommandShell::Cmd => command.encode_utf16().count() > 7_000,
        HookCommandShell::Bash | HookCommandShell::Sh | HookCommandShell::Zsh => {
            command.len() > 100_000
        }
        HookCommandShell::Wsl | HookCommandShell::Auto => false,
    };
    if !needs_script {
        return Ok((build_hook_command(shell, command), None));
    }
    let suffix = match shell.resolve() {
        HookCommandShell::Powershell => ".ps1",
        HookCommandShell::Cmd => ".cmd",
        _ => ".sh",
    };
    let mut file = tempfile::Builder::new()
        .prefix("deepagent-hook-")
        .suffix(suffix)
        .tempfile()
        .map_err(|error| CoreError::other(format!("hook temp script failed: {error}")))?;
    let body = if shell.resolve() == HookCommandShell::Powershell {
        wrap_powershell_command(command)
    } else {
        command.to_string()
    };
    file.write_all(body.as_bytes())
        .map_err(|error| CoreError::other(format!("hook temp script write failed: {error}")))?;
    file.flush()
        .map_err(|error| CoreError::other(format!("hook temp script flush failed: {error}")))?;
    let path = file.into_temp_path();
    let path_text = path.to_string_lossy().to_string();
    let command = match shell.resolve() {
        HookCommandShell::Powershell => {
            let mut command = tokio::process::Command::new(powershell_executable());
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                path_text.as_str(),
            ]);
            command
        }
        HookCommandShell::Cmd => {
            let mut command = tokio::process::Command::new("cmd.exe");
            command.args(["/D", "/S", "/C", path_text.as_str()]);
            command
        }
        HookCommandShell::Bash => {
            let mut command = tokio::process::Command::new("bash");
            command.arg(path_text);
            command
        }
        HookCommandShell::Zsh => {
            let mut command = tokio::process::Command::new("zsh");
            command.arg(path_text);
            command
        }
        HookCommandShell::Sh | HookCommandShell::Auto => {
            let mut command = tokio::process::Command::new("sh");
            command.arg(path_text);
            command
        }
        HookCommandShell::Wsl => unreachable!("WSL hook does not use a host temp script"),
    };
    Ok((command, Some(path)))
}

fn build_hook_command(shell: HookCommandShell, command: &str) -> tokio::process::Command {
    match shell.resolve() {
        HookCommandShell::Cmd => {
            let mut c = tokio::process::Command::new("cmd.exe");
            configure_cmd_args(&mut c, command);
            c
        }
        HookCommandShell::Powershell => {
            let mut c = tokio::process::Command::new(powershell_executable());
            let encoded = encode_powershell_command(command);
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                encoded.as_str(),
            ]);
            c
        }
        HookCommandShell::Wsl => {
            let mut c = if cfg!(windows) {
                tokio::process::Command::new("wsl.exe")
            } else {
                tokio::process::Command::new("sh")
            };
            if cfg!(windows) {
                c.args(["--", "bash", "-lc", command]);
            } else {
                c.args(["-c", command]);
            }
            c
        }
        HookCommandShell::Bash => {
            let mut c = tokio::process::Command::new("bash");
            c.args(["-lc", command]);
            c
        }
        HookCommandShell::Zsh => {
            let mut c = tokio::process::Command::new("zsh");
            c.args(["-lc", command]);
            c
        }
        HookCommandShell::Sh | HookCommandShell::Auto => {
            let mut c = if cfg!(windows) {
                tokio::process::Command::new("cmd.exe")
            } else {
                tokio::process::Command::new("sh")
            };
            if cfg!(windows) {
                configure_cmd_args(&mut c, command);
            } else {
                c.args(["-c", command]);
            }
            c
        }
    }
}

fn powershell_executable() -> &'static str {
    if cfg!(windows) {
        if command_available("pwsh.exe") {
            "pwsh.exe"
        } else {
            "powershell.exe"
        }
    } else if command_available("pwsh") {
        "pwsh"
    } else {
        "powershell"
    }
}

struct ProcessTreeGuard {
    pid: u32,
    armed: bool,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

impl ProcessTreeGuard {
    fn new(pid: u32) -> Self {
        Self {
            pid,
            armed: true,
            #[cfg(windows)]
            job: WindowsJob::attach(pid).ok(),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
        #[cfg(windows)]
        if let Some(job) = self.job.as_mut() {
            job.disarm();
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        if !self.armed || self.pid == 0 {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
struct WindowsJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
    armed: bool,
}

#[cfg(windows)]
unsafe impl Send for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    fn attach(pid: u32) -> std::io::Result<Self> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                CloseHandle(handle);
                return Err(std::io::Error::last_os_error());
            }
            let process = OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if process.is_null() || process == INVALID_HANDLE_VALUE {
                CloseHandle(handle);
                return Err(std::io::Error::last_os_error());
            }
            let assigned = AssignProcessToJobObject(handle, process);
            CloseHandle(process);
            if assigned == 0 {
                CloseHandle(handle);
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                handle,
                armed: true,
            })
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            if self.armed {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
            }
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

fn configure_cmd_args(cmd: &mut tokio::process::Command, command: &str) {
    let command = format!("chcp 65001 >NUL && {command}");
    cmd.arg("/D").arg("/S").arg("/C");
    #[cfg(windows)]
    {
        cmd.raw_arg(command);
    }
    #[cfg(not(windows))]
    {
        cmd.arg(command);
    }
}

fn command_available(bin: &str) -> bool {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn encode_powershell_command(command: &str) -> String {
    let command = wrap_powershell_command(command);
    let mut utf16le = Vec::with_capacity(command.len() * 2);
    for unit in command.encode_utf16() {
        utf16le.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&utf16le)
}

fn wrap_powershell_command(command: &str) -> String {
    format!(
        "{}; if ($null -ne $global:LASTEXITCODE) {{ exit $global:LASTEXITCODE }} elseif ($?) {{ exit 0 }} else {{ exit 1 }}",
        command
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// A [`Hook`] backed by an external command from `hooks.json`.
pub struct ExternalCommandHook {
    event: HookEvent,
    matcher: Option<String>,
    action: HookAction,
    runner: Arc<dyn HookCommandRunner>,
}

impl ExternalCommandHook {
    /// Build a stdin payload for the current context (Claude-Code-compatible).
    fn build_payload(&self, ctx: &HookContext) -> serde_json::Value {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "session_id".into(),
            serde_json::Value::String(ctx.session_id.to_string()),
        );
        payload.insert(
            "hook_event_name".into(),
            serde_json::Value::String(self.event.label().to_string()),
        );
        match &ctx.data {
            HookData::Tool {
                name,
                arguments,
                ok,
            } => {
                payload.insert("tool_name".into(), serde_json::Value::String(name.clone()));
                payload.insert("tool_input".into(), arguments.clone());
                if let Some(ok) = ok {
                    payload.insert("tool_response".into(), serde_json::json!({ "ok": ok }));
                }
            }
            HookData::ToolBatch { tools } => {
                payload.insert("tool_calls".into(), serde_json::json!(tools));
                payload.insert("tool_batch".into(), serde_json::json!(tools));
                payload.insert("tool_count".into(), serde_json::json!(tools.len()));
            }
            HookData::Prompt { text } => {
                payload.insert("prompt".into(), serde_json::Value::String(text.clone()));
            }
            HookData::Verification { command, detail } => {
                payload.insert("command".into(), serde_json::Value::String(command.clone()));
                payload.insert("detail".into(), serde_json::Value::String(detail.clone()));
            }
            HookData::Response { content } => {
                payload.insert("content".into(), serde_json::Value::String(content.clone()));
            }
            HookData::Instructions { paths } => {
                payload.insert("paths".into(), serde_json::json!(paths));
            }
            HookData::Permission {
                tool,
                arguments,
                reason,
            } => {
                payload.insert("tool_name".into(), serde_json::json!(tool));
                payload.insert("tool_input".into(), arguments.clone());
                payload.insert("reason".into(), serde_json::json!(reason));
            }
            HookData::Compact { trigger, summary } => {
                payload.insert("trigger".into(), serde_json::json!(trigger));
                payload.insert("compact_summary".into(), serde_json::json!(summary));
            }
            HookData::Subagent {
                agent_id,
                agent_type,
                summary,
            } => {
                payload.insert("agent_id".into(), serde_json::json!(agent_id));
                payload.insert("agent_type".into(), serde_json::json!(agent_type));
                payload.insert("last_assistant_message".into(), serde_json::json!(summary));
            }
            HookData::Task { task_id, subject } => {
                payload.insert("task_id".into(), serde_json::json!(task_id));
                payload.insert("task_subject".into(), serde_json::json!(subject));
            }
            HookData::FileChange { path, kind } => {
                payload.insert("path".into(), serde_json::json!(path));
                payload.insert("change_kind".into(), serde_json::json!(kind));
            }
            HookData::Path { path } => {
                payload.insert("path".into(), serde_json::json!(path));
            }
            HookData::Notification { message } => {
                payload.insert("message".into(), serde_json::json!(message));
            }
            HookData::None => {}
        }
        serde_json::Value::Object(payload)
    }

    /// Whether this hook applies to `ctx` (matcher test for tool events).
    fn applies(&self, ctx: &HookContext) -> bool {
        if !self.event.uses_matcher() {
            return true;
        }
        let Some(matcher) = &self.matcher else {
            return true; // no matcher → all tools
        };
        match &ctx.data {
            HookData::Tool { name, .. } => matcher_matches(matcher, name),
            _ => true,
        }
    }
}

#[async_trait]
impl Hook for ExternalCommandHook {
    fn name(&self) -> &str {
        "external_command_hook"
    }

    fn dedup_key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.event.label(),
            self.matcher.as_deref().unwrap_or("*"),
            self.action.shell.label(),
            self.action.command.trim()
        )
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if !self.applies(ctx) {
            return Ok(HookOutcome::Continue);
        }

        let payload = self.build_payload(ctx);
        let stdin_json = serde_json::to_string(&payload)?;
        let result = self
            .runner
            .run(
                &self.action.command,
                &stdin_json,
                self.action.shell,
                &self.action.env,
                self.action.timeout_duration(),
            )
            .await?;

        Ok(interpret_result(&result))
    }
}

/// A lifecycle hook whose execution is owned by the embedding application.
pub struct ExternalHostHook {
    event: HookEvent,
    matcher: Option<String>,
    action: HookAction,
    host: Arc<dyn HookActionExecutor>,
}

#[async_trait]
impl Hook for ExternalHostHook {
    fn name(&self) -> &str {
        match self.action.action_type {
            HookActionType::McpTool => "external_mcp_hook",
            HookActionType::Prompt => "external_prompt_hook",
            HookActionType::Agent => "external_agent_hook",
            HookActionType::Command | HookActionType::Http => "external_host_hook",
        }
    }

    fn dedup_key(&self) -> String {
        format!(
            "{}|{}|{:?}|{}|{}|{}",
            self.event.label(),
            self.matcher.as_deref().unwrap_or("*"),
            self.action.action_type,
            self.action.command.trim(),
            self.action.prompt.trim(),
            self.action.agent.as_deref().unwrap_or("")
        )
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if self.event.uses_matcher() {
            if let (Some(matcher), HookData::Tool { name, .. }) = (&self.matcher, &ctx.data) {
                if !matcher_matches(matcher, name) {
                    return Ok(HookOutcome::Continue);
                }
            }
        }
        tokio::time::timeout(
            self.action.timeout_duration(),
            self.host
                .execute(&self.action, build_http_payload(self.event, ctx)),
        )
        .await
        .map_err(|_| {
            CoreError::other(format!(
                "{} hook timed out after {}s",
                self.name(),
                self.action.timeout_duration().as_secs()
            ))
        })?
    }
}

/// A lifecycle hook delivered as a JSON HTTP request.
pub struct ExternalHttpHook {
    event: HookEvent,
    matcher: Option<String>,
    action: HookAction,
    client: reqwest::Client,
}

#[async_trait]
impl Hook for ExternalHttpHook {
    fn name(&self) -> &str {
        "external_http_hook"
    }

    fn dedup_key(&self) -> String {
        format!(
            "{}|{}|http|{}",
            self.event.label(),
            self.matcher.as_deref().unwrap_or("*"),
            self.action.url.trim()
        )
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if self.event.uses_matcher() {
            if let (Some(matcher), HookData::Tool { name, .. }) = (&self.matcher, &ctx.data) {
                if !matcher_matches(matcher, name) {
                    return Ok(HookOutcome::Continue);
                }
            }
        }
        let method = self
            .action
            .method
            .as_deref()
            .unwrap_or("POST")
            .parse::<reqwest::Method>()
            .map_err(|error| CoreError::invalid(format!("invalid HTTP hook method: {error}")))?;
        let mut request = self
            .client
            .request(method, self.action.url.trim())
            .timeout(self.action.timeout_duration())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&build_http_payload(self.event, ctx))?);
        for (name, value) in &self.action.headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| CoreError::other(format!("HTTP hook request failed: {error}")))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| CoreError::other(format!("HTTP hook response failed: {error}")))?;
        let result = if status.is_success() {
            HookCommandResult {
                exit_code: 0,
                stdout: body,
                stderr: String::new(),
            }
        } else if status == reqwest::StatusCode::FORBIDDEN {
            HookCommandResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: if body.trim().is_empty() {
                    "blocked by HTTP hook".to_string()
                } else {
                    body
                },
            }
        } else {
            HookCommandResult {
                exit_code: status.as_u16() as i32,
                stdout: String::new(),
                stderr: body,
            }
        };
        Ok(interpret_result(&result))
    }
}

fn build_http_payload(event: HookEvent, ctx: &HookContext) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "session_id".into(),
        serde_json::json!(ctx.session_id.to_string()),
    );
    payload.insert("hook_event_name".into(), serde_json::json!(event.label()));
    match &ctx.data {
        HookData::Tool {
            name,
            arguments,
            ok,
        } => {
            payload.insert("tool_name".into(), serde_json::json!(name));
            payload.insert("tool_input".into(), arguments.clone());
            if let Some(ok) = ok {
                payload.insert("tool_response".into(), serde_json::json!({ "ok": ok }));
            }
        }
        HookData::ToolBatch { tools } => {
            payload.insert("tool_calls".into(), serde_json::json!(tools));
            payload.insert("tool_batch".into(), serde_json::json!(tools));
            payload.insert("tool_count".into(), serde_json::json!(tools.len()));
        }
        HookData::Prompt { text } => {
            payload.insert("prompt".into(), serde_json::json!(text));
        }
        HookData::Verification { command, detail } => {
            payload.insert("command".into(), serde_json::json!(command));
            payload.insert("detail".into(), serde_json::json!(detail));
        }
        HookData::Response { content } => {
            payload.insert("content".into(), serde_json::json!(content));
        }
        HookData::Instructions { paths } => {
            payload.insert("paths".into(), serde_json::json!(paths));
        }
        HookData::Permission {
            tool,
            arguments,
            reason,
        } => {
            payload.insert("tool_name".into(), serde_json::json!(tool));
            payload.insert("tool_input".into(), arguments.clone());
            payload.insert("reason".into(), serde_json::json!(reason));
        }
        HookData::Compact { trigger, summary } => {
            payload.insert("trigger".into(), serde_json::json!(trigger));
            payload.insert("compact_summary".into(), serde_json::json!(summary));
        }
        HookData::Subagent {
            agent_id,
            agent_type,
            summary,
        } => {
            payload.insert("agent_id".into(), serde_json::json!(agent_id));
            payload.insert("agent_type".into(), serde_json::json!(agent_type));
            payload.insert("last_assistant_message".into(), serde_json::json!(summary));
        }
        HookData::Task { task_id, subject } => {
            payload.insert("task_id".into(), serde_json::json!(task_id));
            payload.insert("task_subject".into(), serde_json::json!(subject));
        }
        HookData::FileChange { path, kind } => {
            payload.insert("path".into(), serde_json::json!(path));
            payload.insert("change_kind".into(), serde_json::json!(kind));
        }
        HookData::Path { path } => {
            payload.insert("path".into(), serde_json::json!(path));
        }
        HookData::Notification { message } => {
            payload.insert("message".into(), serde_json::json!(message));
        }
        HookData::None => {}
    }
    serde_json::Value::Object(payload)
}

/// Map a command result to a [`HookOutcome`] (exit code + structured stdout).
fn interpret_result(result: &HookCommandResult) -> HookOutcome {
    // A structured stdout decision takes precedence when present.
    if let Some(outcome) = parse_structured_hook_output(&result.stdout) {
        return outcome;
    }
    match result.exit_code {
        0 => HookOutcome::Continue,
        2 => {
            let reason = if result.stderr.trim().is_empty() {
                "blocked by external hook".to_string()
            } else {
                result.stderr.trim().to_string()
            };
            HookOutcome::deny_from(reason, DecisionSource::Policy)
        }
        other => {
            tracing::warn!(
                exit_code = other,
                stderr = result.stderr.trim(),
                "external hook exited non-blocking (non-zero); treating as continue"
            );
            HookOutcome::Continue
        }
    }
}

/// Parse a structured decision from a hook's stdout, if any.
/// Parse the common structured decision protocol used by command, MCP,
/// prompt, and agent hooks.
pub fn parse_structured_hook_output(stdout: &str) -> Option<HookOutcome> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    // Claude prompt/agent hook protocol: `{ "ok": boolean, "reason"?: string }`.
    if let Some(ok) = value.get("ok").and_then(|value| value.as_bool()) {
        if ok {
            return Some(HookOutcome::Continue);
        }
        let reason = value
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("denied by model-backed hook");
        return Some(HookOutcome::deny_from(reason, DecisionSource::Coordinator));
    }

    // `{"decision": "block", "reason": "..."}`
    if let Some(decision) = value.get("decision").and_then(|v| v.as_str()) {
        if decision.eq_ignore_ascii_case("block") {
            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("blocked by external hook")
                .to_string();
            return Some(HookOutcome::deny_from(reason, DecisionSource::Policy));
        }
    }

    // `{"permissionDecision": "allow|deny|ask", "permissionDecisionReason": "..."}`
    if let Some(pd) = value.get("permissionDecision").and_then(|v| v.as_str()) {
        let reason = value
            .get("permissionDecisionReason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return match pd.to_ascii_lowercase().as_str() {
            "deny" => Some(HookOutcome::deny_from(
                if reason.is_empty() {
                    "denied by external hook".into()
                } else {
                    reason
                },
                DecisionSource::Policy,
            )),
            "ask" => Some(HookOutcome::ask_from(
                if reason.is_empty() {
                    "approval required by external hook".into()
                } else {
                    reason
                },
                DecisionSource::Policy,
            )),
            "allow" => Some(HookOutcome::Continue),
            _ => None,
        };
    }

    // `{"continue": false, "stopReason": "..."}`
    if value.get("continue").and_then(|v| v.as_bool()) == Some(false) {
        let reason = value
            .get("stopReason")
            .and_then(|v| v.as_str())
            .unwrap_or("stopped by external hook")
            .to_string();
        return Some(HookOutcome::deny_from(reason, DecisionSource::Policy));
    }

    None
}

/// Simplified matcher: alternation (`A|B`), wildcard (`*`/`.*`/empty), and
/// case-insensitive tool-name equality with Claude-Code aliases.
fn matcher_matches(matcher: &str, tool: &str) -> bool {
    let matcher = matcher.trim();
    if matcher.is_empty() || matcher == "*" || matcher == ".*" {
        return true;
    }
    matcher
        .split('|')
        .map(str::trim)
        .any(|alt| alt_matches(alt, tool))
}

/// One alternative of a matcher against the tool name.
fn alt_matches(alt: &str, tool: &str) -> bool {
    if alt.is_empty() || alt == "*" || alt == ".*" {
        return true;
    }
    let norm = |s: &str| s.to_lowercase().replace(['_', '-'], "");
    let a = norm(alt);
    let t = norm(tool);
    if a == t {
        return true;
    }
    // Claude-Code name → our snake_case tool name aliases.
    matches!(
        (a.as_str(), t.as_str()),
        ("bash", "shell")
            | ("shell", "bash")
            | ("edit", "editfile")
            | ("write", "writefile")
            | ("read", "readfile")
            | ("ls", "listdir")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::id::SessionId;
    use std::sync::Mutex;

    /// A runner that returns a canned result and records the stdin it saw.
    struct MockRunner {
        result: HookCommandResult,
        last_stdin: Mutex<Option<String>>,
        last_env: Mutex<Option<BTreeMap<String, String>>>,
    }

    impl MockRunner {
        fn new(exit_code: i32, stdout: &str, stderr: &str) -> Self {
            Self {
                result: HookCommandResult {
                    exit_code,
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                },
                last_stdin: Mutex::new(None),
                last_env: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl HookCommandRunner for MockRunner {
        async fn run(
            &self,
            _command: &str,
            stdin_json: &str,
            _shell: HookCommandShell,
            env: &BTreeMap<String, String>,
            _timeout: Duration,
        ) -> Result<HookCommandResult> {
            *self.last_stdin.lock().unwrap() = Some(stdin_json.to_string());
            *self.last_env.lock().unwrap() = Some(env.clone());
            Ok(self.result.clone())
        }
    }

    struct MockHost {
        outcome: HookOutcome,
        payload: Mutex<Option<serde_json::Value>>,
    }

    #[async_trait]
    impl HookActionExecutor for MockHost {
        async fn execute(
            &self,
            _action: &HookAction,
            payload: serde_json::Value,
        ) -> Result<HookOutcome> {
            *self.payload.lock().unwrap() = Some(payload);
            Ok(self.outcome.clone())
        }
    }

    const SAMPLE: &str = r#"{
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Edit|Write|MultiEdit",
                    "hooks": [
                        { "type": "command", "command": "python3 validate.py", "timeout": 10 }
                    ]
                }
            ],
            "UserPromptSubmit": [
                { "hooks": [ { "type": "command", "command": "./gate.sh" } ] }
            ]
        }
    }"#;

    #[test]
    fn parses_claude_code_schema() {
        let defs = HookDefinitions::parse(SAMPLE).unwrap();
        assert!(!defs.is_empty());
        assert_eq!(defs.action_count(), 2);
        let pre = &defs.hooks["PreToolUse"][0];
        assert_eq!(pre.matcher.as_deref(), Some("Edit|Write|MultiEdit"));
        assert_eq!(pre.hooks[0].timeout, Some(10));
        assert_eq!(pre.hooks[0].action_type, HookActionType::Command);
    }

    #[test]
    fn event_mapping_and_parse() {
        assert_eq!(HookEvent::parse("PreToolUse"), Some(HookEvent::PreToolUse));
        assert_eq!(HookEvent::parse("nope"), None);
        assert_eq!(HookEvent::PreToolUse.point(), HookPoint::BeforeToolUse);
        assert_eq!(HookEvent::PostToolUse.point(), HookPoint::AfterToolUse);
        assert_eq!(HookEvent::Stop.point(), HookPoint::Stop);
        assert!(HookEvent::PreToolUse.uses_matcher());
        assert!(!HookEvent::UserPromptSubmit.uses_matcher());
    }

    #[test]
    fn matcher_alternation_and_wildcard() {
        // "Edit|Write|MultiEdit" does NOT match read_file.
        assert!(!matcher_matches("Edit|Write|MultiEdit", "read_file"));
        // Alias: Write → write_file.
        assert!(matcher_matches("Write", "write_file"));
        assert!(matcher_matches("Edit|Bash", "shell"));
        assert!(matcher_matches("*", "anything"));
        assert!(matcher_matches(".*", "anything"));
        assert!(!matcher_matches("Read", "write_file"));
    }

    #[test]
    fn default_timeout_applies() {
        let action = HookAction {
            action_type: HookActionType::Command,
            command: "x".into(),
            timeout: None,
            shell: HookCommandShell::Auto,
            env: BTreeMap::new(),
            ..HookAction::default()
        };
        assert_eq!(
            action.timeout_duration(),
            Duration::from_secs(DEFAULT_HOOK_TIMEOUT_SECS)
        );
    }

    #[test]
    fn interpret_exit_codes() {
        assert_eq!(
            interpret_result(&HookCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new()
            }),
            HookOutcome::Continue
        );
        let denied = interpret_result(&HookCommandResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "use rg instead of grep".into(),
        });
        assert!(denied.is_deny());
        assert_eq!(denied.deny_reason(), Some("use rg instead of grep"));
        // Non-2 non-zero → non-blocking continue.
        assert_eq!(
            interpret_result(&HookCommandResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "warn".into()
            }),
            HookOutcome::Continue
        );
    }

    #[test]
    fn interpret_structured_stdout() {
        let block = interpret_result(&HookCommandResult {
            exit_code: 0,
            stdout: r#"{"decision":"block","reason":"nope"}"#.into(),
            stderr: String::new(),
        });
        assert_eq!(block.deny_reason(), Some("nope"));

        let ask = interpret_result(&HookCommandResult {
            exit_code: 0,
            stdout: r#"{"permissionDecision":"ask","permissionDecisionReason":"confirm"}"#.into(),
            stderr: String::new(),
        });
        assert!(ask.is_ask());

        let stop = interpret_result(&HookCommandResult {
            exit_code: 0,
            stdout: r#"{"continue":false,"stopReason":"halt"}"#.into(),
            stderr: String::new(),
        });
        assert_eq!(stop.deny_reason(), Some("halt"));
    }

    #[tokio::test]
    async fn hook_blocks_via_exit_2_and_sees_payload() {
        let runner = Arc::new(MockRunner::new(2, "", "blocked: dangerous"));
        let hook = ExternalCommandHook {
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".into()),
            action: HookAction {
                action_type: HookActionType::Command,
                command: "validate".into(),
                timeout: None,
                shell: HookCommandShell::Auto,
                env: BTreeMap::new(),
                ..HookAction::default()
            },
            runner: runner.clone(),
        };
        let ctx = HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("shell", serde_json::json!({"command": "rm -rf /"})),
        );
        let out = hook.run(&ctx).await.unwrap();
        assert!(out.is_deny());
        assert_eq!(out.deny_reason(), Some("blocked: dangerous"));

        // The command saw the Claude-Code stdin payload.
        let stdin = runner.last_stdin.lock().unwrap().clone().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&stdin).unwrap();
        assert_eq!(payload["hook_event_name"], "PreToolUse");
        assert_eq!(payload["tool_name"], "shell");
        assert_eq!(payload["tool_input"]["command"], "rm -rf /");
    }

    #[tokio::test]
    async fn post_tool_batch_hook_sees_batch_payload() {
        let runner = Arc::new(MockRunner::new(0, "", ""));
        let hook = ExternalCommandHook {
            event: HookEvent::PostToolBatch,
            matcher: None,
            action: HookAction {
                action_type: HookActionType::Command,
                command: "observe-batch".into(),
                timeout: None,
                shell: HookCommandShell::Auto,
                env: BTreeMap::new(),
                ..HookAction::default()
            },
            runner: runner.clone(),
        };
        let ctx = HookContext::new(
            SessionId::nil(),
            HookPoint::PostToolBatch,
            HookData::tool_batch(vec![
                crate::lifecycle::ToolBatchItem {
                    name: "read_file".into(),
                    call_id: Some("c1".into()),
                    arguments: serde_json::json!({"path": "one.txt"}),
                    ok: true,
                    output_preview: serde_json::json!({"content": "one"}),
                },
                crate::lifecycle::ToolBatchItem {
                    name: "grep".into(),
                    call_id: Some("c2".into()),
                    arguments: serde_json::json!({"pattern": "fn"}),
                    ok: false,
                    output_preview: serde_json::json!({"error": "not found"}),
                },
            ]),
        );

        assert_eq!(hook.run(&ctx).await.unwrap(), HookOutcome::Continue);
        let stdin = runner.last_stdin.lock().unwrap().clone().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&stdin).unwrap();
        assert_eq!(payload["hook_event_name"], "PostToolBatch");
        assert_eq!(payload["tool_count"], 2);
        assert_eq!(
            payload["tool_calls"][0]["tool_name"],
            serde_json::Value::Null
        );
        assert_eq!(payload["tool_calls"][0]["name"], "read_file");
        assert_eq!(payload["tool_calls"][0]["call_id"], "c1");
        assert_eq!(payload["tool_calls"][1]["ok"], false);
        assert_eq!(payload["tool_batch"], payload["tool_calls"]);
    }

    #[tokio::test]
    async fn file_changed_hook_sees_path_and_kind_payload() {
        let runner = Arc::new(MockRunner::new(0, "", ""));
        let hook = ExternalCommandHook {
            event: HookEvent::FileChanged,
            matcher: None,
            action: HookAction {
                action_type: HookActionType::Command,
                command: "observe-file".into(),
                timeout: None,
                shell: HookCommandShell::Auto,
                env: BTreeMap::new(),
                ..HookAction::default()
            },
            runner: runner.clone(),
        };
        let ctx = HookContext::new(
            SessionId::nil(),
            HookPoint::FileChanged,
            HookData::FileChange {
                path: "src/main.rs".to_string(),
                kind: "modified".to_string(),
            },
        );

        assert_eq!(hook.run(&ctx).await.unwrap(), HookOutcome::Continue);
        let stdin = runner.last_stdin.lock().unwrap().clone().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&stdin).unwrap();
        assert_eq!(payload["hook_event_name"], "FileChanged");
        assert_eq!(payload["path"], "src/main.rs");
        assert_eq!(payload["change_kind"], "modified");
    }

    #[tokio::test]
    async fn hook_forwards_action_env_to_runner() {
        let runner = Arc::new(MockRunner::new(0, "", ""));
        let mut env = BTreeMap::new();
        env.insert(
            "DEEPAGENT_PLUGIN_ID".to_string(),
            "demo@personal".to_string(),
        );
        env.insert("CUSTOM".to_string(), "value".to_string());
        let hook = ExternalCommandHook {
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            action: HookAction {
                action_type: HookActionType::Command,
                command: "validate".into(),
                timeout: None,
                shell: HookCommandShell::Powershell,
                env: env.clone(),
                ..HookAction::default()
            },
            runner: runner.clone(),
        };
        let ctx = HookContext::new(
            SessionId::nil(),
            HookPoint::UserPromptSubmit,
            HookData::Prompt {
                text: "hello".to_string(),
            },
        );

        assert_eq!(hook.run(&ctx).await.unwrap(), HookOutcome::Continue);
        assert_eq!(*runner.last_env.lock().unwrap(), Some(env));
    }

    #[tokio::test]
    async fn hook_skips_when_matcher_excludes_tool() {
        let runner = Arc::new(MockRunner::new(2, "", "would block"));
        let hook = ExternalCommandHook {
            event: HookEvent::PreToolUse,
            matcher: Some("Edit|Write".into()),
            action: HookAction {
                action_type: HookActionType::Command,
                command: "validate".into(),
                timeout: None,
                shell: HookCommandShell::Auto,
                env: BTreeMap::new(),
                ..HookAction::default()
            },
            runner,
        };
        // read_file is not in the matcher → hook is skipped → Continue.
        let ctx = HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("read_file", serde_json::json!({"path": "x"})),
        );
        assert_eq!(hook.run(&ctx).await.unwrap(), HookOutcome::Continue);
    }

    #[tokio::test]
    async fn register_into_wires_hooks() {
        let defs = HookDefinitions::parse(SAMPLE).unwrap();
        let runner: Arc<dyn HookCommandRunner> = Arc::new(MockRunner::new(0, "", ""));
        let mut registry = HookRegistry::new();
        let n = defs.register_into(&mut registry, runner);
        assert_eq!(n, 2);
        assert_eq!(registry.count_at(HookPoint::BeforeToolUse), 1);
        assert_eq!(registry.count_at(HookPoint::UserPromptSubmit), 1);
    }

    #[test]
    fn non_command_action_is_skipped() {
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "mcp_tool", "command": "some_tool" } ] }
                ]
            }
        }"#;
        let defs = HookDefinitions::parse(json).unwrap();
        let runner: Arc<dyn HookCommandRunner> = Arc::new(MockRunner::new(0, "", ""));
        let mut registry = HookRegistry::new();
        let n = defs.register_into(&mut registry, runner);
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn register_with_host_executes_mcp_prompt_and_agent_actions() {
        let defs = HookDefinitions::parse(
            r#"{
                "hooks": {
                    "UserPromptSubmit": [{"hooks": [
                        {"type":"mcp_tool","command":"policy__check"},
                        {"type":"prompt","prompt":"Decide: $ARGUMENTS"},
                        {"type":"agent","prompt":"Investigate: $ARGUMENTS"}
                    ]}]
                }
            }"#,
        )
        .unwrap();
        let host = Arc::new(MockHost {
            outcome: HookOutcome::Continue,
            payload: Mutex::new(None),
        });
        let mut registry = HookRegistry::new();
        let count = defs.register_into_with_host(
            &mut registry,
            Arc::new(MockRunner::new(0, "", "")),
            host.clone(),
        );
        assert_eq!(count, 3);
        let outcome = registry
            .dispatch(&HookContext::new(
                SessionId::nil(),
                HookPoint::UserPromptSubmit,
                HookData::prompt("hello"),
            ))
            .await
            .unwrap();
        assert_eq!(outcome, HookOutcome::Continue);
        let payload = host.payload.lock().unwrap().clone().unwrap();
        assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
        assert_eq!(payload["prompt"], "hello");
    }

    #[test]
    fn model_hook_actions_require_a_prompt() {
        let error = HookDefinitions::parse(r#"{"hooks":{"Stop":[{"hooks":[{"type":"prompt"}]}]}}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("prompt cannot be empty"), "{error}");
    }

    #[test]
    fn parses_ok_reason_protocol() {
        assert_eq!(
            parse_structured_hook_output(r#"{"ok":true}"#),
            Some(HookOutcome::Continue)
        );
        let denied = parse_structured_hook_output(r#"{"ok":false,"reason":"unsafe"}"#)
            .expect("structured decision");
        assert_eq!(denied.deny_reason(), Some("unsafe"));
        assert_eq!(denied.source(), Some(DecisionSource::Coordinator));
    }

    #[test]
    fn parses_action_shell() {
        let json = r#"{
            "hooks": {
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "Write-Output ok", "shell": "powershell" } ] }
                ]
            }
        }"#;
        let defs = HookDefinitions::parse(json).unwrap();
        assert_eq!(
            defs.hooks["UserPromptSubmit"][0].hooks[0].shell,
            HookCommandShell::Powershell
        );
    }

    #[test]
    fn base64_encoder_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn system_runner_powershell_reads_stdin_json_with_unicode() {
        let runner = SystemHookRunner;
        let env = BTreeMap::new();
        let result = runner
            .run(
                "$j=[Console]::In.ReadToEnd() | ConvertFrom-Json; Write-Output $j.prompt",
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"删除中文目录"}"#,
                HookCommandShell::Powershell,
                &env,
                Duration::from_secs(10),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
        assert!(result.stdout.contains("删除中文目录"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn system_runner_uses_temp_file_for_long_powershell_hook() {
        let runner = SystemHookRunner;
        let payload = "x".repeat(20_000);
        let command = format!("$value='{payload}'; Write-Output $value.Length");
        let result = runner
            .run(
                &command,
                "{}",
                HookCommandShell::Powershell,
                &BTreeMap::new(),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
        assert_eq!(result.stdout.trim(), "20000");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn system_runner_timeout_terminates_process_tree_promptly() {
        let started = std::time::Instant::now();
        let result = SystemHookRunner
            .run(
                "Start-Sleep -Seconds 30",
                "{}",
                HookCommandShell::Powershell,
                &BTreeMap::new(),
                Duration::from_millis(100),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 124);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn rejects_nested_powershell_launcher() {
        let error = HookDefinitions::parse(
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","shell":"powershell","command":"powershell -NoProfile -Command '$x=1'"}]}]}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("nesting powershell"), "{error}");
    }

    #[test]
    fn rejects_reserved_hook_environment() {
        let error = HookDefinitions::parse(
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo ok","env":{"PATH":"C:\\unsafe"}}]}]}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("reserved variable"), "{error}");
    }

    #[tokio::test]
    async fn http_hook_posts_payload_and_honors_structured_decision() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 4096];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.contains("UserPromptSubmit"));
            assert!(request.contains("check this"));
            let body =
                r#"{"permissionDecision":"ask","permissionDecisionReason":"review HTTP policy"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let hook = ExternalHttpHook {
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            action: HookAction {
                action_type: HookActionType::Http,
                url: format!("http://{address}/hook"),
                timeout: Some(5),
                ..HookAction::default()
            },
            client: reqwest::Client::new(),
        };
        let outcome = hook
            .run(&HookContext::new(
                SessionId::nil(),
                HookPoint::UserPromptSubmit,
                HookData::prompt("check this"),
            ))
            .await
            .unwrap();
        assert!(outcome.is_ask());
        server.await.unwrap();
    }
}
