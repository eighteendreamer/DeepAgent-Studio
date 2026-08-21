mod app_server;
mod args;
mod jsonl;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use args::{CliCommand, RunOptions};
use deepagent_app_core::{
    ChatService, DirectSandboxBackend, EnvSecretStore, HarnessRunOverrides, SandboxBackend,
    SandboxBackendCommandExecutor, SandboxBackendKind, SandboxCapabilities, SandboxMode,
    SandboxNetworkPolicy, SandboxieBackend, SandboxieExecutor, SandboxieService,
    WindowsSandboxBackend,
};
use deepagent_harness_protocol::{EventContext, HarnessEvent, PROTOCOL_VERSION};
use deepagent_models::{HttpTransport, ReqwestTransport, DEEPSEEK_OFFICIAL_PROVIDER};
use deepagent_persistence::Database;
use deepagent_runtime::RuntimeEvent;

#[tokio::main]
async fn main() {
    let raw_args: Vec<_> = std::env::args_os().collect();
    let command = match args::parse_args(raw_args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    deepagent_tracing::init(deepagent_tracing::TracingConfig {
        default_directive: "warn,deepagent=info".to_string(),
        format: deepagent_tracing::LogFormat::Pretty,
        with_location: false,
    });

    let result = match command {
        CliCommand::Run(options) => run(options).await,
        CliCommand::ToolsList => tools_list().await,
        CliCommand::SandboxStatus => sandbox_status().await,
        CliCommand::Server { transport } => app_server::run(transport).await,
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(options: RunOptions) -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let (chat, backend_capabilities) = build_chat_service(&workspace, &options)?;
    if let Some(capabilities) = backend_capabilities {
        if !capabilities.available {
            return emit_runtime_error(options.json, "sandbox_unavailable", capabilities.message);
        }
    }

    let context = Arc::new(Mutex::new(EventContext::default()));
    let context_for_events = context.clone();
    let json = options.json;
    let on_event = move |event: RuntimeEvent| {
        if let Err(error) = emit_runtime_event(json, &context_for_events, &event) {
            eprintln!("failed to emit runtime event: {error}");
        }
    };

    let context_for_approval = context.clone();
    let chat_for_approval = chat.clone();
    let on_approval = move |approval: deepagent_app_core::ApprovalRequestDto| {
        let context = context_for_approval
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if json {
            let event = HarnessEvent::ApprovalRequested {
                approval_id: Some(approval.call_id.clone()),
                thread_id: context.thread_id,
                turn_id: context.turn_id,
                tool_name: Some(approval.tool.clone()),
                reason: approval.reason.clone(),
                scope: Some("tool".to_string()),
            };
            if let Ok(line) = jsonl::event_line(&event) {
                println!("{line}");
                let _ = io::stdout().flush();
            }
        } else {
            eprintln!(
                "approval required for {}: {} [{}]",
                approval.tool, approval.reason, approval.call_id
            );
        }

        let approved = read_approval_decision();
        if !chat_for_approval
            .pending_approvals()
            .resolve_approved(&approval.call_id, approved)
        {
            eprintln!(
                "approval request was no longer pending: {}",
                approval.call_id
            );
        }
    };

    let overrides = HarnessRunOverrides {
        provider: options.provider,
        model: options.model,
        reasoning_effort: options.reasoning_effort,
        sandbox_backend: options.sandbox_backend,
        permission_profile: options.permission_profile,
    };
    let prompt = if options.prompt.is_empty() {
        "Continue the task from the existing thread.".to_string()
    } else {
        options.prompt
    };
    let thread_id = chat
        .run_in_session_with_overrides(
            &prompt,
            options.continue_thread.as_deref(),
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
            overrides,
            on_event,
            on_approval,
        )
        .await
        .map_err(|error| {
            if json {
                println!("{}", jsonl::error_line("run_failed", error.to_string()));
            }
            error.to_string()
        })?;

    if !json {
        println!("thread: {thread_id}");
    }
    Ok(())
}

async fn tools_list() -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let (chat, _) = build_chat_service(
        &workspace,
        &RunOptions {
            prompt: String::new(),
            continue_thread: None,
            json: true,
            provider: Some(DEEPSEEK_OFFICIAL_PROVIDER.to_string()),
            model: None,
            sandbox_backend: Some("direct".to_string()),
            permission_profile: None,
            reasoning_effort: None,
        },
    )?;
    let descriptors = chat.tool_descriptors().map_err(|error| error.to_string())?;
    let payload = serde_json::json!({
        "type": "tool.list",
        "protocolVersion": PROTOCOL_VERSION,
        "tools": descriptors,
    });
    println!(
        "{}",
        serde_json::to_string(&payload).map_err(|error| error.to_string())?
    );
    Ok(())
}

async fn sandbox_status() -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let options = RunOptions {
        prompt: String::new(),
        continue_thread: None,
        json: true,
        provider: None,
        model: None,
        sandbox_backend: None,
        permission_profile: None,
        reasoning_effort: None,
    };
    let (_, capabilities) = build_chat_service(&workspace, &options)?;
    let capabilities = capabilities.unwrap_or_else(|| SandboxCapabilities {
        kind: SandboxBackendKind::Direct,
        available: true,
        supports_one_shot: true,
        supports_interactive_pty: true,
        supports_network_toggle: false,
        supports_readonly_mapping: false,
        message: "direct host execution".to_string(),
    });
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "type": "sandbox.status",
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": capabilities,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn build_chat_service(
    workspace: &Path,
    options: &RunOptions,
) -> Result<(ChatService, Option<SandboxCapabilities>), String> {
    let db_path = database_path(workspace)?;
    let db = Arc::new(Database::open(&db_path).map_err(|error| format!("open database: {error}"))?);
    let transport: Arc<dyn HttpTransport> = Arc::new(ReqwestTransport::new());
    let settings = Arc::new(deepagent_app_core::SettingsService::new(
        db.clone(),
        transport.clone(),
        Arc::new(EnvSecretStore::new()),
    ));
    let sandbox_mode = sandbox_mode_for_profile(options.permission_profile.as_deref())
        .or_else(|| settings.sandbox_mode().ok())
        .unwrap_or(SandboxMode::WorkspaceWrite);
    let backend_kind = options
        .sandbox_backend
        .as_deref()
        .map(SandboxBackendKind::parse)
        .unwrap_or(Some(SandboxBackendKind::Direct))
        .ok_or_else(|| {
            format!(
                "unsupported sandbox backend: {}",
                options.sandbox_backend.as_deref().unwrap_or_default()
            )
        })?;

    let (backend, capabilities, sandboxie) = build_backend(backend_kind);
    let executor = SandboxBackendCommandExecutor::new(backend, workspace, sandbox_mode)
        .with_network(SandboxNetworkPolicy::Disabled);
    let mut chat = ChatService::new(db, settings, transport, workspace)
        .with_local_command_executor(Arc::new(executor));
    if let Some(sandboxie) = sandboxie {
        chat = chat.with_sandboxie_executor(sandboxie);
    }
    Ok((chat, Some(capabilities)))
}

fn build_backend(
    kind: SandboxBackendKind,
) -> (
    Arc<dyn SandboxBackend>,
    SandboxCapabilities,
    Option<Arc<SandboxieExecutor>>,
) {
    match kind {
        SandboxBackendKind::Direct => {
            let backend = Arc::new(DirectSandboxBackend::new());
            (backend.clone(), backend.capabilities(), None)
        }
        SandboxBackendKind::Sandboxie => {
            let service = Arc::new(SandboxieService::new(None));
            let executor = Arc::new(SandboxieExecutor::new(service));
            let backend = Arc::new(SandboxieBackend::new(executor.clone()));
            (backend.clone(), backend.capabilities(), Some(executor))
        }
        SandboxBackendKind::WindowsSandbox => {
            let backend = Arc::new(WindowsSandboxBackend::new(
                std::env::temp_dir().join("deepagent-windows-sandbox"),
            ));
            (backend.clone(), backend.capabilities(), None)
        }
    }
}

fn database_path(workspace: &Path) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DEEPAGENT_DB_PATH") {
        return Ok(PathBuf::from(path));
    }
    let directory = workspace.join(".deepagent");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    Ok(directory.join("deepagent.db"))
}

fn sandbox_mode_for_profile(profile: Option<&str>) -> Option<SandboxMode> {
    match profile {
        Some("read_only") => Some(SandboxMode::ReadOnly),
        Some("workspace_write") => Some(SandboxMode::WorkspaceWrite),
        Some("developer" | "full_access") => Some(SandboxMode::FullAccess),
        _ => None,
    }
}

fn emit_runtime_event(
    json: bool,
    context: &Arc<Mutex<EventContext>>,
    event: &RuntimeEvent,
) -> Result<(), String> {
    let mut guard = context
        .lock()
        .map_err(|_| "event context lock poisoned".to_string())?;
    let line = jsonl::project_runtime_event_line(event, &guard)?;
    if let Some(line) = line {
        if json {
            println!("{line}");
            let _ = io::stdout().flush();
        } else {
            print_human_event(event);
        }
    }
    match event {
        RuntimeEvent::SessionRegistered { session_id, .. } => {
            guard.thread_id = Some(session_id.clone());
        }
        RuntimeEvent::RunStarted { task_id } => {
            guard.turn_id = Some(task_id.clone());
        }
        _ => {}
    }
    Ok(())
}

fn emit_runtime_error(json: bool, code: &str, message: String) -> Result<(), String> {
    if json {
        println!("{}", jsonl::error_line(code, &message));
    }
    Err(message)
}

fn print_human_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::ContentDelta { text } => print!("{text}"),
        RuntimeEvent::ReasoningDelta { .. } => {}
        RuntimeEvent::RunCompleted { message } => println!("\n{message}"),
        RuntimeEvent::RunFailed { reason } => eprintln!("run failed: {reason}"),
        RuntimeEvent::RunCancelled => eprintln!("run interrupted"),
        _ => {}
    }
    let _ = io::stdout().flush();
}

fn read_approval_decision() -> bool {
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    let trimmed = line.trim();
    if trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("y")
    {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|value| value.get("approved").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}
