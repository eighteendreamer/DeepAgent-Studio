//! DeepAgent Studio desktop shell (Tauri v2).
//!
//! Thin command layer over `deepagent-app-core`. Each `#[command]` delegates to
//! a service and returns the serializable DTOs the React UI consumes. Services:
//! - [`AppService`] — sessions, timeline, commands, diff, fork/rewind/export.
//! - [`SettingsService`] — project init + model discovery (API key → OS keychain).
//! - [`SkillsService`] — skill discovery/install/activation.
//! - [`ChatService`] — streamed chat runs (rooted at the active project).
//! - [`McpService`] — visual MCP server config + live tool registration.
//! - [`ProjectService`] — multi-project registry (folders → sessions).
//! - [`WorkspaceService`] — active-project identity (folder name/path).
//! - [`TerminalService`] — interactive terminal in the active project dir.

use std::sync::{Arc, Mutex};

use deepagent_app_core::{
    AppService, ArchiveProjectResultDto, ArchiveService, ArchivedConversationDto, BalanceDto,
    BudgetConfig, ChatService, CommandDto, ConversationMessageDto, CostService, CostSummary,
    DiagnosticResult, DiffResult, ForkResultDto, KeychainStore, KnowledgeDraftDto, KnowledgeDto,
    KnowledgeHitDto, KnowledgeService, McpServerDto, McpService, ProjectDto, ProjectMapGraphDto,
    ProjectMapHitDto, ProjectMapImpactDto, ProjectMapNeighborsDto, ProjectMapNodeDto,
    ProjectMapOverviewDto, ProjectMapRefreshDto, ProjectMapService, ProjectMapStatusDto,
    ProjectService, RewindResultDto, SessionDetailDto, SessionStateService, SessionSummaryDto,
    SettingsService, SettingsView, SkillActivationDto, SkillDto, SkillsService, TerminalResultDto,
    TerminalService, TranscriptDto, WorkspaceInfoDto, WorkspaceService,
};
use deepagent_models::ReqwestTransport;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

/// Service name used for keychain entries.
const KEYCHAIN_SERVICE: &str = "deepagent-studio";

/// Shared application state.
struct AppState {
    service: Mutex<AppService>,
    settings: Arc<SettingsService>,
    skills: Mutex<SkillsService>,
    chat: Arc<ChatService>,
    mcp: Arc<McpService>,
    knowledge: Arc<KnowledgeService>,
    cost: Arc<CostService>,
    archive: Arc<ArchiveService>,
    session_state: Arc<SessionStateService>,
    projects: Arc<ProjectService>,
    project_map: Arc<ProjectMapService>,
    workspace: Arc<WorkspaceService>,
    terminal: Arc<TerminalService>,
    /// Tokio runtime for async calls invoked from sync commands.
    rt: tokio::runtime::Runtime,
}

#[derive(Debug, Clone, Serialize)]
struct RunEventEnvelope<T: Serialize> {
    run_id: String,
    payload: T,
}

#[derive(Debug, Clone, Serialize)]
struct SessionCompletedPayload {
    run_id: String,
    session_id: Option<String>,
    status: String,
    error: Option<String>,
}

// ---- session / view commands ---------------------------------------------

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummaryDto>, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.list_sessions().map_err(|e| e.to_string())
}

#[tauri::command]
fn session_detail(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionDetailDto, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.session_detail(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn session_conversation(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<ConversationMessageDto>, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.session_conversation(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_session_pinned(
    state: State<'_, AppState>,
    session_id: String,
    pinned: bool,
) -> Result<bool, String> {
    state
        .session_state
        .set_pinned(&session_id, pinned)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn commands(state: State<'_, AppState>, query: String) -> Result<Vec<CommandDto>, String> {
    let mut roots = Vec::new();
    if let Ok(Some(active)) = state.projects.active() {
        roots.push(std::path::PathBuf::from(active));
    }
    roots.push(std::path::PathBuf::from(state.workspace.root_display()));
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    Ok(svc.commands_with_roots(&query, roots))
}

#[tauri::command]
fn compute_diff(
    state: State<'_, AppState>,
    old: String,
    new: String,
) -> Result<DiffResult, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    Ok(svc.diff(&old, &new))
}

#[tauri::command]
fn fork_session(
    state: State<'_, AppState>,
    session_id: String,
    at_seq: u64,
) -> Result<ForkResultDto, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.fork_session(&session_id, at_seq)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn rewind_session(
    state: State<'_, AppState>,
    session_id: String,
    to_seq: u64,
) -> Result<RewindResultDto, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.rewind_session(&session_id, to_seq)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn export_transcript(
    state: State<'_, AppState>,
    session_id: String,
    format: String,
) -> Result<TranscriptDto, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.export_transcript(&session_id, &format)
        .map_err(|e| e.to_string())
}

// ---- settings / initialization commands -----------------------------------

#[tauri::command]
fn initialize_project(state: State<'_, AppState>, api_key: String) -> Result<SettingsView, String> {
    let settings = state.settings.clone();
    state
        .rt
        .block_on(async move { settings.initialize(&api_key).await })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Option<SettingsView>, String> {
    state.settings.view().map_err(|e| e.to_string())
}

#[tauri::command]
fn refresh_models(state: State<'_, AppState>) -> Result<SettingsView, String> {
    let settings = state.settings.clone();
    state
        .rt
        .block_on(async move { settings.refresh_models().await })
        .map_err(|e| e.to_string())
}

/// Query the user's DeepSeek balance via the official `GET /user/balance`
/// endpoint, using the API key from the secret store. Async so the network
/// call doesn't block the main thread.
#[tauri::command]
async fn get_balance(state: State<'_, AppState>) -> Result<BalanceDto, String> {
    let settings = state.settings.clone();
    settings.query_balance().await.map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_api_key(state: State<'_, AppState>) -> Result<(), String> {
    state.settings.clear_key().map_err(|e| e.to_string())
}

/// Switch the active chat model to a discovered model id. Returns the updated
/// (redacted) settings view. The id must be one of `available_models`.
#[tauri::command]
fn set_chat_model(state: State<'_, AppState>, model_id: String) -> Result<SettingsView, String> {
    state
        .settings
        .set_model(deepagent_models::ModelRole::Chat, &model_id)
        .map_err(|e| e.to_string())
}

// ---- skill commands -------------------------------------------------------

#[tauri::command]
fn list_skills(state: State<'_, AppState>) -> Result<Vec<SkillDto>, String> {
    let svc = state.skills.lock().map_err(|e| e.to_string())?;
    Ok(svc.list())
}

#[tauri::command]
fn reload_skills(state: State<'_, AppState>) -> Result<Vec<SkillDto>, String> {
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    svc.reload().map_err(|e| e.to_string())?;
    Ok(svc.list())
}

#[tauri::command]
fn install_skill(state: State<'_, AppState>, source_dir: String) -> Result<SkillDto, String> {
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    svc.install_from_dir(&source_dir).map_err(|e| e.to_string())
}

/// Install a skill from a `.zip` archive: extract to a temp dir, then install
/// the unpacked source (the archive's single top-level folder when present).
#[tauri::command]
fn install_skill_from_zip(
    state: State<'_, AppState>,
    zip_path: String,
) -> Result<SkillDto, String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("temp dir failed: {e}"))?;
    let source = extract_zip(&zip_path, tmp.path()).map_err(|e| e.to_string())?;
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    svc.install_from_dir(&source).map_err(|e| e.to_string())
}

#[tauri::command]
fn uninstall_skill(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    svc.uninstall(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn preview_skill_activation(
    state: State<'_, AppState>,
    query: String,
) -> Result<Option<SkillActivationDto>, String> {
    let svc = state.skills.lock().map_err(|e| e.to_string())?;
    Ok(svc.preview_activation(&query))
}

#[tauri::command]
fn activate_skill(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<SkillActivationDto>, String> {
    let svc = state.skills.lock().map_err(|e| e.to_string())?;
    Ok(svc.activate(&id))
}

// ---- knowledge base -------------------------------------------------------

#[tauri::command]
fn kb_list(state: State<'_, AppState>) -> Result<Vec<KnowledgeDto>, String> {
    Ok(state.knowledge.list())
}

#[tauri::command]
fn kb_search(
    state: State<'_, AppState>,
    query: String,
    kind: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeHitDto>, String> {
    let limit = limit.unwrap_or(10).clamp(1, 50);
    Ok(state.knowledge.search(&query, kind.as_deref(), limit))
}

#[tauri::command]
fn kb_get(state: State<'_, AppState>, id: String) -> Result<Option<KnowledgeDto>, String> {
    Ok(state.knowledge.get(&id))
}

#[tauri::command]
fn kb_save(state: State<'_, AppState>, draft: KnowledgeDraftDto) -> Result<KnowledgeDto, String> {
    state.knowledge.save(draft).map_err(|e| e.to_string())
}

#[tauri::command]
fn kb_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    state.knowledge.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn kb_reload(state: State<'_, AppState>) -> Result<Vec<KnowledgeDto>, String> {
    state.knowledge.reload().map_err(|e| e.to_string())?;
    Ok(state.knowledge.list())
}

#[tauri::command]
fn kb_set_passive(state: State<'_, AppState>, enabled: bool) -> Result<bool, String> {
    state.knowledge.set_passive_enabled(enabled);
    Ok(state.knowledge.passive_enabled())
}

#[tauri::command]
fn kb_passive_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.knowledge.passive_enabled())
}

#[tauri::command]
fn kb_list_drafts(state: State<'_, AppState>) -> Result<Vec<KnowledgeDto>, String> {
    Ok(state.knowledge.list_drafts())
}

#[tauri::command]
fn kb_accept_draft(state: State<'_, AppState>, id: String) -> Result<KnowledgeDto, String> {
    state.knowledge.accept_draft(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn kb_discard_draft(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    state
        .knowledge
        .discard_draft(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn kb_set_auto_capture(state: State<'_, AppState>, enabled: bool) -> Result<bool, String> {
    state.knowledge.set_auto_capture(enabled);
    Ok(state.knowledge.auto_capture_enabled())
}

#[tauri::command]
fn kb_auto_capture_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.knowledge.auto_capture_enabled())
}

// ---- cost tracking + budget -----------------------------------------------

#[tauri::command]
fn get_cost_summary(
    state: State<'_, AppState>,
    session_id: Option<String>,
) -> Result<CostSummary, String> {
    state
        .cost
        .summary(session_id.as_deref().unwrap_or(""))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_budget(
    state: State<'_, AppState>,
    daily_limit: Option<f64>,
    monthly_limit: Option<f64>,
) -> Result<CostSummary, String> {
    state.cost.set_budget(BudgetConfig {
        daily_limit,
        monthly_limit,
    });
    state.cost.summary("").map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_doctor(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<DiagnosticResult>, String> {
    let settings = state.settings.clone();
    let db = {
        let svc = state.service.lock().map_err(|e| e.to_string())?;
        svc.shared_database()
    };
    let root = state
        .projects
        .active()
        .map_err(|e| e.to_string())?
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(state.workspace.info().path));
    let app_data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    Ok(deepagent_app_core::run_diagnostics(&settings, &db, &root, &app_data_dir).await)
}

// ---- chat (streamed) ------------------------------------------------------

#[tauri::command]
async fn run_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    prompt: String,
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<String, String> {
    // IMPORTANT: this command is `async` so Tauri runs it on a worker thread,
    // never the main (UI) thread. A streamed run can pause mid-flight to await
    // a tool-approval decision; that decision arrives via the separate
    // `resolve_approval` command. If `run_chat` blocked the main thread (as a
    // sync command + `block_on` would), `resolve_approval` could never be
    // dispatched and the whole UI would deadlock. We offload the run onto the
    // dedicated async runtime and await its result through a oneshot, keeping
    // every other command responsive while the agent streams.
    let chat = state.chat.clone();
    let run_id = run_id.unwrap_or_else(|| {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        format!("run-{millis}")
    });
    let requested_session_id = session_id.clone();
    let event_emitter = app.clone();
    let approval_emitter = app.clone();
    let completion_emitter = app;
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.rt.spawn(async move {
        let event_run_id = run_id.clone();
        let approval_run_id = run_id.clone();
        let result = chat
            .run_in_session(
                &prompt,
                session_id.as_deref(),
                move |event| {
                    let _ = event_emitter.emit(
                        "chat://event",
                        RunEventEnvelope {
                            run_id: event_run_id.clone(),
                            payload: event,
                        },
                    );
                },
                move |approval| {
                    let _ = approval_emitter.emit(
                        "chat://approval",
                        RunEventEnvelope {
                            run_id: approval_run_id.clone(),
                            payload: approval,
                        },
                    );
                },
            )
            .await;
        // Receiver gone (window closed mid-run) → drop the result silently.
        let completed = match &result {
            Ok(session_id) => SessionCompletedPayload {
                run_id: run_id.clone(),
                session_id: Some(session_id.clone()),
                status: "completed".to_string(),
                error: None,
            },
            Err(error) => SessionCompletedPayload {
                run_id: run_id.clone(),
                session_id: requested_session_id.clone(),
                status: "failed".to_string(),
                error: Some(error.to_string()),
            },
        };
        let _ = completion_emitter.emit("session://completed", completed);
        let _ = tx.send(result);
    });
    match rx.await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err("chat run was cancelled".to_string()),
    }
}

#[tauri::command]
fn resolve_approval(state: State<'_, AppState>, call_id: String, approved: bool) -> bool {
    state
        .chat
        .pending_approvals()
        .resolve_approved(&call_id, approved)
}

/// Request a manual stop of an in-flight run for `session_id`. The run ends
/// cleanly at its next step boundary (partial transcript preserved). Returns
/// whether a matching in-flight run was found.
#[tauri::command]
fn stop_chat(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    Ok(state.chat.cancel_session(&session_id))
}

#[tauri::command]
fn get_plan_mode(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    Ok(state.chat.is_plan_mode(&session_id))
}

#[tauri::command]
fn set_plan_mode(
    state: State<'_, AppState>,
    session_id: String,
    active: bool,
) -> Result<bool, String> {
    Ok(state.chat.set_plan_mode(&session_id, active))
}

#[tauri::command]
fn get_approval_policy(state: State<'_, AppState>) -> Result<String, String> {
    state
        .settings
        .approval_policy()
        .map(|p| p.label().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_approval_policy(state: State<'_, AppState>, policy: String) -> Result<SettingsView, String> {
    let parsed = match policy.as_str() {
        "always_ask" => deepagent_app_core::ApprovalPolicy::AlwaysAsk,
        "auto_review" => deepagent_app_core::ApprovalPolicy::AutoReview,
        "full_access" => deepagent_app_core::ApprovalPolicy::FullAccess,
        other => return Err(format!("unknown approval policy: {other}")),
    };
    state
        .settings
        .set_approval_policy(parsed)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_sandbox_mode(state: State<'_, AppState>) -> Result<String, String> {
    state
        .settings
        .sandbox_mode()
        .map(|m| m.label().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_sandbox_mode(state: State<'_, AppState>, mode: String) -> Result<SettingsView, String> {
    let parsed = match mode.as_str() {
        "read_only" => deepagent_app_core::SandboxMode::ReadOnly,
        "workspace_write" => deepagent_app_core::SandboxMode::WorkspaceWrite,
        "full_access" => deepagent_app_core::SandboxMode::FullAccess,
        other => return Err(format!("unknown sandbox mode: {other}")),
    };
    state
        .settings
        .set_sandbox_mode(parsed)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_thinking_depth(state: State<'_, AppState>, depth: String) -> Result<SettingsView, String> {
    let parsed = match depth.as_str() {
        "simple" => deepagent_models::ThinkingDepth::Simple,
        "medium" => deepagent_models::ThinkingDepth::Medium,
        "deep" => deepagent_models::ThinkingDepth::Deep,
        other => return Err(format!("unknown thinking depth: {other}")),
    };
    state
        .settings
        .set_thinking_depth(parsed)
        .map_err(|e| e.to_string())
}

// ---- MCP server management (visual config) --------------------------------

#[tauri::command]
fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<McpServerDto>, String> {
    state.mcp.list().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_mcp_server(
    state: State<'_, AppState>,
    server: McpServerDto,
) -> Result<McpServerDto, String> {
    state.mcp.upsert(server).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_mcp_server(state: State<'_, AppState>, name: String) -> Result<bool, String> {
    state.mcp.remove(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_mcp_server_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<bool, String> {
    state
        .mcp
        .set_enabled(&name, enabled)
        .map_err(|e| e.to_string())
}

// ---- permission rules + hooks.json ----------------------------------------

#[tauri::command]
fn get_permission_rules(
    state: State<'_, AppState>,
) -> Result<deepagent_app_core::PermissionRules, String> {
    state.settings.permission_rules().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_permission_rules(
    state: State<'_, AppState>,
    rules: deepagent_app_core::PermissionRules,
) -> Result<(), String> {
    state
        .settings
        .set_permission_rules(rules)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_hooks_json(state: State<'_, AppState>) -> Result<String, String> {
    state.settings.hooks_json().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_hooks_json(state: State<'_, AppState>, hooks_json: String) -> Result<(), String> {
    state
        .settings
        .set_hooks_json(&hooks_json)
        .map_err(|e| e.to_string())
}

// ---- workspace (active project) -------------------------------------------

#[tauri::command]
fn workspace_info(state: State<'_, AppState>) -> Result<WorkspaceInfoDto, String> {
    Ok(state.workspace.info())
}

fn resolve_project_root(
    state: &State<'_, AppState>,
    project_path: Option<String>,
) -> Result<std::path::PathBuf, String> {
    if let Some(path) = project_path.filter(|p| !p.trim().is_empty()) {
        return Ok(std::path::PathBuf::from(path));
    }
    Ok(state
        .projects
        .active()
        .map_err(|e| e.to_string())?
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(state.workspace.info().path)))
}

// ---- project map -----------------------------------------------------------

#[tauri::command]
fn project_map_status(
    state: State<'_, AppState>,
    project_path: Option<String>,
) -> Result<ProjectMapStatusDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    Ok(state.project_map.status(&root))
}

#[tauri::command]
fn project_map_overview(
    state: State<'_, AppState>,
    project_path: Option<String>,
) -> Result<ProjectMapOverviewDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    Ok(state.project_map.overview(&root))
}

#[tauri::command]
fn project_map_search(
    state: State<'_, AppState>,
    project_path: Option<String>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ProjectMapHitDto>, String> {
    let root = resolve_project_root(&state, project_path)?;
    state
        .project_map
        .search(&root, &query, limit.unwrap_or(20).clamp(1, 50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_map_node(
    state: State<'_, AppState>,
    project_path: Option<String>,
    node_id: String,
) -> Result<Option<ProjectMapNodeDto>, String> {
    let root = resolve_project_root(&state, project_path)?;
    state
        .project_map
        .node(&root, &node_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_map_neighbors(
    state: State<'_, AppState>,
    project_path: Option<String>,
    node_id: String,
) -> Result<ProjectMapNeighborsDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    state
        .project_map
        .neighbors(&root, &node_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_map_graph(
    state: State<'_, AppState>,
    project_path: Option<String>,
    limit: Option<usize>,
) -> Result<ProjectMapGraphDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    state
        .project_map
        .graph(&root, limit.unwrap_or(80))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_map_impact(
    state: State<'_, AppState>,
    project_path: Option<String>,
    target: String,
) -> Result<ProjectMapImpactDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    state
        .project_map
        .impact(&root, &target)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn project_map_refresh_deep(
    state: State<'_, AppState>,
    project_path: Option<String>,
) -> Result<ProjectMapRefreshDto, String> {
    let root = resolve_project_root(&state, project_path)?;
    let project_map = Arc::clone(&state.project_map);
    tauri::async_runtime::spawn_blocking(move || {
        project_map.refresh_deep(&root).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- projects (sidebar: folders → sessions) -------------------------------

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectDto>, String> {
    state.projects.list().map_err(|e| e.to_string())
}

#[tauri::command]
fn active_project(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state.projects.active().map_err(|e| e.to_string())
}

#[tauri::command]
fn add_project(state: State<'_, AppState>, path: String) -> Result<ProjectDto, String> {
    let dto = state
        .projects
        .add_project(&path)
        .map_err(|e| e.to_string())?;
    state
        .knowledge
        .activate_project(std::path::Path::new(&path))
        .map_err(|e| e.to_string())?;
    Ok(dto)
}

#[tauri::command]
fn set_active_project(state: State<'_, AppState>, path: String) -> Result<(), String> {
    state
        .projects
        .set_active(&path)
        .map_err(|e| e.to_string())?;
    state
        .knowledge
        .activate_project(std::path::Path::new(&path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_project(state: State<'_, AppState>, path: String) -> Result<bool, String> {
    let removed = state
        .projects
        .remove_project(&path)
        .map_err(|e| e.to_string())?;
    if removed {
        if let Some(active) = state.projects.active().map_err(|e| e.to_string())? {
            state
                .knowledge
                .activate_project(std::path::Path::new(&active))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(removed)
}

#[tauri::command]
fn set_project_pinned(
    state: State<'_, AppState>,
    path: String,
    pinned: bool,
) -> Result<ProjectDto, String> {
    state
        .projects
        .set_pinned(&path, pinned)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_project(
    state: State<'_, AppState>,
    path: String,
    name: String,
) -> Result<ProjectDto, String> {
    state
        .projects
        .rename_project(&path, &name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_project_in_file_manager(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = std::process::Command::new("explorer");
        cmd.arg(&path);
        cmd
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(&path);
        cmd
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(&path);
        cmd
    };

    command.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ---- archived conversations ----------------------------------------------

#[tauri::command]
fn archive_project_conversations(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<ArchiveProjectResultDto, String> {
    state
        .archive
        .archive_project(&project_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn archive_conversation(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    let archived = state
        .archive
        .archive_session(&session_id)
        .map_err(|e| e.to_string())?;
    if archived {
        state
            .session_state
            .clear_session(&session_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(archived)
}

#[tauri::command]
fn archive_all_conversations(state: State<'_, AppState>) -> Result<u32, String> {
    let sessions = {
        let svc = state.service.lock().map_err(|e| e.to_string())?;
        svc.list_sessions().map_err(|e| e.to_string())?
    };
    let mut archived_count = 0u32;
    for session in sessions {
        if state
            .archive
            .archive_session(&session.id)
            .map_err(|e| e.to_string())?
        {
            state
                .session_state
                .clear_session(&session.id)
                .map_err(|e| e.to_string())?;
            archived_count += 1;
        }
    }
    Ok(archived_count)
}

#[tauri::command]
fn list_archived_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<ArchivedConversationDto>, String> {
    state.archive.list().map_err(|e| e.to_string())
}

#[tauri::command]
fn unarchive_conversation(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    state
        .archive
        .unarchive_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_archived_conversation(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<bool, String> {
    state
        .archive
        .delete_archived_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_all_archived_conversations(state: State<'_, AppState>) -> Result<u32, String> {
    state.archive.delete_all().map_err(|e| e.to_string())
}

// ---- terminal (interactive command in the active project) -----------------

#[tauri::command]
fn run_terminal(state: State<'_, AppState>, command: String) -> Result<TerminalResultDto, String> {
    let terminal = state.terminal.clone();
    state
        .rt
        .block_on(async move { terminal.run(&command).await })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn terminal_cwd(state: State<'_, AppState>) -> String {
    state.terminal.current_dir()
}

/// Extract `zip_path` into `dest`, returning the directory to install from: the
/// single top-level folder if the archive has exactly one, else `dest` itself.
fn extract_zip(zip_path: &str, dest: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut top_levels = std::collections::BTreeSet::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let Some(path) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        if let Some(first) = path.components().next() {
            top_levels.insert(first.as_os_str().to_string_lossy().into_owned());
        }
        let out = dest.join(&path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut outfile)?;
        }
    }
    if top_levels.len() == 1 {
        let only = top_levels.into_iter().next().unwrap();
        let candidate = dest.join(&only);
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Ok(dest.to_path_buf())
}

/// Entry point invoked by `main.rs`.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            std::fs::create_dir_all(&dir).ok();
            let db_path = dir.join("deepagent.db");

            let service =
                AppService::open(&db_path).map_err(|e| format!("failed to open database: {e}"))?;

            // Settings: share the same DB; key goes to the OS keychain; discovery
            // uses the real reqwest transport against the hard-coded DeepSeek URL.
            let settings_arc = Arc::new(SettingsService::new(
                service.shared_database(),
                Arc::new(ReqwestTransport::new()),
                Arc::new(KeychainStore::new(KEYCHAIN_SERVICE)),
            ));

            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| format!("failed to create tokio runtime: {e}"))?;

            // Skills: discover from the project's `.deepagent/skills` (cwd) and
            // manage installs under the app data dir.
            let workspace_skills = std::env::current_dir()
                .ok()
                .map(|c| c.join(".deepagent").join("skills"));
            let install_dir = dir.join("skills");
            let skills = SkillsService::open(workspace_skills, install_dir)
                .map_err(|e| format!("failed to open skills service: {e}"))?;

            // MCP: visual server management over the shared DB.
            let mcp = Arc::new(McpService::new(service.shared_database()));

            // Workspace + projects: the launch directory is the default project.
            let workspace_root = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
            let workspace = Arc::new(WorkspaceService::new(workspace_root.clone()));
            let projects = Arc::new(ProjectService::new(service.shared_database()));
            let _ = projects.ensure_default(&workspace_root.to_string_lossy());
            let project_map = Arc::new(ProjectMapService::new());

            // Knowledge base: a project-local vault (`<project>/.deepagent/knowledge`)
            // plus a user-global vault (under the app data dir), loaded into one
            // index. Drives passive injection + the knowledge_search/write tools.
            let knowledge = Arc::new(
                KnowledgeService::open(&workspace_root, &dir)
                    .map_err(|e| format!("failed to open knowledge service: {e}"))?,
            );

            // Terminal: interactive commands rooted at the active project.
            let terminal = Arc::new(TerminalService::new(
                projects.clone(),
                workspace_root.to_string_lossy().into_owned(),
            ));

            // Cost tracking: records per-run token cost over the shared DB and
            // enforces optional daily/monthly budget limits.
            let cost = Arc::new(CostService::new(service.shared_database()));

            // Archived conversations: app-level visibility index, separate from
            // the append-only event log.
            let archive = Arc::new(ArchiveService::new(service.shared_database()));
            let session_state = Arc::new(SessionStateService::new(service.shared_database()));

            // Chat: streamed runs; MCP servers connect + live-register tools, each
            // run is rooted at the active project's folder, and the knowledge base
            // is attached for passive injection + active tools.
            let chat = Arc::new(
                ChatService::new(
                    service.shared_database(),
                    settings_arc.clone(),
                    Arc::new(ReqwestTransport::new()),
                    workspace_root,
                )
                .with_mcp(mcp.clone())
                .with_projects(projects.clone())
                .with_knowledge(knowledge.clone())
                .with_project_map(project_map.clone())
                .with_cost(cost.clone())
                .with_tool_results_dir(dir.join("tool_results")),
            );

            app.manage(AppState {
                service: Mutex::new(service),
                settings: settings_arc,
                skills: Mutex::new(skills),
                chat,
                mcp,
                knowledge,
                cost,
                archive,
                session_state,
                projects,
                project_map,
                workspace,
                terminal,
                rt,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            session_detail,
            session_conversation,
            set_session_pinned,
            commands,
            compute_diff,
            fork_session,
            rewind_session,
            export_transcript,
            initialize_project,
            get_settings,
            refresh_models,
            get_balance,
            clear_api_key,
            set_chat_model,
            list_skills,
            reload_skills,
            install_skill,
            install_skill_from_zip,
            uninstall_skill,
            preview_skill_activation,
            activate_skill,
            kb_list,
            kb_search,
            kb_get,
            kb_save,
            kb_delete,
            kb_reload,
            kb_set_passive,
            kb_passive_enabled,
            kb_list_drafts,
            kb_accept_draft,
            kb_discard_draft,
            kb_set_auto_capture,
            kb_auto_capture_enabled,
            get_cost_summary,
            set_budget,
            run_doctor,
            run_chat,
            resolve_approval,
            stop_chat,
            get_plan_mode,
            set_plan_mode,
            get_approval_policy,
            set_approval_policy,
            get_sandbox_mode,
            set_sandbox_mode,
            set_thinking_depth,
            list_mcp_servers,
            save_mcp_server,
            remove_mcp_server,
            set_mcp_server_enabled,
            get_permission_rules,
            set_permission_rules,
            get_hooks_json,
            set_hooks_json,
            workspace_info,
            project_map_status,
            project_map_overview,
            project_map_search,
            project_map_node,
            project_map_neighbors,
            project_map_graph,
            project_map_impact,
            project_map_refresh_deep,
            list_projects,
            active_project,
            add_project,
            set_active_project,
            remove_project,
            set_project_pinned,
            rename_project,
            open_project_in_file_manager,
            archive_project_conversations,
            archive_conversation,
            archive_all_conversations,
            list_archived_conversations,
            unarchive_conversation,
            delete_archived_conversation,
            delete_all_archived_conversations,
            run_terminal,
            terminal_cwd
        ])
        .run(tauri::generate_context!())
        .expect("error while running DeepAgent Studio");
}
