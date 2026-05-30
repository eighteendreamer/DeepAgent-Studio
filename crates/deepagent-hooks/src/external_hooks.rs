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
//! | `Stop`             | [`HookPoint::SessionEnd`]    |
//! | `SubagentStop`     | [`HookPoint::SessionEnd`]    |
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
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

use crate::hook::{DecisionSource, Hook, HookOutcome};
use crate::lifecycle::{HookContext, HookData, HookPoint};
use crate::registry::HookRegistry;

/// Default command timeout when a hook omits `timeout` (seconds).
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 60;

/// A `hooks.json` lifecycle event (Claude Code naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HookEvent {
    /// Before a tool runs (vetoable).
    PreToolUse,
    /// After a tool ran (observational).
    PostToolUse,
    /// When a user prompt is submitted (vetoable).
    UserPromptSubmit,
    /// When the main agent stops.
    Stop,
    /// When a sub-agent stops.
    SubagentStop,
}

impl HookEvent {
    /// Parse from the `hooks.json` key (e.g. "PreToolUse").
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "Stop" => Some(Self::Stop),
            "SubagentStop" => Some(Self::SubagentStop),
            _ => None,
        }
    }

    /// The `hooks.json` key string.
    pub fn label(&self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::SubagentStop => "SubagentStop",
        }
    }

    /// The runtime lifecycle point this event maps to.
    pub fn point(&self) -> HookPoint {
        match self {
            Self::PreToolUse => HookPoint::BeforeToolUse,
            Self::PostToolUse => HookPoint::AfterToolUse,
            Self::UserPromptSubmit => HookPoint::UserPromptSubmit,
            Self::Stop | Self::SubagentStop => HookPoint::SessionEnd,
        }
    }

    /// Whether `matcher` applies (only the tool events filter by tool name).
    pub fn uses_matcher(&self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }
}

/// The action kind a single hook performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookActionType {
    /// Run an external command (the only executable type today).
    #[default]
    Command,
    /// Invoke an MCP tool (declared but not yet wired — see module docs).
    McpTool,
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
    /// Per-hook timeout in seconds (defaults to [`DEFAULT_HOOK_TIMEOUT_SECS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl HookAction {
    /// The effective timeout duration.
    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS))
    }
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
        serde_json::from_str(json)
            .map_err(|e| CoreError::invalid(format!("invalid hooks.json: {e}")))
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
                        tracing::warn!(
                            event = event.label(),
                            "non-command hook action ('{:?}') is not yet supported; skipping",
                            action.action_type
                        );
                        continue;
                    }
                    if action.command.trim().is_empty() {
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
        timeout: Duration,
    ) -> Result<HookCommandResult>;
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
        timeout: Duration,
    ) -> Result<HookCommandResult> {
        use tokio::io::AsyncWriteExt;

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::other(format!("hook spawn failed: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            let payload = stdin_json.as_bytes().to_vec();
            // Best-effort: ignore broken-pipe if the child closed stdin early.
            let _ = stdin.write_all(&payload).await;
            let _ = stdin.shutdown().await;
        }

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(res) => res.map_err(|e| CoreError::other(format!("hook wait failed: {e}")))?,
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
                self.action.timeout_duration(),
            )
            .await?;

        Ok(interpret_result(&result))
    }
}

/// Map a command result to a [`HookOutcome`] (exit code + structured stdout).
fn interpret_result(result: &HookCommandResult) -> HookOutcome {
    // A structured stdout decision takes precedence when present.
    if let Some(outcome) = parse_structured_stdout(&result.stdout) {
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
fn parse_structured_stdout(stdout: &str) -> Option<HookOutcome> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;

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
            }
        }
    }

    #[async_trait]
    impl HookCommandRunner for MockRunner {
        async fn run(
            &self,
            _command: &str,
            stdin_json: &str,
            _timeout: Duration,
        ) -> Result<HookCommandResult> {
            *self.last_stdin.lock().unwrap() = Some(stdin_json.to_string());
            Ok(self.result.clone())
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
        assert_eq!(HookEvent::Stop.point(), HookPoint::SessionEnd);
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
    async fn hook_skips_when_matcher_excludes_tool() {
        let runner = Arc::new(MockRunner::new(2, "", "would block"));
        let hook = ExternalCommandHook {
            event: HookEvent::PreToolUse,
            matcher: Some("Edit|Write".into()),
            action: HookAction {
                action_type: HookActionType::Command,
                command: "validate".into(),
                timeout: None,
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
}
