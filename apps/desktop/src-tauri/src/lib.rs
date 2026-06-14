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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use deepagent_app_core::{
    AppService, ArchiveProjectResultDto, ArchiveService, ArchivedConversationDto, BalanceDto,
    BudgetConfig, ChatService, CommandDto, ConversationMessageDto, CostService, CostSummary,
    DiagnosticResult, DiffResult, ForkResultDto, KeychainStore, KnowledgeDraftDto, KnowledgeDto,
    KnowledgeHitDto, KnowledgeService, McpServerDto, McpService, ProjectDto, ProjectMapGraphDto,
    ProjectMapHitDto, ProjectMapImpactDto, ProjectMapNeighborsDto, ProjectMapNodeDto,
    ProjectMapOverviewDto, ProjectMapRefreshDto, ProjectMapService, ProjectMapStatusDto,
    ProjectService, RewindResultDto, SecretStore, SessionDetailDto, SessionStateService,
    SessionSummaryDto, SettingsService, SettingsView, SkillActivationDto, SkillDto,
    SkillsMpClientHandle, SkillsRoots, SkillsService, TerminalResultDto, TerminalService,
    TranscriptDto, WorkspaceInfoDto, WorkspaceService,
};
use deepagent_models::ReqwestTransport;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

/// Service name used for keychain entries.
const KEYCHAIN_SERVICE: &str = "deepagent-studio";

/// Keychain entry name for the user-supplied SkillsMP API key. Stored under
/// the same `KEYCHAIN_SERVICE` as the DeepSeek API key, so both live together
/// in Windows Credential Manager / macOS Keychain / Linux Secret Service.
const KEYCHAIN_SKILLSMP_KEY_NAME: &str = "skillsmp_api_key";

/// Shared application state.
struct AppState {
    service: Mutex<AppService>,
    settings: Arc<SettingsService>,
    /// Shared skills service. Wrapped in `Arc<Mutex>` so the chat service
    /// (`with_skills`) and the Tauri command layer share the same registry —
    /// installing a skill via `install_skill` / `skill_market_install`
    /// becomes visible to the next chat run on its first turn (subject to
    /// the per-session send-once tracker, which the install paths reset
    /// via [`ChatService::reset_all_sent_skills`] so the new skill is
    /// announced in the next turn's catalog reminder).
    skills: Arc<Mutex<SkillsService>>,
    /// SkillsMP marketplace client wrapped in a swap-able handle so the Tauri
    /// command layer can replace the inner `SkillsMpClient` (rebuilt with a
    /// different API key) without rebuilding the rest of `AppState`.
    skillsmp: Arc<SkillsMpClientHandle>,
    /// Holders for downloaded-but-not-yet-installed skill scans. Keyed by an
    /// opaque temp_id returned from `skill_market_scan`; the entry owns the
    /// `TempSkillDir` so the extracted files survive on disk until the user
    /// confirms install (`skill_market_install`) or cancels
    /// (`skill_market_cancel`). Stale entries (>30 min) are reaped lazily by
    /// every command that touches the map.
    pending_scans: Arc<Mutex<HashMap<String, PendingScan>>>,
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

/// Maximum idle time for an entry in [`AppState::pending_scans`] before the
/// reaper drops it.
const PENDING_SCAN_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// One entry in [`AppState::pending_scans`].
///
/// Owns the [`TempSkillDir`] handle so the extracted skill files stay on disk
/// until the user confirms or cancels — dropping `temp` removes the temp
/// directory. Caches the [`ScanReport`] produced by the scanner so a second
/// frontend read (e.g. when re-rendering the install dialog) does not have to
/// re-scan the disk.
struct PendingScan {
    /// Owning handle to the temp dir — kept alive so the extracted files
    /// stay on disk until the user confirms or cancels.
    temp: deepagent_app_core::TempSkillDir,
    /// Cached scan report (so multiple frontend reads don't re-scan).
    report: deepagent_app_core::ScanReport,
    /// When the scan was initiated. Stale entries (>30 min) are reaped.
    created_at: std::time::Instant,
}

/// Drop entries older than [`PENDING_SCAN_TTL`] from `pending`. Pull-based:
/// every `skill_market_*` command that touches the map calls this first, so
/// no background tokio task is needed (no thread leak at shutdown).
fn reap_pending_scans(pending: &Arc<Mutex<HashMap<String, PendingScan>>>) {
    if let Ok(mut map) = pending.lock() {
        let now = std::time::Instant::now();
        map.retain(|_, scan| {
            now.checked_duration_since(scan.created_at)
                .map(|elapsed| elapsed < PENDING_SCAN_TTL)
                .unwrap_or(true)
        });
    }
}

/// Generate an opaque temp-id for a `skill_market_scan` entry.
///
/// Uses `SystemTime` millis + a process-local atomic counter so we don't pull
/// in `uuid` as a direct dependency. The id is opaque to the frontend; only
/// uniqueness within the running process matters (the map is in-memory).
fn new_temp_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("scan-{millis}-{n}")
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
    let dtos = svc.list();
    drop(svc);
    // Skill set may have changed — clear every session's send-once
    // tracker so the next turn re-announces the full visible registry.
    state.chat.reset_all_sent_skills();
    Ok(dtos)
}

#[tauri::command]
fn install_skill(state: State<'_, AppState>, source_dir: String) -> Result<SkillDto, String> {
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    let dto = svc.install_from_dir(&source_dir).map_err(|e| e.to_string())?;
    drop(svc);
    state.chat.reset_all_sent_skills();
    Ok(dto)
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
    let dto = svc.install_from_dir(&source).map_err(|e| e.to_string())?;
    drop(svc);
    state.chat.reset_all_sent_skills();
    Ok(dto)
}

#[tauri::command]
fn uninstall_skill(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    let removed = svc.uninstall(&id).map_err(|e| e.to_string())?;
    drop(svc);
    if removed {
        state.chat.reset_all_sent_skills();
    }
    Ok(removed)
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

// ---- skill marketplace (skillsmp.com + GitHub) ----------------------------
//
// The 9 `skill_market_*` commands implement the marketplace install flow
// described in design.md §Tauri 命令清单 (R3.1, R4.1, R4.9-R4.12, R9.1, R9.2,
// R9.4-R9.6).
//
// Async-safe lock discipline: `SkillsMpClientHandle::with_client` holds a
// `std::sync::Mutex` for the duration of its closure. Every async command
// here clones the inner `SkillsMpClient` *out* of the closure (cheap — the
// reqwest client is `Arc`-backed and the rate-limit cache is shared via
// `Arc<Mutex<…>>`) so the handle's mutex is always released before any
// `.await`. Holding a sync mutex across an await would stall the executor.

/// Search-side input passed by the frontend `skillMarketSearch` wrapper.
/// Empty / `None` fields are dropped on the wire by [`SkillsMpClient::search`]
/// so the SkillsMP defaults apply.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketSearchInput {
    q: Option<String>,
    page: Option<u32>,
    limit: Option<u32>,
    sort_by: Option<deepagent_app_core::SortBy>,
    category: Option<String>,
    occupation: Option<String>,
}

impl From<MarketSearchInput> for deepagent_app_core::SearchQuery {
    fn from(input: MarketSearchInput) -> Self {
        deepagent_app_core::SearchQuery {
            q: input.q.unwrap_or_default(),
            page: input.page,
            limit: input.limit,
            sort_by: input.sort_by,
            category: input.category,
            occupation: input.occupation,
        }
    }
}

/// `GET https://skillsmp.com/api/v1/skills/search` — used by both the Market
/// tab's first paint (empty `q`, `sortBy=stars`) and live search-as-you-type.
///
/// _Validates: Requirements R3.1._
#[tauri::command]
async fn skill_market_search(
    state: State<'_, AppState>,
    input: MarketSearchInput,
) -> Result<deepagent_app_core::MarketSearchData, String> {
    // Clone the inner client so we drop the handle's std::sync::Mutex BEFORE
    // awaiting the HTTP call.
    let client = state.skillsmp.with_client(|c| c.clone());
    let query: deepagent_app_core::SearchQuery = input.into();
    client.search(&query).await.map_err(|e| e.to_string())
}

/// Result of [`skill_market_test_key`].
///
/// Renders the "Test connection" outcome in the Provider Config popover.
/// `daily_remaining` is the value of the most recent
/// `X-RateLimit-Daily-Remaining` header observed by the client (returned for
/// both success and failure responses, when present).
#[derive(Debug, serde::Serialize)]
struct TestKeyResult {
    ok: bool,
    daily_remaining: Option<u32>,
    error: Option<String>,
}

/// Run a tiny `search` to exercise the configured SkillsMP API key.
///
/// _Validates: Requirements R9.5._
#[tauri::command]
async fn skill_market_test_key(state: State<'_, AppState>) -> Result<TestKeyResult, String> {
    let client = state.skillsmp.with_client(|c| c.clone());
    let query = deepagent_app_core::SearchQuery {
        q: "test".to_string(),
        limit: Some(1),
        ..Default::default()
    };
    match client.search(&query).await {
        Ok(_) => Ok(TestKeyResult {
            ok: true,
            daily_remaining: client.last_daily_remaining(),
            error: None,
        }),
        Err(e) => Ok(TestKeyResult {
            ok: false,
            daily_remaining: client.last_daily_remaining(),
            error: Some(e.to_string()),
        }),
    }
}

/// API-key state surfaced to the Provider Config popover.
///
/// **Never** carries the key value itself — only the boolean / source label
/// the UI needs to render the "using built-in / using custom" badge. The key
/// stays in the OS keychain + the backend `SkillsMpClient::api_key` field.
#[derive(Debug, serde::Serialize)]
struct ApiKeyInfo {
    has_user_key: bool,
    source: deepagent_app_core::ApiKeySource,
}

/// Inspect the current SkillsMP API-key configuration.
///
/// _Validates: Requirements R9.4._
#[tauri::command]
fn skill_market_get_api_key(state: State<'_, AppState>) -> Result<ApiKeyInfo, String> {
    let source = state.skillsmp.source();
    Ok(ApiKeyInfo {
        has_user_key: source == deepagent_app_core::ApiKeySource::User,
        source,
    })
}

/// Save a user-supplied SkillsMP API key to the OS keychain and rebuild the
/// in-memory client so subsequent calls use it.
///
/// _Validates: Requirements R9.2._
#[tauri::command]
fn skill_market_set_api_key(state: State<'_, AppState>, key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty".into());
    }
    let keychain = KeychainStore::new(KEYCHAIN_SERVICE);
    SecretStore::set(&keychain, KEYCHAIN_SKILLSMP_KEY_NAME, trimmed).map_err(|e| e.to_string())?;
    state.skillsmp.set_user_key(Some(trimmed.to_string()));
    Ok(())
}

/// Delete the user-supplied SkillsMP API key from the keychain and fall the
/// in-memory client back to the built-in / anonymous tier.
///
/// _Validates: Requirements R9.6._
#[tauri::command]
fn skill_market_clear_api_key(state: State<'_, AppState>) -> Result<(), String> {
    let keychain = KeychainStore::new(KEYCHAIN_SERVICE);
    // Best-effort delete: ignore "no entry" errors. If the user never set a
    // custom key, the keychain has no entry to remove and we still want the
    // in-memory client to fall back to the built-in tier.
    let _ = SecretStore::delete(&keychain, KEYCHAIN_SKILLSMP_KEY_NAME);
    state.skillsmp.set_user_key(None);
    Ok(())
}

/// Result of [`skill_market_scan`]: an opaque temp-id (used as the install
/// confirmation handle) plus the static-scan report rendered by the install
/// dialog.
#[derive(Debug, serde::Serialize)]
struct ScanResult {
    temp_id: String,
    report: deepagent_app_core::ScanReport,
}

/// Download a skill from GitHub via codeload, run the static safety scan,
/// and stash the unpacked tempdir keyed by an opaque temp-id. The frontend
/// renders `ScanResult.report` in the install dialog and passes `temp_id`
/// back to [`skill_market_install`] / [`skill_market_cancel`] /
/// [`skill_market_ai_review`].
///
/// _Validates: Requirements R3.1, R4.1, R4.2._
#[tauri::command]
async fn skill_market_scan(
    state: State<'_, AppState>,
    github_url: String,
) -> Result<ScanResult, String> {
    reap_pending_scans(&state.pending_scans);
    let client = state.skillsmp.with_client(|c| c.clone());
    let locator = deepagent_app_core::SkillsMpClient::parse_github_url(&github_url)
        .map_err(|e| e.to_string())?;
    let temp = client
        .download_skill_to_temp(&locator)
        .await
        .map_err(|e| e.to_string())?;
    let report = deepagent_app_core::scan_dir(&temp.root).map_err(|e| e.to_string())?;
    let temp_id = new_temp_id();
    {
        let mut map = state.pending_scans.lock().map_err(|e| e.to_string())?;
        map.insert(
            temp_id.clone(),
            PendingScan {
                temp,
                report: report.clone(),
                created_at: std::time::Instant::now(),
            },
        );
    }
    Ok(ScanResult { temp_id, report })
}

/// Kick off the streaming AI security review for a pending scan. The review
/// runs on a background task that emits two Tauri events with the
/// `temp_id` carried in the payload:
///   - `skill-ai-review`       — one event per token, payload `{ temp_id, token }`
///   - `skill-ai-review-done`  — one event when the review settles, payload
///     `{ temp_id, result, error }` (`result` is the parsed [`AiReviewResult`]
///     or `null`; `error` carries a string when the review failed).
///
/// `re_review = Some(true)` switches the call from the default
/// [`ReviewDepth::Initial`] (Simple thinking, 2K reply cap) to
/// [`ReviewDepth::ReReview`] (Medium thinking, 3K reply cap), so the
/// frontend can offer a "更深入复审" button without a separate command.
///
/// Returns immediately so the install dialog can stay responsive.
///
/// _Validates: Requirements R4.6, R4.7, R4.8._
#[tauri::command]
async fn skill_market_ai_review(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    temp_id: String,
    re_review: Option<bool>,
) -> Result<(), String> {
    // Snapshot the report so the background task does not need to take the
    // pending_scans mutex during the (potentially long) AI call.
    let report = {
        reap_pending_scans(&state.pending_scans);
        let map = state.pending_scans.lock().map_err(|e| e.to_string())?;
        let entry = map.get(&temp_id).ok_or("temp_id not found or expired")?;
        entry.report.clone()
    };
    let chat = state.chat.clone();
    let id = temp_id.clone();
    let app_handle = app.clone();
    let depth = if re_review.unwrap_or(false) {
        deepagent_app_core::ReviewDepth::ReReview
    } else {
        deepagent_app_core::ReviewDepth::Initial
    };
    tokio::spawn(async move {
        let id_for_token = id.clone();
        let app_for_token = app_handle.clone();
        let result =
            deepagent_app_core::ai_security_review(&chat, &report, depth, move |tok| {
                let _ = app_for_token.emit(
                    "skill-ai-review",
                    serde_json::json!({ "temp_id": id_for_token.clone(), "token": tok }),
                );
            })
            .await;
        let payload = match &result {
            Ok(r) => serde_json::json!({
                "temp_id": id,
                "result": r,
                "error": serde_json::Value::Null,
            }),
            Err(e) => serde_json::json!({
                "temp_id": id,
                "result": serde_json::Value::Null,
                "error": e.to_string(),
            }),
        };
        let _ = app_handle.emit("skill-ai-review-done", payload);
    });
    Ok(())
}

/// Confirm-and-install: take the pending scan keyed by `temp_id` out of the
/// map, copy its tempdir into the marketplace install root, and return the
/// freshly registered [`SkillDto`]. Dropping the entry's `TempSkillDir`
/// after install removes the tempdir on disk.
///
/// _Validates: Requirements R4.10, R4.11._
#[tauri::command]
fn skill_market_install(state: State<'_, AppState>, temp_id: String) -> Result<SkillDto, String> {
    reap_pending_scans(&state.pending_scans);
    // Remove the entry BEFORE installing so we own the `TempSkillDir`. The
    // tempdir on disk lives until `pending` is dropped at the end of this
    // function; `install_from_temp` copies the contents into marketplace,
    // so dropping the temp afterwards is safe.
    let pending = {
        let mut map = state.pending_scans.lock().map_err(|e| e.to_string())?;
        map.remove(&temp_id).ok_or(
            "temp_id not found or expired (the install dialog has timed out — please re-download)",
        )?
    };
    let mut svc = state.skills.lock().map_err(|e| e.to_string())?;
    let dto = svc
        .install_from_temp(&pending.temp.root)
        .map_err(|e| e.to_string())?;
    drop(svc);
    // Skill set has materially changed — clear every session's send-once
    // tracker so the next turn re-announces the new skill via channel A
    // (skill-marketplace task 14).
    state.chat.reset_all_sent_skills();
    // `pending` is dropped here → temp dir removed from disk.
    Ok(dto)
}

/// User cancelled the install dialog: drop the pending scan (and its
/// tempdir). Idempotent: a missing `temp_id` is treated as already-cleared
/// (e.g. the entry was reaped or already installed).
///
/// _Validates: Requirements R4.9._
#[tauri::command]
fn skill_market_cancel(state: State<'_, AppState>, temp_id: String) -> Result<(), String> {
    reap_pending_scans(&state.pending_scans);
    let mut map = state.pending_scans.lock().map_err(|e| e.to_string())?;
    map.remove(&temp_id);
    Ok(())
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

// ---- post-edit verification policy (Phase 4C) ------------------------------

#[tauri::command]
fn get_verification_policy(state: State<'_, AppState>) -> Result<String, String> {
    state
        .settings
        .verification_policy()
        .map(|p| p.label().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_verification_policy(state: State<'_, AppState>, policy: String) -> Result<String, String> {
    let parsed = deepagent_app_core::VerificationPolicy::parse(&policy)
        .ok_or_else(|| format!("unknown verification policy: {policy}"))?;
    state
        .settings
        .set_verification_policy(parsed)
        .map_err(|e| e.to_string())?;
    Ok(parsed.label().to_string())
}

// ---- tool-search lazy loading (tool-search spec) --------------------------

#[tauri::command]
fn get_tool_search_mode(state: State<'_, AppState>) -> Result<String, String> {
    state
        .settings
        .tool_search_mode()
        .map(|m| m.label().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_tool_search_mode(state: State<'_, AppState>, mode: String) -> Result<String, String> {
    let parsed = deepagent_app_core::ToolSearchMode::parse(&mode)
        .ok_or_else(|| format!("unknown tool-search mode: {mode}"))?;
    state
        .settings
        .set_tool_search_mode(parsed)
        .map_err(|e| e.to_string())?;
    Ok(parsed.label().to_string())
}

#[tauri::command]
fn get_tool_search_threshold(state: State<'_, AppState>) -> Result<usize, String> {
    state
        .settings
        .tool_search_auto_threshold()
        .map_err(|e| e.to_string())
}

/// Persist the Auto-mode threshold (characters of deferred-tool schema). Pass
/// `None` to revert to the built-in default. Negative or zero values are
/// rejected; the backend itself clamps via `set_tool_search_auto_threshold`.
#[tauri::command]
fn set_tool_search_threshold(
    state: State<'_, AppState>,
    value: Option<usize>,
) -> Result<usize, String> {
    state
        .settings
        .set_tool_search_auto_threshold(value)
        .map_err(|e| e.to_string())?;
    state
        .settings
        .tool_search_auto_threshold()
        .map_err(|e| e.to_string())
}

// ---- Skill marketplace settings (Skill Marketplace spec, R10) -------------

#[tauri::command]
fn get_skill_catalog_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .settings
        .skill_catalog_enabled()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_skill_catalog_enabled(state: State<'_, AppState>, enabled: bool) -> Result<bool, String> {
    state
        .settings
        .set_skill_catalog_enabled(enabled)
        .map_err(|e| e.to_string())?;
    state
        .settings
        .skill_catalog_enabled()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_skill_catalog_char_budget(state: State<'_, AppState>) -> Result<usize, String> {
    state
        .settings
        .skill_catalog_char_budget()
        .map_err(|e| e.to_string())
}

/// Persist the catalog character budget. `0` is allowed and is treated as
/// "disabled" by the consumer site (R10.5).
#[tauri::command]
fn set_skill_catalog_char_budget(
    state: State<'_, AppState>,
    budget: usize,
) -> Result<usize, String> {
    state
        .settings
        .set_skill_catalog_char_budget(budget)
        .map_err(|e| e.to_string())?;
    state
        .settings
        .skill_catalog_char_budget()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_skill_install_ai_review_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .settings
        .skill_install_ai_review_enabled()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_skill_install_ai_review_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    state
        .settings
        .set_skill_install_ai_review_enabled(enabled)
        .map_err(|e| e.to_string())?;
    state
        .settings
        .skill_install_ai_review_enabled()
        .map_err(|e| e.to_string())
}

/// Returns the AI review model override, or `None` to mean "follow chat model".
#[tauri::command]
fn get_skill_install_ai_review_model(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    state
        .settings
        .skill_install_ai_review_model()
        .map_err(|e| e.to_string())
}

/// Persist the AI review model override. Pass `None` (or an empty string,
/// which is normalized to `None` by the service) to fall back to the chat
/// model.
#[tauri::command]
fn set_skill_install_ai_review_model(
    state: State<'_, AppState>,
    model: Option<String>,
) -> Result<Option<String>, String> {
    state
        .settings
        .set_skill_install_ai_review_model(model)
        .map_err(|e| e.to_string())?;
    state
        .settings
        .skill_install_ai_review_model()
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

            // SkillsMP API key: read from the OS keychain at boot. Errors and
            // missing entries are non-fatal — the handle falls through to the
            // built-in key (or anonymous if no built-in is configured).
            let user_skillsmp_key = {
                let kc = KeychainStore::new(KEYCHAIN_SERVICE);
                match kc.get(KEYCHAIN_SKILLSMP_KEY_NAME) {
                    Ok(opt) => opt,
                    Err(e) => {
                        eprintln!("skillsmp keychain read failed (falling back to builtin): {e}");
                        None
                    }
                }
            };
            let skillsmp = Arc::new(SkillsMpClientHandle::new(user_skillsmp_key));

            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| format!("failed to create tokio runtime: {e}"))?;

            // Skills: four-tier storage layout (built-in + user + marketplace +
            // workspace).
            //
            // - BuiltIn:   Tauri resource dir → `<exe>/resources/skills/` (the
            //              7 skills bundled by `prebundle-skills.cjs`)
            // - User:      `~/.deepagent/skills/` (top-level, hand-managed by
            //              the user; compatible with Claude Code's
            //              `~/.claude/skills/` convention)
            // - Installed: `~/.deepagent/skills/marketplace/` (managed by the
            //              marketplace install flow; lives under the user dir
            //              so app reinstalls don't wipe it)
            // - Workspace: `<cwd>/.deepagent/skills/` (project-scoped, optional)
            //
            // Conflict precedence (high → low):
            //   Workspace > User > Installed > BuiltIn.
            //
            // _Validates: Requirements R1.1, R1.2, R1.4, R2.1, R2.4, R2.5, R2.6,
            // R8.1._
            let resource_skills_dir = app
                .path()
                .resource_dir()
                .map(|d| d.join("resources").join("skills"))
                .unwrap_or_else(|_| std::env::temp_dir().join("deepagent-builtin-skills-missing"));

            let user_root = app
                .path()
                .home_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(".deepagent")
                .join("skills");
            let marketplace_dir = user_root.join("marketplace");
            // Make sure both dirs exist so first-run on a clean machine doesn't
            // error when the loader tries to canonicalize them.
            let _ = std::fs::create_dir_all(&user_root);
            let _ = std::fs::create_dir_all(&marketplace_dir);

            let workspace_skills = std::env::current_dir()
                .ok()
                .map(|c| c.join(".deepagent").join("skills"));

            let skills = SkillsService::open_v2(SkillsRoots {
                builtin: resource_skills_dir,
                user: user_root,
                marketplace: marketplace_dir,
                workspace: workspace_skills,
            })
            .map_err(|e| format!("failed to open skills service: {e}"))?;
            // Wrap in Arc<Mutex> so the chat service shares the same handle.
            // The Tauri command layer locks this mutex on every skill command
            // (list / install / uninstall / reload) and the chat service
            // snapshots the registry from it once per run for catalog
            // reminder + `skill` tool wiring (skill-marketplace task 14).
            let skills = Arc::new(Mutex::new(skills));

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
            // run is rooted at the active project's folder, the knowledge base
            // is attached for passive injection + active tools, and the skill
            // service powers channel-A catalog reminders + the channel-B
            // `skill` tool (skill-marketplace task 14).
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
                .with_skills(skills.clone())
                .with_tool_results_dir(dir.join("tool_results")),
            );

            app.manage(AppState {
                service: Mutex::new(service),
                settings: settings_arc,
                skills,
                skillsmp,
                pending_scans: Arc::new(Mutex::new(HashMap::new())),
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
            skill_market_search,
            skill_market_test_key,
            skill_market_get_api_key,
            skill_market_set_api_key,
            skill_market_clear_api_key,
            skill_market_scan,
            skill_market_ai_review,
            skill_market_install,
            skill_market_cancel,
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
            get_verification_policy,
            set_verification_policy,
            get_tool_search_mode,
            set_tool_search_mode,
            get_tool_search_threshold,
            set_tool_search_threshold,
            get_skill_catalog_enabled,
            set_skill_catalog_enabled,
            get_skill_catalog_char_budget,
            set_skill_catalog_char_budget,
            get_skill_install_ai_review_enabled,
            set_skill_install_ai_review_enabled,
            get_skill_install_ai_review_model,
            set_skill_install_ai_review_model,
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
