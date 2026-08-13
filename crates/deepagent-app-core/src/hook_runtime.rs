use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{
    parse_structured_hook_output, HookAction, HookActionExecutor, HookActionType,
    HookCommandResult, HookCommandRunner, HookCommandShell, HookContext, HookData, HookDefinitions,
    HookEvent, HookOutcome, HookRegistry, SystemHookRunner, ToolBatchItem,
};
use deepagent_models::{ModelClient, ResponseRequest, ThinkingDepth, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::{ModelAgent, RuntimeConfig, RuntimeEngine, RuntimeEvent, RuntimeEventSink};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, RiskLevel, ToolRegistry};

static HOOK_EVENT_SEQ: AtomicU64 = AtomicU64::new(1);

/// Result of a settings-page hook test run (any of the 5 action types).
#[derive(Debug, Clone)]
pub struct HookActionTestResult {
    /// `continued` | `blocked` | `approval_required` | `modified` | `error`.
    pub outcome: String,
    /// Reason / error / modification detail (bounded).
    pub detail: String,
    /// Wall-clock duration of the dispatch.
    pub duration_ms: u64,
}

/// Run one declared hook action against a synthetic lifecycle payload,
/// through the SAME registration + dispatch path production uses
/// ([`HookDefinitions::register_into_with_host`] → [`HookRegistry::dispatch`]).
/// Supports all 5 action types: `command` and `http` execute directly;
/// `prompt`/`agent` need `model` (client, id, thinking depth); `mcp_tool`
/// needs `mcp`. Missing dependencies surface as an `error` outcome rather
/// than a panic so the settings page can explain what to configure.
pub async fn test_hook_action(
    event: &str,
    matcher: Option<&str>,
    action: HookAction,
    model: Option<(Arc<ModelClient>, String, ThinkingDepth)>,
    mcp: Option<Arc<deepagent_mcp::McpRegistry>>,
) -> Result<HookActionTestResult> {
    let hook_event = HookEvent::parse(event)
        .ok_or_else(|| CoreError::invalid(format!("unknown hook event: {event}")))?;
    let defs_json = serde_json::json!({
        "hooks": {
            hook_event.label(): [{
                "matcher": matcher,
                "hooks": [serde_json::to_value(&action)?],
            }],
        }
    });
    let defs = HookDefinitions::parse(&defs_json.to_string())?;

    let mut registry = HookRegistry::new();
    let runner: Arc<dyn HookCommandRunner> = Arc::new(SystemHookRunner);
    // Keep the no-op sink alive for the duration of the dispatch — the
    // executor only holds a Weak reference.
    let sink: Arc<dyn RuntimeEventSink> = Arc::new(NoopEventSink);
    let host: Arc<dyn HookActionExecutor> = match model {
        Some((client, model, thinking_depth)) => Arc::new(AppHookActionExecutor {
            client,
            model,
            thinking_depth,
            mcp,
            agent_registry: Arc::new(ToolRegistry::new()),
            events: Arc::downgrade(&sink),
        }),
        None => Arc::new(UnavailableHookHost),
    };
    let registered = defs.register_into_with_host(&mut registry, runner, host);
    if registered == 0 {
        return Err(CoreError::invalid(
            "hook action could not be registered (invalid event or action)",
        ));
    }

    let data = sample_hook_data(hook_event, matcher);
    let started = Instant::now();
    let dispatched = registry
        .dispatch(&HookContext::new(
            deepagent_core::id::SessionId::nil(),
            hook_event.point(),
            data,
        ))
        .await;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    drop(sink);

    let (outcome, detail) = match dispatched {
        Ok(HookOutcome::Continue) => ("continued".to_string(), String::new()),
        Ok(HookOutcome::Modify { updated_input, .. }) => (
            "modified".to_string(),
            bounded_hook_text(&updated_input.to_string(), 2_000),
        ),
        Ok(HookOutcome::Ask { reason, .. }) => (
            "approval_required".to_string(),
            bounded_hook_text(&reason, 2_000),
        ),
        Ok(HookOutcome::Deny { reason, .. }) => {
            ("blocked".to_string(), bounded_hook_text(&reason, 2_000))
        }
        Err(error) => (
            "error".to_string(),
            bounded_hook_text(&error.to_string(), 2_000),
        ),
    };
    Ok(HookActionTestResult {
        outcome,
        detail,
        duration_ms,
    })
}

/// Synthetic, event-appropriate payload for a settings-page test dispatch.
/// Covers every [`HookEvent`] so "test run" works for the full lifecycle,
/// not only the 5 legacy events.
fn sample_hook_data(event: HookEvent, matcher: Option<&str>) -> HookData {
    let tool_name = matcher
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "*" && *value != ".*")
        .and_then(|value| value.split('|').next())
        .unwrap_or("bash")
        .to_string();
    let tool_arguments = serde_json::json!({ "command": "echo hook-test" });
    match event {
        HookEvent::PreToolUse => HookData::before_tool(tool_name, tool_arguments),
        HookEvent::PostToolUse => HookData::after_tool(tool_name, tool_arguments, true),
        HookEvent::PostToolUseFailure => HookData::after_tool(tool_name, tool_arguments, false),
        HookEvent::PostToolBatch => HookData::tool_batch(vec![ToolBatchItem {
            name: tool_name,
            call_id: Some("call_test".to_string()),
            arguments: tool_arguments,
            ok: true,
            output_preview: serde_json::json!({ "ok": true }),
        }]),
        HookEvent::PermissionRequest | HookEvent::PermissionDenied => HookData::Permission {
            tool: tool_name,
            arguments: tool_arguments,
            reason: "settings-page hook test".to_string(),
        },
        HookEvent::UserPromptSubmit => HookData::prompt("Test prompt from hooks settings"),
        HookEvent::Stop | HookEvent::StopFailure => HookData::Response {
            content: "Test session completed".to_string(),
        },
        HookEvent::PreCompact => HookData::Compact {
            trigger: "manual_test".to_string(),
            summary: None,
        },
        HookEvent::PostCompact => HookData::Compact {
            trigger: "manual_test".to_string(),
            summary: Some("Test summary".to_string()),
        },
        HookEvent::SubagentStart => HookData::Subagent {
            agent_id: "sub_test".to_string(),
            agent_type: "general".to_string(),
            summary: None,
        },
        HookEvent::SubagentStop => HookData::Subagent {
            agent_id: "sub_test".to_string(),
            agent_type: "general".to_string(),
            summary: Some("Test subagent completed".to_string()),
        },
        HookEvent::TaskCreated | HookEvent::TaskCompleted => HookData::Task {
            task_id: "task_test".to_string(),
            subject: "Test task".to_string(),
        },
        HookEvent::WorktreeCreate | HookEvent::WorktreeRemove | HookEvent::CwdChanged => {
            HookData::Path {
                path: "test-worktree".to_string(),
            }
        }
        HookEvent::FileChanged => HookData::FileChange {
            path: "src/main.rs".to_string(),
            kind: "modified".to_string(),
        },
        HookEvent::Notification => HookData::Notification {
            message: "Test notification from hooks settings".to_string(),
        },
        HookEvent::InstructionsLoaded => HookData::Instructions {
            paths: vec!["CLAUDE.md".to_string()],
        },
        HookEvent::SessionStart | HookEvent::SessionEnd => HookData::None,
    }
}

/// Host executor used when no model client is configured: mcp/prompt/agent
/// hook tests fail with an actionable message instead of panicking.
struct UnavailableHookHost;

#[async_trait]
impl HookActionExecutor for UnavailableHookHost {
    async fn execute(
        &self,
        action: &HookAction,
        _payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        Err(CoreError::invalid(format!(
            "{:?} hook test requires a configured model/API key",
            action.action_type
        )))
    }
}

struct NoopEventSink;

impl RuntimeEventSink for NoopEventSink {
    fn emit(&self, _event: RuntimeEvent) {}
}

pub(crate) struct ObservableHookRunner {
    inner: SystemHookRunner,
    events: std::sync::Weak<dyn RuntimeEventSink>,
    cwd: Option<PathBuf>,
    runtime_environment: BTreeMap<String, String>,
}

pub(crate) struct AppHookActionExecutor {
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) thinking_depth: ThinkingDepth,
    pub(crate) mcp: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(crate) agent_registry: Arc<ToolRegistry>,
    pub(crate) events: std::sync::Weak<dyn RuntimeEventSink>,
}

impl AppHookActionExecutor {
    async fn execute_inner(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        match action.action_type {
            HookActionType::McpTool => self.execute_mcp(action, payload).await,
            HookActionType::Prompt => self.execute_prompt(action, payload).await,
            HookActionType::Agent => self.execute_agent(action, payload).await,
            HookActionType::Command | HookActionType::Http => Err(CoreError::invalid(
                "command/http hooks cannot be executed by the host action executor",
            )),
        }
    }

    pub(crate) async fn execute_mcp(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        let registry = self.mcp.as_ref().ok_or_else(|| {
            CoreError::other("MCP hook requested but no MCP registry is connected")
        })?;
        let arguments = if let Some(static_arguments) = action.arguments.as_object() {
            let mut arguments = static_arguments.clone();
            arguments.insert("hook_input".to_string(), payload);
            serde_json::Value::Object(arguments)
        } else {
            payload
        };
        let result = registry.invoke(action.command.trim(), arguments).await?;
        let output = bounded_hook_text(&result.text(), 8_192);
        if result.is_error {
            return Err(CoreError::other(if output.is_empty() {
                "MCP hook tool returned an error".to_string()
            } else {
                format!("MCP hook tool returned an error: {output}")
            }));
        }
        Ok(parse_structured_hook_output(&output).unwrap_or(HookOutcome::Continue))
    }

    pub(crate) async fn execute_prompt(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        let prompt = render_model_hook_prompt(action, &payload)?;
        let model = action
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.model);
        let request = ResponseRequest::new(
            model,
            vec![
                deepagent_core::message::Message::system(model_hook_system_prompt(false)),
                deepagent_core::message::Message::user(prompt),
            ],
        )
        .with_temperature(0.0)
        .with_max_output_tokens(512)
        .with_thinking_depth(ThinkingDepth::Simple);
        let response = self.client.stream_response(request).await?;
        parse_model_hook_decision(&response.output_text_projection())
    }

    pub(crate) async fn execute_agent(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        let prompt = render_model_hook_prompt(action, &payload)?;
        let model = action
            .model
            .clone()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| self.model.clone());
        let profile = action.agent.as_deref().unwrap_or("general");
        let system = format!(
            "{}\nYou are the isolated '{profile}' hook agent. Use only the supplied read-only tools. Never modify files, run commands, or create another agent.",
            model_hook_system_prompt(true)
        );
        let permissions = PermissionSet::developer();
        let tools = self
            .agent_registry
            .visible_to(&permissions)
            .into_iter()
            .map(|descriptor| {
                ToolSchema::function(
                    descriptor.name,
                    descriptor.description,
                    descriptor.parameters,
                )
            })
            .collect();
        let db = Database::open_in_memory()?;
        let mut session = Session::create(&db, &SystemClock, Some("hook-agent"))?;
        let task = session.create_task(&prompt)?;
        let mut agent = ModelAgent::new(self.client.clone(), model, system, prompt, tools)
            .with_thinking_depth(self.thinking_depth)
            .with_max_model_attempts(2);
        let config = RuntimeConfig {
            max_steps: 6,
            task_timeout: Some(action.timeout_duration()),
            max_total_tokens: Some(32_000),
            auto_approve: false,
            permissions,
            ..RuntimeConfig::default()
        };
        let engine: RuntimeEngine<'_, SystemClock> =
            RuntimeEngine::new(&self.agent_registry, Default::default(), config);
        match engine.run(&mut session, task, &mut agent).await? {
            deepagent_runtime::RunOutcome::Completed(output) => parse_model_hook_decision(&output),
            deepagent_runtime::RunOutcome::Cancelled => {
                Err(CoreError::other("agent hook was cancelled"))
            }
            other => Err(CoreError::other(format!(
                "agent hook did not produce a decision: {other:?}"
            ))),
        }
    }

    fn action_label(action: &HookAction) -> String {
        match action.action_type {
            HookActionType::McpTool => format!("mcp:{}", action.command.trim()),
            HookActionType::Prompt => {
                format!("prompt:{}", action.model.as_deref().unwrap_or("default"))
            }
            HookActionType::Agent => {
                format!("agent:{}", action.agent.as_deref().unwrap_or("general"))
            }
            HookActionType::Command => "command".to_string(),
            HookActionType::Http => "http".to_string(),
        }
    }
}

#[async_trait]
impl HookActionExecutor for AppHookActionExecutor {
    async fn execute(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        let id = format!("hook-{}", HOOK_EVENT_SEQ.fetch_add(1, Ordering::Relaxed));
        let event = payload
            .get("hook_event_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Hook")
            .to_string();
        let label = Self::action_label(action);
        if let Some(events) = self.events.upgrade() {
            events.emit(RuntimeEvent::HookStarted {
                id: id.clone(),
                event: event.clone(),
                shell: None,
                command: label.clone(),
            });
        }
        let started = Instant::now();
        let result = self.execute_inner(action, payload).await;
        if let Some(events) = self.events.upgrade() {
            let (exit_code, stderr, outcome) = match &result {
                Ok(HookOutcome::Continue | HookOutcome::Modify { .. }) => {
                    (0, String::new(), "continued".to_string())
                }
                Ok(HookOutcome::Ask { reason, .. }) => (
                    0,
                    bounded_hook_text(reason, 2_000),
                    "approval_required".to_string(),
                ),
                Ok(HookOutcome::Deny { reason, .. }) => {
                    (2, bounded_hook_text(reason, 2_000), "blocked".to_string())
                }
                Err(error) => (
                    -1,
                    bounded_hook_text(&error.to_string(), 2_000),
                    "error".to_string(),
                ),
            };
            events.emit(RuntimeEvent::HookCompleted {
                id,
                event,
                shell: None,
                command: label,
                exit_code,
                stdout: String::new(),
                stderr,
                outcome,
                duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            });
        }
        result
    }
}

fn model_hook_system_prompt(agent: bool) -> &'static str {
    if agent {
        "Evaluate the hook input and return exactly one JSON object: {\"ok\":true} or {\"ok\":false,\"reason\":\"concise factual reason\"}. Do not return markdown or prose."
    } else {
        "You are a deterministic lifecycle hook. Return exactly one JSON object: {\"ok\":true} or {\"ok\":false,\"reason\":\"concise factual reason\"}. Do not return markdown or prose."
    }
}

pub(crate) fn render_model_hook_prompt(
    action: &HookAction,
    payload: &serde_json::Value,
) -> Result<String> {
    let input = serde_json::to_string(payload)?;
    if input.chars().count() > 65_536 {
        return Err(CoreError::invalid(
            "hook lifecycle payload exceeds 65536 characters",
        ));
    }
    let template = if action.prompt.trim().is_empty() {
        action.command.trim()
    } else {
        action.prompt.trim()
    };
    Ok(if template.contains("$ARGUMENTS") {
        template.replace("$ARGUMENTS", &input)
    } else {
        format!("{template}\n\nHook input JSON:\n{input}")
    })
}

pub(crate) fn parse_model_hook_decision(output: &str) -> Result<HookOutcome> {
    let output = bounded_hook_text(output, 8_192);
    parse_structured_hook_output(&output).ok_or_else(|| {
        CoreError::invalid(
            "model-backed hook must return JSON {\"ok\": boolean, \"reason\"?: string}",
        )
    })
}

fn bounded_hook_text(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut output = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        output.push_str("...[truncated]");
    }
    output
}

pub(crate) fn build_hook_agent_registry(source: &ToolRegistry) -> Result<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    for spec in source.iter_specs().filter(|spec| {
        spec.descriptor.risk == RiskLevel::Safe
            && spec.descriptor.name != "task"
            && spec.descriptor.name != "shell"
    }) {
        registry.register(spec.tool.clone())?;
    }
    Ok(registry)
}

impl ObservableHookRunner {
    pub(crate) fn new(
        events: Arc<dyn RuntimeEventSink>,
        runtime_environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            inner: SystemHookRunner,
            events: Arc::downgrade(&events),
            cwd: None,
            runtime_environment,
        }
    }

    pub(crate) fn new_in_dir(
        events: Arc<dyn RuntimeEventSink>,
        cwd: PathBuf,
        runtime_environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            inner: SystemHookRunner,
            events: Arc::downgrade(&events),
            cwd: Some(cwd),
            runtime_environment,
        }
    }
}

#[async_trait]
impl HookCommandRunner for ObservableHookRunner {
    async fn run(
        &self,
        command: &str,
        stdin_json: &str,
        shell: HookCommandShell,
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<HookCommandResult> {
        let id = format!("hook-{}", HOOK_EVENT_SEQ.fetch_add(1, Ordering::Relaxed));
        let event = hook_event_name(stdin_json);
        if let Some(events) = self.events.upgrade() {
            events.emit(RuntimeEvent::HookStarted {
                id: id.clone(),
                event: event.clone(),
                shell: Some(shell.resolved_label().to_string()),
                command: command.to_string(),
            });
        }
        let started = Instant::now();
        let mut effective_env = self.runtime_environment.clone();
        effective_env.extend(env.clone());
        let result = if let Some(cwd) = &self.cwd {
            self.inner
                .run_in_dir(command, stdin_json, shell, &effective_env, timeout, cwd)
                .await
        } else {
            self.inner
                .run(command, stdin_json, shell, &effective_env, timeout)
                .await
        };
        let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        if let Ok(result) = &result {
            if let Some(events) = self.events.upgrade() {
                events.emit(RuntimeEvent::HookCompleted {
                    id,
                    event,
                    shell: Some(shell.resolved_label().to_string()),
                    command: command.to_string(),
                    exit_code: result.exit_code,
                    stdout: result.stdout.clone(),
                    stderr: result.stderr.clone(),
                    outcome: hook_command_outcome(result),
                    duration_ms,
                });
            }
        }
        result
    }
}

fn hook_event_name(stdin_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(stdin_json)
        .ok()
        .and_then(|value| {
            value
                .get("hook_event_name")
                .and_then(|event| event.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "Hook".to_string())
}

fn hook_command_outcome(result: &HookCommandResult) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(result.stdout.trim()) {
        if value
            .get("decision")
            .and_then(|decision| decision.as_str())
            .is_some_and(|decision| decision.eq_ignore_ascii_case("block"))
        {
            return "blocked".to_string();
        }
    }
    match result.exit_code {
        0 => "continued".to_string(),
        2 => "blocked".to_string(),
        _ => "error".to_string(),
    }
}
