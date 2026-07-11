//! Streamed chat orchestration (P1-C): run an agent and push live events.
//!
//! This is the connection layer between the kernel and the desktop UI's chat:
//! it assembles the tool registry (built-ins), a DeepSeek-backed [`ModelAgent`],
//! a [`RuntimeEngine`], and a [`ChannelSink`], then runs one turn-loop while
//! forwarding every [`RuntimeEvent`] to a caller-supplied callback (which the
//! Tauri layer bridges to `app.emit`, or a web layer to SSE/WS).
//!
//! The model client is built from the persisted [`ModelCatalog`] + the API key
//! from the secret store, so the UI only needs to call [`ChatService::run`].

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_builtins::WorkspaceRoot;
use deepagent_context::{
    CompactionPolicy, HeuristicSummarizer, HeuristicTokenizer, ModelCompactor, Summarizer,
    TaskSummary, TokenCounter,
};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::message::{Message, ToolCall};
use deepagent_hooks::{
    DecisionSource, Hook, HookCommandRunner, HookContext, HookData, HookOutcome, HookPoint,
    HookRegistry, PermissionRulesHook, SystemHookRunner,
};
use deepagent_intent::{CommandContext, CommandDef, SlashAction, SlashRegistry};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelClient, ModelConfig, ModelRole, ThinkingDepth, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::{
    Agent, ChannelSink, ModelAgent, RuntimeConfig, RuntimeEngine, RuntimeEvent, RuntimeEventSink,
};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
use crate::cost_service::CostRecordRequest;
use crate::dto::{ApprovalRequestDto, PreflightToolCallDto};
use crate::office_service::OfficeService;
use crate::project_map_service::ProjectMapService;
use crate::settings::SettingsService;

/// The base system prompt seeded into every chat run, modeled on Claude Code's
/// layered prompt (System / Doing tasks / Using your tools / Tone & style /
/// Output efficiency). A dynamic environment block carrying the **current
/// date**, OS, and working directory is appended at runtime by
/// [`build_system_prompt`] so the model never reasons from a stale year (the
/// root cause of a web_search that searched the wrong year). The full layered
/// assembly lives in `deepagent-prompts`; this is the runtime's always-on
/// baseline.
/// The full layered assembly lives in [`crate::system_prompt`]; the runtime
/// pulls the cacheable static prefix from there via
/// [`crate::system_prompt::system_prompt_base`]. Phase 1A of the
/// coding-amplifier spec extracted the prompt into named topical sections so
/// later phases can edit individual sections without rewriting the whole text.
/// Marker separating the **static** (prefix-cacheable) portion of the system
/// prompt from the **dynamic** (per-request) portion, mirroring Claude Code's
/// `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`.
///
/// DeepSeek (like Anthropic/OpenAI) caches by **longest common prefix**: the
/// moment a token near the start changes — e.g. a freshly formatted date — the
/// cache invalidates from that point on and every later token is recomputed at
/// full price. So everything that is stable across requests (identity, working
/// style, tool guidance) must come BEFORE this boundary, and everything
/// volatile (today's date, cwd) must come AFTER it. Keeping the prefix
/// byte-identical across an agent loop is what lets DeepSeek serve the system
/// prompt + tool schemas from cache (~5–10x cheaper, lower first-token latency).
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "\n\n<<<DYNAMIC>>>\n\n";
const SESSION_TITLE_SYSTEM_PROMPT: &str = concat!(
    "You generate concise conversation titles for a coding assistant session.\n",
    "Return only the title text.\n",
    "Do not use quotes, markdown, numbering, or any explanation.\n",
    "Use the user's language when possible.\n",
    "Focus on the user's concrete task or goal, not greetings or assistant boilerplate.\n",
    "Keep it short and specific."
);

/// Build the effective system prompt for a run: the stable, prefix-cacheable
/// base, then the [`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`], then a dynamic environment
/// block carrying the current date, OS, and working directory. The date line is
/// what stops the model from searching a stale year; placing it AFTER the
/// boundary keeps the cached prefix intact across requests.
fn build_system_prompt(root: &std::path::Path) -> String {
    let today = current_date_string();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!(
        "{base}{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}# Environment\n- Today's date: {today}\n- Operating system: {os} ({arch})\n- Working directory: {cwd}\n- When you need current information, use this date — especially the year — in web_search queries.",
        base = crate::system_prompt::system_prompt_base(),
        cwd = root.display(),
    )
}

/// Today's date as `YYYY-MM-DD` (local time, falling back to UTC if the local
/// offset can't be determined). Kept dependency-light via the `time` crate.
fn current_date_string() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    now.format(&fmt)
        .unwrap_or_else(|_| format!("{}", now.year()))
}

#[cfg(feature = "web")]
fn configured_searxng_url(setting: Option<String>) -> Option<String> {
    setting
        .or_else(|| std::env::var("DEEPAGENT_SEARXNG_URL").ok())
        .or_else(|| std::env::var("SEARXNG_URL").ok())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn web_search_summary(settings: &crate::settings::WebSearchSettings) -> String {
    if !settings.enabled {
        return "disabled".to_string();
    }
    let provider = settings.provider.label();
    match settings.searxng_url.as_deref() {
        Some(url) if !url.trim().is_empty() => format!("{provider} (SearXNG: {url})"),
        _ => provider.to_string(),
    }
}

#[derive(Clone)]
struct ProjectMapToolBackend {
    service: Arc<ProjectMapService>,
    root: PathBuf,
}

#[async_trait]
impl deepagent_builtins::ProjectMapBackend for ProjectMapToolBackend {
    async fn overview(&self) -> Result<serde_json::Value> {
        serde_json::to_value(self.service.overview(&self.root)).map_err(Into::into)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<serde_json::Value> {
        let hits = self.service.search(&self.root, query, limit)?;
        Ok(serde_json::json!({
            "count": hits.len(),
            "hits": hits,
        }))
    }

    async fn neighbors(&self, node_id: &str) -> Result<serde_json::Value> {
        serde_json::to_value(self.service.neighbors(&self.root, node_id)?).map_err(Into::into)
    }

    async fn impact(&self, target: &str) -> Result<serde_json::Value> {
        serde_json::to_value(self.service.impact(&self.root, target)?).map_err(Into::into)
    }
}

/// Bridges the `office_*` tools to the app-core [`OfficeService`] (Tier C
/// pure-Rust read/generate; Tier R when a conversion runtime is installed).
#[cfg(feature = "runtimes")]
#[derive(Clone)]
struct OfficeToolBackend {
    service: Arc<OfficeService>,
}

#[cfg(feature = "runtimes")]
#[async_trait]
impl deepagent_builtins::OfficeBackend for OfficeToolBackend {
    async fn read_text(&self, path: &str) -> Result<serde_json::Value> {
        let text = self.service.read_text(path)?;
        Ok(serde_json::json!({ "path": path, "text": text }))
    }

    async fn create_docx_from_markdown(
        &self,
        markdown: &str,
        title: Option<String>,
        out_path: &str,
        overwrite: bool,
    ) -> Result<serde_json::Value> {
        if !overwrite && std::path::Path::new(out_path).exists() {
            return Err(CoreError::invalid(
                "file already exists — pass overwrite=true to replace it",
            ));
        }
        self.service
            .create_docx_from_markdown(markdown, title.as_deref(), out_path)?;
        Ok(serde_json::json!({ "path": out_path, "ok": true }))
    }

    async fn create_xlsx(
        &self,
        sheets: serde_json::Value,
        out_path: &str,
        overwrite: bool,
    ) -> Result<serde_json::Value> {
        if !overwrite && std::path::Path::new(out_path).exists() {
            return Err(CoreError::invalid(
                "file already exists — pass overwrite=true to replace it",
            ));
        }
        let parsed = parse_office_sheets(&sheets)?;
        self.service.create_xlsx(&parsed, out_path)?;
        Ok(serde_json::json!({ "path": out_path, "ok": true }))
    }
}

/// Parse the `sheets` tool argument (`[{ name, rows: [[..]] }]`) into the
/// `(name, rows)` shape [`OfficeService::create_xlsx`] expects.
#[cfg(feature = "runtimes")]
fn parse_office_sheets(value: &serde_json::Value) -> Result<Vec<(String, Vec<Vec<String>>)>> {
    let arr = value
        .as_array()
        .ok_or_else(|| CoreError::invalid("'sheets' must be an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, sheet) in arr.iter().enumerate() {
        let name = sheet
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Sheet{}", i + 1));
        let rows = sheet
            .get("rows")
            .and_then(|r| r.as_array())
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.as_array()
                            .map(|cells| {
                                cells
                                    .iter()
                                    .map(|c| {
                                        c.as_str()
                                            .map(str::to_string)
                                            .unwrap_or_else(|| c.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push((name, rows));
    }
    Ok(out)
}

#[derive(Clone)]
struct CodeGraphToolBackend {
    root: PathBuf,
}

#[async_trait]
impl deepagent_builtins::CodeGraphBackend for CodeGraphToolBackend {
    async fn search(
        &self,
        query: &str,
        kind: Option<String>,
        limit: usize,
    ) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            let kind = kind
                .as_deref()
                .and_then(deepagent_codegraph::types::NodeKind::try_parse);
            serde_json::to_value(graph.search(query, kind, limit)?).map_err(Into::into)
        })
    }

    async fn explore(
        &self,
        symbols: &[String],
        budget: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            serde_json::to_value(graph.explore(symbols, parse_explore_budget(&budget))?)
                .map_err(Into::into)
        })
    }

    async fn callers(&self, symbol: &str, limit: usize) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            serde_json::to_value(graph.callers(symbol, limit)?).map_err(Into::into)
        })
    }

    async fn callees(&self, symbol: &str, limit: usize) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            serde_json::to_value(graph.callees(symbol, limit)?).map_err(Into::into)
        })
    }

    async fn impact(&self, symbol: &str, depth: usize) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            serde_json::to_value(graph.impact(symbol, depth)?).map_err(Into::into)
        })
    }

    async fn node(&self, target: &str) -> Result<serde_json::Value> {
        self.with_graph(|graph| serde_json::to_value(graph.node(target)?).map_err(Into::into))
    }

    async fn node_at_location(&self, file: &str, line: u32) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            serde_json::to_value(graph.store().node_at_location(file, line)?).map_err(Into::into)
        })
    }

    async fn locate(&self, text: &str) -> Result<serde_json::Value> {
        self.with_graph(|graph| {
            let files = graph
                .store()
                .all_file_nodes()?
                .into_iter()
                .map(|node| node.file_path)
                .collect();
            let parser =
                deepagent_codegraph::error_locator::ErrorParser::with_project(&self.root, files);
            let frames = parser.parse(text);
            let mut located = Vec::new();
            let mut external = Vec::new();
            for frame in frames {
                let frame_json = serde_json::json!({
                    "file": frame.file,
                    "line": frame.line,
                    "col": frame.col,
                    "symbol": frame.symbol,
                    "errorCode": frame.error_code,
                    "isProject": frame.is_project,
                });
                if !frame.is_project {
                    external.push(frame_json);
                    continue;
                }
                let node = graph.store().node_at_location(&frame.file, frame.line)?;
                let detail = match &node {
                    Some(node) => graph.node(&node.id)?,
                    None => None,
                };
                located.push(serde_json::json!({
                    "frame": frame_json,
                    "node": node,
                    "detail": detail,
                }));
            }
            Ok(serde_json::json!({
                "indexed": true,
                "located": located,
                "externalFrames": external,
            }))
        })
    }
}

impl CodeGraphToolBackend {
    fn with_graph<F>(&self, f: F) -> Result<serde_json::Value>
    where
        F: FnOnce(&deepagent_codegraph::CodeGraph) -> Result<serde_json::Value>,
    {
        let graph = deepagent_codegraph::CodeGraph::open(&self.root)?;
        if !graph.has_existing_index() {
            return Ok(codegraph_not_indexed());
        }
        f(&graph)
    }
}

fn codegraph_not_indexed() -> serde_json::Value {
    serde_json::json!({
        "indexed": false,
        "message": "Code graph is not indexed yet. Run project map refresh/deep indexing for this workspace, then retry the codegraph tool.",
    })
}

fn parse_explore_budget(value: &serde_json::Value) -> deepagent_codegraph::query::ExploreBudget {
    let mut budget = deepagent_codegraph::query::ExploreBudget::default();
    set_usize(value, "maxHitsPerSymbol", &mut budget.max_hits_per_symbol);
    set_usize(value, "maxBridgeHops", &mut budget.max_bridge_hops);
    set_usize(value, "maxFiles", &mut budget.max_files);
    set_usize(value, "maxSymbolsPerFile", &mut budget.max_symbols_per_file);
    set_usize(value, "maxFlowHops", &mut budget.max_flow_hops);
    budget
}

fn set_usize(value: &serde_json::Value, key: &str, target: &mut usize) {
    if let Some(n) = value.get(key).and_then(|v| v.as_u64()) {
        *target = n as usize;
    }
}

/// Orchestrates streamed chat runs over the kernel.
pub struct ChatService {
    db: Arc<Database>,
    settings: Arc<SettingsService>,
    transport: Arc<dyn HttpTransport>,
    /// Default workspace root (the launch directory) used when no project
    /// registry is attached or no project is active.
    workspace: PathBuf,
    /// Allow-listed bash command prefixes.
    bash_allow: Vec<String>,
    /// Shared registry of in-flight approval requests (the UI resolves these).
    pending: PendingApprovals,
    /// Optional MCP server manager: when set, enabled MCP servers are connected
    /// at run time and their tools registered into the runtime tool registry.
    mcp: Option<Arc<crate::mcp_service::McpService>>,
    /// Optional project registry: when set, each run is rooted at (and the new
    /// session attached to) the **active** project's folder.
    projects: Option<Arc<crate::project_service::ProjectService>>,
    /// Optional knowledge base: when set, relevant entries are passively
    /// injected each turn and the `knowledge_search` / `knowledge_write` tools
    /// are registered. When unset, behavior is identical to before the feature
    /// (no injection, no tools) — preserving backward compatibility.
    knowledge: Option<Arc<crate::knowledge_service::KnowledgeService>>,
    /// Optional project-map reader. When set, read-only `code_map_*` tools are
    /// registered for the active project so the model can locate code before
    /// broad file reads.
    project_map: Option<Arc<ProjectMapService>>,
    /// Optional office service: when set, the chat run registers the
    /// `office_*` read/generate tools so the model can read and produce
    /// Word/Excel documents (office-agent).
    office: Option<Arc<OfficeService>>,
    /// Optional cost tracker: when set, each completed run records its token
    /// cost and runs are refused when a configured budget is exhausted. When
    /// unset, behavior is identical to before the feature (no recording, no
    /// budget enforcement) — preserving backward compatibility.
    cost: Option<Arc<crate::cost_service::CostService>>,
    /// Base directory for persisted large tool results.
    tool_results_dir: PathBuf,
    /// Per-session Plan-mode flags. Plan mode is a read-only planning state:
    /// while active, the BeforeToolUse plan-mode hook denies write tools. The
    /// flag is shared (cheap `Arc<AtomicBool>`) so the enter/exit tools, the
    /// hook, and the UI toggle all view the same state. Sessions with no entry
    /// are in normal mode.
    plan_modes:
        Arc<std::sync::Mutex<std::collections::HashMap<String, deepagent_builtins::PlanMode>>>,
    /// Per-session cancellation flags for in-flight runs. The UI sets one via
    /// [`ChatService::cancel_session`] to stop a run; the engine checks it at
    /// each step boundary.
    cancellations: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    /// Per-session discovered-tool sets for lazy tool loading (tool-search
    /// spec). Each entry is the set of deferred tool names the model has
    /// already pulled into its active toolset via `tool_search`. Sessions
    /// without entries default to empty (= no deferred tool loaded).
    discovered_tools: DiscoveredToolsMap,
    /// Optional skills service: when set, the chat run registers the
    /// `skill` tool (channel B of the auto-activation design) and injects
    /// the `<available-skills>` catalog reminder (channel A) on each turn.
    /// When unset, behavior is identical to before this feature existed —
    /// preserving backward compatibility (Property 9).
    skills: Option<Arc<std::sync::Mutex<crate::skills_service::SkillsService>>>,
    /// Per-session catalog send-once tracker. The chat service consults
    /// (and mutates) this each turn to figure out the delta to inject into
    /// the system prompt. Mutation of the registry (install / uninstall /
    /// reload / marketplace install) clears entries via
    /// [`ChatService::reset_sent_skills`] / [`reset_all_sent_skills`] so
    /// the next turn re-announces the changed entries.
    skill_catalog_state: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, crate::skill_catalog_reminder::SkillCatalogSendState>,
        >,
    >,
    /// Per-session set of skills that have been successfully invoked through
    /// the `skill` tool. Used by office tool guards so specialized document
    /// tools cannot bypass the matching docx/xlsx/pdf/pptx skill.
    invoked_skills: InvokedSkillMap,
    /// Optional executor factory for remote (SSH) sessions. When set and the
    /// session is in `SessionMode::Remote`, the factory creates a
    /// [`CommandExecutor`] that routes bash/git commands through SSH instead
    /// of local execution.
    executor_factory: Option<ExecutorFactory>,
    /// Optional executor for local command execution. The desktop app uses this
    /// to wrap local shell/git commands in Sandboxie-Plus when available.
    local_command_executor: Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>,
    /// Typed reference to the sandboxie executor for per-run mode updates.
    sandboxie_executor: Option<Arc<crate::sandboxie_service::SandboxieExecutor>>,
    /// Optional remote-context factory for remote (SSH) sessions. When set,
    /// remote runs can inject a concise SSH snapshot so the model reasons from
    /// the remote host's actual state rather than the local workspace alone.
    remote_context_factory: Option<RemoteContextFactory>,
    /// Optional remote-ops factory for remote (SSH) sessions. When set, remote
    /// sessions gain probe / push / install tools backed by the active SSH
    /// connection so the model can inspect capabilities before acting.
    remote_ops_factory: Option<RemoteOpsFactory>,
}

/// Factory that creates a [`CommandExecutor`] for a given connection id.
/// Used for remote (SSH) sessions — the factory is set up by the desktop
/// app and captures the `SshService` handle.
type ExecutorFactory =
    Arc<dyn Fn(String) -> Arc<dyn deepagent_builtins::bash_tool::CommandExecutor> + Send + Sync>;

type RemoteContextFuture = Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>>;
type RemoteContextFactory = Arc<dyn Fn(String) -> RemoteContextFuture + Send + Sync>;
type RemoteOpsFactory =
    Arc<dyn Fn(String) -> Arc<dyn deepagent_builtins::RemoteOpsBackend> + Send + Sync>;

/// One session's discovered-tool set: shared between the runtime tool
/// (`ToolSearchTool`) and the chat-service's per-turn tools-array assembly.
type DiscoveredToolSet = Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// Per-session map of [`DiscoveredToolSet`]s, keyed by session id.
type DiscoveredToolsMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, DiscoveredToolSet>>>;

type InvokedSkillMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvokedSkillRecord {
    id: String,
    name: String,
    body: String,
    base_dir: Option<String>,
    resources: Vec<String>,
}

#[derive(Clone)]
struct OfficeSkillGuardHook {
    invoked_skills: InvokedSkillMap,
    enforce_skills: std::collections::HashSet<String>,
}

impl OfficeSkillGuardHook {
    fn new(
        invoked_skills: InvokedSkillMap,
        enforce_skills: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            invoked_skills,
            enforce_skills,
        }
    }

    fn seed_session(&self, session_id: &str, skills: std::collections::HashSet<String>) {
        if skills.is_empty() {
            return;
        }
        let mut map = self
            .invoked_skills
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let entry = map.entry(session_id.to_string()).or_default();
        entry.extend(skills);
    }

    fn record_skill(&self, session_id: &str, skill_id: &str) {
        let skill_id = skill_id.trim();
        if skill_id.is_empty() {
            return;
        }
        let mut map = self
            .invoked_skills
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.entry(session_id.to_string())
            .or_default()
            .insert(skill_id.to_string());
    }

    fn has_skill(&self, session_id: &str, skill_id: &str) -> bool {
        let map = self
            .invoked_skills
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.get(session_id)
            .map(|skills| skills.contains(skill_id))
            .unwrap_or(false)
    }
}

#[async_trait]
impl Hook for OfficeSkillGuardHook {
    fn name(&self) -> &str {
        "office_skill_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        let HookData::Tool {
            name,
            arguments,
            ok,
        } = &ctx.data
        else {
            return Ok(HookOutcome::Continue);
        };
        let session_id = ctx.session_id.to_string();

        if ctx.point == HookPoint::AfterToolUse && name == deepagent_builtins::SKILL_TOOL_NAME {
            if ok == &Some(true) {
                if let Some(id) = arguments.get("id").and_then(|v| v.as_str()) {
                    self.record_skill(&session_id, id);
                }
            }
            return Ok(HookOutcome::Continue);
        }

        if ctx.point != HookPoint::BeforeToolUse {
            return Ok(HookOutcome::Continue);
        }

        let Some(required) = required_skill_for_office_tool(name, arguments) else {
            return Ok(HookOutcome::Continue);
        };
        if !self.enforce_skills.contains(required) || self.has_skill(&session_id, required) {
            return Ok(HookOutcome::Continue);
        }

        Ok(HookOutcome::deny_from(
            format!(
                "{name} requires the `{required}` skill. Call the `skill` tool first with {{\"id\":\"{required}\"}}, follow that skill's document-formatting rules, then retry {name}."
            ),
            DecisionSource::Policy,
        ))
    }
}

fn required_skill_for_office_tool(name: &str, args: &serde_json::Value) -> Option<&'static str> {
    match name {
        deepagent_builtins::OFFICE_DOCX_CREATE_TOOL_NAME => Some("docx"),
        deepagent_builtins::OFFICE_XLSX_CREATE_TOOL_NAME => Some("xlsx"),
        deepagent_builtins::OFFICE_READ_TOOL_NAME => {
            let path = args.get("path").and_then(|v| v.as_str())?;
            office_skill_for_path(path)
        }
        _ => None,
    }
}

fn office_skill_for_path(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "doc" | "docx" => Some("docx"),
        "xls" | "xlsx" | "xlsm" | "csv" | "tsv" => Some("xlsx"),
        "ppt" | "pptx" => Some("pptx"),
        "pdf" => Some("pdf"),
        _ => None,
    }
}

impl ChatService {
    /// Build a chat service over the shared DB, settings, model transport, and
    /// workspace root.
    pub fn new(
        db: Arc<Database>,
        settings: Arc<SettingsService>,
        transport: Arc<dyn HttpTransport>,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        let workspace = workspace.into();
        let tool_results_dir = workspace.join(".deepagent").join("tool_results");
        Self {
            db,
            settings,
            transport,
            workspace,
            bash_allow: default_bash_allow(),
            pending: PendingApprovals::new(),
            mcp: None,
            projects: None,
            knowledge: None,
            project_map: None,
            office: None,
            cost: None,
            tool_results_dir,
            plan_modes: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            cancellations: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            discovered_tools: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            skills: None,
            skill_catalog_state: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            invoked_skills: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            executor_factory: None,
            local_command_executor: None,
            sandboxie_executor: None,
            remote_context_factory: None,
            remote_ops_factory: None,
        }
    }

    /// Request cancellation of an in-flight run for `session_id`. Returns
    /// whether a matching in-flight run was found. The run stops at its next
    /// step boundary and ends as cancelled (partial transcript preserved).
    pub fn cancel_session(&self, session_id: &str) -> bool {
        let map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(flag) = map.get(session_id) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Attach an [`McpService`](crate::mcp_service::McpService) so enabled MCP
    /// servers are connected and their tools live-registered on each run.
    pub fn with_mcp(mut self, mcp: Arc<crate::mcp_service::McpService>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach a [`ProjectService`](crate::project_service::ProjectService) so
    /// each run is rooted at the active project's folder and the new session is
    /// attached to it.
    pub fn with_projects(mut self, projects: Arc<crate::project_service::ProjectService>) -> Self {
        self.projects = Some(projects);
        self
    }

    /// Attach a [`KnowledgeService`](crate::knowledge_service::KnowledgeService)
    /// so each run passively injects relevant knowledge and exposes the
    /// `knowledge_search` / `knowledge_write` tools. Without it, runs behave
    /// exactly as before this feature existed.
    pub fn with_knowledge(
        mut self,
        knowledge: Arc<crate::knowledge_service::KnowledgeService>,
    ) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    /// Attach a [`ProjectMapService`] so runs expose read-only `code_map_*`
    /// tools for the active project.
    pub fn with_project_map(mut self, project_map: Arc<ProjectMapService>) -> Self {
        self.project_map = Some(project_map);
        self
    }

    /// Attach an [`OfficeService`] so runs expose the `office_*` read/generate
    /// tools (read docx/xlsx/pptx/pdf; create docx/xlsx) for the agent.
    pub fn with_office(mut self, office: Arc<OfficeService>) -> Self {
        self.office = Some(office);
        self
    }

    /// Attach a [`CostService`](crate::cost_service::CostService) so each
    /// completed run records its token cost and runs are refused when a
    /// configured budget is exhausted. Without it, runs behave exactly as
    /// before this feature existed (no recording, no enforcement).
    pub fn with_cost(mut self, cost: Arc<crate::cost_service::CostService>) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Attach a shared [`SkillsService`](crate::skills_service::SkillsService)
    /// so each run:
    ///
    /// - registers the `skill` built-in tool (channel B of the auto-activation
    ///   design) over a fresh [`SkillRegistry`][deepagent_skills::SkillRegistry]
    ///   snapshot, and
    /// - injects the `<available-skills>` catalog reminder (channel A) into
    ///   the system prompt whenever the per-session send-once tracker shows
    ///   a non-empty delta.
    ///
    /// Without it, runs behave exactly as before this feature existed: no
    /// `skill` tool, no catalog reminder. This preserves the byte-equivalent
    /// default behavior for callers that don't opt in.
    pub fn with_skills(
        mut self,
        skills: Arc<std::sync::Mutex<crate::skills_service::SkillsService>>,
    ) -> Self {
        self.skills = Some(skills);
        self
    }

    /// Forget the per-session catalog send-once state for `session_id` so
    /// the next turn re-announces the full visible registry.
    ///
    /// The Tauri command layer calls this after `reload_skills` /
    /// `install_skill` / `uninstall_skill` / `skill_market_install` succeed
    /// — anything that materially changes the skill set. Without the reset,
    /// a freshly-installed skill would not appear in the next turn's
    /// reminder until the session restarted.
    pub fn reset_sent_skills(&self, session_id: &str) {
        if let Ok(mut map) = self.skill_catalog_state.lock() {
            map.remove(session_id);
        }
    }

    /// Forget every session's catalog send-once state. Used by the Tauri
    /// command layer when a global change to the skill registry has
    /// happened (e.g. `reload_skills`, marketplace install): the next turn
    /// of every active session re-announces the full visible registry.
    pub fn reset_all_sent_skills(&self) {
        if let Ok(mut map) = self.skill_catalog_state.lock() {
            map.clear();
        }
    }

    /// Store oversized tool results under `dir` (usually app_data/tool_results).
    pub fn with_tool_results_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.tool_results_dir = dir.into();
        self
    }

    /// Bind a factory that creates a [`CommandExecutor`] for a given SSH
    /// connection id. When a session is in [`SessionMode::Remote`], the
    /// factory is called at runtime to produce an executor that routes
    /// bash/git commands through SSH instead of local execution.
    pub fn with_executor_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(String) -> Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>
            + Send
            + Sync
            + 'static,
    {
        self.executor_factory = Some(Arc::new(factory));
        self
    }

    /// Bind a local command executor. Used by the desktop shell to run local
    /// shell/git commands through Sandboxie-Plus while remote sessions keep
    /// using the SSH executor factory above.
    pub fn with_local_command_executor(
        mut self,
        executor: Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>,
    ) -> Self {
        self.local_command_executor = Some(executor);
        self
    }

    /// Bind a typed Sandboxie executor for per-run mode updates.
    pub fn with_sandboxie_executor(
        mut self,
        executor: Arc<crate::sandboxie_service::SandboxieExecutor>,
    ) -> Self {
        self.sandboxie_executor = Some(executor);
        self
    }

    /// Bind a factory that gathers a concise remote snapshot for a given SSH
    /// connection id. The result is injected as a system reminder during
    /// remote runs so the model can see the remote host and current directory.
    pub fn with_remote_context_factory<F, Fut>(mut self, factory: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>>> + Send + 'static,
    {
        self.remote_context_factory = Some(Arc::new(move |connection_id: String| {
            Box::pin(factory(connection_id))
        }));
        self
    }

    /// Bind a factory that creates the remote probe / transfer / install
    /// backend for a given SSH connection id. Registered only for remote
    /// sessions.
    pub fn with_remote_ops_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(String) -> Arc<dyn deepagent_builtins::RemoteOpsBackend> + Send + Sync + 'static,
    {
        self.remote_ops_factory = Some(Arc::new(factory));
        self
    }

    /// The effective project root for this run: the active project's folder when
    /// a project registry is attached and a project is active, else the default
    /// launch workspace.
    fn effective_root(&self) -> PathBuf {
        if let Some(projects) = &self.projects {
            if let Ok(Some(active)) = projects.active() {
                if !active.trim().is_empty() {
                    return PathBuf::from(active);
                }
            }
        }
        self.workspace.clone()
    }

    /// The shared pending-approvals registry. The UI calls
    /// [`PendingApprovals::resolve_approved`] on this to answer a dialog.
    pub fn pending_approvals(&self) -> PendingApprovals {
        self.pending.clone()
    }

    /// Return the shared plan-mode flag for a session, creating an inactive
    /// flag the first time this process sees the session.
    fn plan_mode_for_session(&self, session_id: &str) -> deepagent_builtins::PlanMode {
        let mut map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(session_id.to_string()).or_default().clone()
    }

    /// Whether the session is currently in read-only Plan mode.
    pub fn is_plan_mode(&self, session_id: &str) -> bool {
        let map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
        map.get(session_id)
            .map(deepagent_builtins::PlanMode::is_active)
            .unwrap_or(false)
    }

    /// Set the session's read-only Plan mode flag and return the new state.
    pub fn set_plan_mode(&self, session_id: &str, active: bool) -> bool {
        let plan = self.plan_mode_for_session(session_id);
        plan.set(active);
        plan.is_active()
    }

    /// Return the shared discovered-tools set for a session, creating an
    /// empty one the first time this process sees the session. Used by the
    /// `tool_search` built-in (it captures the handle at registration time)
    /// and by the per-turn tools-array assembly (it reads the names back).
    fn discovered_tools_for_session(&self, session_id: &str) -> DiscoveredToolSet {
        let mut map = self
            .discovered_tools
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())))
            .clone()
    }

    /// Snapshot the names currently in a session's discovered set
    /// (read-only). Mostly for tests / diagnostics.
    pub fn discovered_tool_names(&self, session_id: &str) -> Vec<String> {
        let map = self
            .discovered_tools
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match map.get(session_id) {
            Some(set) => {
                let names = set.lock().unwrap_or_else(|p| p.into_inner());
                let mut out: Vec<String> = names.iter().cloned().collect();
                out.sort();
                out
            }
            None => Vec::new(),
        }
    }

    /// Handle slash commands locally. They create/continue a session and append
    /// ordinary messages so the command and result remain in conversation
    /// history, but the raw slash line is not sent to the model.
    async fn maybe_handle_slash_command<F>(
        &self,
        prompt: &str,
        continue_session: Option<&str>,
        on_event: &F,
    ) -> Result<Option<String>>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
    {
        let registry = SlashRegistry::with_builtins();
        let mut ctx = CommandContext {
            session_id: continue_session.map(str::to_string),
        };
        let Some((name, _)) = parse_slash_invocation(prompt) else {
            return Ok(None);
        };
        if registry.get(name).is_none() {
            return Ok(None);
        }
        let Some(result) = registry.execute_line(prompt, &mut ctx) else {
            return Ok(None);
        };
        let result = result?;

        let root = self.effective_root();
        let project = root.to_string_lossy().into_owned();
        let clock = SystemClock;
        let target_session = match &result.action {
            SlashAction::Resume { session_id } => Some(session_id.as_str()),
            _ => continue_session,
        };
        let mut session = match target_session {
            Some(id_str) => {
                let id = deepagent_core::id::SessionId::from_str(id_str)
                    .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
                Session::recover(&self.db, &clock, id)?
            }
            None => Session::create_in_project(
                &self.db,
                &clock,
                Some(prompt),
                Default::default(),
                Some(&project),
            )?,
        };

        let session_id = session.id().to_string();
        let reply = self
            .apply_slash_action(&session_id, &mut session, result)
            .await?;
        session.append(EventPayload::MessageAppended {
            message: Message::user(prompt),
        })?;
        let task = session.create_task(prompt)?;
        session.transition_task(task, deepagent_core::task::TaskState::Running)?;

        on_event(RuntimeEvent::RunStarted {
            task_id: task.to_string(),
        });
        on_event(RuntimeEvent::SessionRegistered {
            session_id: session_id.clone(),
            title: session.state().title.clone(),
        });
        on_event(RuntimeEvent::TurnStarted { step: 0 });
        on_event(RuntimeEvent::ContentDelta {
            text: reply.clone(),
        });

        session.append(EventPayload::MessageAppended {
            message: Message::assistant(&reply),
        })?;
        session.transition_task(task, deepagent_core::task::TaskState::Completed)?;
        on_event(RuntimeEvent::RunCompleted { message: reply });

        Ok(Some(session_id))
    }

    fn dynamic_command_prompt(&self, prompt: &str) -> Result<Option<String>> {
        let Some((name, args)) = parse_slash_invocation(prompt) else {
            return Ok(None);
        };
        if SlashRegistry::with_builtins().get(name).is_some() {
            return Ok(None);
        }
        let Some(def) = self.find_dynamic_command(name)? else {
            return Err(CoreError::invalid(format!(
                "unknown slash command: /{name}"
            )));
        };
        Ok(Some(def.render(args)))
    }

    fn find_dynamic_command(&self, name: &str) -> Result<Option<CommandDef>> {
        let mut roots = vec![self.effective_root(), self.workspace.clone()];
        roots.dedup();
        for dir in crate::commands::command_dirs(roots) {
            let path = dir.join(format!("{name}.md"));
            if !path.exists() {
                continue;
            }
            return deepagent_prompts::load_command_file(path).map(Some);
        }
        Ok(None)
    }

    async fn apply_slash_action(
        &self,
        session_id: &str,
        session: &mut Session<'_, SystemClock>,
        result: deepagent_intent::CommandResult,
    ) -> Result<String> {
        let message = match result.action {
            SlashAction::EnterPlanMode => {
                self.set_plan_mode(session_id, true);
                result.message
            }
            SlashAction::ExitPlanMode => {
                self.set_plan_mode(session_id, false);
                result.message
            }
            SlashAction::Compact => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let rendered: Vec<String> = history
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect();
                let counter = HeuristicTokenizer::new();
                let tokens_before: usize = rendered.iter().map(|t| counter.count(t)).sum();
                let tokens_after = tokens_before / 2;
                session.append(EventPayload::ContextCompacted {
                    tokens_before: tokens_before as u64,
                    tokens_after: tokens_after as u64,
                    strategy: "manual".to_string(),
                })?;
                format!(
                    "Compacted current session context. Tokens before: {tokens_before}; target after: {tokens_after}."
                )
            }
            SlashAction::Cost => match &self.cost {
                Some(cost) => {
                    let s = cost.summary(session_id)?;
                    format!(
                        "Cost summary: session ${:.4}, today ${:.4}, month ${:.4}, total ${:.4}.",
                        s.session_cost, s.today_cost, s.month_cost, s.total_cost
                    )
                }
                None => "Cost tracking is not enabled for this runtime.".to_string(),
            },
            SlashAction::Doctor => {
                let root = self.effective_root();
                let results = crate::doctor::run_diagnostics(
                    &self.settings,
                    &self.db,
                    &root,
                    &self.tool_results_dir,
                )
                .await;
                crate::doctor::format_diagnostics(&results)
            }
            SlashAction::Help => {
                let registry = SlashRegistry::with_builtins();
                let mut lines = vec!["Available slash commands:".to_string()];
                for name in registry.names() {
                    if let Some(command) = registry.get(&name) {
                        lines.push(format!("- /{}: {}", command.name, command.description));
                    }
                }
                lines.join("\n")
            }
            SlashAction::Status => {
                let root = self.effective_root();
                let plan = if self.is_plan_mode(session_id) {
                    "on"
                } else {
                    "off"
                };
                let settings = self.settings.view()?;
                let (configured, chat_model, thinking_depth, web_search) = settings
                    .as_ref()
                    .map(|s| {
                        (
                            s.configured,
                            s.chat_model.as_str(),
                            s.thinking_depth.as_str(),
                            web_search_summary(&s.web_search),
                        )
                    })
                    .unwrap_or((false, "(not initialized)", "medium", "default".to_string()));
                let approval = self.settings.approval_policy()?.label();
                format!(
                    "Status:\n- project: {}\n- configured: {}\n- chat model: {}\n- thinking: {}\n- approvals: {}\n- web search: {}\n- plan mode: {}",
                    root.display(),
                    configured,
                    chat_model,
                    thinking_depth,
                    approval,
                    web_search,
                    plan
                )
            }
            SlashAction::Settings => match self.settings.view()? {
                Some(s) => format!(
                    "Settings:\n- configured: {}\n- api key: {}\n- base URL: {}\n- chat model: {}\n- reasoner model: {}\n- thinking: {}\n- approvals: {}\n- web search: {}\n- available models: {}",
                    s.configured,
                    s.api_key_masked,
                    s.base_url,
                    s.chat_model,
                    s.reasoner_model,
                    s.thinking_depth,
                    s.approval_policy,
                    web_search_summary(&s.web_search),
                    if s.available_models.is_empty() {
                        "(none)".to_string()
                    } else {
                        s.available_models.join(", ")
                    }
                ),
                None => "Settings are not initialized. Add a DeepSeek API key first.".to_string(),
            },
            SlashAction::Permissions => {
                let policy = self.settings.approval_policy()?;
                let rules = self.settings.permission_rules()?;
                format!(
                    "Permissions:\n- policy: {}\n- allow rules: {}\n- ask rules: {}\n- deny rules: {}",
                    policy.label(),
                    format_rule_count(&rules.allow),
                    format_rule_count(&rules.ask),
                    format_rule_count(&rules.deny)
                )
            }
            SlashAction::Knowledge => match &self.knowledge {
                Some(knowledge) => format!(
                    "Knowledge:\n- project entries: {}\n- pending drafts: {}\n- passive injection: {}\n- auto capture: {}",
                    knowledge.list().len(),
                    knowledge.list_drafts().len(),
                    on_off(knowledge.passive_enabled()),
                    on_off(knowledge.auto_capture_enabled())
                ),
                None => "Knowledge is not enabled for this runtime.".to_string(),
            },
            SlashAction::Mcp => match &self.mcp {
                Some(mcp) => {
                    let servers = mcp.list()?;
                    let enabled = servers.iter().filter(|s| s.enabled).count();
                    let names = servers
                        .iter()
                        .take(8)
                        .map(|s| {
                            format!(
                                "{}:{}:{}",
                                s.name,
                                s.transport,
                                if s.enabled { "enabled" } else { "disabled" }
                            )
                        })
                        .collect::<Vec<_>>();
                    format!(
                        "MCP servers: {enabled}/{} enabled{}",
                        servers.len(),
                        if names.is_empty() {
                            ".".to_string()
                        } else {
                            format!(".\n- {}", names.join("\n- "))
                        }
                    )
                }
                None => "MCP is not configured for this runtime.".to_string(),
            },
            SlashAction::Projects => match &self.projects {
                Some(projects) => {
                    let active = projects.active()?.unwrap_or_else(|| "(none)".to_string());
                    let list = projects.list()?;
                    let mut lines = vec![format!("Projects: {} open.", list.len())];
                    lines.push(format!("- active: {active}"));
                    for project in list.iter().take(8) {
                        lines.push(format!(
                            "- {} ({}) - {} session(s)",
                            project.name, project.path, project.session_count
                        ));
                    }
                    lines.join("\n")
                }
                None => format!("Active project: {}", self.effective_root().display()),
            },
            SlashAction::Sessions => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let sessions = store.list_sessions()?;
                let mut lines = vec![format!("Recent sessions: {}", sessions.len())];
                for record in sessions.iter().take(8) {
                    let title = record.title.as_deref().unwrap_or("(untitled)");
                    let project = record
                        .project
                        .as_deref()
                        .map(crate::project_service::folder_name)
                        .unwrap_or_else(|| "(no project)".to_string());
                    lines.push(format!(
                        "- {} - {} - {} - updated {}",
                        record.id,
                        title,
                        project,
                        record.updated_at.as_millis()
                    ));
                }
                lines.join("\n")
            }
            SlashAction::Thinking { depth } => match depth {
                Some(depth) => {
                    let parsed = parse_thinking_depth(&depth)?;
                    let view = self.settings.set_thinking_depth(parsed)?;
                    format!("Thinking depth set to {}.", view.thinking_depth)
                }
                None => {
                    let depth = self.settings.thinking_depth()?;
                    format!(
                        "Thinking depth is {}. Usage: /thinking <simple|medium|deep>.",
                        depth.label()
                    )
                }
            },
            SlashAction::Resume { session_id } => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let rendered: Vec<String> = history
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect();
                let counter = HeuristicTokenizer::new();
                let tokens_before: usize = rendered.iter().map(|t| counter.count(t)).sum();
                let goal = history
                    .first()
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| format!("Resume session {session_id}"));
                let summary =
                    HeuristicSummarizer.summarize(&goal, &TaskSummary::default(), &rendered);
                let summary_block = summary.to_context_block();
                let injected =
                    format!("[Earlier conversation compacted to summary]\n{summary_block}");
                let tokens_after = counter.count(&injected);
                session.append(EventPayload::ContextCompacted {
                    tokens_before: tokens_before as u64,
                    tokens_after: tokens_after as u64,
                    strategy: "resume".to_string(),
                })?;
                session.append(EventPayload::MessageAppended {
                    message: Message::user(injected),
                })?;
                format!(
                    "Resumed session {session_id}. Loaded {} event(s), compacted recovered context from {tokens_before} to {tokens_after} estimated tokens. Continue with your next prompt.",
                    events.len()
                )
            }
            SlashAction::Model { model_id } => match model_id {
                Some(model_id) => {
                    self.settings.set_model(ModelRole::Chat, &model_id)?;
                    format!("Switched chat model to {model_id}.")
                }
                None => match self.settings.view()? {
                    Some(view) if !view.available_models.is_empty() => format!(
                        "可用模型:\n- {}\n\n用法: /model <model_id>",
                        view.available_models.join("\n- ")
                    ),
                    Some(_) => "没有发现可用模型。请先刷新 DeepSeek 模型列表。".to_string(),
                    None => "设置尚未初始化。请先填写并验证 DeepSeek API Key。".to_string(),
                },
            },
            SlashAction::Clear => {
                "Cleared the chat surface. Start a new chat from the sidebar for a fresh session."
                    .to_string()
            }
            SlashAction::Verify => {
                // Phase 4C `/verify`: run the post-edit verifier across the
                // workspace and surface a one-shot summary. We delegate to a
                // best-effort workspace walk so the slash command works even
                // when no recent edit happened.
                self.run_workspace_verification().await
            }
        };
        Ok(message)
    }

    /// Run the post-edit verifier across the active workspace's source files,
    /// returning a short summary. Used by the `/verify` slash command. The
    /// scan is bounded to a small number of files so the command stays cheap;
    /// users who want a thorough run should invoke `cargo check`, `tsc`, etc.
    /// directly through the bash tool.
    async fn run_workspace_verification(&self) -> String {
        let root = self.effective_root();
        let dispatcher =
            Arc::new(crate::verification_dispatcher::VerificationDispatcher::standard());

        // Sample up to 50 verifier-eligible files at the root level — enough
        // to surface obvious breakage without spawning a full project check.
        let mut targets: Vec<PathBuf> = Vec::new();
        collect_verifiable_files(&root, &mut targets, 50);
        if targets.is_empty() {
            return "/verify: no Rust / TS / Python / JSON files found at workspace root."
                .to_string();
        }

        let mut passed = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let mut skipped = 0usize;
        let mut timed_out = 0usize;
        for path in &targets {
            match dispatcher.verify_file(path).await {
                crate::verification_dispatcher::VerificationOutcome::Passed => passed += 1,
                crate::verification_dispatcher::VerificationOutcome::Failed { detail, .. } => {
                    let display = path
                        .strip_prefix(&root)
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    let trimmed: String = detail.lines().take(2).collect::<Vec<_>>().join(" / ");
                    failed.push(format!("{display}: {trimmed}"));
                }
                crate::verification_dispatcher::VerificationOutcome::Skipped { .. } => skipped += 1,
                crate::verification_dispatcher::VerificationOutcome::TimedOut => timed_out += 1,
            }
        }

        let mut lines = Vec::new();
        lines.push(format!(
            "/verify: scanned {n} files in {root}",
            n = targets.len(),
            root = root.display()
        ));
        lines.push(format!(
            "passed: {passed}, failed: {failed_n}, skipped: {skipped}, timed_out: {timed_out}",
            failed_n = failed.len()
        ));
        if !failed.is_empty() {
            lines.push("failures:".into());
            for f in failed {
                lines.push(format!("  - {f}"));
            }
        }
        lines.join("\n")
    }

    /// Model-driven context compaction (Phase 2B). Given the recovered chat
    /// `history`, when token pressure exceeds the policy threshold, compress the
    /// older turns into a structured [`TaskSummary`] (via the model, falling
    /// back to the heuristic summarizer) and return `[summary_turn, recent…]`.
    /// Records a `ContextCompacted` event on success. Returns `history`
    /// unchanged when below threshold (Property 7: backward compatible).
    async fn maybe_compact_history(
        &self,
        session: &mut Session<'_, SystemClock>,
        history: Vec<Message>,
        client: &Arc<ModelClient>,
        model: &str,
    ) -> Vec<Message> {
        let policy = CompactionPolicy::default();
        // Render each turn to a rough "role: content" string for counting +
        // summarization input.
        let rendered: Vec<String> = history
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content))
            .collect();
        let counter = HeuristicTokenizer::new();
        let total: usize = rendered.iter().map(|t| counter.count(t)).sum();

        if !policy.should_compact(total) || history.len() <= policy.keep_recent_turns {
            return history;
        }

        let split = history.len() - policy.keep_recent_turns;
        let older = &rendered[..split];
        let goal = history
            .first()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Model summary with heuristic fallback baked into ModelCompactor.
        let compactor = ModelCompactor::new(client.clone(), model.to_string());
        let summary: TaskSummary = compactor
            .summarize(&goal, &TaskSummary::default(), older)
            .await;
        let summary_block = summary.to_context_block();

        let tokens_after = counter.count(&summary_block)
            + rendered[split..]
                .iter()
                .map(|t| counter.count(t))
                .sum::<usize>();

        // Record the compaction in the session log (best-effort).
        if let Err(e) = session.append(EventPayload::ContextCompacted {
            tokens_before: total as u64,
            tokens_after: tokens_after as u64,
            strategy: "model".to_string(),
        }) {
            tracing::warn!(error = %e, "failed to record ContextCompacted event");
        }

        // Seed the agent with [summary as a user-context turn] + recent turns.
        let mut compacted = Vec::with_capacity(policy.keep_recent_turns + 1);
        compacted.push(Message::user(format!(
            "[Earlier conversation compacted to summary]\n{summary_block}"
        )));
        compacted.extend(history.into_iter().skip(split));
        compacted
    }

    /// Map the sandbox mode to the filesystem access mode used by the built-in
    /// file tools and the path guard:
    /// - 默认权限 (AlwaysAsk) → workspace-confined (out-of-workspace asks).
    /// - 自动审核 (AutoReview) → reads anywhere; writes confined; bash asks.
    /// - 完全访问 (FullAccess) → unrestricted reads + writes.
    fn fs_access_for(mode: crate::settings::SandboxMode) -> deepagent_builtins::FsAccess {
        use crate::settings::SandboxMode;
        use deepagent_builtins::FsAccess;
        match mode {
            SandboxMode::ReadOnly => FsAccess::ReadOnly,
            SandboxMode::WorkspaceWrite => FsAccess::Workspace,
            SandboxMode::FullAccess => FsAccess::Full,
        }
    }

    #[cfg(feature = "web")]
    fn register_web_tools(&self, registry: &mut ToolRegistry) -> Result<()> {
        use crate::settings::WebSearchProvider;
        use deepagent_builtins::{ReqwestWebClient, WebFetchTool, WebSearchTool};

        registry.register(Arc::new(WebFetchTool::new(ReqwestWebClient::new())))?;
        let settings = self.settings.web_search_settings()?;
        if !settings.enabled {
            return Ok(());
        }

        let (deepseek, searxng_url) = match settings.provider {
            WebSearchProvider::DeepSeekFirst => (
                self.deepseek_web_search_config(),
                configured_searxng_url(settings.searxng_url),
            ),
            WebSearchProvider::Searxng => (None, configured_searxng_url(settings.searxng_url)),
            WebSearchProvider::DuckDuckGo => (None, None),
        };
        registry.register(Arc::new(WebSearchTool::new(
            ReqwestWebClient::with_search_config(deepseek, searxng_url),
        )))?;
        Ok(())
    }

    #[cfg(feature = "web")]
    fn deepseek_web_search_config(&self) -> Option<deepagent_builtins::DeepSeekWebSearchConfig> {
        use deepagent_builtins::DeepSeekWebSearchConfig;

        let api_key = self.settings.api_key().ok().flatten()?;
        if api_key.trim().is_empty() {
            return None;
        }
        let settings = self.settings.load().ok().flatten();
        let base_url = settings
            .as_ref()
            .map(|s| s.catalog.base_url.clone())
            .unwrap_or_else(|| "https://api.deepseek.com".to_string());
        let model = std::env::var("DEEPAGENT_DEEPSEEK_WEB_SEARCH_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                settings
                    .as_ref()
                    .map(|s| s.catalog.chat_model.clone())
                    .filter(|s| !s.trim().is_empty())
            })?;
        Some(DeepSeekWebSearchConfig::new(api_key, base_url, model))
    }

    /// Build the tool registry with the built-ins confined to `root`.
    ///
    /// Includes `ask_user_question` (wired to a headless-safe responder), the
    /// file/bash/search/todo built-ins, and the network web tools (with the
    /// `web` feature). It deliberately does **not** include the `task`
    /// sub-agent tool — that is added only to the *main* run's registry (see
    /// [`ChatService::run_in_session`]) so sub-agents can't recurse into more
    /// sub-agents, mirroring Claude Code's agent-disallowed-tools rule.
    fn build_registry(
        &self,
        root: &std::path::Path,
        access: deepagent_builtins::FsAccess,
        env_mode: Option<&str>,
        connection_id: Option<&str>,
        local_exec_mode: Option<crate::settings::LocalExecutionMode>,
    ) -> Result<(ToolRegistry, deepagent_builtins::TodoStore)> {
        use deepagent_builtins::{
            register_builtins, AskUserQuestionTool, BuiltinConfig, DeclineResponder, WorkspaceRoot,
        };
        let mut registry = ToolRegistry::new();
        let mut config = BuiltinConfig::new(
            WorkspaceRoot::new(root.to_path_buf()).with_access(access),
            self.bash_allow.clone(),
        );
        // When the session is remote, create an SSH-backed executor through the
        // factory and attach it to the builtin config so bash/git commands are
        // routed through SSH instead of local execution.
        if let (Some("remote"), Some(factory), Some(conn_id)) =
            (env_mode, &self.executor_factory, connection_id)
        {
            config = config.with_command_executor(factory(conn_id.to_string()));
        } else if env_mode != Some("remote") {
            let use_sandbox = match local_exec_mode {
                Some(crate::settings::LocalExecutionMode::Direct) => false,
                _ => true,
            };
            if use_sandbox {
                if let Some(executor) = &self.local_command_executor {
                    config = config.with_command_executor(executor.clone());
                }
            }
        }
        let todo_store = register_builtins(&mut registry, config)?;
        // Network web tools (web_fetch / web_search) when built with `web`.
        #[cfg(feature = "web")]
        self.register_web_tools(&mut registry)?;

        // Interactive tool (Claude-Code parity): surfaces multiple-choice
        // questions to the user. Wired to DeclineResponder here (headless-safe);
        // the desktop app can later supply a dialog-backed responder.
        registry.register(Arc::new(AskUserQuestionTool::new(DeclineResponder)))?;

        // Knowledge base active channel: `knowledge_search` is read-only and
        // safe, so BOTH the main run and sub-agents get it (registered here in
        // the shared builder). `knowledge_write` is added only to the main run
        // (see `run_in_session`) so sub-agents cannot litter the vault.
        if let Some(knowledge) = &self.knowledge {
            use deepagent_builtins::KnowledgeSearchTool;
            let backend = crate::knowledge_service::KnowledgeServiceBackend::new(knowledge.clone());
            registry.register(Arc::new(KnowledgeSearchTool::new(backend)))?;
        }
        if let Some(project_map) = &self.project_map {
            use deepagent_builtins::{
                CodeMapImpactTool, CodeMapNeighborsTool, CodeMapOverviewTool, CodeMapSearchTool,
            };
            let backend = ProjectMapToolBackend {
                service: project_map.clone(),
                root: root.to_path_buf(),
            };
            registry.register(Arc::new(CodeMapOverviewTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeMapSearchTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeMapNeighborsTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeMapImpactTool::new(backend)))?;
        }
        {
            use deepagent_builtins::{
                CodeGraphCalleesTool, CodeGraphCallersTool, CodeGraphExploreTool,
                CodeGraphImpactTool, CodeGraphLocateTool, CodeGraphNodeTool, CodeGraphSearchTool,
            };
            let backend = CodeGraphToolBackend {
                root: root.to_path_buf(),
            };
            registry.register(Arc::new(CodeGraphSearchTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphExploreTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphCallersTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphCalleesTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphImpactTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphNodeTool::new(backend.clone())))?;
            registry.register(Arc::new(CodeGraphLocateTool::new(backend)))?;
        }
        #[cfg(feature = "runtimes")]
        {
            if let Some(office) = &self.office {
                use deepagent_builtins::{
                    OfficeDocxCreateTool, OfficeReadTool, OfficeXlsxCreateTool,
                };
                let backend = OfficeToolBackend {
                    service: office.clone(),
                };
                registry.register(Arc::new(OfficeReadTool::new(backend.clone())))?;
                registry.register(Arc::new(OfficeDocxCreateTool::new(backend.clone())))?;
                registry.register(Arc::new(OfficeXlsxCreateTool::new(backend)))?;
            }
        }
        if let (Some("remote"), Some(factory), Some(conn_id)) =
            (env_mode, &self.remote_ops_factory, connection_id)
        {
            use deepagent_builtins::{
                RemoteInstallTool, RemoteProbeTool, RemotePushBundleTool, RemotePushFileTool,
                RemoteRequireTool,
            };
            let backend = factory(conn_id.to_string());
            registry.register(Arc::new(RemoteProbeTool::new(backend.clone())))?;
            registry.register(Arc::new(RemotePushFileTool::new(backend.clone())))?;
            registry.register(Arc::new(RemotePushBundleTool::new(backend.clone())))?;
            registry.register(Arc::new(RemoteRequireTool::new(backend.clone())))?;
            registry.register(Arc::new(RemoteInstallTool::new(backend)))?;
        }

        Ok((registry, todo_store))
    }

    /// Wire the `tool_search` built-in into a registry that already contains
    /// every other tool (built-ins + MCP + knowledge + code-map). No-op when
    /// `mode == Disabled` or when `Auto` mode's deferred-tool schema budget
    /// is below the threshold.
    ///
    /// Returns the **names** of every deferred tool captured in the snapshot
    /// (empty vec when no-op). The caller uses this list to produce the
    /// `<available-deferred-tools>` system-prompt block: undiscovered names
    /// = `returned_names \ discovered_set`.
    fn maybe_register_tool_search(
        &self,
        registry: &mut ToolRegistry,
        mode: deepagent_builtins::ToolSearchMode,
        discovered: DiscoveredToolSet,
        auto_threshold_chars: usize,
    ) -> Result<Vec<String>> {
        register_tool_search_into(registry, mode, discovered, auto_threshold_chars)
    }

    /// Wire the `skill` built-in into a registry. No-op when no
    /// [`SkillsService`](crate::skills_service::SkillsService) was attached
    /// via [`ChatService::with_skills`] (the byte-equivalent default for
    /// callers that don't opt in).
    ///
    /// The registry held by [`SkillTool`][deepagent_builtins::SkillTool] is
    /// an immutable [`Arc`]-wrapped snapshot. We clone the live
    /// [`SkillRegistry`][deepagent_skills::SkillRegistry] once per run
    /// (cheap — `SkillRegistry` is a `BTreeMap` of `Skill`s and is
    /// `Clone`); subsequent installs / uninstalls / reloads take effect on
    /// the NEXT run, not this one. That matches
    /// [`ToolSearchTool`][deepagent_builtins::ToolSearchTool]'s
    /// deferred-tool snapshot semantics and keeps the in-flight loop
    /// stable.
    ///
    /// _Validates: Requirements R6.1, R6.2, R6.3, R6.4, R6.5, R6.6._
    fn maybe_register_skill_tool(&self, registry: &mut ToolRegistry) -> Result<()> {
        if let Some(skills) = &self.skills {
            let registry_snapshot = {
                let svc = skills
                    .lock()
                    .map_err(|_| CoreError::invalid("skills service mutex poisoned"))?;
                Arc::new(svc.manager().registry().clone())
            };
            registry.register(Arc::new(deepagent_builtins::SkillTool::new(
                registry_snapshot,
            )))?;
        }
        Ok(())
    }

    fn office_skill_guard_hook(
        &self,
        session_id: &str,
        prior_events: &[deepagent_core::event::Event],
    ) -> Result<Option<OfficeSkillGuardHook>> {
        if self.office.is_none() {
            return Ok(None);
        }
        let Some(skills) = &self.skills else {
            return Ok(None);
        };
        let enforce_skills: std::collections::HashSet<String> = {
            let svc = skills
                .lock()
                .map_err(|_| CoreError::invalid("skills service mutex poisoned"))?;
            ["docx", "xlsx", "pptx", "pdf"]
                .into_iter()
                .filter(|id| svc.manager().registry().contains(id))
                .map(str::to_string)
                .collect()
        };
        if enforce_skills.is_empty() {
            return Ok(None);
        }
        let hook = OfficeSkillGuardHook::new(self.invoked_skills.clone(), enforce_skills);
        hook.seed_session(
            session_id,
            collect_invoked_skill_ids_from_events(prior_events),
        );
        Ok(Some(hook))
    }

    /// Build a model client for the given role from persisted settings + the
    /// stored API key. Thinking depth is a request-parameter concern only; it
    /// does not implicitly swap the selected model role.
    fn build_model(&self, role: ModelRole) -> Result<(Arc<ModelClient>, String, ThinkingDepth)> {
        let settings = self
            .settings
            .load()?
            .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
        let api_key = self
            .settings
            .api_key()?
            .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
        let thinking_depth = settings.thinking_depth;
        let model = settings.catalog.model_for(role).to_string();
        let config = ModelConfig::from_catalog(api_key, &settings.catalog, role);
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));
        Ok((client, model, thinking_depth))
    }

    /// Run a single non-session, non-tool, streaming LLM completion.
    ///
    /// Used by [`crate::skills_service::ai_security_review`] (skill-marketplace
    /// task 5) and any other ephemeral one-shot prompt needing the user's
    /// configured chat model + API key without polluting the session log,
    /// running tools, or starting the runtime engine. Each visible content
    /// fragment streamed by the provider is forwarded through `on_token`; the
    /// fully assembled assistant text is returned at the end.
    ///
    /// Reuses [`ChatService::build_model`] so model selection (incl. Deep
    /// thinking → reasoner role), API-key resolution, and the persisted
    /// `ThinkingDepth` profile stay consistent with the regular chat run.
    pub async fn run_oneshot_streaming<F>(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        on_token: F,
    ) -> Result<String>
    where
        F: FnMut(&str) + Send + 'static,
    {
        let (client, model, thinking_depth) = self.build_model(ModelRole::Chat)?;
        let messages = vec![
            deepagent_core::message::Message::system(system_prompt),
            deepagent_core::message::Message::user(user_prompt),
        ];
        let request = deepagent_models::chat::ChatRequest::new(model, messages)
            .streaming()
            .with_thinking_depth(thinking_depth);

        struct CallbackObserver<F: FnMut(&str) + Send> {
            on_token: F,
        }
        impl<F: FnMut(&str) + Send> deepagent_models::stream::DeltaObserver for CallbackObserver<F> {
            fn on_content(&mut self, delta: &str) {
                (self.on_token)(delta);
            }
        }

        let mut observer = CallbackObserver { on_token };
        let response = client.stream_chat_observed(request, &mut observer).await?;
        Ok(response.message.content)
    }

    /// Specialized one-shot streaming variant for the **AI skill review** path.
    ///
    /// Differs from [`Self::run_oneshot_streaming`] in three deliberate ways
    /// to keep skill installs snappy without sacrificing the structured
    /// PASS / FAIL audit (per skill-marketplace QA feedback: 32K reasoning
    /// budgets and Reasoner-model swaps are wasted overhead for what is
    /// essentially a yes/no security classification):
    ///
    /// 1. **Model selection respects the user's `skill_install_ai_review_model`
    ///    override** (already a public R10.4 setting). When that's `None`
    ///    the call falls back to the catalog's chat model (Flash by
    ///    default) — never the Reasoner. The Deep-thinking → Reasoner
    ///    swap that [`Self::build_model`] applies for normal chat is
    ///    intentionally skipped here because skill audits don't benefit
    ///    from that role change.
    /// 2. **Caller picks the [`ThinkingDepth`]** explicitly (typically
    ///    `Simple` for the install-dialog initial pass and `Medium` for an
    ///    explicit re-review). The user's persisted global thinking depth
    ///    is intentionally NOT consulted.
    /// 3. **Output token ceiling is set explicitly** via `max_output_tokens`
    ///    BEFORE [`with_thinking_depth`][deepagent_models::ChatRequest::with_thinking_depth]
    ///    is applied — that helper only fills `max_tokens` when it's still
    ///    `None`, so the explicit ceiling survives and acts as a hard cap
    ///    on the model's combined reasoning + reply budget.
    pub async fn run_review_streaming<F>(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        thinking_depth: ThinkingDepth,
        max_output_tokens: u32,
        on_token: F,
    ) -> Result<String>
    where
        F: FnMut(&str) + Send + 'static,
    {
        let settings = self
            .settings
            .load()?
            .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
        let api_key = self
            .settings
            .api_key()?
            .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;

        // Model resolution: user override > catalog chat model. Never
        // promote to the Reasoner — skill review is a structured task, not a
        // long-form reasoning workload.
        let configured = self.settings.skill_install_ai_review_model()?;
        let chat_model = settings.catalog.model_for(ModelRole::Chat).to_string();
        let review_model = configured
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(chat_model);

        let config = ModelConfig::from_catalog(api_key, &settings.catalog, ModelRole::Chat);
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));

        let messages = vec![
            deepagent_core::message::Message::system(system_prompt),
            deepagent_core::message::Message::user(user_prompt),
        ];
        // Order matters: `with_max_tokens` must come BEFORE
        // `with_thinking_depth` so the explicit cap survives. The depth
        // helper only fills `max_tokens` when it's still `None`.
        let request = deepagent_models::chat::ChatRequest::new(review_model, messages)
            .streaming()
            .with_max_tokens(max_output_tokens)
            .with_thinking_depth(thinking_depth);

        struct CallbackObserver<F: FnMut(&str) + Send> {
            on_token: F,
        }
        impl<F: FnMut(&str) + Send> deepagent_models::stream::DeltaObserver for CallbackObserver<F> {
            fn on_content(&mut self, delta: &str) {
                (self.on_token)(delta);
            }
        }

        let mut observer = CallbackObserver { on_token };
        let response = client.stream_chat_observed(request, &mut observer).await?;
        Ok(response.message.content)
    }

    /// Generate and persist an AI title for a session when it is still
    /// untitled. Intended for post-run refinement: if the user renames the
    /// session before generation finishes, the second title check prevents the
    /// auto title from overwriting the explicit one.
    pub async fn maybe_generate_session_title(&self, session_id: &str) -> Result<Option<String>> {
        use deepagent_core::message::Role;
        use deepagent_persistence::event_store::EventStore;

        let id = deepagent_core::id::SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let store = EventStore::new(&self.db);
        let Some(record) = store.get_session(id)? else {
            return Err(CoreError::not_found(format!("session {session_id}")));
        };
        if record
            .title
            .as_deref()
            .map(|title| !title.trim().is_empty())
            .unwrap_or(false)
        {
            return Ok(None);
        }

        let events = store.load_session(id)?;
        let history = conversation_from_events(&events);
        let mut lines = Vec::new();
        let mut user_messages = 0usize;
        for message in &history {
            let text = message.content.trim();
            if text.is_empty() {
                continue;
            }
            match message.role {
                Role::User => {
                    user_messages += 1;
                    lines.push(format!("User: {text}"));
                    if user_messages >= 3 {
                        break;
                    }
                }
                Role::Assistant => {
                    if user_messages == 0 {
                        continue;
                    }
                    lines.push(format!("Assistant: {text}"));
                }
                _ => {}
            }
        }
        if lines.is_empty() {
            return Ok(None);
        }

        let settings = self
            .settings
            .load()?
            .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
        let api_key = self
            .settings
            .api_key()?
            .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
        let model = settings.catalog.model_for(ModelRole::Chat).to_string();
        let config = ModelConfig::from_catalog(api_key, &settings.catalog, ModelRole::Chat);
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));

        let request = deepagent_models::chat::ChatRequest::new(
            model,
            vec![
                deepagent_core::message::Message::system(SESSION_TITLE_SYSTEM_PROMPT),
                deepagent_core::message::Message::user(format!(
                    "Create a short conversation title from this transcript:\n\n{}",
                    lines.join("\n")
                )),
            ],
        )
        .streaming()
        .with_max_tokens(48)
        .with_thinking_depth(ThinkingDepth::Simple);
        let response = client.stream_chat(request).await?;
        let Some(title) = normalize_generated_session_title(&response.message.content) else {
            return Ok(None);
        };

        let current = store.get_session(id)?;
        if current
            .as_ref()
            .and_then(|session| session.title.as_deref())
            .map(|title| !title.trim().is_empty())
            .unwrap_or(false)
        {
            return Ok(None);
        }

        let clock = SystemClock;
        if !store.rename_session(id, Some(&title), clock.now())? {
            return Err(CoreError::not_found(format!("session {session_id}")));
        }
        Ok(Some(title))
    }

    /// Run one streamed chat turn-loop for `prompt`, forwarding every
    /// [`RuntimeEvent`] to `on_event` and any approval request to `on_approval`.
    /// Returns the new session id.
    ///
    /// Approval handling follows the persisted approval policy: `AutoReview` /
    /// `FullAccess` resolve automatically (no prompt); `AlwaysAsk` emits an
    /// [`ApprovalRequestDto`] via `on_approval` and the run **pauses** until the
    /// UI calls `resolve_approved` on [`ChatService::pending_approvals`].
    ///
    /// This always starts a **new** session; use [`ChatService::run_in_session`]
    /// to continue an existing one.
    pub async fn run<F, A>(&self, prompt: &str, on_event: F, on_approval: A) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        self.run_in_session(
            prompt,
            None,
            None,
            None,
            Vec::new(),
            None,
            on_event,
            on_approval,
        )
        .await
    }

    /// Like [`ChatService::run`], but when `continue_session` names an existing
    /// session the new turn is **appended** to it (the prior conversation is
    /// recovered from the event log and replayed to the model) instead of
    /// starting a fresh session. Returns the session id used (the continued one,
    /// or a newly created one when `continue_session` is `None`).
    pub async fn run_in_session<F, A>(
        &self,
        prompt: &str,
        continue_session: Option<&str>,
        env_mode: Option<&str>,
        connection_id: Option<&str>,
        preflight_tools: Vec<PreflightToolCallDto>,
        preflight_abort_message: Option<String>,
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        if let Some(session_id) = self
            .maybe_handle_slash_command(prompt, continue_session, &on_event)
            .await?
        {
            return Ok(session_id);
        }

        let model_prompt = self
            .dynamic_command_prompt(prompt)?
            .unwrap_or_else(|| prompt.to_string());
        let prompt_for_model = model_prompt.as_str();

        // Budget gate: refuse a new run when a configured daily/monthly limit is
        // already exhausted. No-op when no cost tracker is attached or no budget
        // is set (Property 7: backward-compatible default).
        if let Some(cost) = &self.cost {
            cost.check_budget()?;
        }

        let root = self.effective_root();
        if let Some(knowledge) = &self.knowledge {
            knowledge.activate_project(&root)?;
        }
        let profile = self.settings.effective_permission_profile()?;
        let policy = profile.approval_policy;
        let sandbox_mode = profile.sandbox_mode;
        let local_execution_mode = profile.local_execution_mode;
        let access = Self::fs_access_for(sandbox_mode);
        // Update the Sandboxie executor's OS-level confinement for this run.
        if let Some(sbox) = &self.sandboxie_executor {
            sbox.set_sandbox_mode(sandbox_mode);
        }
        let plan = continue_session
            .map(|id| self.plan_mode_for_session(id))
            .unwrap_or_default();
        // The main run's tools are built permissive (Full, sensitive-blocked):
        // the BeforeToolUse path guard is the SINGLE policy gate, asking/denying
        // per the sandbox-derived `access`. This way an out-of-workspace access
        // the sandbox allows actually executes, instead of being
        // re-rejected inside the tool. Sub-agents (below) have no interactive
        // gate, so their tools stay confined to the sandbox `access`.
        let clock = SystemClock;
        // Bind the session to the active project (its folder path) so the
        // sidebar groups it under the right project folder.
        let project = root.to_string_lossy().into_owned();
        // Continuation vs new session: when `continue_session` names an existing
        // session, recover it and append the new turn (so one chat thread keeps
        // accumulating); otherwise start a fresh session. Recovery also lets us
        // rebuild the prior conversation to seed the model with context.
        let (mut session, history, prior_events) = match continue_session {
            Some(id_str) => {
                let id = deepagent_core::id::SessionId::from_str(id_str)
                    .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
                let session = Session::recover(&self.db, &clock, id)?;
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(id)?;
                let history = conversation_from_events(&events);
                (session, history, events)
            }
            None => {
                let mode = match env_mode {
                    Some("remote") => deepagent_core::SessionMode::Remote,
                    _ => deepagent_core::SessionMode::Normal,
                };
                let session =
                    Session::create_in_project(&self.db, &clock, None, mode, Some(&project))?;
                (session, Vec::new(), Vec::new())
            }
        };

        // Derive the effective env_mode: for continuation sessions, use the
        // persisted session mode; for new sessions, use the caller-supplied
        // env_mode. This ensures the SSH executor is used when the session was
        // created in Remote mode, even if the frontend doesn't re-send env_mode.
        let effective_env_mode = match continue_session {
            Some(_) => match session.state().mode {
                deepagent_core::SessionMode::Remote => Some("remote"),
                _ => None,
            },
            None => env_mode,
        };

        let (registry, todo_store) = self.build_registry(
            &root,
            deepagent_builtins::FsAccess::Full,
            effective_env_mode,
            connection_id,
            Some(local_execution_mode),
        )?;
        let (client, model, thinking_depth) = self.build_model(ModelRole::Chat)?;

        let session_id_str = session.id().to_string();

        // Restore previously-discovered tools from the event log (Phase 3C).
        // `ToolsDiscovered` events carry deltas; the cumulative set is the
        // union across every such event in the session history. Matters only
        // for resumed sessions; fresh sessions have an empty `prior_events`.
        let restored_discovered = collect_discovered_tools_from_events(&prior_events);
        if !restored_discovered.is_empty() {
            let set = self.discovered_tools_for_session(&session_id_str);
            let mut guard = set.lock().unwrap_or_else(|p| p.into_inner());
            for name in restored_discovered {
                guard.insert(name);
            }
        }

        // Live MCP tool registration: connect enabled servers and register their
        // namespaced tools into the runtime registry, so they are advertised to
        // the model and routed like built-ins. Connection failures are
        // non-fatal (logged + skipped) so one bad server never blocks a run.
        let mut registry = registry;
        if let Some(mcp) = &self.mcp {
            match mcp.connect_enabled().await {
                Ok((mcp_registry, failures)) => {
                    if !failures.is_empty() {
                        tracing::warn!(
                            count = failures.len(),
                            "some MCP servers failed to connect"
                        );
                    }
                    let mcp_registry = std::sync::Arc::new(mcp_registry);
                    for adapter in deepagent_mcp::adapters_for(mcp_registry) {
                        if let Err(e) = registry.register(adapter) {
                            tracing::warn!(error = %e, "failed to register MCP tool adapter");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "MCP connect_enabled failed; continuing without MCP tools");
                }
            }
        }

        // Tool-search settings (read once per run) — used both for the main
        // run's tool-search wiring and for seeding the sub-agent runner with
        // the parent's discovered tool set.
        let tool_search_mode = self.settings.tool_search_mode().unwrap_or_default();
        let tool_search_threshold = self
            .settings
            .tool_search_auto_threshold()
            .unwrap_or(SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS);
        let tool_search_discovered = self.discovered_tools_for_session(&session_id_str);

        // Sub-agent orchestration (Claude-Code parity): register the `task`
        // tool into the MAIN run's registry only. Its runner executes a nested
        // agent loop over a fresh sub-registry (the same built-ins, minus
        // `task` itself) so a sub-agent cannot spawn further sub-agents. The
        // nested run uses an ephemeral in-memory session and returns only its
        // final message, keeping intermediate output out of the main context.
        //
        // Tool-search inheritance: the runner captures a snapshot of the
        // parent's discovered set (cloned out of the Mutex) so the sub-agent
        // starts with the parent's active toolset. Each sub-agent invocation
        // gets its own fresh `Arc<Mutex<HashSet>>` seeded from that snapshot,
        // so discoveries inside a sub-agent don't leak back into the parent
        // (Req 4.6 default behavior, now extended with snapshot inheritance).
        {
            use deepagent_builtins::TaskTool;
            let sub_registry = Arc::new(self.build_registry(&root, access, None, None, Some(local_execution_mode))?.0);
            let parent_discovered_snapshot: std::collections::HashSet<String> =
                tool_search_discovered
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
            let runner = ChatSubagentRunner {
                client: client.clone(),
                model: model.clone(),
                thinking_depth,
                registry: sub_registry,
                root: root.clone(),
                tool_search_mode,
                tool_search_auto_threshold: tool_search_threshold,
                parent_discovered_snapshot,
            };
            registry.register(Arc::new(TaskTool::new(runner, Vec::<String>::new())))?;
        }

        // Knowledge capture: the `knowledge_write` tool is added to the MAIN
        // run's registry only (sub-agents get search but not write), so the
        // agent can persist reusable knowledge it discovers this turn.
        if let Some(knowledge) = &self.knowledge {
            use deepagent_builtins::KnowledgeWriteTool;
            let backend = crate::knowledge_service::KnowledgeServiceBackend::new(knowledge.clone());
            registry.register(Arc::new(KnowledgeWriteTool::new(backend)))?;
        }

        registry.register(Arc::new(deepagent_builtins::EnterPlanModeTool::new(
            plan.clone(),
        )))?;
        registry.register(Arc::new(deepagent_builtins::ExitPlanModeTool::new(
            plan.clone(),
        )))?;

        // Skill tool (channel B of the auto-activation design). Only wired
        // when a `SkillsService` was attached via [`ChatService::with_skills`]
        // — without it, the chat runtime has no way to look up skills and
        // the catalog reminder injection below is also a no-op (Property 9).
        self.maybe_register_skill_tool(&mut registry)?;

        // Tool-search wiring (lazy tool loading). Snapshot deferred tools
        // AFTER built-ins, MCP, knowledge, code-map, plan-mode, and skill
        // tools are all registered. No-op when the user hasn't enabled
        // tool-search mode (default Disabled is byte-equivalent to the old
        // behavior).
        let deferred_tool_names = self.maybe_register_tool_search(
            &mut registry,
            tool_search_mode,
            tool_search_discovered.clone(),
            tool_search_threshold,
        )?;

        // Advertise the registry's visible tools to the model.
        let granted = PermissionSet::developer();
        let tools: Vec<ToolSchema> = build_visible_tool_schemas(
            &registry,
            &granted,
            tool_search_mode,
            &tool_search_discovered,
        );

        // Wire the event sink: a channel the loop emits into, drained by a task
        // that calls `on_event`.
        let (sink, mut rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        let pump = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                on_event(ev);
            }
        });

        // Wire the approval gate: AlwaysAsk → channel gate (prompts the UI);
        // auto policies short-circuit to allow.
        let channel_gate = ChannelApprovalGate::new(self.pending.clone(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> = Arc::new(
            PolicyGate::new(policy, Arc::new(channel_gate))
                .with_classifier(deepagent_builtins::SafetyClassifier::with_defaults()),
        );

        // Wire hooks: declarative permission rules + path/bash safety guards +
        // declarative external hooks (hooks.json), all at their lifecycle
        // points. The rules resolve allow/ask/deny; the guards add
        // path-confinement and command-safety as a centralized boundary; the
        // external hooks run user/plugin-declared commands (e.g. a PreToolUse
        // validator that blocks dangerous bash via exit code 2).
        let rules = self.settings.permission_rules().unwrap_or_default();
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(deepagent_builtins::PlanModeHook::new(plan.clone())),
        );
        if let Some(office_skill_guard) =
            self.office_skill_guard_hook(&session_id_str, &prior_events)?
        {
            let hook: Arc<dyn Hook> = Arc::new(office_skill_guard);
            hooks.register(HookPoint::BeforeToolUse, hook.clone());
            hooks.register(HookPoint::AfterToolUse, hook);
        }
        if !rules.is_empty() {
            hooks.register(
                HookPoint::BeforeToolUse,
                Arc::new(PermissionRulesHook::new(rules)),
            );
        }
        // Declarative external hooks from hooks.json (best-effort: malformed
        // JSON is logged and skipped rather than failing the run).
        match self.settings.hook_definitions() {
            Ok(defs) if !defs.is_empty() => {
                let runner: Arc<dyn HookCommandRunner> = Arc::new(SystemHookRunner);
                let n = defs.register_into(&mut hooks, runner);
                tracing::info!(count = n, "registered external hooks from hooks.json");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "ignoring malformed hooks.json");
            }
        }
        deepagent_builtins::register_guard_hooks(
            &mut hooks,
            WorkspaceRoot::new(root.clone()).with_access(access),
            self.bash_allow.clone(),
        );

        // Model-driven context compaction (Phase 2B): when the recovered history
        // is large (token pressure over the policy threshold), compress the
        // older turns into a structured summary and seed the agent with
        // [summary + recent turns] instead of the full transcript. Falls back to
        // the heuristic summarizer if the model call fails, and records a
        // `ContextCompacted` event. No-op for new sessions / short history.
        let history = self
            .maybe_compact_history(&mut session, history, &client, &model)
            .await;
        let session_id = session.id().to_string();
        {
            let mut map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
            map.entry(session_id.clone())
                .or_insert_with(|| plan.clone());
        }
        // Record the incoming user turn so the thread's history is complete.
        session.append(EventPayload::MessageAppended {
            message: Message::user(prompt),
        })?;
        let task = session.create_task(prompt)?;
        for tool in &preflight_tools {
            let call = ToolCall {
                id: tool.call_id.clone(),
                name: tool.name.clone(),
                arguments: tool.arguments.clone(),
            };
            sink.emit(RuntimeEvent::ToolStarted {
                name: call.name.clone(),
                call_id: call.id.clone(),
                arguments: call.arguments.clone(),
            });
            session.append(EventPayload::ToolCallRequested { call })?;
            session.append(EventPayload::ToolCallCompleted {
                call_id: tool.call_id.clone(),
                ok: tool.ok,
                output: tool.output.clone(),
                duration_ms: tool.duration_ms,
            })?;
            sink.emit(RuntimeEvent::ToolCompleted {
                name: tool.name.clone(),
                call_id: tool.call_id.clone(),
                ok: tool.ok,
                output: tool.output.clone(),
                duration_ms: tool.duration_ms,
            });
        }
        if let Some(abort_message) = preflight_abort_message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            let abort_message = abort_message.to_string();
            session.transition_task(task, deepagent_core::task::TaskState::Running)?;
            sink.emit(RuntimeEvent::RunStarted {
                task_id: task.to_string(),
            });
            sink.emit(RuntimeEvent::SessionRegistered {
                session_id: session_id.clone(),
                title: session.state().title.clone(),
            });
            sink.emit(RuntimeEvent::TurnStarted { step: 0 });
            sink.emit(RuntimeEvent::ContentDelta {
                text: abort_message.clone(),
            });
            session.append(EventPayload::MessageAppended {
                message: Message::assistant(&abort_message),
            })?;
            session.transition_task(task, deepagent_core::task::TaskState::Completed)?;
            sink.emit(RuntimeEvent::RunCompleted {
                message: abort_message,
            });
            drop(sink);
            let _ = pump.await;
            return Ok(session_id);
        }

        // Passive knowledge injection (primary precision channel): retrieve
        // entries relevant to this prompt and inject them as a `<system-
        // reminder>` block prepended to the user-facing message we send to
        // the model. This keeps the system prompt's static + dynamic split
        // unchanged and routes the hint through the well-known reminder
        // meta-channel (Phase 3C), so the model treats it as system metadata
        // rather than authentic user wording.
        let mut system_prompt = build_system_prompt(&root);
        // Git context injection (Phase 2A): append a compact VCS snapshot after
        // the DYNAMIC boundary so the cacheable static prefix stays intact.
        // Best-effort: no git / non-repo yields nothing (backward compatible).
        if let Some(git) = deepagent_workspace::detect_git_context(&root) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&git.to_prompt_block());
        }
        // Sandbox permissions injection: tell the model its current sandbox
        // constraints so it doesn't attempt blocked operations or retry
        // indefinitely after a denial.
        {
            let perm_block = crate::permissions_prompt::sandbox_instructions(sandbox_mode);
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&perm_block);
        }
        // Tool-search announcement (Phase 3B). Lists the deferred tool names
        // the model can `tool_search` for. Lives in the dynamic section so
        // the static prefix stays cache-stable; emitted only when at least
        // one deferred tool is currently undiscovered.
        let undiscovered_deferred_names: Vec<String> = {
            let set = tool_search_discovered
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut out: Vec<String> = deferred_tool_names
                .iter()
                .filter(|n| !set.contains(n.as_str()))
                .cloned()
                .collect();
            out.sort();
            out.dedup();
            out
        };
        if let Some(block) = deferred_tools_announcement(&undiscovered_deferred_names) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&block);
        }
        // Skill-catalog reminder injection (channel A of the auto-activation
        // design). Only when a `SkillsService` is attached AND the catalog
        // master switch is on AND something new has appeared since the last
        // turn. The state-tracker mutates per-session so a fresh session
        // sees the full visible registry on turn 0; subsequent turns only
        // see deltas (Property 11). The block is wrapped in
        // `<system-reminder>` so the model treats it as meta-channel
        // commentary rather than authentic user wording — the rendered
        // body is itself an `<available-skills>` envelope produced by
        // [`SkillRegistry::formatted_catalog`]
        // (deepagent-skills/src/registry.rs).
        if let Some(skills) = &self.skills {
            let settings = self.settings.load().ok().flatten();
            let catalog_block = if let Some(settings) = settings {
                let svc = skills
                    .lock()
                    .map_err(|_| CoreError::invalid("skills service mutex poisoned"))?;
                let mut state_map = self.skill_catalog_state.lock().unwrap_or_else(|p| {
                    // Poisoned guard: clear the inner map so subsequent
                    // turns recover. The current turn re-announces the full
                    // catalog (default state).
                    let mut inner = p.into_inner();
                    inner.clear();
                    inner
                });
                let entry = state_map.entry(session_id.clone()).or_default();
                entry.next_delta(svc.manager().registry(), &settings)
            } else {
                // Settings not initialized yet (the user hasn't set an API
                // key). The chat run won't actually reach the model — it'll
                // fail upstream — but we still don't want to crash the
                // catalog injection here.
                None
            };
            if let Some(block) = catalog_block {
                system_prompt.push_str("\n\n");
                system_prompt.push_str(&crate::system_reminder::wrap(&block));
            }
            let invoked = collect_invoked_skill_records_from_events(&prior_events);
            if let Some(block) = invoked_skills_reminder(&invoked) {
                system_prompt.push_str("\n\n");
                system_prompt.push_str(&crate::system_reminder::wrap(&block));
            }
        }
        let knowledge_reminder = self
            .knowledge
            .as_ref()
            .map(|k| k.passive_block(prompt_for_model))
            .filter(|b| !b.trim().is_empty())
            .map(|b| crate::system_reminder::wrap(&b));
        let remote_reminder = if matches!(effective_env_mode, Some("remote")) {
            if let (Some(factory), Some(conn_id)) = (&self.remote_context_factory, connection_id) {
                match factory(conn_id.to_string()).await {
                    Ok(Some(block)) if !block.trim().is_empty() => {
                        Some(crate::system_reminder::wrap(&block))
                    }
                    Ok(_) => None,
                    Err(err) => {
                        tracing::warn!(connection_id = conn_id, error = %err, "failed to collect remote context");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let mut prompt_prefixes: Vec<String> = Vec::new();
        if let Some(reminder) = remote_reminder {
            prompt_prefixes.push(reminder);
        }
        if let Some(reminder) = knowledge_reminder.clone() {
            prompt_prefixes.push(reminder);
        }
        let final_user_prompt: String = if prompt_prefixes.is_empty() {
            prompt_for_model.to_string()
        } else {
            format!("{}\n\n{}", prompt_prefixes.join("\n\n"), prompt_for_model)
        };

        // Clone the model handle for the post-run auto-capture (the originals
        // are moved into the agent below).
        let capture_client = client.clone();
        let capture_model = model.clone();
        // Model name for cost attribution (the original `model` is moved into
        // the agent below).
        let model_name_for_cost = model.clone();

        let mut agent = ModelAgent::new(client, model, system_prompt, final_user_prompt, tools)
            .with_thinking_depth(thinking_depth)
            .with_history(history)
            .with_events(sink.clone());

        let verification_policy = self.settings.verification_policy().unwrap_or_default();

        let mut per_tool_max_tokens = std::collections::BTreeMap::new();
        per_tool_max_tokens.insert(deepagent_builtins::SKILL_TOOL_NAME.to_string(), 24_000);
        let config = RuntimeConfig {
            permissions: granted,
            tool_result_budget: deepagent_runtime::ToolResultBudgetConfig {
                output_dir: self.tool_results_dir.clone(),
                per_tool_max_tokens,
                ..Default::default()
            },
            tool_result_decorator: Some(Arc::new(
                deepagent_runtime::ChainDecorator::new()
                    .push(Arc::new(
                        crate::plan_mode_reminder::PlanModeReminderDecorator::new(plan.clone()),
                    ))
                    .push(Arc::new(
                        crate::todo_snapshot_reminder::TodoSnapshotReminderDecorator::new(
                            todo_store.clone(),
                        ),
                    ))
                    .push(Arc::new(
                        crate::verification_decorator::VerificationDecorator::with_policy(
                            Arc::new(
                                crate::verification_dispatcher::VerificationDispatcher::standard(),
                            ),
                            Some(root.clone()),
                            verification_policy,
                        ),
                    )),
            )),
            ..Default::default()
        };

        // Register a cancellation flag for this session so the UI can stop it.
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(session_id.clone(), cancel.clone());
        }

        let engine = RuntimeEngine::new(&registry, Default::default(), config)
            .with_events(sink.clone())
            .with_approvals(gate)
            .with_hooks(&hooks)
            .with_cancel(cancel);

        // Snapshot the current discovered set before the engine starts so we
        // can compute the delta after — only newly-discovered names get
        // appended to the event log (Phase 3C). The set may have been
        // pre-populated above from prior `ToolsDiscovered` events.
        let discovered_before_run: std::collections::HashSet<String> = tool_search_discovered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();

        // Run the loop. Errors are surfaced as a terminal RunFailed event so the
        // UI always gets a clean end, then returned to the caller.
        let run_result = engine.run(&mut session, task, &mut agent).await;

        // Persist the discovered-tools delta (Phase 3C). Anything in the
        // current set that wasn't there before the run is new — append it as
        // a `ToolsDiscovered` event so a future resume reconstructs the same
        // active toolset without forcing the model to re-issue `tool_search`.
        let new_discovered: Vec<String> = {
            let now = tool_search_discovered
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut out: Vec<String> = now
                .iter()
                .filter(|n| !discovered_before_run.contains(n.as_str()))
                .cloned()
                .collect();
            out.sort();
            out
        };
        if !new_discovered.is_empty() {
            if let Err(e) = session.append(EventPayload::ToolsDiscovered {
                names: new_discovered,
            }) {
                tracing::warn!(error = %e, "failed to persist ToolsDiscovered event");
            }
        }

        // Cost recording: persist this run's token cost (Phase 1B). Done before
        // dropping the agent so `cumulative_usage()` is still reachable. No-op
        // when no cost tracker is attached. Failures are logged, never fatal.
        if let Some(cost) = &self.cost {
            if let Some(u) = agent.cumulative_usage() {
                if u.total_tokens > 0 {
                    match cost.record(CostRecordRequest {
                        session_id: session_id.clone(),
                        model: model_name_for_cost.clone(),
                        input_tokens: u.prompt_tokens,
                        output_tokens: u.completion_tokens,
                        cache_hit_tokens: u.prompt_cache_hit_tokens,
                        cache_miss_tokens: u.prompt_cache_miss_tokens,
                        total_tokens: u.total_tokens,
                    }) {
                        Ok(cny) => {
                            tracing::info!(cost_yuan = cny, "recorded run cost");
                            sink.emit(RuntimeEvent::Usage {
                                prompt_tokens: 0,
                                completion_tokens: 0,
                                total_tokens: 0,
                                prompt_cache_hit_tokens: 0,
                                prompt_cache_miss_tokens: 0,
                                cost_yuan: Some(cny),
                            });
                        }
                        Err(e) => tracing::warn!(error = %e, "failed to record run cost"),
                    }
                }
            }
        }

        // Drop the cancellation flag for this session (run is over).
        {
            let mut map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
            map.remove(&session_id);
        }

        // Drop everything holding a clone of the event-sink sender so the
        // channel closes and the pump task can finish; then await it to ensure
        // all events were delivered.
        drop(engine);
        drop(agent);
        drop(sink);
        let _ = pump.await;

        // Session auto-capture: if the run succeeded and a knowledge base with
        // auto-capture is attached, persist a reusable recovery lesson in the
        // background. Spawned detached so it never delays the user's answer; all
        // failures are silent.
        if run_result.is_ok() {
            if let Some(knowledge) = &self.knowledge {
                if knowledge.auto_capture_enabled() {
                    let knowledge = knowledge.clone();
                    let db = self.db.clone();
                    let sid = session_id.clone();
                    let client = capture_client;
                    let model = capture_model;
                    tokio::spawn(async move {
                        let events = match deepagent_core::id::SessionId::from_str(&sid) {
                            Ok(id) => {
                                let store =
                                    deepagent_persistence::event_store::EventStore::new(&db);
                                match store.load_session(id) {
                                    Ok(evs) => evs,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "auto-capture: load session failed");
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "auto-capture: bad session id");
                                return;
                            }
                        };
                        if let Some(dto) = knowledge
                            .capture_from_session(client, model, &events, &sid)
                            .await
                        {
                            tracing::info!(id = %dto.id, "auto-captured knowledge");
                        }
                    });
                }
            }
        }

        run_result.map(|_| session_id)
    }
}

/// Runs a sub-agent for the `task` tool: a nested agent loop over a sub-registry
/// (the built-ins minus `task`, so no recursion), on an ephemeral in-memory
/// session, returning only the sub-agent's final message.
struct ChatSubagentRunner {
    client: Arc<ModelClient>,
    model: String,
    thinking_depth: ThinkingDepth,
    registry: Arc<ToolRegistry>,
    root: PathBuf,
    /// Tool-search mode at the time the parent session built this runner.
    /// Captured per-runner (not per-call) so swapping the user setting
    /// mid-run doesn't change behavior of an in-flight sub-agent.
    tool_search_mode: deepagent_builtins::ToolSearchMode,
    /// Auto-mode threshold inherited from the parent.
    tool_search_auto_threshold: usize,
    /// Snapshot of parent's discovered tool names at runner construction.
    /// Each `run()` call seeds a fresh `Arc<Mutex<HashSet>>` from this
    /// snapshot — sub-agent discoveries DON'T propagate back to the parent.
    parent_discovered_snapshot: std::collections::HashSet<String>,
}

#[async_trait::async_trait]
impl deepagent_builtins::SubagentRunner for ChatSubagentRunner {
    async fn run(&self, request: deepagent_builtins::SubagentRequest) -> Result<String> {
        use deepagent_runtime::{ModelAgent, RunOutcome, RuntimeConfig, RuntimeEngine};

        // Sub-agent gets a CLONE of the parent's discovered set so it starts
        // with the same active toolset, but writes here don't affect the
        // parent (independent context — Req 4.6).
        let sub_discovered: DiscoveredToolSet = Arc::new(std::sync::Mutex::new(
            self.parent_discovered_snapshot.clone(),
        ));

        // Clone the (shared) parent sub-registry so we can register the
        // sub-agent's own `tool_search` tool without mutating the Arc the
        // chat_service hands out to every sub-agent invocation. ToolRegistry
        // is a `BTreeMap<String, ToolSpec>` clone — cheap by Rust standards
        // and amortized over the entire sub-agent run.
        let mut sub_registry: ToolRegistry = (*self.registry).clone();
        let _ = register_tool_search_into(
            &mut sub_registry,
            self.tool_search_mode,
            sub_discovered.clone(),
            self.tool_search_auto_threshold,
        );

        // A sub-agent gets the same tool schemas (minus task) advertised to
        // it, with deferred-but-not-yet-discovered tools filtered out exactly
        // like the main run.
        let granted = PermissionSet::developer();
        let tools: Vec<ToolSchema> = build_visible_tool_schemas(
            &sub_registry,
            &granted,
            self.tool_search_mode,
            &sub_discovered,
        );

        let system = format!(
            "{base}{boundary}# Sub-agent task\nYou are a focused sub-agent. Do exactly the \
             delegated task and return a complete, self-contained final answer — the calling \
             agent sees only your final message, not your intermediate steps.\n- Working \
             directory: {cwd}",
            base = crate::system_prompt::system_prompt_base(),
            boundary = SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
            cwd = self.root.display(),
        );

        // Ephemeral in-memory session: sub-agent runs are not persisted to the
        // main project DB (only the final result re-enters the main transcript
        // as the tool result).
        let db = Database::open_in_memory()?;
        let clock = SystemClock;
        let mut session = Session::create(&db, &clock, Some(&request.description))?;
        let task = session.create_task(&request.prompt)?;

        let mut agent = ModelAgent::new(
            self.client.clone(),
            self.model.clone(),
            system,
            &request.prompt,
            tools,
        )
        .with_thinking_depth(self.thinking_depth);
        let config = RuntimeConfig {
            permissions: granted,
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&sub_registry, Default::default(), config);
        match engine.run(&mut session, task, &mut agent).await? {
            RunOutcome::Completed(msg) => Ok(msg),
            RunOutcome::AwaitingApproval(msg) => {
                Ok(format!("sub-agent paused awaiting approval: {msg}"))
            }
            RunOutcome::StepLimitReached => Ok(
                "sub-agent stopped after reaching its step limit without a final answer."
                    .to_string(),
            ),
            RunOutcome::Cancelled => Ok("sub-agent was cancelled.".to_string()),
        }
    }
}

/// Rebuild a plain conversation (user/assistant text turns) from a session's
/// event log, for seeding the model when continuing an existing session.
///
/// Only [`EventPayload::MessageAppended`] user/assistant turns are taken, and
/// any `tool_calls` are stripped: tool *requests* live as separate
/// `ToolCallRequested`/`ToolCallCompleted` events (not assistant messages), so
/// replaying them as bare `tool_calls` would dangle without their matching
/// `tool` results and the API would reject the request. Plain text turns are
/// enough context for a follow-up question.
fn conversation_from_events(events: &[deepagent_core::event::Event]) -> Vec<Message> {
    use deepagent_core::message::Role;
    let mut out = Vec::new();
    for ev in events {
        if let EventPayload::MessageAppended { message } = &ev.payload {
            if message
                .content
                .starts_with("[Earlier conversation compacted to summary]")
            {
                out.clear();
                out.push(Message::text(message.role, message.content.clone()));
                continue;
            }
            match message.role {
                Role::User | Role::Assistant if !message.content.trim().is_empty() => {
                    out.push(Message::text(message.role, message.content.clone()));
                }
                _ => {}
            }
        }
    }
    out
}

fn parse_slash_invocation(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('/')?;
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    };
    if name.is_empty() {
        None
    } else {
        Some((name, args))
    }
}

fn normalize_generated_session_title(raw: &str) -> Option<String> {
    let mut title = raw.trim().replace(['\r', '\n'], " ");
    for prefix in ["Title:", "title:", "标题：", "标题:"] {
        if let Some(stripped) = title.strip_prefix(prefix) {
            title = stripped.trim().to_string();
            break;
        }
    }
    title = title
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return None;
    }
    let max_chars = 48usize;
    let normalized = if title.chars().count() > max_chars {
        let mut truncated = title.chars().take(max_chars).collect::<String>();
        truncated = truncated
            .trim()
            .trim_end_matches([':', '-', ' ', '，', '。'])
            .to_string();
        truncated
    } else {
        title
    };
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn parse_thinking_depth(depth: &str) -> Result<ThinkingDepth> {
    match depth.trim().to_ascii_lowercase().as_str() {
        "simple" => Ok(ThinkingDepth::Simple),
        "medium" => Ok(ThinkingDepth::Medium),
        "deep" => Ok(ThinkingDepth::Deep),
        _ => Err(CoreError::invalid("usage: /thinking <simple|medium|deep>")),
    }
}

fn format_rule_count(rules: &[String]) -> String {
    if rules.is_empty() {
        "0".to_string()
    } else {
        format!("{} ({})", rules.len(), rules.join(", "))
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// Decide whether to actually activate tool-search for this registry.
/// `Disabled` is rejected upstream; `Enabled` is always active; `Auto` is
/// active only when the deferred-tool schema size meets `threshold_chars`.
fn should_activate_tool_search(
    registry: &ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    threshold_chars: usize,
) -> bool {
    match mode {
        deepagent_builtins::ToolSearchMode::Disabled => false,
        deepagent_builtins::ToolSearchMode::Enabled => true,
        deepagent_builtins::ToolSearchMode::Auto => {
            let total: usize = registry
                .iter_specs()
                .filter(|spec| deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode))
                .map(|spec| {
                    spec.descriptor.name.len()
                        + spec.descriptor.description.len()
                        + spec.descriptor.parameters.to_string().len()
                })
                .sum();
            total >= threshold_chars
        }
    }
}

/// Register the `tool_search` built-in into `registry` if `mode` activates
/// (subject to the threshold for `Auto`). Returns the names of every
/// deferred tool the snapshot captured (or empty when the mode short-
/// circuits / no tools are eligible).
///
/// Free function so both the main session (via `ChatService`) and the
/// sub-agent runner (`ChatSubagentRunner`) can call it without sharing
/// service-level state.
fn register_tool_search_into(
    registry: &mut ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: DiscoveredToolSet,
    auto_threshold_chars: usize,
) -> Result<Vec<String>> {
    if !mode.is_active() || !should_activate_tool_search(registry, mode, auto_threshold_chars) {
        return Ok(Vec::new());
    }
    let deferred: Vec<deepagent_builtins::DeferredToolSnapshot> = registry
        .iter_specs()
        .filter(|spec| deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode))
        .map(|spec| deepagent_builtins::DeferredToolSnapshot {
            name: spec.descriptor.name.clone(),
            description: spec.descriptor.description.clone(),
        })
        .collect();
    if deferred.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = deferred.iter().map(|s| s.name.clone()).collect();
    registry.register(Arc::new(deepagent_builtins::ToolSearchTool::new(
        deferred, discovered,
    )))?;
    Ok(names)
}

/// Extract the cumulative set of discovered-tool names from a session's
/// event stream. Walks every `ToolsDiscovered` payload (each carrying a
/// delta) and returns the union, preserving first-seen order. Used at the
/// start of `run_in_session` to seed `tool_search_discovered` on resume.
fn collect_discovered_tools_from_events(events: &[deepagent_core::event::Event]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in events {
        if let EventPayload::ToolsDiscovered { names } = &e.payload {
            for name in names {
                if seen.insert(name.clone()) {
                    out.push(name.clone());
                }
            }
        }
    }
    out
}

fn collect_invoked_skill_ids_from_events(
    events: &[deepagent_core::event::Event],
) -> std::collections::HashSet<String> {
    let mut pending: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut invoked = std::collections::HashSet::new();
    for event in events {
        match &event.payload {
            EventPayload::ToolCallRequested { call }
                if call.name == deepagent_builtins::SKILL_TOOL_NAME =>
            {
                if let Some(id) = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    pending.insert(call.id.clone(), id.to_string());
                }
            }
            EventPayload::ToolCallCompleted { call_id, ok, .. } if *ok => {
                if let Some(id) = pending.remove(call_id) {
                    invoked.insert(id);
                }
            }
            _ => {}
        }
    }
    invoked
}

fn collect_invoked_skill_records_from_events(
    events: &[deepagent_core::event::Event],
) -> Vec<InvokedSkillRecord> {
    let mut pending: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut index_by_id: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut records = Vec::new();

    for event in events {
        match &event.payload {
            EventPayload::ToolCallRequested { call }
                if call.name == deepagent_builtins::SKILL_TOOL_NAME =>
            {
                if let Some(id) = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    pending.insert(call.id.clone(), id.to_string());
                }
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } if *ok => {
                let Some(requested_id) = pending.remove(call_id) else {
                    continue;
                };
                let Some(record) = invoked_skill_record_from_output(&requested_id, output) else {
                    continue;
                };
                if let Some(index) = index_by_id.get(&record.id).copied() {
                    records[index] = record;
                } else {
                    index_by_id.insert(record.id.clone(), records.len());
                    records.push(record);
                }
            }
            _ => {}
        }
    }

    records
}

fn invoked_skill_record_from_output(
    requested_id: &str,
    output: &serde_json::Value,
) -> Option<InvokedSkillRecord> {
    let body = output
        .get("body")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let id = output
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(requested_id)
        .to_string();
    let name = output
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_string();
    let base_dir = output
        .get("base_dir")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let resources = output
        .get("resources")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Some(InvokedSkillRecord {
        id,
        name,
        body,
        base_dir,
        resources,
    })
}

fn invoked_skills_reminder(records: &[InvokedSkillRecord]) -> Option<String> {
    if records.is_empty() {
        return None;
    }

    let mut out = String::from(
        "The following skills have already been invoked in this session. Continue following these instructions. Do not re-invoke a listed skill unless you need fresh arguments or updated resources.\n\n<invoked-skills>\n",
    );
    for record in records {
        out.push_str("\n### Skill: ");
        out.push_str(&record.name);
        if record.id != record.name {
            out.push_str(" (`");
            out.push_str(&record.id);
            out.push_str("`)");
        }
        out.push('\n');
        if let Some(base_dir) = &record.base_dir {
            out.push_str("Base directory: ");
            out.push_str(base_dir);
            out.push('\n');
        }
        if !record.resources.is_empty() {
            out.push_str("Resources:\n");
            for resource in &record.resources {
                out.push_str("- ");
                out.push_str(resource);
                out.push('\n');
            }
        }
        out.push('\n');
        out.push_str(&record.body);
        out.push('\n');
    }
    out.push_str("\n</invoked-skills>");
    Some(out)
}

/// Render the dynamic-section "available deferred tools" block.
///
/// Returned only when `undiscovered` is non-empty; otherwise `None` so the
/// caller can skip the append and avoid burning cache on an empty section.
/// The block lives in the dynamic section (after `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`)
/// so the static prefix stays byte-stable across turns and across modes.
fn deferred_tools_announcement(undiscovered: &[String]) -> Option<String> {
    if undiscovered.is_empty() {
        return None;
    }
    let mut out =
        String::with_capacity(256 + undiscovered.iter().map(|n| n.len() + 4).sum::<usize>());
    out.push_str(
        "## Lazy-loaded tools

The tools below are NOT yet loaded in this session — only their names are visible. To call one, first invoke `tool_search` to fetch its full schema:
- `select:Name1,Name2` — load these specific names.
- `slack send` — keyword search; returns the best matches by name + description.
- `+slack send` — `+`-prefixed terms are required (must appear in name or description).

Once the matching schema lands, the tool becomes callable on the next turn.

<available-deferred-tools>
",
    );
    for name in undiscovered {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("</available-deferred-tools>");
    Some(out)
}

/// Build the per-turn `tools` array sent to the model.
///
/// When `mode == Disabled`, this is byte-identical to the pre-feature
/// implementation: every tool returned by `registry.visible_to(granted)` is
/// converted to a `ToolSchema`.
///
/// When `mode.is_active()`, deferred tools whose name is NOT in `discovered`
/// are filtered out — the model only sees their names via the
/// `<available-deferred-tools>` block (Phase 3B / Task 5) and pulls schemas
/// in on demand through `tool_search`.
fn build_visible_tool_schemas(
    registry: &ToolRegistry,
    granted: &PermissionSet,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: &DiscoveredToolSet,
) -> Vec<ToolSchema> {
    let active = mode.is_active();
    let descriptors = registry.visible_to(granted);
    if !active {
        return descriptors
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();
    }
    let discovered_snapshot: std::collections::HashSet<String> = discovered
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect();
    descriptors
        .into_iter()
        .filter(|d| {
            // Look up the live tool spec to apply `is_deferred_tool` (which
            // touches `should_defer` / `always_load`). If the tool isn't in
            // the registry (race with concurrent edits) keep it visible
            // — that matches the pre-feature default.
            let Some(spec) = registry.get(&d.name) else {
                return true;
            };
            if !deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode) {
                return true;
            }
            discovered_snapshot.contains(&d.name)
        })
        .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
        .collect()
}

/// Collect up to `cap` verifier-eligible files from `root`, walking one
/// directory level deep. Used by the `/verify` slash command.
fn collect_verifiable_files(root: &std::path::Path, out: &mut Vec<PathBuf>, cap: usize) {
    fn is_eligible(path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rs" | "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "py" | "json")
        )
    }
    fn skip_dir(name: &str) -> bool {
        matches!(
            name,
            ".git" | "target" | "node_modules" | "dist" | ".venv" | "__pycache__" | "build" | "out"
        )
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || skip_dir(&name) {
                continue;
            }
            // One directory level deep — keep the scan bounded.
            if let Ok(children) = std::fs::read_dir(&path) {
                for c in children.flatten() {
                    if out.len() >= cap {
                        return;
                    }
                    let cp = c.path();
                    if cp.is_file() && is_eligible(&cp) {
                        out.push(cp);
                    }
                }
            }
        } else if is_eligible(&path) {
            out.push(path);
        }
    }
}

/// A conservative default bash allow-list (read-ish / build commands).
fn default_bash_allow() -> Vec<String> {
    [
        "git status",
        "git diff",
        "git log",
        "git show",
        "ls",
        "cat",
        "echo",
        "pwd",
        "cargo build",
        "cargo test",
        "cargo check",
        "cargo fmt",
        "cargo clippy",
        "npm run",
        "pnpm",
        "node",
        "python",
        "rustc",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::MemorySecretStore;
    use deepagent_models::transport::{EventSink, MockTransport, TransportRequest};

    /// A transport that answers model discovery (GET) AND a streamed chat (the
    /// agent's first turn) so a full run completes offline.
    fn chat_transport() -> Arc<dyn HttpTransport> {
        // The mock streams its `events` for `stream`, and returns `get_response`
        // for discovery. We only need streaming here (settings are seeded
        // separately), so build one that completes immediately.
        Arc::new(MockTransport::new([
            r#"{"choices":[{"delta":{"content":"Hello from the agent."},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ]))
    }

    #[derive(Debug, Default)]
    struct RecordingTransport {
        last_body: Arc<std::sync::Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl HttpTransport for RecordingTransport {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            *self.last_body.lock().unwrap() = Some(request.body);
            sink.on_event(
                r#"{"choices":[{"delta":{"content":"dynamic reply"},"finish_reason":"stop"}]}"#,
            )?;
            sink.on_event("[DONE]")?;
            Ok(())
        }
    }

    fn discovery_transport() -> Arc<dyn HttpTransport> {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
        ]}"#;
        Arc::new(MockTransport::with_get_json(body))
    }

    async fn seeded() -> (Arc<Database>, Arc<SettingsService>, tempfile::TempDir) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery_transport(),
            secrets,
        ));
        settings.initialize("sk-test-1234").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        (db, settings, dir)
    }

    #[tokio::test]
    async fn streams_a_chat_run_end_to_end() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());

        let collected = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = collected.clone();
        let session_id = chat
            .run(
                "say hello",
                move |ev| {
                    sink.lock().unwrap().push(ev.label().to_string());
                },
                |_approval| {},
            )
            .await
            .unwrap();

        assert!(session_id.starts_with("ses_"));
        let labels = collected.lock().unwrap().clone();
        assert_eq!(labels.first().map(String::as_str), Some("run_started"));
        assert!(labels.iter().any(|l| l == "content_delta"));
        assert_eq!(labels.last().map(String::as_str), Some("run_completed"));
    }

    #[tokio::test]
    async fn preflight_tools_are_persisted_as_session_tool_events() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db.clone(), settings, chat_transport(), dir.path());
        let events = Arc::new(std::sync::Mutex::new(Vec::<RuntimeEvent>::new()));
        let sink = events.clone();

        let session_id = chat
            .run_in_session(
                "analyze this screenshot",
                None,
                None,
                None,
                vec![PreflightToolCallDto {
                    call_id: "system_vision:test".to_string(),
                    name: "system_vision".to_string(),
                    arguments: serde_json::json!({"images":[{"name":"shot.png"}]}),
                    ok: true,
                    output: serde_json::json!({"recognized_images":1}),
                    duration_ms: 42,
                }],
                None,
                move |ev| {
                    sink.lock().unwrap().push(ev);
                },
                |_| {},
            )
            .await
            .unwrap();

        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let id = deepagent_core::id::SessionId::from_str(&session_id).unwrap();
        let persisted = store.load_session(id).unwrap();
        assert!(persisted.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolCallRequested { call }
                if call.id == "system_vision:test" && call.name == "system_vision"
        )));
        assert!(persisted.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolCallCompleted { call_id, ok, duration_ms, .. }
                if call_id == "system_vision:test" && *ok && *duration_ms == 42
        )));

        let live = events.lock().unwrap();
        assert!(live.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolStarted { call_id, name, .. }
                if call_id == "system_vision:test" && name == "system_vision"
        )));
        assert!(live.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolCompleted { call_id, name, ok, .. }
                if call_id == "system_vision:test" && name == "system_vision" && *ok
        )));
    }

    #[tokio::test]
    async fn preflight_abort_persists_failure_without_calling_model() {
        let (db, settings, dir) = seeded().await;
        let transport = Arc::new(RecordingTransport::default());
        let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());

        let session_id = chat
            .run_in_session(
                "analyze this broken screenshot",
                None,
                None,
                None,
                vec![PreflightToolCallDto {
                    call_id: "system_vision:error".to_string(),
                    name: "system_vision".to_string(),
                    arguments: serde_json::json!({"images":[{"name":"shot.png"}]}),
                    ok: false,
                    output: serde_json::json!({"error":"input size exceed limit"}),
                    duration_ms: 9,
                }],
                Some("图片识别失败，已停止本轮请求。".to_string()),
                |_| {},
                |_| {},
            )
            .await
            .unwrap();

        assert!(transport.last_body.lock().unwrap().is_none());

        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let id = deepagent_core::id::SessionId::from_str(&session_id).unwrap();
        let persisted = store.load_session(id).unwrap();
        assert!(persisted.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolCallCompleted { call_id, ok, .. }
                if call_id == "system_vision:error" && !*ok
        )));
        assert!(persisted.iter().any(|event| matches!(
            &event.payload,
            EventPayload::MessageAppended { message }
                if message.role == deepagent_core::message::Role::Assistant
                    && message.content.contains("图片识别失败")
        )));
    }

    #[tokio::test]
    async fn slash_plan_and_execute_toggle_session_state_without_model() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db.clone(), settings, chat_transport(), dir.path());

        let collected = Arc::new(std::sync::Mutex::new(Vec::<RuntimeEvent>::new()));
        let sink = collected.clone();
        let sid = chat
            .run(
                "/plan",
                move |ev| {
                    sink.lock().unwrap().push(ev);
                },
                |_| {},
            )
            .await
            .unwrap();
        assert!(chat.is_plan_mode(&sid));
        assert!(collected.lock().unwrap().iter().any(|ev| {
            matches!(ev, RuntimeEvent::RunCompleted { message } if message.contains("Entered Plan mode"))
        }));

        chat.run_in_session(
            "/execute",
            Some(&sid),
            None,
            None,
            Vec::new(),
            None,
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        assert!(!chat.is_plan_mode(&sid));

        let id = deepagent_core::id::SessionId::from_str(&sid).unwrap();
        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let history = conversation_from_events(&store.load_session(id).unwrap());
        assert!(history.iter().any(|m| m.content == "/plan"));
        assert!(history
            .iter()
            .any(|m| m.content.contains("Exited Plan mode")));
    }

    #[tokio::test]
    async fn slash_help_and_thinking_are_handled_without_model() {
        let (_db, settings, dir) = seeded().await;
        let chat = ChatService::new(_db, settings.clone(), chat_transport(), dir.path());

        let help_events = Arc::new(std::sync::Mutex::new(Vec::<RuntimeEvent>::new()));
        let help_sink = help_events.clone();
        chat.run(
            "/help",
            move |ev| {
                help_sink.lock().unwrap().push(ev);
            },
            |_| {},
        )
        .await
        .unwrap();
        assert!(help_events.lock().unwrap().iter().any(|ev| {
            matches!(ev, RuntimeEvent::RunCompleted { message } if message.contains("/thinking"))
        }));

        let thinking_events = Arc::new(std::sync::Mutex::new(Vec::<RuntimeEvent>::new()));
        let thinking_sink = thinking_events.clone();
        chat.run(
            "/thinking deep",
            move |ev| {
                thinking_sink.lock().unwrap().push(ev);
            },
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(settings.thinking_depth().unwrap(), ThinkingDepth::Deep);
        assert!(thinking_events.lock().unwrap().iter().any(|ev| {
            matches!(ev, RuntimeEvent::RunCompleted { message } if message.contains("deep"))
        }));
    }

    #[tokio::test]
    async fn slash_model_without_args_lists_available_models() {
        let (_db, settings, dir) = seeded().await;
        let chat = ChatService::new(_db, settings, chat_transport(), dir.path());
        let events = Arc::new(std::sync::Mutex::new(Vec::<RuntimeEvent>::new()));
        let sink = events.clone();

        chat.run(
            "/model",
            move |ev| {
                sink.lock().unwrap().push(ev);
            },
            |_| {},
        )
        .await
        .unwrap();

        assert!(events.lock().unwrap().iter().any(|ev| {
            matches!(
                ev,
                RuntimeEvent::RunCompleted { message }
                    if message.contains("deepseek-v4-pro") && message.contains("/model <model_id>")
            )
        }));
    }

    #[tokio::test]
    async fn dynamic_slash_command_file_renders_into_model_prompt() {
        let (db, settings, dir) = seeded().await;
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("triage.md"),
            "---\ndescription: Triage a bug report\n---\nReview this bug:\n$ARGUMENTS",
        )
        .unwrap();

        let last_body = Arc::new(std::sync::Mutex::new(None));
        let transport = Arc::new(RecordingTransport {
            last_body: last_body.clone(),
        });
        let chat = ChatService::new(db, settings, transport, dir.path());

        chat.run("/triage issue-42", |_| {}, |_| {}).await.unwrap();

        let body = last_body.lock().unwrap().clone().unwrap();
        assert!(body.contains("Review this bug:"));
        assert!(body.contains("issue-42"));
        assert!(!body.contains("$ARGUMENTS"));
    }

    #[tokio::test]
    async fn deep_thinking_keeps_chat_model_and_uses_max_effort() {
        let (db, settings, dir) = seeded().await;
        settings
            .set_thinking_depth(ThinkingDepth::Deep)
            .expect("thinking depth can be updated");

        let last_body = Arc::new(std::sync::Mutex::new(None));
        let transport = Arc::new(RecordingTransport {
            last_body: last_body.clone(),
        });
        let chat = ChatService::new(db, settings, transport, dir.path());

        chat.run("solve a complex task", |_| {}, |_| {})
            .await
            .unwrap();

        let body = last_body.lock().unwrap().clone().unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["model"], "deepseek-v4-flash");
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["reasoning_effort"], "max");
        assert!(json.get("max_tokens").is_none());
    }

    #[tokio::test]
    async fn run_without_init_errors() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery_transport(),
            secrets,
        ));
        let dir = tempfile::tempdir().unwrap();
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        // No initialize() call → no settings → error.
        let res = chat.run("hi", |_| {}, |_| {}).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn continuing_a_session_appends_instead_of_creating() {
        let (db, settings, dir) = seeded().await;
        // A transport that can serve two streamed turns back to back.
        let transport = Arc::new(MockTransport::new([
            r#"{"choices":[{"delta":{"content":"first reply"},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
            r#"{"choices":[{"delta":{"content":"second reply"},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ]));
        let chat = ChatService::new(db.clone(), settings, transport, dir.path());

        // First turn → new session.
        let first = chat.run("hello", |_| {}, |_| {}).await.unwrap();
        // Second turn → continue the same session.
        let second = chat
            .run_in_session(
                "follow up",
                Some(&first),
                None,
                None,
                Vec::new(),
                None,
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(first, second, "continuation reuses the same session id");

        // The session must now contain both user turns in its event log.
        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let id = deepagent_core::id::SessionId::from_str(&first).unwrap();
        let events = store.load_session(id).unwrap();
        let user_turns: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::MessageAppended { message }
                    if message.role == deepagent_core::message::Role::User =>
                {
                    Some(message.content.clone())
                }
                _ => None,
            })
            .collect();
        assert!(user_turns.iter().any(|c| c == "hello"));
        assert!(user_turns.iter().any(|c| c == "follow up"));

        // And there must be exactly ONE session in the store.
        assert_eq!(store.list_sessions().unwrap().len(), 1);
    }

    #[test]
    fn conversation_from_events_keeps_text_turns_only() {
        use deepagent_core::event::{Event, EventPayload};
        use deepagent_core::id::{EventId, SessionId, TaskId};
        use deepagent_core::message::{Message, Role};

        let sid = SessionId::new();
        let ev = |seq: u64, payload: EventPayload| Event {
            id: EventId::new(),
            session_id: sid,
            sequence: seq,
            timestamp: deepagent_core::clock::Timestamp::from_millis(seq as i64),
            payload,
        };
        let events = vec![
            ev(
                0,
                EventPayload::SessionStarted {
                    title: Some("t".into()),
                    mode: Default::default(),
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            ),
            ev(
                2,
                EventPayload::MessageAppended {
                    message: Message::assistant("hello"),
                },
            ),
            // Empty assistant turn (pure tool-call placeholder) is dropped.
            ev(
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant(""),
                },
            ),
            ev(
                4,
                EventPayload::TaskCreated {
                    task_id: TaskId::new(),
                    goal: "g".into(),
                },
            ),
        ];
        let convo = conversation_from_events(&events);
        assert_eq!(convo.len(), 2);
        assert_eq!(convo[0].role, Role::User);
        assert_eq!(convo[0].content, "hi");
        assert_eq!(convo[1].role, Role::Assistant);
        assert_eq!(convo[1].content, "hello");
    }

    #[test]
    fn system_prompt_carries_current_date_and_cwd() {
        let root = std::path::Path::new("/tmp/myproject");
        let prompt = build_system_prompt(root);
        // The environment block must carry today's actual year so the model
        // never searches a stale one (the web_search bug we hit).
        let year = time::OffsetDateTime::now_utc().year();
        assert!(
            prompt.contains(&year.to_string()),
            "prompt missing current year"
        );
        assert!(prompt.contains("Today's date:"));
        assert!(prompt.contains("myproject"));
        // Core agentic guidance is present.
        assert!(prompt.contains("web_search"));
        assert!(prompt.contains("status\":\"error\""));
        assert!(prompt.contains("Match the user's language"));
        assert!(prompt.contains("same natural language as the user's latest message"));
        // Frontend renderer contract: model output must stay parseable by the
        // Markdown/LaTeX/ECharts renderer without backend rewriting.
        assert!(prompt.contains("language `echarts`"));
        assert!(prompt.contains("pure, valid JSON object"));
        assert!(prompt.contains("$...$"));
        assert!(prompt.contains("$$...$$"));
        assert!(prompt.contains("\\ce{...}"));
        assert!(prompt.contains("do not escape backticks"));
        // The dynamic boundary separates the cacheable prefix from the volatile
        // env block; the date must come AFTER it and the base before it.
        let boundary = prompt
            .find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
            .expect("boundary present");
        assert!(prompt.find("Today's date:").unwrap() > boundary);
        assert!(prompt.find("# Doing tasks").unwrap() < boundary);
    }

    #[test]
    fn current_date_string_is_iso_like() {
        let d = current_date_string();
        // YYYY-MM-DD → at least 3 dash-separated numeric parts.
        let parts: Vec<&str> = d.split('-').collect();
        assert!(parts.len() >= 3, "unexpected date format: {d}");
        assert!(parts[0].chars().all(|c| c.is_ascii_digit()));
    }

    // ---- Knowledge base wiring ------------------------------------------

    use crate::knowledge_service::{KnowledgeDraftDto, KnowledgeService};

    fn knowledge_with(tmp: &std::path::Path, title: &str, body: &str) -> Arc<KnowledgeService> {
        let svc = KnowledgeService::open(&tmp.join("proj"), &tmp.join("glob")).unwrap();
        svc.save(KnowledgeDraftDto {
            title: title.to_string(),
            body: body.to_string(),
            kind: Some("pitfall".into()),
            tags: vec![],
            scope: Some("project".into()),
            source_session: None,
        })
        .unwrap();
        Arc::new(svc)
    }

    #[tokio::test]
    async fn with_knowledge_registers_search_and_write_tools() {
        let (_db, _settings, dir) = seeded().await;
        let kb = knowledge_with(
            dir.path(),
            "PowerShell pipe interrupt",
            "Piping cargo output to Select-String exits -1; redirect to a file.",
        );
        let chat = ChatService::new(_db.clone(), _settings, chat_transport(), dir.path())
            .with_knowledge(kb);

        // The main registry must advertise both knowledge tools.
        let (registry, _todo_store) = chat
            .build_registry(dir.path(), deepagent_builtins::FsAccess::Full, None, None, None)
            .unwrap();
        assert!(
            registry
                .get(deepagent_builtins::KNOWLEDGE_SEARCH_TOOL_NAME)
                .is_some(),
            "knowledge_search must be registered when a KB is attached"
        );
        // knowledge_write is added in run_in_session, not build_registry; the
        // search tool is the shared-registry one.
    }

    #[tokio::test]
    async fn without_knowledge_registers_no_knowledge_tools() {
        let (_db, _settings, dir) = seeded().await;
        let chat = ChatService::new(_db, _settings, chat_transport(), dir.path());
        let (registry, _todo_store) = chat
            .build_registry(dir.path(), deepagent_builtins::FsAccess::Full, None, None, None)
            .unwrap();
        assert!(
            registry
                .get(deepagent_builtins::KNOWLEDGE_SEARCH_TOOL_NAME)
                .is_none(),
            "no knowledge tools without a KB (backward compatibility)"
        );
        assert!(registry
            .get(deepagent_builtins::KNOWLEDGE_WRITE_TOOL_NAME)
            .is_none());
    }

    #[cfg(feature = "web")]
    #[tokio::test]
    async fn disabled_web_search_settings_omit_web_search_tool() {
        let (_db, settings, dir) = seeded().await;
        settings
            .set_web_search_settings(crate::settings::WebSearchSettings {
                enabled: false,
                ..Default::default()
            })
            .unwrap();
        let chat = ChatService::new(_db, settings, chat_transport(), dir.path());
        let (registry, _todo_store) = chat
            .build_registry(dir.path(), deepagent_builtins::FsAccess::Full, None, None, None)
            .unwrap();
        assert!(
            registry.get("web_fetch").is_some(),
            "web_fetch remains available for known URLs"
        );
        assert!(
            registry.get("web_search").is_none(),
            "web_search should honor the persisted disabled setting"
        );
    }

    #[test]
    fn passive_block_renders_as_system_reminder_in_user_prompt() {
        // Phase 3C: passive knowledge injection no longer touches the system
        // prompt. Instead it's wrapped in `<system-reminder>` and prepended to
        // the user-facing prompt, so the cacheable static prefix stays
        // byte-stable and the model treats the block as a meta-channel hint
        // rather than authentic user wording.
        let tmp = tempfile::tempdir().unwrap();
        let kb = knowledge_with(
            tmp.path(),
            "Keyring service name",
            "The DeepSeek API key is stored under service deepagent-studio.",
        );
        let prompt = "where is the api key stored keyring service";
        let block = kb.passive_block(prompt);
        assert!(!block.is_empty(), "expected a relevant passive hit");
        let reminder = crate::system_reminder::wrap(&block);
        let composed = format!("{reminder}\n\n{prompt}");

        // Reminder is wrapped, mentions the retrieved-block header, and the
        // user prompt itself comes after the closing tag.
        assert!(composed.starts_with("<system-reminder>"));
        assert!(composed.contains("# 相关知识 (knowledge base, retrieved)"));
        let close = composed
            .find("</system-reminder>")
            .expect("reminder closes properly");
        let prompt_pos = composed
            .find(prompt)
            .expect("user prompt present after reminder");
        // The user prompt body must appear AFTER the reminder closes — this is
        // the contract the model relies on to distinguish "this came from
        // the runtime" from "this came from the user".
        assert!(prompt_pos > close);
    }

    #[test]
    fn passive_block_no_longer_lands_in_system_prompt() {
        // Regression guard for Phase 3C: build_system_prompt must NOT carry
        // the retrieved-knowledge header. (The static base prompt does still
        // mention "相关知识" inside its tool guidance — that's the bullet
        // describing knowledge_search to the model, not an injected hit.)
        let tmp = tempfile::tempdir().unwrap();
        let system_prompt = build_system_prompt(tmp.path());
        assert!(!system_prompt.contains("# 相关知识 (knowledge base, retrieved)"));
    }

    #[test]
    fn no_passive_block_when_irrelevant() {
        let tmp = tempfile::tempdir().unwrap();
        let kb = knowledge_with(
            tmp.path(),
            "Keyring service name",
            "The DeepSeek API key is stored under service deepagent-studio.",
        );
        // A totally unrelated query should not clear the score threshold.
        assert!(kb
            .passive_block("how do I bake a chocolate cake")
            .is_empty());
    }

    // ----- Tool-search per-turn tools-array filter (Phase 3A) -----

    /// A stub tool with configurable name + `should_defer` so we can build a
    /// registry that mixes deferred and non-deferred tools without dragging
    /// in the full builtins set.
    #[derive(Debug)]
    struct FilterTestTool {
        name: String,
        should_defer: bool,
    }

    #[async_trait::async_trait]
    impl deepagent_tools::Tool for FilterTestTool {
        fn descriptor(&self) -> deepagent_tools::ToolDescriptor {
            deepagent_tools::ToolDescriptor {
                name: self.name.clone(),
                description: format!("test tool {}", self.name),
                parameters: serde_json::json!({"type": "object"}),
                risk: deepagent_tools::permission::RiskLevel::Safe,
                required_permissions: deepagent_tools::PermissionSet::read_only(),
            }
        }
        async fn invoke(
            &self,
            _: serde_json::Value,
        ) -> deepagent_core::error::Result<deepagent_tools::ToolOutput> {
            Ok(deepagent_tools::ToolOutput::success(serde_json::json!(
                null
            )))
        }
        fn should_defer(&self) -> bool {
            self.should_defer
        }
    }

    fn registry_with(tools: Vec<(String, bool)>) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for (name, defer) in tools {
            reg.register(Arc::new(FilterTestTool {
                name,
                should_defer: defer,
            }))
            .unwrap();
        }
        reg
    }

    #[test]
    fn disabled_mode_passes_every_visible_tool_through() {
        // With ToolSearchMode::Disabled, build_visible_tool_schemas must
        // behave byte-for-byte like the pre-feature implementation: every
        // visible tool gets a ToolSchema, no filtering on `should_defer`.
        let reg = registry_with(vec![
            ("read_file".into(), false),
            ("mcp__svc__one".into(), true),
            ("mcp__svc__two".into(), true),
        ]);
        let granted = PermissionSet::developer();
        let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let tools = build_visible_tool_schemas(
            &reg,
            &granted,
            deepagent_builtins::ToolSearchMode::Disabled,
            &discovered,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        // All three present; names sorted deterministically by registry's BTreeMap.
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"mcp__svc__one"));
        assert!(names.contains(&"mcp__svc__two"));
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn enabled_mode_with_empty_discovered_set_hides_deferred_tools() {
        let reg = registry_with(vec![
            ("read_file".into(), false),
            ("mcp__svc__one".into(), true),
            ("mcp__svc__two".into(), true),
        ]);
        let granted = PermissionSet::developer();
        let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let tools = build_visible_tool_schemas(
            &reg,
            &granted,
            deepagent_builtins::ToolSearchMode::Enabled,
            &discovered,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"mcp__svc__one"));
        assert!(!names.contains(&"mcp__svc__two"));
    }

    #[test]
    fn enabled_mode_surfaces_discovered_deferred_tools() {
        let reg = registry_with(vec![
            ("read_file".into(), false),
            ("mcp__svc__one".into(), true),
            ("mcp__svc__two".into(), true),
        ]);
        let granted = PermissionSet::developer();
        let mut set = std::collections::HashSet::new();
        set.insert("mcp__svc__one".to_string());
        let discovered = Arc::new(std::sync::Mutex::new(set));
        let tools = build_visible_tool_schemas(
            &reg,
            &granted,
            deepagent_builtins::ToolSearchMode::Enabled,
            &discovered,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"mcp__svc__one"));
        // The non-discovered deferred tool stays hidden.
        assert!(!names.contains(&"mcp__svc__two"));
    }

    #[test]
    fn auto_threshold_short_circuits_when_below_8000_chars() {
        // Three tiny MCP tools whose total schema is well below 8 KB → Auto
        // mode should NOT activate (function returns false).
        let reg = registry_with(vec![
            ("mcp__a__op".into(), true),
            ("mcp__b__op".into(), true),
            ("mcp__c__op".into(), true),
        ]);
        assert!(!should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Auto,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        ));
    }

    #[test]
    fn auto_threshold_activates_when_schema_size_exceeds_threshold() {
        // Build one tool with a description big enough to exceed the threshold
        // on its own (the test threshold is 8000; we push 10K of description).
        let reg = registry_with(vec![("mcp__svc__heavy".into(), true)]);
        // Replace the descriptor's description directly via a hand-rolled tool.
        // Easier: register a fresh tool whose descriptor is enormous.
        #[derive(Debug)]
        struct HeavyTool;
        #[async_trait::async_trait]
        impl deepagent_tools::Tool for HeavyTool {
            fn descriptor(&self) -> deepagent_tools::ToolDescriptor {
                deepagent_tools::ToolDescriptor {
                    name: "mcp__svc__bulky".into(),
                    description: "x".repeat(10_000),
                    parameters: serde_json::json!({"type": "object"}),
                    risk: deepagent_tools::permission::RiskLevel::Safe,
                    required_permissions: deepagent_tools::PermissionSet::read_only(),
                }
            }
            async fn invoke(
                &self,
                _: serde_json::Value,
            ) -> deepagent_core::error::Result<deepagent_tools::ToolOutput> {
                Ok(deepagent_tools::ToolOutput::success(serde_json::json!(
                    null
                )))
            }
            fn should_defer(&self) -> bool {
                true
            }
        }
        let mut reg = reg;
        reg.register(Arc::new(HeavyTool)).unwrap();
        assert!(should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Auto,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        ));
    }

    #[test]
    fn enabled_mode_always_activates() {
        let reg = registry_with(vec![("mcp__a__op".into(), true)]);
        assert!(should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Enabled,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        ));
    }

    #[test]
    fn disabled_mode_never_activates() {
        let reg = registry_with(vec![("mcp__a__op".into(), true)]);
        assert!(!should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Disabled,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        ));
    }

    #[test]
    fn auto_threshold_honors_custom_value() {
        // With a tighter threshold, a small registry that wouldn't trip the
        // default 8000 must activate Auto.
        let reg = registry_with(vec![
            ("mcp__a__op".into(), true),
            ("mcp__b__op".into(), true),
            ("mcp__c__op".into(), true),
        ]);
        assert!(!should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Auto,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        ));
        assert!(should_activate_tool_search(
            &reg,
            deepagent_builtins::ToolSearchMode::Auto,
            100,
        ));
    }

    // ----- register_tool_search_into (free function — used by both main + sub-agent paths) -----

    #[test]
    fn register_tool_search_into_no_op_when_disabled() {
        let mut reg = registry_with(vec![
            ("read_file".into(), false),
            ("mcp__svc__one".into(), true),
        ]);
        let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let names = register_tool_search_into(
            &mut reg,
            deepagent_builtins::ToolSearchMode::Disabled,
            discovered,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        )
        .unwrap();
        // No tool_search registered, no names returned.
        assert!(names.is_empty());
        assert!(reg.get(deepagent_builtins::TOOL_SEARCH_TOOL_NAME).is_none());
    }

    #[test]
    fn register_tool_search_into_registers_when_enabled() {
        let mut reg = registry_with(vec![
            ("read_file".into(), false),
            ("mcp__svc__one".into(), true),
            ("mcp__svc__two".into(), true),
        ]);
        let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let names = register_tool_search_into(
            &mut reg,
            deepagent_builtins::ToolSearchMode::Enabled,
            discovered,
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        )
        .unwrap();
        // Two MCP tools deferred; tool_search now registered.
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"mcp__svc__one".to_string()));
        assert!(names.contains(&"mcp__svc__two".to_string()));
        assert!(reg.get(deepagent_builtins::TOOL_SEARCH_TOOL_NAME).is_some());
    }

    #[test]
    fn register_tool_search_into_seeds_discovered_set_for_writes() {
        // Verifies the wired `tool_search` tool actually mutates the
        // discovered set we passed in. Sub-agent inheritance relies on
        // sub-agent's writes going into a *separate* set from the parent's.
        let mut reg = registry_with(vec![("mcp__svc__a".into(), true)]);
        let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        register_tool_search_into(
            &mut reg,
            deepagent_builtins::ToolSearchMode::Enabled,
            discovered.clone(),
            SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS,
        )
        .unwrap();
        // Invoke tool_search to add mcp__svc__a to discovered.
        let tool_search = reg
            .get(deepagent_builtins::TOOL_SEARCH_TOOL_NAME)
            .unwrap()
            .tool
            .clone();
        let out = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(tool_search.invoke(serde_json::json!({"query": "select:mcp__svc__a"})))
            .unwrap();
        assert!(out.ok);
        assert!(discovered.lock().unwrap().contains("mcp__svc__a"));
    }

    // ----- deferred_tools_announcement (Phase 3B) -----

    #[test]
    fn announcement_is_none_when_undiscovered_is_empty() {
        // Disabled mode / no deferred tools / fully discovered → no block at
        // all so the dynamic section stays clean.
        assert!(deferred_tools_announcement(&[]).is_none());
    }

    #[test]
    fn announcement_lists_names_in_order() {
        let names = vec!["mcp__alpha__one".to_string(), "mcp__beta__two".to_string()];
        let block = deferred_tools_announcement(&names).unwrap();
        // Has the explanatory header.
        assert!(block.starts_with("## Lazy-loaded tools"));
        // Lists each name on its own bullet line inside the XML envelope.
        assert!(block.contains("- mcp__alpha__one"));
        assert!(block.contains("- mcp__beta__two"));
        // Envelope tags are present and ordered correctly.
        let open_idx = block.find("<available-deferred-tools>").unwrap();
        let close_idx = block.find("</available-deferred-tools>").unwrap();
        assert!(open_idx < close_idx);
        // The first bullet must be inside the envelope (between open and close).
        let first_bullet = block.find("- mcp__alpha__one").unwrap();
        assert!(first_bullet > open_idx && first_bullet < close_idx);
    }

    #[test]
    fn announcement_explains_select_and_keyword_syntax() {
        let names = vec!["x".to_string()];
        let block = deferred_tools_announcement(&names).unwrap();
        assert!(block.contains("select:"));
        assert!(block.contains("keyword search"));
        assert!(block.contains("+"));
        assert!(block.contains("required"));
    }

    #[test]
    fn announcement_does_not_appear_in_static_prompt_for_disabled_mode() {
        // The static prefix must NOT mention deferred-tool machinery — that's
        // the whole point of putting the block in the dynamic section. This
        // test guards against accidental leakage into `system_prompt_base`.
        let base = crate::system_prompt::system_prompt_base();
        assert!(!base.contains("<available-deferred-tools>"));
        assert!(!base.contains("Lazy-loaded tools"));
    }

    // ----- Phase 3C: ToolsDiscovered persistence + restore -----

    fn ev(seq: u64, payload: EventPayload) -> deepagent_core::event::Event {
        deepagent_core::event::Event {
            id: deepagent_core::id::EventId::new(),
            session_id: deepagent_core::id::SessionId::nil(),
            sequence: seq,
            timestamp: deepagent_core::clock::Timestamp::from_millis(0),
            payload,
        }
    }

    #[test]
    fn collect_discovered_returns_empty_for_no_events() {
        assert!(collect_discovered_tools_from_events(&[]).is_empty());
    }

    #[test]
    fn collect_discovered_unions_across_events_preserving_order() {
        // Two ToolsDiscovered events. Resume must yield the union, with
        // first-seen order preserved so the tools-array assembly is stable.
        let events = vec![
            ev(
                0,
                EventPayload::ToolsDiscovered {
                    names: vec!["mcp__a__one".into(), "mcp__b__two".into()],
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user("hi"),
                },
            ),
            ev(
                2,
                EventPayload::ToolsDiscovered {
                    names: vec!["mcp__b__two".into(), "mcp__c__three".into()],
                },
            ),
        ];
        let out = collect_discovered_tools_from_events(&events);
        assert_eq!(
            out,
            vec![
                "mcp__a__one".to_string(),
                "mcp__b__two".to_string(),
                "mcp__c__three".to_string(),
            ]
        );
    }

    #[test]
    fn collect_discovered_skips_unrelated_payloads() {
        let events = vec![
            ev(
                0,
                EventPayload::SessionStarted {
                    title: None,
                    mode: Default::default(),
                },
            ),
            ev(
                1,
                EventPayload::Note {
                    text: "hello".into(),
                },
            ),
        ];
        assert!(collect_discovered_tools_from_events(&events).is_empty());
    }

    #[test]
    fn collect_invoked_skill_ids_restores_successful_skill_calls() {
        let events = vec![
            ev(
                0,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "skill-1".into(),
                        name: deepagent_builtins::SKILL_TOOL_NAME.into(),
                        arguments: serde_json::json!({"id": "docx"}),
                    },
                },
            ),
            ev(
                1,
                EventPayload::ToolCallCompleted {
                    call_id: "skill-1".into(),
                    ok: true,
                    output: serde_json::json!({"id": "docx"}),
                    duration_ms: 1,
                },
            ),
            ev(
                2,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "skill-2".into(),
                        name: deepagent_builtins::SKILL_TOOL_NAME.into(),
                        arguments: serde_json::json!({"id": "xlsx"}),
                    },
                },
            ),
            ev(
                3,
                EventPayload::ToolCallCompleted {
                    call_id: "skill-2".into(),
                    ok: false,
                    output: serde_json::json!({"error": "missing"}),
                    duration_ms: 1,
                },
            ),
        ];
        let invoked = collect_invoked_skill_ids_from_events(&events);
        assert!(invoked.contains("docx"));
        assert!(!invoked.contains("xlsx"));
    }

    #[test]
    fn collect_invoked_skill_records_restores_bodies() {
        let events = vec![
            ev(
                0,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "skill-1".into(),
                        name: deepagent_builtins::SKILL_TOOL_NAME.into(),
                        arguments: serde_json::json!({"id": "docx"}),
                    },
                },
            ),
            ev(
                1,
                EventPayload::ToolCallCompleted {
                    call_id: "skill-1".into(),
                    ok: true,
                    output: serde_json::json!({
                        "id": "docx",
                        "name": "docx",
                        "body": "Follow DOCX rules.",
                        "base_dir": "C:/skills/docx",
                        "resources": ["references/style.md"]
                    }),
                    duration_ms: 1,
                },
            ),
            ev(
                2,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "skill-2".into(),
                        name: deepagent_builtins::SKILL_TOOL_NAME.into(),
                        arguments: serde_json::json!({"id": "xlsx"}),
                    },
                },
            ),
            ev(
                3,
                EventPayload::ToolCallCompleted {
                    call_id: "skill-2".into(),
                    ok: false,
                    output: serde_json::json!({"id": "xlsx", "body": "ignore"}),
                    duration_ms: 1,
                },
            ),
        ];

        let records = collect_invoked_skill_records_from_events(&events);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "docx");
        assert_eq!(records[0].body, "Follow DOCX rules.");
        let reminder = invoked_skills_reminder(&records).unwrap();
        assert!(reminder.contains("<invoked-skills>"));
        assert!(reminder.contains("Follow DOCX rules."));
        assert!(reminder.contains("references/style.md"));
    }

    #[tokio::test]
    async fn office_skill_guard_blocks_docx_until_skill_invoked() {
        let session_id = deepagent_core::id::SessionId::new();
        let mut enforce = std::collections::HashSet::new();
        enforce.insert("docx".to_string());
        let hook = OfficeSkillGuardHook::new(
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            enforce,
        );

        let before_docx = deepagent_hooks::HookContext::new(
            session_id,
            HookPoint::BeforeToolUse,
            HookData::before_tool(
                deepagent_builtins::OFFICE_DOCX_CREATE_TOOL_NAME,
                serde_json::json!({"outPath": "report.docx"}),
            ),
        );
        assert!(hook.run(&before_docx).await.unwrap().is_deny());

        let after_skill = deepagent_hooks::HookContext::new(
            session_id,
            HookPoint::AfterToolUse,
            HookData::after_tool(
                deepagent_builtins::SKILL_TOOL_NAME,
                serde_json::json!({"id": "docx"}),
                true,
            ),
        );
        assert_eq!(hook.run(&after_skill).await.unwrap(), HookOutcome::Continue);
        assert_eq!(hook.run(&before_docx).await.unwrap(), HookOutcome::Continue);
    }

    #[test]
    fn tools_discovered_event_kind_label() {
        // Kind label is the discriminant string used by analytics / DB
        // indexing. Stability matters — be loud if anyone changes it.
        let payload = EventPayload::ToolsDiscovered { names: vec![] };
        assert_eq!(payload.kind(), "tools_discovered");
    }

    #[tokio::test]
    async fn discovered_tools_for_session_persists_across_reads() {
        // The per-session set is keyed by id and shared via Arc; the same id
        // returns the same Arc instance.
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        let s1 = chat.discovered_tools_for_session("ses_x");
        let s2 = chat.discovered_tools_for_session("ses_x");
        s1.lock().unwrap().insert("mcp__svc__op".to_string());
        // Same id → writes through one handle visible via the other.
        assert!(s2.lock().unwrap().contains("mcp__svc__op"));
        // Different id → separate set.
        let s3 = chat.discovered_tools_for_session("ses_y");
        assert!(!s3.lock().unwrap().contains("mcp__svc__op"));
    }

    #[tokio::test]
    async fn discovered_tool_names_returns_sorted_snapshot() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        let set = chat.discovered_tools_for_session("ses_x");
        {
            let mut g = set.lock().unwrap();
            g.insert("zeta".into());
            g.insert("alpha".into());
            g.insert("beta".into());
        }
        let names = chat.discovered_tool_names("ses_x");
        assert_eq!(names, vec!["alpha", "beta", "zeta"]);
    }

    #[tokio::test]
    async fn trivial_run_with_knowledge_creates_no_draft() {
        // A run with no tool failures must not auto-capture anything, and the
        // main run must complete normally with a KB attached (Property 12).
        let (db, settings, dir) = seeded().await;
        let kb = Arc::new(
            KnowledgeService::open(&dir.path().join("proj"), &dir.path().join("glob")).unwrap(),
        );
        let chat =
            ChatService::new(db, settings, chat_transport(), dir.path()).with_knowledge(kb.clone());
        let sid = chat.run("say hello", |_| {}, |_| {}).await.unwrap();
        assert!(sid.starts_with("ses_"));
        // Give any (incorrectly) spawned capture task a moment; there should be
        // none because the trivial run had no failures.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(kb.list_drafts().is_empty());
    }

    // ---- Permission-level scenarios -------------------------------------
    //
    // These exercise the exact decision pipeline a run builds: the
    // BeforeToolUse guards (path + bash, policy-aware via FsAccess) feed any
    // `Ask` into the `PolicyGate` for the active `ApprovalPolicy`. The helper
    // resolves a tool call to one of three terminal outcomes.

    use crate::approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
    use crate::settings::{ApprovalPolicy, SandboxMode};
    use deepagent_builtins::{register_guard_hooks, WorkspaceRoot};
    use deepagent_hooks::{HookData, HookOutcome, HookPoint, HookRegistry};
    use deepagent_runtime::{ApprovalDecision, ApprovalGate, ApprovalRequest};

    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        /// Auto-allowed with no user prompt.
        AutoAllow,
        /// Hard-denied by a guard (never reaches the user).
        Denied,
        /// The user was prompted (the floating approval dialog would show).
        Prompted,
    }

    /// Resolve one tool call through the guards + policy gate exactly as a run
    /// would, reporting whether it was auto-allowed, denied, or prompted.
    async fn decide(
        policy: ApprovalPolicy,
        sandbox: SandboxMode,
        tool: &str,
        args: serde_json::Value,
    ) -> Outcome {
        let root = "/work/proj";
        let access = ChatService::fs_access_for(sandbox);

        // Compose the BeforeToolUse guards exactly like run_in_session.
        let mut hooks = HookRegistry::new();
        register_guard_hooks(
            &mut hooks,
            WorkspaceRoot::new(root).with_access(access),
            default_bash_allow(),
        );
        let ctx = deepagent_hooks::HookContext::new(
            deepagent_core::id::SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool(tool, args.clone()),
        );
        let guard_outcome = hooks.dispatch(&ctx).await.unwrap();

        let reason = match guard_outcome {
            HookOutcome::Deny { .. } => return Outcome::Denied,
            HookOutcome::Continue | HookOutcome::Modify { .. } => return Outcome::AutoAllow,
            HookOutcome::Ask { reason, .. } => reason,
        };

        // The guard asked: the PolicyGate decides whether to auto-resolve or
        // actually prompt the user. We detect "prompted" by observing that the
        // request reached the channel gate's notify callback.
        let prompted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let p2 = prompted.clone();
        let pending = PendingApprovals::new();
        let channel = ChannelApprovalGate::new(
            pending.clone(),
            Arc::new(move |_dto| {
                p2.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        );
        let gate = PolicyGate::new(policy, Arc::new(channel));
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool: tool.to_string(),
            reason,
            risk: "ask".into(),
            arguments: args,
        };
        // If the policy will prompt, the gate blocks on the user; drive it
        // concurrently and answer "approve" so the future resolves.
        let handle = tokio::spawn(async move { gate.request(req).await });
        for _ in 0..50 {
            if prompted.load(std::sync::atomic::Ordering::SeqCst) {
                pending.resolve_approved("c1", true);
                break;
            }
            if handle.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let decision = handle.await.unwrap();
        if prompted.load(std::sync::atomic::Ordering::SeqCst) {
            Outcome::Prompted
        } else {
            // Auto-resolved by policy without prompting.
            assert_eq!(decision, ApprovalDecision::Allow);
            Outcome::AutoAllow
        }
    }

    /// 默认权限 (AlwaysAsk): workspace edits free; computer ops + out-of-workspace
    /// access is denied by the sandbox; sensitive files are denied.
    #[tokio::test]
    async fn permission_default_prompts_for_computer_ops_and_outside_access() {
        let p = ApprovalPolicy::AlwaysAsk;
        // Editing a file inside the workspace → no prompt.
        assert_eq!(
            decide(
                p,
                SandboxMode::WorkspaceWrite,
                "write_file",
                serde_json::json!({"path": "src/a.rs"})
            )
            .await,
            Outcome::AutoAllow
        );
        // Running a (non-allow-listed) computer command → prompt.
        assert_eq!(
            decide(
                p,
                SandboxMode::WorkspaceWrite,
                "bash",
                serde_json::json!({"command": "rm -rf build"})
            )
            .await,
            Outcome::Prompted
        );
        // Reading a file outside the workspace is blocked by WorkspaceWrite.
        assert_eq!(
            decide(
                p,
                SandboxMode::WorkspaceWrite,
                "read_file",
                serde_json::json!({"path": "/etc/hosts"})
            )
            .await,
            Outcome::Denied
        );
        // Sensitive credential file → hard denied regardless.
        assert_eq!(
            decide(
                p,
                SandboxMode::WorkspaceWrite,
                "read_file",
                serde_json::json!({"path": ".env"})
            )
            .await,
            Outcome::Denied
        );
    }

    /// 自动审核 (AutoReview): with FullAccess sandbox, out-of-workspace reads
    /// are allowed without prompting; computer ops still prompt the user;
    /// sensitive files are denied.
    #[tokio::test]
    async fn permission_auto_review_allows_outside_reads_but_prompts_computer_ops() {
        let p = ApprovalPolicy::AutoReview;
        // Reading another directory's file → auto-approved (no prompt).
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "read_file",
                serde_json::json!({"path": "/etc/hosts"})
            )
            .await,
            Outcome::AutoAllow
        );
        // Running a computer command → still prompts the user.
        assert_eq!(
            decide(
                p,
                SandboxMode::WorkspaceWrite,
                "bash",
                serde_json::json!({"command": "rm -rf build"})
            )
            .await,
            Outcome::Prompted
        );
        // Sensitive credential file → still denied.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "read_file",
                serde_json::json!({"path": "id_rsa"})
            )
            .await,
            Outcome::Denied
        );
    }

    /// 完全访问 (FullAccess): everything runs without prompting (sensitive files
    /// remain blocked to avoid silent credential leaks).
    #[tokio::test]
    async fn permission_full_access_runs_everything_without_prompt() {
        let p = ApprovalPolicy::FullAccess;
        // Computer command → no prompt.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "bash",
                serde_json::json!({"command": "rm -rf build"})
            )
            .await,
            Outcome::AutoAllow
        );
        // Writing outside the workspace → no prompt.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "write_file",
                serde_json::json!({"path": "/tmp/out.txt"})
            )
            .await,
            Outcome::AutoAllow
        );
        // Sensitive credential file → still denied even at full access.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "read_file",
                serde_json::json!({"path": "config/.env.production"})
            )
            .await,
            Outcome::Denied
        );
    }

    // ----------------------------------------------------------------------
    // Skill marketplace task 14 — with_skills + reset hooks.
    // ----------------------------------------------------------------------

    use crate::skills_service::SkillsService;
    use deepagent_skills::{frontmatter, Skill, SkillManager, SkillOrigin};

    /// Build a [`SkillsService`] backed by an in-memory manager seeded with
    /// the given skills. Wraps it in the `Arc<Mutex<…>>` shape the chat
    /// service expects from [`ChatService::with_skills`].
    fn skills_with(
        tmp: &std::path::Path,
        skills: Vec<(&str, &str, &str, SkillOrigin)>,
    ) -> Arc<std::sync::Mutex<SkillsService>> {
        let mut manager = SkillManager::new(None, tmp.join("inst"));
        for (id, name, desc, origin) in skills {
            let fm = frontmatter::parse(&format!(
                "---\nname: {name}\ndescription: \"{desc}\"\n---\nbody"
            ));
            let skill =
                Skill::from_frontmatter(id, &fm, origin).expect("valid frontmatter for test");
            manager.register(skill);
        }
        Arc::new(std::sync::Mutex::new(SkillsService::from_manager(manager)))
    }

    /// _Validates: Requirements R6.1, R6.2._
    #[tokio::test]
    async fn with_skills_registers_skill_tool_in_run_registry() {
        let (db, settings, dir) = seeded().await;
        let skills = skills_with(
            dir.path(),
            vec![
                ("alpha", "Alpha", "alpha skill", SkillOrigin::User),
                ("bravo", "Bravo", "bravo skill", SkillOrigin::Installed),
            ],
        );
        let chat = ChatService::new(db, settings, chat_transport(), dir.path())
            .with_skills(skills.clone());

        // Build the same registry the run uses (built-ins shared between
        // main and sub-agents) and apply the skill-tool wiring helper.
        let (mut registry, _todo) = chat
            .build_registry(dir.path(), deepagent_builtins::FsAccess::Full, None, None, None)
            .unwrap();
        chat.maybe_register_skill_tool(&mut registry).unwrap();

        assert!(
            registry.get(deepagent_builtins::SKILL_TOOL_NAME).is_some(),
            "skill tool must be registered when SkillsService is attached"
        );
    }

    /// _Validates: Requirements 8.1, 10.3 (Property 9 — backward-compatible
    /// default for callers that don't opt in)._
    #[tokio::test]
    async fn without_skills_does_not_register_skill_tool() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());

        let (mut registry, _todo) = chat
            .build_registry(dir.path(), deepagent_builtins::FsAccess::Full, None, None, None)
            .unwrap();
        chat.maybe_register_skill_tool(&mut registry).unwrap();

        assert!(
            registry.get(deepagent_builtins::SKILL_TOOL_NAME).is_none(),
            "skill tool must NOT be registered when no SkillsService is attached"
        );
    }

    /// _Validates: Requirements 5.6 (reset triggers re-announce on next turn)._
    #[tokio::test]
    async fn reset_all_sent_skills_clears_every_session() {
        let (db, settings, dir) = seeded().await;
        let skills = skills_with(
            dir.path(),
            vec![("alpha", "Alpha", "alpha skill", SkillOrigin::User)],
        );
        let chat = ChatService::new(db, settings, chat_transport(), dir.path()).with_skills(skills);

        // Seed two sessions' worth of state directly on the inner map.
        {
            let mut map = chat.skill_catalog_state.lock().unwrap();
            map.insert(
                "ses-1".into(),
                crate::skill_catalog_reminder::SkillCatalogSendState::default(),
            );
            map.insert(
                "ses-2".into(),
                crate::skill_catalog_reminder::SkillCatalogSendState::default(),
            );
        }

        chat.reset_all_sent_skills();

        let map = chat.skill_catalog_state.lock().unwrap();
        assert!(
            map.is_empty(),
            "reset_all_sent_skills must drop every session entry"
        );
    }

    /// _Validates: Requirements 5.6 (per-session reset path)._
    #[tokio::test]
    async fn reset_sent_skills_only_clears_named_session() {
        let (db, settings, dir) = seeded().await;
        let skills = skills_with(
            dir.path(),
            vec![("alpha", "Alpha", "alpha skill", SkillOrigin::User)],
        );
        let chat = ChatService::new(db, settings, chat_transport(), dir.path()).with_skills(skills);

        {
            let mut map = chat.skill_catalog_state.lock().unwrap();
            map.insert(
                "ses-1".into(),
                crate::skill_catalog_reminder::SkillCatalogSendState::default(),
            );
            map.insert(
                "ses-2".into(),
                crate::skill_catalog_reminder::SkillCatalogSendState::default(),
            );
        }

        chat.reset_sent_skills("ses-1");

        let map = chat.skill_catalog_state.lock().unwrap();
        assert!(!map.contains_key("ses-1"));
        assert!(map.contains_key("ses-2"), "untouched session must survive");
    }

    /// _Validates: Requirements 5.6 — calling the reset hook with a
    /// nonexistent session id is a benign no-op (no panic, no allocation
    /// on the absent entry)._
    #[tokio::test]
    async fn reset_sent_skills_handles_unknown_session() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        // Idempotent: should not panic even with no skills attached.
        chat.reset_sent_skills("never-existed");
        chat.reset_all_sent_skills();
    }
}
