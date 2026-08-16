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
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_context::{
    CompactionPolicy, ContextPolicy, HeuristicSummarizer, HeuristicTokenizer, ModelCompactor,
    Summarizer, TaskSummary, TokenCounter,
};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::message::{Message, ToolCall};
use deepagent_hooks::{
    DecisionSource, Hook, HookContext, HookData, HookDefinitions, HookOutcome, HookPoint,
    HookRegistry,
};
use deepagent_intent::{CommandContext, CommandDef, SlashAction, SlashRegistry};
use deepagent_models::transport::HttpTransport;
#[cfg(test)]
use deepagent_models::ToolSchema;
use deepagent_models::{
    ModelCapabilityResolver, ModelClient, ModelConfig, ModelRole, ThinkingDepth,
};
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_persistence::Database;
use deepagent_runtime::{
    tool_ui_metadata, Agent, AgentKernel, ChannelSink, InputIngress, InputLeaseRegistry, InputMode,
    ModelAgent, PromptDecision, ReactiveContextCompactor, RunRequest, RuntimeEvent,
    RuntimeEventSink,
};
use deepagent_session::Session;
#[cfg(test)]
use deepagent_tools::RiskLevel;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
use crate::context_runtime::{
    build_run_context, collect_invoked_skill_ids_from_events, HookedReactiveContextCompactor,
    RemoteContextFactory, RunContextRequest,
};
#[cfg(test)]
use crate::context_runtime::{
    collect_invoked_skill_records_from_events, invoked_skills_reminder,
    pairing_safe_compaction_split, plugin_output_styles_prompt, render_message_for_compaction,
};
use crate::dto::{ApprovalRequestDto, PreflightToolCallDto};
use crate::hook_assembly::{assemble_run_hooks, HookAssemblyRequest};
use crate::input_runtime::{
    accept_input_turn, collect_discovered_tools_from_events, conversation_from_events,
};
use crate::kernel_runtime::{build_kernel_runtime_config, KernelRuntimeConfigRequest};
use crate::model_runtime::{build_model_client, select_run_model};
use crate::office_service::OfficeService;
use crate::project_map_service::ProjectMapService;
use crate::prompt_gate::{finalize_blocked_user_prompt, submit_user_prompt};
use crate::run_environment::RunEnvironment;
use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
use crate::runtime_event_log::{append_runtime_log, spawn_runtime_event_pump};
use crate::settings::SettingsService;
use crate::slash_panel::{kv, SlashPanel, SlashPanelItem, SlashSection};
#[cfg(test)]
use crate::subagent_runner::{apply_runtime_agent_tool_filter, subagent_system_prompt};
use crate::subagent_runner::{
    collect_runtime_agent_definitions, ChatSubagentRunner, RuntimeAgentDefinition,
};
pub use crate::system_context::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
#[cfg(test)]
use crate::system_context::{build_system_manifest, build_system_prompt, current_date_string};
#[cfg(test)]
use crate::tool_manifest::deferred_tools_announcement;
#[cfg(test)]
use crate::tool_manifest::should_activate_tool_search;
use crate::tool_manifest::DiscoveredToolSet;
#[cfg(test)]
use crate::tool_manifest::{build_visible_tool_schemas, register_tool_search_into};
#[cfg(test)]
use crate::tool_runtime::register_skill_tool;
use crate::tool_runtime::{
    build_base_tool_registry, build_main_run_toolset, CommandExecutorFactory,
    MainRunToolsetRequest, RemoteOpsFactory, RuntimeCommandExecutor, ToolRegistryBuildRequest,
};
const SESSION_TITLE_SYSTEM_PROMPT: &str = concat!(
    "You generate concise conversation titles for a coding assistant session.\n",
    "Return only the title text.\n",
    "Do not use quotes, markdown, numbering, or any explanation.\n",
    "Use the user's language when possible.\n",
    "Focus on the user's concrete task or goal, not greetings or assistant boilerplate.\n",
    "Keep it short and specific."
);

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

/// Orchestrates streamed chat runs over the kernel.
#[derive(Clone)]
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
    /// Optional plugin manager: when set, enabled plugins contribute runtime
    /// overlays for skills, MCP servers, hooks, slash commands, and agents.
    plugins: Option<Arc<crate::plugin_service::PluginService>>,
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
    /// Optional dedicated runtime diagnostics log. This is separate from the
    /// session event log and is used only for troubleshooting execution flow.
    runtime_logs: Option<Arc<RuntimeLogStore>>,
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
    /// Per-session input dispatch lease. A continued session may receive a new
    /// prompt while the previous turn is still streaming; the lease serializes
    /// those turns and lets the new prompt request interruption first.
    input_leases: Arc<InputLeaseRegistry>,
    /// Background child cancellation handles survive the parent tool registry,
    /// so desktop APIs can still inspect or stop a child after the parent turn.
    subagent_controls: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
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
    runtime_broker: Option<Arc<crate::RuntimeBroker>>,
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
type ExecutorFactory = CommandExecutorFactory;

struct RunCancellationRegistration {
    map: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    flag: Arc<AtomicBool>,
    keys: std::sync::Mutex<Vec<String>>,
}

impl RunCancellationRegistration {
    fn new(
        map: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
        run_id: String,
        session_id: Option<&str>,
    ) -> Self {
        let registration = Self {
            map,
            flag: Arc::new(AtomicBool::new(false)),
            keys: std::sync::Mutex::new(Vec::new()),
        };
        registration.add_alias(run_id);
        if let Some(session_id) = session_id {
            registration.add_alias(session_id.to_string());
        }
        registration
    }

    fn add_alias(&self, key: String) {
        let mut keys = self.keys.lock().unwrap_or_else(|p| p.into_inner());
        if keys.contains(&key) {
            return;
        }
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), self.flag.clone());
        keys.push(key);
    }

    fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }
}

impl Drop for RunCancellationRegistration {
    fn drop(&mut self) {
        let keys = self.keys.lock().unwrap_or_else(|p| p.into_inner());
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        for key in keys.iter() {
            if map
                .get(key)
                .is_some_and(|flag| Arc::ptr_eq(flag, &self.flag))
            {
                map.remove(key);
            }
        }
    }
}

/// Per-session map of [`DiscoveredToolSet`]s, keyed by session id.
type DiscoveredToolsMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, DiscoveredToolSet>>>;

type InvokedSkillMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>>;

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
            plugins: None,
            projects: None,
            knowledge: None,
            project_map: None,
            office: None,
            cost: None,
            runtime_logs: None,
            tool_results_dir,
            plan_modes: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            cancellations: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            input_leases: Arc::new(InputLeaseRegistry::default()),
            subagent_controls: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            discovered_tools: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            skills: None,
            skill_catalog_state: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            invoked_skills: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            executor_factory: None,
            local_command_executor: None,
            runtime_broker: None,
            sandboxie_executor: None,
            remote_context_factory: None,
            remote_ops_factory: None,
        }
    }

    /// Request cancellation of an in-flight run by session id or diagnostic
    /// run id. Both keys point at the same flag while a run is active. Returns
    /// whether a matching in-flight run was found. The run stops at its next
    /// step boundary and ends as cancelled (partial transcript preserved).
    pub fn cancel_session(&self, session_id: &str) -> bool {
        let map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
        let found = if let Some(flag) = map.get(session_id) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        };
        drop(map);
        append_runtime_log(
            &self.runtime_logs,
            NewRuntimeLogEntry::info("cancel", "cancel_requested")
                .with_session_id(session_id)
                .with_source("deepagent-app-core::chat_service")
                .with_message(if found {
                    "cancel flag set"
                } else {
                    "cancel requested but no in-flight run found"
                })
                .with_data(serde_json::json!({ "found": found })),
        );
        found
    }

    /// Request cancellation of one background child without stopping its
    /// parent run. Returns false when the child is already terminal or unknown.
    pub fn cancel_subagent(&self, subagent_id: &str) -> bool {
        self.subagent_controls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(subagent_id)
            .map(|flag| !flag.swap(true, std::sync::atomic::Ordering::AcqRel))
            .unwrap_or(false)
    }

    /// List durable child runs for one parent run.
    pub fn subagent_runs(
        &self,
        parent_run_id: &str,
    ) -> Result<Vec<deepagent_persistence::subagent_store::SubagentRunRecord>> {
        deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db)
            .list_for_parent(parent_run_id)
    }

    /// Ordered Agent Kernel v2 events for reconnect/replay consumers.
    pub fn run_events(
        &self,
        run_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<Vec<deepagent_persistence::run_store::StoredRunEvent>> {
        deepagent_persistence::run_store::RunStore::new(&self.db)
            .events_after(run_id, after_sequence)
    }

    /// Attach an [`McpService`](crate::mcp_service::McpService) so enabled MCP
    /// servers are connected and their tools live-registered on each run.
    pub fn with_mcp(mut self, mcp: Arc<crate::mcp_service::McpService>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach a [`PluginService`](crate::plugin_service::PluginService) so
    /// enabled plugins can contribute runtime overlays without mutating the
    /// user's persisted MCP/hooks/skills settings.
    pub fn with_plugins(mut self, plugins: Arc<crate::plugin_service::PluginService>) -> Self {
        self.plugins = Some(plugins);
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

    /// Attach a dedicated runtime diagnostics log.
    pub fn with_runtime_logs(mut self, logs: Arc<RuntimeLogStore>) -> Self {
        self.runtime_logs = Some(logs);
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

    /// Re-read enabled plugins and update the shared skill registry's plugin
    /// roots. Returns the projection so the same run can also use its MCP,
    /// hooks, command, and agent overlays.
    fn sync_plugin_runtime(
        &self,
    ) -> Result<Option<crate::plugin_runtime::PluginRuntimeProjection>> {
        let Some(plugins) = &self.plugins else {
            return Ok(None);
        };
        let projection = plugins.runtime_projection()?;
        if let Some(skills) = &self.skills {
            let mut svc = skills
                .lock()
                .map_err(|_| CoreError::other("skills lock poisoned"))?;
            svc.set_plugin_roots(projection.skill_roots.clone())?;
        }
        Ok(Some(projection))
    }

    fn plugin_runtime_projection(
        &self,
    ) -> Result<Option<crate::plugin_runtime::PluginRuntimeProjection>> {
        match &self.plugins {
            Some(plugins) => plugins.runtime_projection().map(Some),
            None => Ok(None),
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

    /// Bind the shared runtime broker used by local built-in commands.
    pub fn with_runtime_broker(mut self, broker: Arc<crate::RuntimeBroker>) -> Self {
        self.runtime_broker = Some(broker);
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

    fn project_hook_definitions(&self, root: &Path) -> Result<Option<HookDefinitions>> {
        let paths = project_hook_paths(root)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(None);
        }

        let Some(projects) = &self.projects else {
            tracing::info!(
                paths = ?paths,
                "skipping project hooks because no project registry is attached"
            );
            return Ok(None);
        };
        let project_path = root.to_string_lossy().into_owned();
        if !projects.hooks_trusted(&project_path)? {
            tracing::info!(
                project = project_path.as_str(),
                paths = ?paths,
                "skipping untrusted project hooks"
            );
            return Ok(None);
        }

        let mut defs = HookDefinitions::default();
        for path in paths {
            let raw = std::fs::read_to_string(&path).map_err(|e| {
                CoreError::other(format!("read project hooks '{}': {e}", path.display()))
            })?;
            if raw.trim().is_empty() {
                continue;
            }
            let parsed = HookDefinitions::parse(&raw)
                .map_err(|e| CoreError::invalid(format!("{}: {e}", path.display())))?;
            for (event, groups) in parsed.hooks {
                defs.hooks.entry(event).or_default().extend(groups);
            }
        }
        if defs.is_empty() {
            return Ok(None);
        }
        for groups in defs.hooks.values_mut() {
            for group in groups {
                for action in &mut group.hooks {
                    action
                        .env
                        .insert("DEEPAGENT_PROJECT_ROOT".to_string(), project_path.clone());
                }
            }
        }
        Ok(Some(defs))
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
        if let Some((plugin_name, command_name)) = name.split_once(':') {
            if let Some(projection) = self.plugin_runtime_projection()? {
                for root in projection.command_roots {
                    if root.plugin_name != plugin_name {
                        continue;
                    }
                    let path = root.path.join(format!("{command_name}.md"));
                    if !path.exists() {
                        continue;
                    }
                    let mut def = deepagent_prompts::load_command_file(path)?;
                    def.name = name.to_string();
                    def.body.push_str(&format!(
                        "\n\nPlugin runtime context:\nDEEPAGENT_PLUGIN_ROOT={}\nDEEPAGENT_PLUGIN_DATA={}\n",
                        root.path.parent().unwrap_or(&root.path).display(),
                        root.data_dir.display()
                    ));
                    return Ok(Some(def));
                }
            }
        }

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
                    SlashPanel::new("费用")
                        .items(vec![
                            kv("本会话", format!("{} {:.4}", s.currency, s.session_cost)),
                            kv("今日", format!("{} {:.4}", s.currency, s.today_cost)),
                            kv("本月", format!("{} {:.4}", s.currency, s.month_cost)),
                            kv("累计", format!("{} {:.4}", s.currency, s.total_cost)),
                        ])
                        .to_fenced()
                }
                None => "费用跟踪未启用。".to_string(),
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
                let ok = results
                    .iter()
                    .filter(|r| r.status == crate::doctor::DiagStatus::Ok)
                    .count();
                let items: Vec<SlashPanelItem> = results
                    .iter()
                    .map(|r| {
                        let accent = match r.status {
                            crate::doctor::DiagStatus::Ok => "ok",
                            crate::doctor::DiagStatus::Warning => "warn",
                            crate::doctor::DiagStatus::Error => "error",
                        };
                        let value = match &r.fix_hint {
                            Some(h) if r.status != crate::doctor::DiagStatus::Ok => {
                                format!("{} · {h}", r.detail)
                            }
                            _ => r.detail.clone(),
                        };
                        SlashPanelItem::new(&r.name).status(accent).value(value)
                    })
                    .collect();
                SlashPanel::new("环境诊断")
                    .subtitle(format!("{}/{} 项通过", ok, results.len()))
                    .items(items)
                    .to_fenced()
            }
            SlashAction::Help => {
                let registry = SlashRegistry::with_builtins();
                let items: Vec<SlashPanelItem> = registry
                    .names()
                    .iter()
                    .filter_map(|name| {
                        registry.get(name).map(|command| {
                            SlashPanelItem::new(format!("/{}", command.name))
                                .monospace()
                                .value(command.description.clone())
                        })
                    })
                    .collect();
                SlashPanel::new("斜杠命令")
                    .subtitle(format!("{} 个可用命令", items.len()))
                    .items(items)
                    .to_fenced()
            }
            SlashAction::Status => {
                let root = self.effective_root();
                let plan = if self.is_plan_mode(session_id) {
                    "开启"
                } else {
                    "关闭"
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
                    .unwrap_or((false, "(未初始化)", "medium", "default".to_string()));
                let approval = self.settings.approval_policy()?.label();
                SlashPanel::new("状态")
                    .section(SlashSection::new(
                        "项目",
                        vec![
                            kv("目录", root.display().to_string()).monospace(),
                            SlashPanelItem::new("已配置")
                                .status(if configured { "ok" } else { "warn" })
                                .value(if configured { "是" } else { "否" }),
                            kv("计划模式", plan),
                        ],
                    ))
                    .section(SlashSection::new(
                        "模型与运行时",
                        vec![
                            kv("聊天模型", chat_model).monospace(),
                            kv("思考档位", thinking_depth),
                            kv("审批策略", approval),
                            kv("网页搜索", web_search),
                        ],
                    ))
                    .to_fenced()
            }
            SlashAction::Settings => match self.settings.view()? {
                Some(s) => {
                    let mut items = vec![
                        SlashPanelItem::new("已配置")
                            .status(if s.configured { "ok" } else { "warn" })
                            .value(if s.configured { "是" } else { "否" }),
                        kv("API Key", s.api_key_masked.clone()).monospace(),
                        kv("Base URL", s.base_url.clone()).monospace(),
                        kv("聊天模型", s.chat_model.clone()).monospace(),
                        kv("推理模型", s.reasoner_model.clone()).monospace(),
                        kv("思考档位", s.thinking_depth.clone()),
                        kv("审批策略", s.approval_policy.clone()),
                        kv("网页搜索", web_search_summary(&s.web_search)),
                    ];
                    let mut models = SlashPanelItem::new("可用模型");
                    if s.available_models.is_empty() {
                        models = models.value("(无)").status("muted");
                    } else {
                        for m in &s.available_models {
                            models = models.badge(m.clone());
                        }
                    }
                    items.push(models);
                    SlashPanel::new("设置").items(items).to_fenced()
                }
                None => "设置尚未初始化。请先添加 DeepSeek API Key。".to_string(),
            },
            SlashAction::Permissions => {
                let policy = self.settings.approval_policy()?;
                let rules = self.settings.permission_rules()?;
                let rule_items = |list: &[String]| -> Vec<SlashPanelItem> {
                    if list.is_empty() {
                        vec![SlashPanelItem::new("(无)").status("muted")]
                    } else {
                        list.iter()
                            .map(|r| SlashPanelItem::new(r).monospace())
                            .collect()
                    }
                };
                SlashPanel::new("权限")
                    .items(vec![kv("策略", policy.label())])
                    .section(SlashSection::new(
                        format!("允许 ({})", rules.allow.len()),
                        rule_items(&rules.allow),
                    ))
                    .section(SlashSection::new(
                        format!("询问 ({})", rules.ask.len()),
                        rule_items(&rules.ask),
                    ))
                    .section(SlashSection::new(
                        format!("拒绝 ({})", rules.deny.len()),
                        rule_items(&rules.deny),
                    ))
                    .to_fenced()
            }
            SlashAction::Knowledge => match &self.knowledge {
                Some(knowledge) => SlashPanel::new("知识库")
                    .items(vec![
                        kv("项目条目", knowledge.list().len().to_string()),
                        kv("待处理草稿", knowledge.list_drafts().len().to_string()),
                        SlashPanelItem::new("被动注入")
                            .status(if knowledge.passive_enabled() { "ok" } else { "muted" })
                            .value(on_off(knowledge.passive_enabled())),
                        SlashPanelItem::new("自动采集")
                            .status(if knowledge.auto_capture_enabled() { "ok" } else { "muted" })
                            .value(on_off(knowledge.auto_capture_enabled())),
                    ])
                    .to_fenced(),
                None => "当前运行时未启用知识库。".to_string(),
            },
            SlashAction::Mcp => match &self.mcp {
                Some(mcp) => {
                    let plugin_projection = self.plugin_runtime_projection()?;
                    let servers = match plugin_projection.as_ref() {
                        Some(projection) if !projection.mcp_config.servers.is_empty() => mcp
                            .list_with_plugin_overlay(
                                projection.mcp_config.clone(),
                                &projection.mcp_server_sources,
                            )?,
                        _ => mcp.list()?,
                    };
                    if servers.is_empty() {
                        SlashPanel::new("MCP 服务器")
                            .subtitle("还没有 MCP 服务器")
                            .items(vec![])
                            .to_fenced()
                    } else {
                        // Live status + tools (connects enabled servers), merged
                        // with the persisted config for transport labels.
                        let statuses = match plugin_projection.as_ref() {
                            Some(projection) if !projection.mcp_config.servers.is_empty() => mcp
                                .connection_status_with_plugin_overlay(
                                    projection.mcp_config.clone(),
                                    &projection.mcp_server_sources,
                                )
                                .await?,
                            _ => mcp.connection_status().await?,
                        };
                        let enabled = servers.iter().filter(|s| s.enabled).count();
                        let items: Vec<SlashPanelItem> = servers
                            .iter()
                            .map(|s| {
                                let st = statuses.iter().find(|x| x.name == s.name);
                                let accent = match st.map(|x| x.status.as_str()) {
                                    Some("connected") => "ok",
                                    Some("failed") => "error",
                                    Some("disabled") => "muted",
                                    _ if s.enabled => "info",
                                    _ => "muted",
                                };
                                let mut item =
                                    SlashPanelItem::new(&s.name).status(accent).monospace();
                                match st {
                                    Some(st) if st.status == "connected" => {
                                        item = item
                                            .value(&s.transport)
                                            .badge(format!("{} 工具", st.tools.len()))
                                            .children(
                                                st.tools
                                                    .iter()
                                                    .map(|t| {
                                                        let c = SlashPanelItem::new(&t.name)
                                                            .monospace();
                                                        if t.description.is_empty() {
                                                            c
                                                        } else {
                                                            c.value(&t.description)
                                                        }
                                                    })
                                                    .collect(),
                                            );
                                    }
                                    Some(st) if st.status == "failed" => {
                                        item = item.value(match &st.error {
                                            Some(e) => format!("{} · {e}", s.transport),
                                            None => s.transport.clone(),
                                        });
                                    }
                                    _ => {
                                        item = item.value(&s.transport);
                                    }
                                }
                                if s.read_only {
                                    item = item.badge("插件");
                                }
                                if let Some(conflict) = &s.conflict {
                                    item = item.value(conflict).status("warn");
                                }
                                item
                            })
                            .collect();
                        SlashPanel::new("MCP 服务器")
                            .subtitle(format!("{enabled}/{} 已启用", servers.len()))
                            .items(items)
                            .to_fenced()
                    }
                }
                None => "当前运行时未配置 MCP。".to_string(),
            },
            SlashAction::Projects => match &self.projects {
                Some(projects) => {
                    let active = projects.active()?;
                    let list = projects.list()?;
                    let items: Vec<SlashPanelItem> = list
                        .iter()
                        .map(|p| {
                            let is_active = active.as_deref() == Some(p.path.as_str());
                            let mut item = SlashPanelItem::new(&p.name)
                                .value(&p.path)
                                .status(if is_active { "ok" } else { "muted" })
                                .badge(format!("{} 会话", p.session_count));
                            if is_active {
                                item = item.badge("当前");
                            }
                            item
                        })
                        .collect();
                    SlashPanel::new("项目")
                        .subtitle(format!("{} 个已打开", list.len()))
                        .items(items)
                        .to_fenced()
                }
                None => SlashPanel::new("项目")
                    .items(vec![
                        kv("当前", self.effective_root().display().to_string()).monospace()
                    ])
                    .to_fenced(),
            },
            SlashAction::Sessions => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let sessions = store.list_sessions()?;
                let items: Vec<SlashPanelItem> = sessions
                    .iter()
                    .take(12)
                    .map(|record| {
                        let title = record.title.as_deref().unwrap_or("(未命名)");
                        let project = record
                            .project
                            .as_deref()
                            .map(crate::project_service::folder_name)
                            .unwrap_or_else(|| "(无项目)".to_string());
                        SlashPanelItem::new(title)
                            .value(record.id.to_string())
                            .badge(project)
                    })
                    .collect();
                SlashPanel::new("近期会话")
                    .subtitle(format!("{} 个会话", sessions.len()))
                    .items(items)
                    .to_fenced()
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
            SlashAction::Export => {
                // Render the current conversation to Markdown and persist it
                // under the project's `.deepagent/exports/` directory.
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let title = session
                    .state()
                    .title
                    .clone()
                    .unwrap_or_else(|| "session".to_string());
                let mut md = format!("# {title}\n\n");
                for m in &history {
                    md.push_str(&format!("## {:?}\n\n{}\n\n", m.role, m.content));
                }
                let dir = self.effective_root().join(".deepagent").join("exports");
                std::fs::create_dir_all(&dir)
                    .map_err(|e| CoreError::other(format!("create export dir: {e}")))?;
                let file = dir.join(format!("{session_id}.md"));
                std::fs::write(&file, md)
                    .map_err(|e| CoreError::other(format!("write export: {e}")))?;
                format!("已导出 {} 条消息到 {}", history.len(), file.display())
            }
            SlashAction::Rewind => {
                // Rewind is destructive and self-referential when run from
                // inside the live session, so the slash command is advisory:
                // it reports the event count and points to the safe entry
                // points (chat-menu Rewind / non-destructive Fork).
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(session.id())?;
                format!(
                    "当前会话共有 {} 条事件。回退是破坏性操作：它会永久删除某个检查点之后的所有事件。请使用聊天菜单中的 Rewind 选择回退点，或用 Fork 进行非破坏性分支。",
                    events.len()
                )
            }
            SlashAction::Rename { title } => match title {
                Some(title) => {
                    let trimmed = title.trim();
                    if trimmed.is_empty() {
                        "用法: /rename <新标题>".to_string()
                    } else {
                        let store =
                            deepagent_persistence::event_store::EventStore::new(&self.db);
                        let clock = SystemClock;
                        if store.rename_session(session.id(), Some(trimmed), clock.now())? {
                            format!("会话已重命名为「{trimmed}」。")
                        } else {
                            "重命名失败：找不到当前会话。".to_string()
                        }
                    }
                }
                None => "用法: /rename <新标题>".to_string(),
            },
            SlashAction::Skills => match &self.skills {
                Some(skills) => {
                    let list = skills
                        .lock()
                        .map_err(|_| CoreError::other("skills lock poisoned"))?
                        .list();
                    if list.is_empty() {
                        "尚未安装任何技能。打开「技能」页可浏览技能市场。".to_string()
                    } else {
                        let mut by_origin: std::collections::BTreeMap<String, Vec<SlashPanelItem>> =
                            std::collections::BTreeMap::new();
                        for s in &list {
                            by_origin.entry(s.origin.clone()).or_default().push(
                                SlashPanelItem::new(&s.name)
                                    .value(truncate_desc(&s.description, 90)),
                            );
                        }
                        let mut panel = SlashPanel::new("已安装技能")
                            .subtitle(format!("{} 个技能", list.len()));
                        for (origin, items) in by_origin {
                            panel = panel.section(SlashSection::new(origin, items));
                        }
                        panel.to_fenced()
                    }
                }
                None => "当前运行时未启用技能系统。".to_string(),
            },
            SlashAction::Plugins => {
                "插件在「插件」页管理（安装、启用、配置）。可用的运行时能力包括终端、文件预览、录音、浏览器、侧栏聊天等。".to_string()
            }
            SlashAction::Hooks => {
                let defs = self.settings.hook_definitions()?;
                let rules = self.settings.permission_rules()?;
                let event_items: Vec<SlashPanelItem> = defs
                    .hooks
                    .iter()
                    .map(|(event, groups)| {
                        let children: Vec<SlashPanelItem> = groups
                            .iter()
                            .map(|g| {
                                SlashPanelItem::new(
                                    g.matcher.clone().unwrap_or_else(|| "*".to_string()),
                                )
                                .monospace()
                                .badge(format!("{} hook", g.hooks.len()))
                            })
                            .collect();
                        SlashPanelItem::new(event)
                            .badge(format!("{} matcher", groups.len()))
                            .children(children)
                    })
                    .collect();
                let rule_items = |list: &[String]| -> Vec<SlashPanelItem> {
                    if list.is_empty() {
                        vec![SlashPanelItem::new("(无)").status("muted")]
                    } else {
                        list.iter()
                            .map(|r| SlashPanelItem::new(r).monospace())
                            .collect()
                    }
                };
                let mut panel = SlashPanel::new("Hooks")
                    .subtitle(format!("{} 个事件类型", defs.hooks.len()));
                if event_items.is_empty() {
                    panel = panel.items(vec![SlashPanelItem::new("(未配置钩子)").status("muted")]);
                } else {
                    panel = panel.section(SlashSection::new("事件", event_items));
                }
                panel
                    .section(SlashSection::new(
                        format!("允许 ({})", rules.allow.len()),
                        rule_items(&rules.allow),
                    ))
                    .section(SlashSection::new(
                        format!("询问 ({})", rules.ask.len()),
                        rule_items(&rules.ask),
                    ))
                    .section(SlashSection::new(
                        format!("拒绝 ({})", rules.deny.len()),
                        rule_items(&rules.deny),
                    ))
                    .to_fenced()
            }
            SlashAction::Theme => {
                "主题与外观在「设置 → 外观」中调整（暗色/亮色、界面选项）。".to_string()
            }
            SlashAction::Agents => {
                let mut roots = vec![self.effective_root(), self.workspace.clone()];
                roots.dedup();
                let plugin_projection = self.plugin_runtime_projection()?;
                let mut items: Vec<SlashPanelItem> = Vec::new();
                let mut count = 0usize;
                for root in roots {
                    let dir = root.join(".deepagent").join("agents");
                    collect_agent_items(&dir, None, &mut items, &mut count);
                }
                if let Some(projection) = plugin_projection {
                    for root in projection.agent_roots {
                        collect_agent_items(
                            &root.path,
                            Some(root.plugin_name.as_str()),
                            &mut items,
                            &mut count,
                        );
                    }
                }
                if count == 0 {
                    "未发现子代理定义。可在 .deepagent/agents/ 下添加 <name>.md（YAML frontmatter + 系统提示）。".to_string()
                } else {
                    SlashPanel::new("子代理")
                        .subtitle(format!("{count} 个可用"))
                        .items(items)
                        .to_fenced()
                }
            }
            SlashAction::Context => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let counter = HeuristicTokenizer::new();
                let tokens: usize = history
                    .iter()
                    .map(|m| counter.count(&format!("{:?}: {}", m.role, m.content)))
                    .sum();
                SlashPanel::new("上下文使用")
                    .subtitle("可用 /compact 压缩上下文以降低后续请求体积")
                    .items(vec![
                        kv("消息条数", history.len().to_string()),
                        kv("事件条数", events.len().to_string()),
                        SlashPanelItem::new("估算 token")
                            .value(tokens.to_string())
                            .status(if tokens > 100_000 { "warn" } else { "ok" }),
                    ])
                    .to_fenced()
            }
            SlashAction::Init => {
                let root = self.effective_root();
                let file = root.join("AGENTS.md");
                if file.exists() {
                    format!("项目说明文档已存在：{}", file.display())
                } else {
                    let project_name = root
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Project")
                        .to_string();
                    let template = format!(
                        "# {project_name}\n\n## 项目概述\n\n<!-- 一句话描述这个项目的目标 -->\n\n## 技术栈\n\n<!-- 主要语言、框架、构建工具 -->\n\n## 目录结构\n\n<!-- 关键目录及其职责 -->\n\n## 开发约定\n\n<!-- 代码风格、命名、提交规范 -->\n\n## 构建与测试\n\n<!-- 常用命令 -->\n\n## 注意事项\n\n<!-- 易踩的坑、需要人工确认的高风险操作 -->\n"
                    );
                    std::fs::write(&file, template)
                        .map_err(|e| CoreError::other(format!("write AGENTS.md: {e}")))?;
                    format!(
                        "已生成项目说明模板：{}。请补全其中的占位内容。",
                        file.display()
                    )
                }
            }
            SlashAction::Usage => {
                let store = deepagent_persistence::event_store::EventStore::new(&self.db);
                let sessions = store.list_sessions()?;
                let mut items = vec![kv("会话总数", sessions.len().to_string())];
                match &self.cost {
                    Some(cost) => {
                        let s = cost.summary(session_id)?;
                        items.push(kv("本会话", format!("{} {:.4}", s.currency, s.session_cost)));
                        items.push(kv("今日", format!("{} {:.4}", s.currency, s.today_cost)));
                        items.push(kv("本月", format!("{} {:.4}", s.currency, s.month_cost)));
                        items.push(kv("累计", format!("{} {:.4}", s.currency, s.total_cost)));
                    }
                    None => {
                        items.push(
                            SlashPanelItem::new("费用跟踪").status("muted").value("未启用"),
                        );
                    }
                }
                SlashPanel::new("用量统计").items(items).to_fenced()
            }
            SlashAction::AddDir { path } => match path {
                Some(path) => match &self.projects {
                    Some(projects) => {
                        let trimmed = path.trim();
                        if trimmed.is_empty() {
                            "用法: /add-dir <目录路径>".to_string()
                        } else if !std::path::Path::new(trimmed).is_dir() {
                            format!("目录不存在或不是文件夹：{trimmed}")
                        } else {
                            let dto = projects.add_project(trimmed)?;
                            format!(
                                "已添加工作目录：{} ({})。在侧栏切换项目即可以它为根开始新会话。",
                                dto.name, dto.path
                            )
                        }
                    }
                    None => "当前运行时未启用项目管理。".to_string(),
                },
                None => "用法: /add-dir <目录路径>".to_string(),
            },
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
        context_policy: &ContextPolicy,
        hooks: &HookRegistry,
    ) -> (Vec<Message>, bool) {
        let policy = CompactionPolicy {
            trigger_tokens: context_policy.compaction_trigger_tokens(),
            ..CompactionPolicy::default()
        };
        // Render each turn to a rough "role: content" string for counting +
        // summarization input.
        let rendered: Vec<String> = history
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content))
            .collect();
        let counter = HeuristicTokenizer::new();
        let total: usize = rendered.iter().map(|t| counter.count(t)).sum();

        if !policy.should_compact(total) || history.len() <= policy.keep_recent_turns {
            return (history, false);
        }

        let pre_compact = hooks
            .dispatch(&deepagent_hooks::HookContext::new(
                session.id(),
                HookPoint::BeforeCompact,
                deepagent_hooks::HookData::Compact {
                    trigger: "token_pressure".to_string(),
                    summary: None,
                },
            ))
            .await;
        match pre_compact {
            Ok(deepagent_hooks::HookOutcome::Deny { reason, .. })
            | Ok(deepagent_hooks::HookOutcome::Ask { reason, .. }) => {
                tracing::warn!(reason, "context compaction blocked by PreCompact hook");
                return (history, false);
            }
            Err(error) => {
                tracing::warn!(error = %error, "PreCompact hook failed; keeping full history");
                return (history, false);
            }
            _ => {}
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
        if let Err(error) = hooks
            .dispatch(&deepagent_hooks::HookContext::new(
                session.id(),
                HookPoint::PostCompact,
                deepagent_hooks::HookData::Compact {
                    trigger: "token_pressure".to_string(),
                    summary: Some(summary_block),
                },
            ))
            .await
        {
            tracing::warn!(error = %error, "PostCompact hook failed");
        }
        (compacted, true)
    }

    /// Build the tool registry with the built-ins confined to `root`.
    ///
    /// Includes `ask_user_question` (wired to a headless-safe responder), the
    /// file/bash/search/todo built-ins, and the network web tools (with the
    /// `web` feature). It deliberately does **not** include the `task`
    /// sub-agent tool — that is added only to the *main* run's registry (see
    /// [`ChatService::run_in_session`]) so sub-agents can't recurse into more
    /// sub-agents, mirroring Claude Code's agent-disallowed-tools rule.
    pub(crate) fn build_registry(
        &self,
        root: &std::path::Path,
        access: deepagent_builtins::FsAccess,
        env_mode: Option<&str>,
        connection_id: Option<&str>,
        local_exec_mode: Option<crate::settings::LocalExecutionMode>,
        bash_external_safety_gate: bool,
    ) -> Result<(ToolRegistry, deepagent_builtins::TodoStore)> {
        build_base_tool_registry(self.base_registry_request(
            root,
            access,
            env_mode,
            connection_id,
            local_exec_mode,
            bash_external_safety_gate,
        ))
    }

    /// Assemble the shared [`ToolRegistryBuildRequest`] from this service's
    /// wiring. Used by both [`ChatService::build_registry`] (sub-agent /
    /// standalone registries) and the main run's
    /// [`build_main_run_toolset`] single entry point.
    fn base_registry_request<'a>(
        &self,
        root: &'a std::path::Path,
        access: deepagent_builtins::FsAccess,
        env_mode: Option<&'a str>,
        connection_id: Option<&'a str>,
        local_exec_mode: Option<crate::settings::LocalExecutionMode>,
        bash_external_safety_gate: bool,
    ) -> ToolRegistryBuildRequest<'a> {
        let local_command_executor = match (&self.runtime_broker, &self.local_command_executor) {
            (Some(broker), Some(executor)) => Some(Arc::new(RuntimeCommandExecutor::new(
                executor.clone(),
                broker.clone(),
                root,
            ))
                as Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>),
            (Some(broker), None) => Some(Arc::new(RuntimeCommandExecutor::new(
                Arc::new(deepagent_builtins::bash_tool::SystemExecutor),
                broker.clone(),
                root,
            ))
                as Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>),
            (None, executor) => executor.clone(),
        };
        ToolRegistryBuildRequest {
            root,
            access,
            env_mode,
            connection_id,
            local_exec_mode,
            bash_external_safety_gate,
            bash_allow: self.bash_allow.clone(),
            settings: self.settings.clone(),
            executor_factory: self.executor_factory.clone(),
            local_command_executor,
            knowledge: self.knowledge.clone(),
            project_map: self.project_map.clone(),
            office: self.office.clone(),
            remote_ops_factory: self.remote_ops_factory.clone(),
        }
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
    #[cfg(test)]
    fn maybe_register_skill_tool(&self, registry: &mut ToolRegistry) -> Result<()> {
        register_skill_tool(registry, self.skills.as_ref())
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
        build_model_client(&self.settings, self.transport.clone(), role)
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
        let request = deepagent_models::chat::ResponseRequest::with_instructions_and_user_input(
            model,
            system_prompt,
            user_prompt,
        )
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
        let response = client
            .stream_response_observed(request, &mut observer)
            .await?;
        Ok(response.output_text_projection())
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
    ///    BEFORE [`with_thinking_depth`][deepagent_models::ResponseRequest::with_thinking_depth]
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

        let config = ModelConfig::from_catalog(api_key, &settings.catalog, ModelRole::Chat)
            .with_defaults(deepagent_models::ResponseDefaults {
                temperature: settings.responses.effective_temperature(),
                top_p: settings.responses.effective_top_p(),
                max_output_tokens: settings.responses.effective_max_output_tokens(),
                top_logprobs: settings.responses.effective_top_logprobs(),
                reasoning_effort: settings.responses.effective_reasoning_effort(),
                text: settings.responses.effective_text(),
                tool_choice: settings.responses.effective_tool_choice(),
                user: settings.responses.effective_user(),
                native_web_search: settings.web_search.enabled
                    && matches!(
                        settings.web_search.provider,
                        crate::settings::WebSearchProvider::DeepSeekFirst
                    ),
            });
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));

        // Order matters: `with_max_output_tokens` must come BEFORE
        // `with_thinking_depth` so the explicit cap survives. The depth
        // helper only fills `max_tokens` when it's still `None`.
        let request = deepagent_models::chat::ResponseRequest::with_instructions_and_user_input(
            review_model,
            system_prompt,
            user_prompt,
        )
        .streaming()
        .with_max_output_tokens(max_output_tokens)
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
        let response = client
            .stream_response_observed(request, &mut observer)
            .await?;
        Ok(response.output_text_projection())
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
        let config = ModelConfig::from_catalog(api_key, &settings.catalog, ModelRole::Chat)
            .with_defaults(deepagent_models::ResponseDefaults {
                temperature: settings.responses.effective_temperature(),
                top_p: settings.responses.effective_top_p(),
                max_output_tokens: settings.responses.effective_max_output_tokens(),
                top_logprobs: settings.responses.effective_top_logprobs(),
                reasoning_effort: settings.responses.effective_reasoning_effort(),
                text: settings.responses.effective_text(),
                tool_choice: settings.responses.effective_tool_choice(),
                user: settings.responses.effective_user(),
                native_web_search: settings.web_search.enabled
                    && matches!(
                        settings.web_search.provider,
                        crate::settings::WebSearchProvider::DeepSeekFirst
                    ),
            });
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));

        let request = deepagent_models::chat::ResponseRequest::with_instructions_and_user_input(
            model,
            SESSION_TITLE_SYSTEM_PROMPT,
            format!(
                "Create a short conversation title from this transcript:\n\n{}",
                lines.join("\n")
            ),
        )
        .streaming()
        .with_max_output_tokens(48)
        .with_thinking_depth(ThinkingDepth::Simple);
        let response = client.stream_response(request).await?;
        let Some(title) = normalize_generated_session_title(&response.output_text_projection())
        else {
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
            false,
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
    #[allow(clippy::too_many_arguments)]
    pub async fn run_in_session<F, A>(
        &self,
        prompt: &str,
        continue_session: Option<&str>,
        env_mode: Option<&str>,
        connection_id: Option<&str>,
        preflight_tools: Vec<PreflightToolCallDto>,
        preflight_abort_message: Option<String>,
        initial_plan_mode: bool,
        diagnostic_run_id: Option<&str>,
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let root = self.effective_root();
        let normalized_input = InputIngress::normalize(
            continue_session.map(ToOwned::to_owned),
            root.clone(),
            prompt,
            InputMode::Prompt,
            Vec::new(),
        )?;
        let raw_prompt = prompt;
        let effective_input_text = normalized_input.effective_text.clone();
        let prompt = effective_input_text.as_str();

        if let Some(session_id) = self
            .maybe_handle_slash_command(prompt, continue_session, &on_event)
            .await?
        {
            return Ok(session_id);
        }

        let run_id = diagnostic_run_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("run_{}", deepagent_core::id::EventId::new()));
        // §7.2 observability: a per-run W3C trace context. Its trace id is
        // stamped into the run's structured logs so every log line for this
        // run is correlatable ("W3C trace_id 贯穿"), and an OTLP exporter can
        // later reuse the traceparent verbatim without touching call sites.
        let run_trace = deepagent_tracing::trace_context::TraceContext::new_root();
        let run_trace_id = run_trace.trace_id.to_hex();
        let run_traceparent = run_trace.traceparent();
        let cancellation = RunCancellationRegistration::new(
            self.cancellations.clone(),
            run_id.clone(),
            continue_session,
        );
        append_runtime_log(
            &self.runtime_logs,
            NewRuntimeLogEntry::info("chat", "run_requested")
                .with_run_id(run_id.clone())
                .with_source("deepagent-app-core::chat_service")
                .with_message("chat run requested")
                .with_data(serde_json::json!({
                    "trace_id": run_trace_id,
                    "traceparent": run_traceparent,
                    "continue_session": continue_session,
                    "env_mode": env_mode,
                    "connection_id": connection_id,
                    "preflight_tool_count": preflight_tools.len(),
                    "preflight_abort": preflight_abort_message.is_some(),
                    "initial_plan_mode": initial_plan_mode,
                    "input_id": normalized_input.input_id.clone(),
                    "input_kind": format!("{:?}", normalized_input.kind),
                    "raw_prompt_len": raw_prompt.chars().count(),
                    "effective_prompt_len": prompt.chars().count(),
                })),
        );

        let plugin_projection = self.sync_plugin_runtime()?;
        let RunEnvironment {
            config: run_config,
            profile: _profile,
            policy,
            sandbox_mode,
            local_execution_mode,
            access,
        } = RunEnvironment::resolve(
            &root,
            &self.settings,
            &self.runtime_logs,
            &self.sandboxie_executor,
            &run_id,
        )?;

        let mut model_prompt = self
            .dynamic_command_prompt(prompt)?
            .unwrap_or_else(|| prompt.to_string());
        let mut prompt_to_record = prompt.to_string();

        // Budget gate: refuse a new run when a configured daily/monthly limit is
        // already exhausted. No-op when no cost tracker is attached or no budget
        // is set (Property 7: backward-compatible default).
        if let Some(cost) = &self.cost {
            cost.check_budget()?;
        }

        if let Some(knowledge) = &self.knowledge {
            knowledge.activate_project(&root)?;
        }
        // The main run's tools are built permissive (Full, sensitive-blocked):
        // the BeforeToolUse path guard is the SINGLE policy gate, asking/denying
        // per the sandbox-derived `access`. This way an out-of-workspace access
        // the sandbox allows actually executes, instead of being
        // re-rejected inside the tool. Sub-agents (below) have no interactive
        // gate, so their tools stay confined to the sandbox `access`.
        let clock = SystemClock;
        let project = root.to_string_lossy().into_owned();
        let accepted_turn = accept_input_turn(
            &self.db,
            &clock,
            self.input_leases.clone(),
            self.runtime_logs.as_ref().map(Arc::clone),
            &run_id,
            continue_session,
            env_mode,
            &project,
            normalized_input.clone(),
            cancellation.flag(),
            |active_run| self.cancel_session(active_run),
        )
        .await?;
        let mut session = accepted_turn.session;
        let history = accepted_turn.history;
        let response_history = accepted_turn.response_history;
        let prior_events = accepted_turn.prior_events;
        let session_id_str = accepted_turn.session_id;
        let _input_lease = accepted_turn.lease;

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

        let run_model = select_run_model(
            &self.settings,
            self.transport.clone(),
            ModelRole::Chat,
            ModelRole::Reasoner,
            run_config.model_override(),
        )?;
        let client = run_model.client;
        let model = run_model.model;
        let thinking_depth = run_model.thinking_depth;
        let fallback_model = run_model.fallback_model;

        let plan = self.plan_mode_for_session(&session_id_str);
        if continue_session.is_none() && initial_plan_mode {
            plan.set(true);
        }

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

        // Tool-search settings (read once per run) — used both for the main
        // run's tool-search wiring and for seeding the sub-agent runner with
        // the parent's discovered tool set.
        let tool_search_mode = self.settings.tool_search_mode().unwrap_or_default();
        let tool_search_threshold = self
            .settings
            .tool_search_auto_threshold()
            .unwrap_or(SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS);
        let tool_search_discovered = self.discovered_tools_for_session(&session_id_str);
        // Thinking depth is the user's explicit setting — never inferred from
        // prompt keywords. Upstream parity: codex reasoning effort is pure
        // config; Claude Code keys thinking off model capability + explicit
        // settings, and only a user-typed magic word (ultrathink) nudges it.
        let subagent_thinking_depth = thinking_depth;

        // Create the live event channel before registering the task tool so
        // child-agent lifecycle events use the same UI/logging pump. The
        // runner stores only a Weak sender; detached children must not keep the
        // parent run waiting for this channel to close.
        let (sink, rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        let subagent_hooks: Arc<std::sync::OnceLock<Arc<HookRegistry>>> =
            Arc::new(std::sync::OnceLock::new());
        let subagent_parent_checkpoint: Arc<
            std::sync::OnceLock<Arc<deepagent_runtime::CheckpointManager>>,
        > = Arc::new(std::sync::OnceLock::new());
        let pump = spawn_runtime_event_pump(
            rx,
            self.runtime_logs.clone(),
            run_id.clone(),
            session_id_str.clone(),
            on_event,
        );

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
        let (task_runner, task_agent_types) = {
            let sub_registry = Arc::new(
                self.build_registry(
                    &root,
                    access,
                    None,
                    None,
                    Some(local_execution_mode),
                    matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
                )?
                .0,
            );
            let runtime_agents = collect_runtime_agent_definitions(
                [root.clone(), self.workspace.clone()],
                plugin_projection.as_ref(),
            );
            let task_agent_types: Vec<deepagent_builtins::TaskAgentType> = runtime_agents
                .iter()
                .map(RuntimeAgentDefinition::task_agent_type)
                .collect();
            let agent_definitions: std::collections::BTreeMap<String, RuntimeAgentDefinition> =
                runtime_agents
                    .into_iter()
                    .map(|agent| (agent.type_name.clone(), agent))
                    .collect();
            let parent_discovered_snapshot: std::collections::HashSet<String> =
                tool_search_discovered
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
            let runner = ChatSubagentRunner {
                db: self.db.clone(),
                parent_run_id: run_id.clone(),
                transcript_root: self.tool_results_dir.join("subagents"),
                client: client.clone(),
                model: model.clone(),
                thinking_depth: subagent_thinking_depth,
                registry: sub_registry,
                root: root.clone(),
                tool_search_mode,
                tool_search_auto_threshold: tool_search_threshold,
                parent_discovered_snapshot,
                agent_definitions,
                background: self.subagent_controls.clone(),
                events: Arc::downgrade(&sink),
                skills: self.skills.clone(),
                host: self.clone(),
                access,
                local_execution_mode,
                bash_full_access: matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
                hooks: subagent_hooks.clone(),
                parent_checkpoint: subagent_parent_checkpoint.clone(),
            };
            (runner, task_agent_types)
        };

        // Single-entry main-run tool wiring (Phase A): base built-ins -> MCP
        // adapters -> `task` -> `knowledge_write` -> plan-mode toggles ->
        // `skill` -> tool-search manifest snapshot. The manifest is prepared
        // LAST so deferred-tool snapshots cover every registered tool.
        let toolset = build_main_run_toolset(MainRunToolsetRequest {
            base: self.base_registry_request(
                &root,
                deepagent_builtins::FsAccess::Full,
                effective_env_mode,
                connection_id,
                Some(local_execution_mode),
                true,
            ),
            mcp: self.mcp.as_deref(),
            plugin_projection: plugin_projection.as_ref(),
            task_runner,
            task_agent_types,
            plan: plan.clone(),
            skills: self.skills.as_ref(),
            tool_search_mode,
            tool_search_discovered: tool_search_discovered.clone(),
            tool_search_threshold,
        })
        .await?;
        let registry = toolset.registry;
        let todo_store = toolset.todo_store;
        let hook_mcp_registry = toolset.hook_mcp_registry;
        let tool_manifest = toolset.manifest;
        let tools = tool_manifest.tools.clone();
        let granted = PermissionSet::developer();
        append_runtime_log(
            &self.runtime_logs,
            NewRuntimeLogEntry::info("runtime", "registry_ready")
                .with_run_id(run_id.clone())
                .with_session_id(session_id_str.clone())
                .with_source("deepagent-app-core::chat_service")
                .with_message("tool registry prepared")
                .with_data(serde_json::json!({
                    "registered_tools": registry.len(),
                    "visible_tools": tools.len(),
                    "deferred_tools": tool_manifest.deferred_tool_names.clone(),
                    "tool_search_mode": tool_search_mode.label(),
                    "tool_search_threshold": tool_search_threshold,
                    "effective_env_mode": effective_env_mode,
                })),
        );

        // Wire the approval gate: AlwaysAsk → channel gate (prompts the UI);
        // auto policies short-circuit to allow.
        let channel_gate = ChannelApprovalGate::new(self.pending.clone(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> = Arc::new(
            PolicyGate::new(policy, Arc::new(channel_gate))
                .with_classifier(deepagent_builtins::SafetyClassifier::with_defaults()),
        );

        // Wire hooks through a dedicated assembler: rules, declarative hooks,
        // plugin/project overlays, office guards, and path/bash guards keep one
        // deterministic order outside the chat entrypoint.
        let project_hooks = match self.project_hook_definitions(&root) {
            Ok(defs) => defs,
            Err(e) => {
                tracing::warn!(
                    project = root.display().to_string(),
                    error = %e,
                    "ignoring malformed project hooks.json"
                );
                None
            }
        };
        let office_skill_guard = self
            .office_skill_guard_hook(&session_id_str, &prior_events)?
            .map(|hook| Arc::new(hook) as Arc<dyn Hook>);
        let hooks = assemble_run_hooks(HookAssemblyRequest {
            settings: &self.settings,
            run_config: &run_config,
            project_hooks,
            plugin_projection: plugin_projection.as_ref(),
            root: &root,
            sink: sink.clone(),
            client: client.clone(),
            model: model.clone(),
            thinking_depth,
            mcp: hook_mcp_registry,
            registry: &registry,
            plan: plan.clone(),
            office_skill_guard,
            access,
            bash_allow: self.bash_allow.clone(),
            bash_full_access: matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
            is_trusted: crate::trust_service::TrustService::new(self.db.clone()).is_trusted(&root),
            runtime_environment: self
                .runtime_broker
                .as_ref()
                .map(|broker| broker.build_process_environment(Some(&root)))
                .unwrap_or_default(),
        })?
        .hooks;
        let _ = subagent_hooks.set(hooks.clone());
        append_runtime_log(
            &self.runtime_logs,
            NewRuntimeLogEntry::info("hook", "hooks_registered")
                .with_run_id(run_id.clone())
                .with_session_id(session_id_str.clone())
                .with_source("deepagent-app-core::chat_service")
                .with_message("runtime hooks registered")
                .with_data(serde_json::json!({
                    "before_tool_use": hooks.count_at(HookPoint::BeforeToolUse),
                    "after_tool_use": hooks.count_at(HookPoint::AfterToolUse),
                    "user_prompt_submit": hooks.count_at(HookPoint::UserPromptSubmit),
                    "session_start": hooks.count_at(HookPoint::SessionStart),
                    "session_end": hooks.count_at(HookPoint::SessionEnd),
                    "verification_failed": hooks.count_at(HookPoint::VerificationFailed),
                    "approval_policy": policy.label(),
                    "sandbox_mode": sandbox_mode.label(),
                })),
        );

        let prompt_decision = {
            submit_user_prompt(
                &registry,
                &hooks,
                session.id(),
                model_prompt.clone(),
                cancellation.flag(),
            )
            .await?
        };
        match prompt_decision {
            PromptDecision::Accept(effective_prompt) => {
                append_runtime_log(
                    &self.runtime_logs,
                    NewRuntimeLogEntry::info("hook", "user_prompt_submit_accepted")
                        .with_run_id(run_id.clone())
                        .with_session_id(session_id_str.clone())
                        .with_source("deepagent-app-core::chat_service")
                        .with_message("UserPromptSubmit accepted prompt")
                        .with_data(serde_json::json!({
                            "modified": effective_prompt != model_prompt,
                            "prompt_len": effective_prompt.chars().count(),
                        })),
                );
                if effective_prompt != model_prompt {
                    prompt_to_record = effective_prompt.clone();
                }
                model_prompt = effective_prompt;
            }
            PromptDecision::Rejected { reason } => {
                append_runtime_log(
                    &self.runtime_logs,
                    NewRuntimeLogEntry {
                        level: "warn".into(),
                        ..NewRuntimeLogEntry::info("hook", "user_prompt_submit_rejected")
                            .with_run_id(run_id.clone())
                            .with_session_id(session_id_str.clone())
                            .with_source("deepagent-app-core::chat_service")
                            .with_message(format!("UserPromptSubmit rejected prompt: {reason}"))
                            .with_data(serde_json::json!({ "reason": reason.clone() }))
                    },
                );
                let message = format!("UserPromptSubmit hook blocked the prompt: {reason}");
                let session_id =
                    finalize_blocked_user_prompt(&mut session, prompt, message, sink.as_ref())?;
                drop(hooks);
                drop(sink);
                let _ = pump.await;
                return Ok(session_id);
            }
            PromptDecision::NeedsApproval { reason, .. } => {
                append_runtime_log(
                    &self.runtime_logs,
                    NewRuntimeLogEntry::info("hook", "user_prompt_submit_needs_approval")
                        .with_run_id(run_id.clone())
                        .with_session_id(session_id_str.clone())
                        .with_source("deepagent-app-core::chat_service")
                        .with_message(format!(
                            "UserPromptSubmit needs approval before prompt can run: {reason}"
                        ))
                        .with_data(serde_json::json!({ "reason": reason.clone() })),
                );
                let message = format!(
                    "UserPromptSubmit hook requires approval before this prompt can run: {reason}"
                );
                let session_id =
                    finalize_blocked_user_prompt(&mut session, prompt, message, sink.as_ref())?;
                drop(hooks);
                drop(sink);
                let _ = pump.await;
                return Ok(session_id);
            }
        }
        let prompt_for_model = model_prompt.as_str();
        let effective_thinking_depth = thinking_depth;
        let model_capability = ModelCapabilityResolver::new().resolve_model_id(&model);
        let context_policy =
            ContextPolicy::for_capability(&model_capability, effective_thinking_depth);

        // Model-driven context compaction (Phase 2B): when the recovered history
        // is large (token pressure over the policy threshold), compress the
        // older turns into a structured summary and seed the agent with
        // [summary + recent turns] instead of the full transcript. Falls back to
        // the heuristic summarizer if the model call fails, and records a
        // `ContextCompacted` event. No-op for new sessions / short history.
        let (history, context_compacted) = self
            .maybe_compact_history(
                &mut session,
                history,
                &client,
                &model,
                &context_policy,
                &hooks,
            )
            .await;
        let response_history = if context_compacted {
            Vec::new()
        } else {
            response_history
        };
        let session_id = session.id().to_string();
        {
            let mut map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
            map.entry(session_id.clone())
                .or_insert_with(|| plan.clone());
        }
        // Record the incoming user turn so the thread's history is complete.
        session.append(EventPayload::MessageAppended {
            message: Message::user(&prompt_to_record),
        })?;
        let task = session.create_task(&prompt_to_record)?;
        for tool in &preflight_tools {
            let call = ToolCall {
                id: tool.call_id.clone(),
                name: tool.name.clone(),
                arguments: tool.arguments.clone(),
            };
            let started_meta = tool_ui_metadata(&call.name, &call.arguments, None);
            sink.emit(RuntimeEvent::ToolStarted {
                name: call.name.clone(),
                call_id: call.id.clone(),
                arguments: call.arguments.clone(),
                tool_kind: started_meta.tool_kind,
                file_path: started_meta.file_path,
                summary: started_meta.summary,
                meta: started_meta.meta,
            });
            session.append(EventPayload::ToolCallRequested { call })?;
            session.append(EventPayload::ToolCallCompleted {
                call_id: tool.call_id.clone(),
                ok: tool.ok,
                output: tool.output.clone(),
                duration_ms: tool.duration_ms,
            })?;
            let completed_meta = tool_ui_metadata(&tool.name, &tool.arguments, Some(&tool.output));
            sink.emit(RuntimeEvent::ToolCompleted {
                name: tool.name.clone(),
                call_id: tool.call_id.clone(),
                ok: tool.ok,
                output: tool.output.clone(),
                duration_ms: tool.duration_ms,
                tool_kind: completed_meta.tool_kind,
                file_path: completed_meta.file_path,
                summary: completed_meta.summary,
                meta: completed_meta.meta,
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
            drop(hooks);
            drop(sink);
            let _ = pump.await;
            return Ok(session_id);
        }

        let run_context = build_run_context(RunContextRequest {
            root: &root,
            sandbox_mode,
            plugin_projection: plugin_projection.as_ref(),
            tool_manifest: &tool_manifest,
            skills: self.skills.as_ref(),
            settings: &self.settings,
            skill_catalog_state: &self.skill_catalog_state,
            session_id: &session_id,
            prior_events: &prior_events,
            knowledge: self.knowledge.as_ref(),
            prompt_for_model,
            effective_env_mode,
            connection_id,
            remote_context_factory: self.remote_context_factory.as_ref(),
            context_policy: &context_policy,
            history: &history,
            tools: &tools,
            context_compacted,
        })
        .await?;
        let system_manifest = run_context.system_manifest;
        if !system_manifest.loaded_paths.is_empty() {
            if let Err(error) = hooks
                .dispatch(&deepagent_hooks::HookContext::new(
                    session.id(),
                    HookPoint::InstructionsLoaded,
                    deepagent_hooks::HookData::Instructions {
                        paths: system_manifest
                            .loaded_paths
                            .iter()
                            .map(|path| path.to_string_lossy().to_string())
                            .collect(),
                    },
                ))
                .await
            {
                tracing::warn!(error = %error, "InstructionsLoaded hook failed");
            }
        }
        let system_prompt = run_context.system_prompt;
        let final_user_prompt = run_context.final_user_prompt;
        sink.emit(RuntimeEvent::ContextUsage {
            snapshot: run_context.context_usage,
        });

        // Clone the model handle for the post-run auto-capture (the originals
        // are moved into the agent below).
        let capture_client = client.clone();
        let capture_model = model.clone();
        let reactive_compactor: Arc<dyn ReactiveContextCompactor> =
            Arc::new(HookedReactiveContextCompactor::new(
                client.clone(),
                model.clone(),
                hooks.clone(),
                session.id(),
            ));
        // Model name for cost attribution (the original `model` is moved into
        // the agent below).
        let model_name_for_cost = model.clone();

        // Proactive auto-compact threshold (Claude Code autoCompact.ts): checked
        // before every model request. `reserve` overrides the 13k buffer via
        // settings / DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS; the pct override is a
        // testing knob (CC CLAUDE_AUTOCOMPACT_PCT_OVERRIDE).
        let autocompact_reserve = self.settings.autocompact_reserve_tokens();
        let autocompact_pct_override = std::env::var("DEEPAGENT_AUTOCOMPACT_PCT_OVERRIDE")
            .ok()
            .and_then(|raw| raw.trim().parse::<f32>().ok());
        let proactive_threshold = context_policy
            .autocompact_threshold_tokens(autocompact_reserve, autocompact_pct_override)
            as u64;
        // Prefire (Grok two-pass) start line: begin the background pass-1 when
        // usage reaches `threshold - lead%` of the effective window, giving
        // pass-1 runway before the hard threshold. Lead defaults to 10%
        // (Grok `DEFAULT_PREFIRE_LEAD_PERCENT`), overridable via
        // `DEEPAGENT_PREFIRE_LEAD_PERCENT`.
        let prefire_lead_percent = std::env::var("DEEPAGENT_PREFIRE_LEAD_PERCENT")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(10)
            .min(100);
        let prefire_lead_tokens =
            context_policy.effective_context_window() as u64 * prefire_lead_percent / 100;
        let prefire_start = proactive_threshold.saturating_sub(prefire_lead_tokens);
        append_runtime_log(
            &self.runtime_logs,
            NewRuntimeLogEntry::info("context", "autocompact_threshold_ready")
                .with_run_id(&run_id)
                .with_session_id(&session_id)
                .with_data(serde_json::json!({
                    "threshold_tokens": proactive_threshold,
                    "reserve_override": autocompact_reserve,
                    "pct_override": autocompact_pct_override,
                    "context_window": context_policy.context_window,
                    "prefire_start_tokens": prefire_start,
                    "prefire_lead_percent": prefire_lead_percent,
                })),
        );

        // Cloned before the agent takes ownership, for the opt-in stall
        // detector wiring below (§2.3).
        let stall_client = client.clone();
        let stall_model = model.clone();
        // Opt-in advanced execution safeguards resolve settings-first, with the
        // matching `DEEPAGENT_*` env var as a force-enable override.
        let execution_features = self.settings.execution_features();
        // Captured before the agent takes ownership, for the opt-in advisory
        // adversarial goal verifier (§2.2). `None` when the feature is off, so
        // the default build pays nothing.
        let adversarial_bits = (execution_features.adversarial_verify
            || crate::verification_panel::adversarial_verify_enabled())
        .then(|| (client.clone(), model.clone(), prompt_for_model.to_string()));
        let mut agent = ModelAgent::new(client, model, system_prompt, final_user_prompt, tools)
            .with_thinking_depth(effective_thinking_depth)
            .with_fallback_model(fallback_model)
            .with_reactive_compactor(reactive_compactor)
            .with_history(history)
            .with_response_history(response_history)
            .with_proactive_compaction(proactive_threshold)
            .with_prefire(prefire_start)
            .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME)
            .with_events(sink.clone());

        // Background relevant-memory prefetch (§3.2): supplement the seeded
        // passive knowledge block with an off-critical-path retrieval that
        // surfaces newly-relevant entries as the conversation evolves. Only
        // when a knowledge base is configured; no-op otherwise.
        if let Some(knowledge) = &self.knowledge {
            agent = agent.with_relevant_memory_provider(Arc::new(
                crate::knowledge_service::KnowledgeMemoryProvider::new(knowledge.clone()),
            ));
        }

        // Periodic on-plan todo reminder (§3.1, Claude Code
        // getTodoReminderAttachments): after a stretch of todo inactivity, nudge
        // the model to track/clean up its plan so long runs don't drift. Reads
        // the same session TodoStore the tools write to.
        agent = agent.with_todo_reminder_source(Arc::new(
            crate::todo_snapshot_reminder::TodoReminderAdapter::new(todo_store.clone()),
        ));

        // Stall/laziness detector (§2.3, Grok laziness_classifier): when a
        // final answer is emitted, a lightweight DeepSeek pass audits it for
        // false completion / premature stop and, on a confident stalled
        // verdict, injects one advisory nudge to re-enter the turn. Opt-in
        // (DEEPAGENT_STALL_DETECTOR) and fail-open — never blocks a run.
        if execution_features.stall_detector || crate::stall_classifier::stall_detector_enabled() {
            agent = agent.with_stall_classifier(Arc::new(
                crate::stall_classifier::ModelStallClassifier::new(stall_client, stall_model),
            ));
        }

        let verification_policy = self.settings.verification_policy().unwrap_or_default();

        let session_sequence = deepagent_persistence::event_store::EventStore::new(&self.db)
            .load_session(session.id())?
            .last()
            .map(|event| event.sequence as i64)
            .unwrap_or(0);

        let kernel_runtime = build_kernel_runtime_config(KernelRuntimeConfigRequest {
            db: self.db.clone(),
            run_id: &run_id,
            session_sequence,
            root: &root,
            tool_results_dir: &self.tool_results_dir,
            plan: plan.clone(),
            todo_store: todo_store.clone(),
            verification_policy,
            fire_session_start: continue_session.is_none(),
            granted,
            nested_instructions: Some(Arc::new(
                crate::nested_instructions::NestedInstructionsDecorator::new(
                    root.clone(),
                    system_manifest.loaded_paths.iter().cloned(),
                    hooks.clone(),
                    session.id(),
                ),
            )),
        })?;
        let _ = subagent_parent_checkpoint.set(kernel_runtime.checkpoint.clone());
        let config = kernel_runtime.config;

        // Register a cancellation flag for this session so the UI can stop it.
        cancellation.add_alias(session_id.clone());
        let cancel = cancellation.flag();

        // Snapshot the current discovered set before the engine starts so we
        // can compute the delta after — only newly-discovered names get
        // appended to the event log (Phase 3C). The set may have been
        // pre-populated above from prior `ToolsDiscovered` events.
        let discovered_before_run: std::collections::HashSet<String> = tool_search_discovered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();

        // Run the loop through AgentKernel v2. The legacy RuntimeEngine branch
        // is no longer a production fallback for root chat runs; RuntimeEngine
        // remains available only as an internal execution primitive while the
        // kernel is being expanded.
        // Auto-discovered acceptance plan (Phase E; intent-layer cleanup):
        // discovery is purely structural — a recognized build system yields a
        // plan. The runtime loop runs it ONLY when the run actually created or
        // modified files (fact-based gate), so a pure question in a buildable
        // repo never triggers a build. Prompt text is never inspected.
        let verification_plan = crate::completion_plan::discover_verification_plan(&root);
        let (run_result, run_succeeded): (Result<()>, bool) = {
            let mut kernel = AgentKernel::<SystemClock>::new(
                self.db.clone(),
                &registry,
                Default::default(),
                config,
                run_id.clone(),
            )
            .with_events(sink.clone())
            .with_approvals(gate)
            .with_hooks(&hooks)
            .with_cancellation_flag(cancel);
            if let Some(plan) = verification_plan.as_ref() {
                kernel = kernel.with_verification(plan);
            }
            // Advisory adversarial goal verification (§2.2, opt-in
            // DEEPAGENT_ADVERSARIAL_VERIFY): after the fact-gate accepts, a
            // read-only skeptic panel judges whether the change met the goal;
            // a refuting majority feeds gaps back once (never hard-fails).
            if let Some((av_client, av_model, av_goal)) = adversarial_bits {
                let spawner = Arc::new(
                    crate::verification_panel::ModelSkepticSpawner::new(av_client, av_model)
                        .with_system_prompt(
                            crate::verification_panel::GOAL_COVERAGE_SKEPTIC_PROMPT,
                        ),
                );
                kernel = kernel.with_adversarial_verifier(Arc::new(
                    crate::verification_panel::ModelAdversarialVerifier::new(
                        spawner,
                        av_goal,
                        crate::verification_panel::DEFAULT_SKEPTIC_COUNT,
                    ),
                ));
            }
            match kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await
            {
                Ok(terminal) => {
                    let succeeded = terminal.succeeded();
                    (terminal.into_completion_result(), succeeded)
                }
                Err(error) => (Err(error), false),
            }
        };

        AppRunFinalizer::new(
            self.db.clone(),
            self.cost.clone(),
            self.knowledge.clone(),
            self.cancellations.clone(),
        )
        .finalize_after_kernel(
            &mut session,
            AppRunFinalizerRequest {
                session_id: &session_id,
                run_id: &run_id,
                discovered_before_run: &discovered_before_run,
                discovered_tools: &tool_search_discovered,
                usage: agent.cumulative_usage(),
                model_name: &model_name_for_cost,
                sink: sink.as_ref(),
                run_succeeded,
                capture_client,
                capture_model,
            },
        )?;

        // Drop everything holding a clone of the event-sink sender so the
        // channel closes and the pump task can finish; then await it to ensure
        // all events were delivered.
        drop(agent);
        drop(hooks);
        drop(sink);
        let _ = pump.await;

        run_result.map(|_| session_id)
    }
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

fn collect_agent_items(
    dir: &std::path::Path,
    plugin_name: Option<&str>,
    items: &mut Vec<SlashPanelItem>,
    count: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(def) = deepagent_prompts::AgentDef::parse(&content) {
            *count += 1;
            if items.len() < 30 {
                let label = match plugin_name {
                    Some(plugin) => format!("{plugin}:{}", def.name),
                    None => def.name,
                };
                items.push(SlashPanelItem::new(label).value(truncate_desc(&def.description, 90)));
            }
        }
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

/// Format a rule count with an optional inline listing (diagnostics helper).
#[allow(dead_code)]
fn format_rule_count(rules: &[String]) -> String {
    if rules.is_empty() {
        "0".to_string()
    } else {
        format!("{} ({})", rules.len(), rules.join(", "))
    }
}

/// Truncate a one-line description to at most `max` characters (on a char
/// boundary), collapsing internal newlines to spaces so it renders cleanly as
/// a single Markdown list item. Appends an ellipsis when truncated.
fn truncate_desc(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let head: String = flat.chars().take(max).collect();
        format!("{head}\u{2026}")
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
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
fn project_hook_paths(root: &Path) -> Vec<PathBuf> {
    // Lower-precedence Claude-compatible files are loaded first; native
    // DeepAgent files are appended last at the same project scope.
    vec![
        root.join(".claude").join("settings.json"),
        root.join(".claude").join("settings.local.json"),
        root.join(".deepagent").join("settings.json"),
        root.join(".deepagent").join("settings.local.json"),
        root.join(".deepagent").join("hooks.json"),
    ]
}

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
    use crate::hook_runtime::{
        build_hook_agent_registry, parse_model_hook_decision, render_model_hook_prompt,
        AppHookActionExecutor,
    };
    use crate::secret_store::MemorySecretStore;
    use crate::settings::SandboxMode;
    use deepagent_context::ContextSourceKind;
    use deepagent_hooks::{HookAction, HookActionType};
    use deepagent_models::transport::{EventSink, MockTransport, TransportRequest};

    /// A transport that answers model discovery (GET) AND a streamed chat (the
    /// agent's first turn) so a full run completes offline.
    fn chat_transport() -> Arc<dyn HttpTransport> {
        // The mock streams its `events` for `stream`, and returns `get_response`
        // for discovery. We only need streaming here (settings are seeded
        // separately), so build one that completes immediately.
        Arc::new(MockTransport::new([
            r#"{"type":"response.output_text.delta","delta":"Hello from the agent."}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
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
            sink.on_event(r#"{"type":"response.output_text.delta","delta":"dynamic reply"}"#)?;
            sink.on_event(r#"{"type":"response.completed","response":{"status":"completed"}}"#)?;
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

    #[test]
    fn model_hook_prompt_replaces_arguments_without_leaking_placeholder() {
        let action = HookAction {
            action_type: HookActionType::Prompt,
            prompt: "Review this lifecycle input: $ARGUMENTS".to_string(),
            ..HookAction::default()
        };
        let rendered = render_model_hook_prompt(
            &action,
            &serde_json::json!({"hook_event_name":"PreToolUse","tool_name":"shell"}),
        )
        .unwrap();
        assert!(!rendered.contains("$ARGUMENTS"));
        assert!(rendered.contains("PreToolUse"));
        assert!(rendered.contains("shell"));
    }

    #[test]
    fn model_hook_decision_is_strict_and_structured() {
        assert_eq!(
            parse_model_hook_decision(r#"{"ok":true}"#).unwrap(),
            HookOutcome::Continue
        );
        assert_eq!(
            parse_model_hook_decision(r#"{"ok":false,"reason":"blocked"}"#)
                .unwrap()
                .deny_reason(),
            Some("blocked")
        );
        assert!(parse_model_hook_decision("Looks fine").is_err());
    }

    #[tokio::test]
    async fn hook_agent_registry_contains_only_safe_non_recursive_tools() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        let (mut source, _) = chat
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Workspace,
                None,
                None,
                None,
                false,
            )
            .unwrap();
        source
            .register(Arc::new(deepagent_builtins::TaskTool::new(
                deepagent_builtins::UnavailableSubagentRunner,
                Vec::<String>::new(),
            )))
            .unwrap();
        let isolated = build_hook_agent_registry(&source).unwrap();
        assert!(isolated.get("task").is_none());
        assert!(isolated.get("shell").is_none());
        assert!(isolated
            .iter_specs()
            .all(|spec| spec.descriptor.risk == RiskLevel::Safe));
        assert!(
            !isolated.is_empty(),
            "read-only hook agent should retain safe tools"
        );
    }

    #[tokio::test]
    async fn prompt_hook_blocks_user_input_before_main_agent_turn() {
        let (db, settings, dir) = seeded().await;
        settings
            .set_hooks_json(
                r#"{
                    "hooks": {
                        "UserPromptSubmit": [{"hooks": [{
                            "type": "prompt",
                            "prompt": "Reject destructive requests: $ARGUMENTS",
                            "timeout": 5
                        }]}]
                    }
                }"#,
            )
            .unwrap();
        let transport = Arc::new(MockTransport::new([
            r#"{"type":"response.output_text.delta","delta":"{\"ok\":false,\"reason\":\"destructive request\"}"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
        ]));
        let chat = ChatService::new(db, settings, transport, dir.path());
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let event_sink = events.clone();
        chat.run(
            "delete everything",
            move |event| event_sink.lock().unwrap().push(event),
            |_| {},
        )
        .await
        .unwrap();

        assert!(events.lock().unwrap().iter().any(|event| {
            matches!(event, RuntimeEvent::RunCompleted { message } if message.contains("destructive request"))
        }));
    }

    fn hook_executor_with(
        transport: Arc<dyn HttpTransport>,
        mcp: Option<Arc<deepagent_mcp::McpRegistry>>,
    ) -> AppHookActionExecutor {
        let (sink, _rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        AppHookActionExecutor {
            client: Arc::new(ModelClient::new(
                transport,
                ModelConfig::deepseek("test-key"),
            )),
            model: "deepseek-v4-flash".to_string(),
            thinking_depth: ThinkingDepth::Simple,
            mcp,
            agent_registry: Arc::new(ToolRegistry::new()),
            events: Arc::downgrade(&sink),
        }
    }

    #[tokio::test]
    async fn mcp_hook_invokes_connected_tool_and_honors_decision() {
        let transport = deepagent_mcp::MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[{
                    "name":"check",
                    "description":"check policy",
                    "inputSchema":{"type":"object"}
                }]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({
                    "content":[{"type":"text","text":"{\"ok\":false,\"reason\":\"MCP policy denied\"}"}],
                    "isError":false
                }),
            );
        let mut registry = deepagent_mcp::McpRegistry::new();
        registry
            .register(
                "policy",
                Arc::new(deepagent_mcp::McpClient::new(Arc::new(transport))),
            )
            .await
            .unwrap();
        let executor = hook_executor_with(chat_transport(), Some(Arc::new(registry)));
        let action = HookAction {
            action_type: HookActionType::McpTool,
            command: "mcp__policy__check".to_string(),
            ..HookAction::default()
        };
        let outcome = executor
            .execute_mcp(&action, serde_json::json!({"hook_event_name":"PreToolUse"}))
            .await
            .unwrap();
        assert_eq!(outcome.deny_reason(), Some("MCP policy denied"));
    }

    #[tokio::test]
    async fn agent_hook_runs_isolated_runtime_and_honors_decision() {
        let transport = Arc::new(MockTransport::new([
            r#"{"type":"response.output_text.delta","delta":"{\"ok\":false,\"reason\":\"agent review denied\"}"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
        ]));
        let executor = hook_executor_with(transport, None);
        let action = HookAction {
            action_type: HookActionType::Agent,
            prompt: "Review: $ARGUMENTS".to_string(),
            timeout: Some(5),
            ..HookAction::default()
        };
        let outcome = executor
            .execute_agent(
                &action,
                serde_json::json!({"hook_event_name":"PreToolUse","tool_name":"shell"}),
            )
            .await
            .unwrap();
        assert_eq!(outcome.deny_reason(), Some("agent review denied"));
    }

    fn style_entry(
        name: &str,
        description: &str,
        prompt: &str,
        force_for_plugin: Option<bool>,
    ) -> crate::plugin_runtime::PluginOutputStyleEntry {
        crate::plugin_runtime::PluginOutputStyleEntry {
            plugin_id: "writer@personal".to_string(),
            plugin_name: "writer".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            force_for_plugin,
            source_path: None,
        }
    }

    #[test]
    fn plugin_output_styles_prompt_uses_forced_style() {
        let block = plugin_output_styles_prompt(&[
            style_entry(
                "writer:plain",
                "Plain style",
                "Use plain language.",
                Some(false),
            ),
            style_entry(
                "writer:release",
                "Release style",
                "Write crisp release notes.",
                Some(true),
            ),
        ])
        .unwrap();

        assert!(block.contains("writer:release"));
        assert!(block.contains("forced for this run"));
        assert!(block.contains("Write crisp release notes."));
        assert!(!block.contains("Use plain language."));
    }

    #[test]
    fn plugin_output_styles_prompt_lists_optional_styles() {
        let block = plugin_output_styles_prompt(&[style_entry(
            "writer:plain",
            "Plain style\nwith whitespace",
            "Use plain language.",
            None,
        )])
        .unwrap();

        assert!(block.contains("# Plugin output styles"));
        assert!(block.contains("`writer:plain`"));
        assert!(block.contains("Plain style with whitespace"));
        assert!(block.contains("Use plain language."));
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
        assert!(labels.iter().any(|l| l == "run_started"));
        assert!(labels.iter().any(|l| l == "context_usage"));
        assert!(labels.iter().any(|l| l == "model_request_started"));
        assert!(labels.iter().any(|l| l == "model_first_token"));
        assert!(labels.iter().any(|l| l == "model_request_completed"));
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
                false,
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
        let logs = Arc::new(
            deepagent_persistence::runtime_log_store::RuntimeLogStore::open_in_memory().unwrap(),
        );
        let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path())
            .with_runtime_logs(logs.clone());

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
                false,
                Some("test-run-preflight-abort"),
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
        let log_entries = logs.recent_for_session(&session_id, 100).unwrap();
        assert!(log_entries
            .iter()
            .any(|entry| entry.run_id.as_deref() == Some("test-run-preflight-abort")));
        assert!(log_entries
            .iter()
            .any(|entry| { entry.category == "runtime" && entry.event == "registry_ready" }));
        assert!(log_entries
            .iter()
            .any(|entry| { entry.category == "model" && entry.event == "content_delta_batch" }));
    }

    #[tokio::test]
    async fn run_with_external_hooks_returns_after_pump_drains() {
        let (_db, settings, dir) = seeded().await;
        settings
            .set_hooks_json(
                r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo ok","timeout":5}]}]}}"#,
            )
            .unwrap();
        let chat = ChatService::new(_db, settings, chat_transport(), dir.path());

        let session_id = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chat.run_in_session(
                "say hello",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                None,
                |_| {},
                |_| {},
            ),
        )
        .await
        .expect("run should not hang waiting for hook-held event sink")
        .unwrap();

        assert!(session_id.starts_with("ses_"));
        assert!(!chat.cancel_session(&session_id));
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
            false,
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
        assert_eq!(json["reasoning"]["effort"], "max");
        assert!(json.get("thinking").is_none());
        assert!(json.get("reasoning_effort").is_none());
        assert_eq!(json["max_output_tokens"], 32_768);
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
            r#"{"type":"response.output_text.delta","delta":"first reply"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
            r#"{"type":"response.output_text.delta","delta":"second reply"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
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
                false,
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
    fn system_manifest_tracks_dynamic_context_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = build_system_manifest(
            tmp.path(),
            SandboxMode::FullAccess,
            None,
            Some("# Plugin output style\nUse a terse style.".to_string()),
            Some("# Deferred tools\n- tool_search".to_string()),
            vec!["<system-reminder>\n<available-skills />\n</system-reminder>".to_string()],
        );
        let sources = manifest
            .entries
            .iter()
            .map(|entry| entry.source)
            .collect::<Vec<_>>();

        assert!(sources.contains(&ContextSourceKind::System));
        assert!(sources.contains(&ContextSourceKind::RuntimeEnvironment));
        assert!(sources.contains(&ContextSourceKind::PermissionContext));
        assert!(sources.contains(&ContextSourceKind::PluginContext));
        assert!(sources.contains(&ContextSourceKind::ToolCatalog));
        assert!(sources.contains(&ContextSourceKind::SkillCatalog));

        let rendered = manifest.render();
        assert!(rendered.contains("Current sandbox mode: **full-access**"));
        assert!(rendered.contains("# Plugin output style"));
        assert!(rendered.contains("# Deferred tools"));
        assert!(rendered.contains("<available-skills"));
        assert!(rendered.contains("Today's date:"));
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
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Full,
                None,
                None,
                None,
                false,
            )
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
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Full,
                None,
                None,
                None,
                false,
            )
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
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Full,
                None,
                None,
                None,
                false,
            )
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
    fn runtime_agent_definitions_include_local_and_plugin_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let local_agents = project.join(".deepagent").join("agents");
        std::fs::create_dir_all(&local_agents).unwrap();
        std::fs::write(
            local_agents.join("review.md"),
            "---\nname: review\ndescription: Review project code\ntools: Read, Grep\n---\nReview carefully.",
        )
        .unwrap();

        let plugin_agents = tmp.path().join("plugin-agents");
        std::fs::create_dir_all(&plugin_agents).unwrap();
        std::fs::write(
            plugin_agents.join("inspect.md"),
            "---\nname: inspect\ndescription: Inspect plugin-specific surfaces\n---\nInspect broadly.",
        )
        .unwrap();

        let mut projection = crate::plugin_runtime::PluginRuntimeProjection::default();
        projection
            .agent_roots
            .push(crate::plugin_runtime::PluginAgentRoot {
                plugin_id: "audit-pack@workspace".to_string(),
                plugin_name: "audit-pack".to_string(),
                path: plugin_agents,
            });

        let definitions = collect_runtime_agent_definitions([project], Some(&projection));
        let names: Vec<&str> = definitions
            .iter()
            .map(|agent| agent.type_name.as_str())
            .collect();
        // Local + plugin agents come first; the built-in explore/plan agents
        // are appended last (CC precedence: built-in < user/project).
        assert_eq!(
            names,
            vec!["review", "audit-pack:inspect", "explore", "plan"]
        );
        let advertised: Vec<deepagent_builtins::TaskAgentType> = definitions
            .iter()
            .map(RuntimeAgentDefinition::task_agent_type)
            .collect();
        assert_eq!(advertised[0].name, "review");
        assert_eq!(advertised[1].name, "audit-pack:inspect");
        assert_eq!(
            advertised[1].description,
            "Inspect plugin-specific surfaces"
        );
    }

    #[test]
    fn chat_service_syncs_plugin_runtime_after_install_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_roots = crate::plugin_loader::PluginRoots {
            session: Vec::new(),
            builtin: tmp.path().join("plugin-builtin"),
            workspace: None,
            personal: tmp.path().join("plugins").join("personal"),
            marketplace_cache: tmp.path().join("plugins").join("cache"),
            marketplaces: tmp.path().join("plugins").join("marketplaces"),
        };
        let plugins = Arc::new(crate::plugin_service::PluginService::new(
            plugin_roots,
            tmp.path().join("app-data"),
        ));
        let skills = Arc::new(std::sync::Mutex::new(
            crate::skills_service::SkillsService::open_v2(deepagent_skills::SkillsRoots {
                builtin: tmp.path().join("skills").join("builtin"),
                user: tmp.path().join("skills").join("user"),
                marketplace: tmp.path().join("skills").join("marketplace"),
                workspace: None,
            })
            .unwrap(),
        ));
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery_transport(),
            secrets,
        ));
        let chat = ChatService::new(db, settings, chat_transport(), tmp.path())
            .with_plugins(plugins.clone())
            .with_skills(skills.clone());

        let initial_projection = chat.sync_plugin_runtime().unwrap().unwrap();
        assert!(initial_projection.skill_roots.is_empty());
        assert!(!skills
            .lock()
            .unwrap()
            .manager()
            .registry()
            .contains("plugin-planning"));
        assert!(initial_projection.command_roots.is_empty());
        assert!(initial_projection.mcp_server_sources.is_empty());
        assert!(initial_projection.hook_definitions.is_empty());
        assert!(initial_projection.app_entries.is_empty());
        assert!(plugins.list_apps().unwrap().is_empty());
        assert!(initial_projection.output_styles.is_empty());

        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("chat-live");
        std::fs::create_dir_all(plugin_source.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_source.join("skills").join("plugin-planning")).unwrap();
        std::fs::create_dir_all(plugin_source.join("commands")).unwrap();
        std::fs::create_dir_all(plugin_source.join("scripts")).unwrap();
        std::fs::create_dir_all(plugin_source.join("output-styles")).unwrap();
        std::fs::write(
            plugin_source.join(".codex-plugin").join("plugin.json"),
            serde_json::json!({
                "name": "chat-live",
                "version": "0.1.0",
                "skills": "skills",
                "commands": "commands",
                "hooks": "hooks.json",
                "mcpServers": {
                    "hosted": {
                        "type": "http",
                        "url": "https://127.0.0.1:9/mcp",
                        "oauth_resource": "https://127.0.0.1:9/mcp"
                    }
                },
                "apps": ".app.json",
                "outputStyles": "output-styles"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            plugin_source
                .join("skills")
                .join("plugin-planning")
                .join("SKILL.md"),
            "---\nname: plugin-planning\ndescription: Plan with the freshly installed plugin\n---\nUse the live plugin skill.",
        )
        .unwrap();
        std::fs::write(
            plugin_source.join("commands").join("inspect.md"),
            "---\ndescription: Inspect live plugin state\n---\nInspect ${ARGUMENTS}",
        )
        .unwrap();
        std::fs::write(
            plugin_source.join("scripts").join("post-tool.ps1"),
            "exit 0\n",
        )
        .unwrap();
        std::fs::write(
            plugin_source.join("hooks.json"),
            serde_json::json!({
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "Write|Edit",
                            "hooks": [
                                { "type": "command", "command": "./scripts/post-tool.ps1" }
                            ]
                        }
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            plugin_source.join("output-styles").join("brief.md"),
            "# Brief live plugin style\n\nKeep marketplace plugin output brief.",
        )
        .unwrap();
        std::fs::write(
            plugin_source.join(".app.json"),
            serde_json::json!({
                "apps": [
                    {
                        "id": "chat-live-browser",
                        "title": "Chat Live Browser",
                        "description": "Open the freshly installed plugin app",
                        "placement": "right-sidebar",
                        "component": "builtin:browser",
                        "icon": "browser",
                        "category": "Developer Tools"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "chat-live",
                  "version": "0.1.0",
                  "description": "Chat runtime sync plugin",
                  "source": { "source": "local", "path": "./plugins/chat-live" }
                }
              ]
            }"#,
        )
        .unwrap();
        plugins
            .add_marketplace(crate::plugin_marketplace::AddPluginMarketplaceDto {
                name: Some("team".to_string()),
                source: marketplace_root.display().to_string(),
                git_ref: None,
                sparse_path: None,
            })
            .unwrap();
        let prepared = plugins
            .prepare_plugin_install("team", "chat-live", false)
            .unwrap();
        let installed = plugins.commit_plugin_install(&prepared.token).unwrap();
        assert_eq!(installed.id, "chat-live@team");

        let refreshed_projection = chat.sync_plugin_runtime().unwrap().unwrap();
        assert_eq!(refreshed_projection.skill_roots.len(), 1);
        assert!(refreshed_projection.skill_roots[0].ends_with("skills"));
        assert_eq!(refreshed_projection.command_roots.len(), 1);
        assert_eq!(
            refreshed_projection.command_roots[0].plugin_id,
            "chat-live@team"
        );
        assert!(refreshed_projection.command_roots[0]
            .path
            .ends_with("commands"));
        assert!(
            refreshed_projection
                .mcp_server_sources
                .values()
                .any(|source| source.plugin_id == "chat-live@team"
                    && source.declared_name == "hosted")
        );
        assert!(refreshed_projection
            .hook_definitions
            .hooks
            .get("PostToolUse")
            .into_iter()
            .flatten()
            .any(|group| group.matcher.as_deref() == Some("Write|Edit")));
        assert_eq!(refreshed_projection.output_styles.len(), 1);
        assert_eq!(
            refreshed_projection.output_styles[0].name,
            "chat-live:brief"
        );
        assert_eq!(refreshed_projection.app_entries.len(), 1);
        assert_eq!(
            refreshed_projection.app_entries[0].plugin_id,
            "chat-live@team"
        );
        assert_eq!(
            refreshed_projection.app_entries[0].component,
            "builtin:browser"
        );
        let renderable_apps = plugins.list_apps().unwrap();
        assert_eq!(renderable_apps.len(), 1);
        assert_eq!(renderable_apps[0].plugin_id, "chat-live@team");
        assert_eq!(renderable_apps[0].id, "chat-live-browser");
        let skills_guard = skills.lock().unwrap();
        assert_eq!(
            skills_guard.plugin_roots(),
            refreshed_projection.skill_roots.as_slice()
        );
        assert!(
            skills_guard
                .manager()
                .registry()
                .contains("plugin-planning"),
            "the same ChatService instance must see installed plugin skills without restart"
        );
    }

    #[test]
    fn builtin_explore_plan_agents_are_read_only_and_overridable() {
        // With no project/plugin agents, the built-in explore/plan are present.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("empty-project");
        std::fs::create_dir_all(&project).unwrap();
        let definitions = collect_runtime_agent_definitions([project.clone()], None);
        let by_type: std::collections::BTreeMap<&str, &RuntimeAgentDefinition> = definitions
            .iter()
            .map(|agent| (agent.type_name.as_str(), agent))
            .collect();
        let explore = by_type.get("explore").expect("built-in explore present");
        let plan = by_type.get("plan").expect("built-in plan present");
        assert_eq!(explore.source_label, "built-in");
        // Read-only allowlist: no write/edit/bash tools.
        for forbidden in ["write_file", "edit_file", "multi_edit", "bash", "task"] {
            assert!(
                !explore.def.tools.iter().any(|t| t == forbidden),
                "explore must not advertise {forbidden}"
            );
            assert!(
                !plan.def.tools.iter().any(|t| t == forbidden),
                "plan must not advertise {forbidden}"
            );
        }
        assert!(explore.def.tools.iter().any(|t| t == "read_file"));
        assert!(plan.def.tools.iter().any(|t| t == "todo_write"));

        // A project agent named `explore` overrides the built-in.
        let local_agents = project.join(".deepagent").join("agents");
        std::fs::create_dir_all(&local_agents).unwrap();
        std::fs::write(
            local_agents.join("explore.md"),
            "---\nname: explore\ndescription: Custom explore\ntools: Read\n---\nCustom.",
        )
        .unwrap();
        let overridden = collect_runtime_agent_definitions([project], None);
        let explore_defs: Vec<&RuntimeAgentDefinition> = overridden
            .iter()
            .filter(|a| a.type_name == "explore")
            .collect();
        assert_eq!(explore_defs.len(), 1, "no duplicate explore");
        assert_eq!(explore_defs[0].source_label, "project");
        assert_eq!(explore_defs[0].def.description, "Custom explore");
    }

    #[test]
    fn subagent_system_prompt_includes_selected_agent_body() {
        let tmp = tempfile::tempdir().unwrap();
        let def = deepagent_prompts::AgentDef::parse(
            "---\nname: inspect\ndescription: Inspect plugin-specific surfaces\ntools: Read, Grep\nmodel: inherit\n---\nUse the plugin inspection checklist.",
        )
        .unwrap();
        let agent = RuntimeAgentDefinition {
            type_name: "audit-pack:inspect".to_string(),
            source_label: "plugin:audit-pack".to_string(),
            def,
        };

        let prompt = subagent_system_prompt(tmp.path(), Some(&agent), "");
        assert!(prompt.contains("Agent type: audit-pack:inspect"));
        assert!(prompt.contains("plugin:audit-pack"));
        assert!(prompt.contains("Declared tools: Read, Grep"));
        assert!(prompt.contains("Use the plugin inspection checklist."));
        assert!(prompt.contains(&tmp.path().display().to_string()));

        let general = subagent_system_prompt(tmp.path(), None, "");
        assert!(!general.contains("# Sub-agent identity"));
        assert!(general.contains("# Sub-agent task"));
    }

    #[test]
    fn runtime_agent_tool_filter_maps_claude_tool_names() {
        let def = deepagent_prompts::AgentDef::parse(
            "---\nname: focused\ndescription: Focus on code reading\ntools: Read, Grep, TodoWrite\n---\nRead only.",
        )
        .unwrap();
        let agent = RuntimeAgentDefinition {
            type_name: "focused".to_string(),
            source_label: "project".to_string(),
            def,
        };
        let mut tools = ["read_file", "grep", "todo_write", "bash", "write_file"]
            .into_iter()
            .map(|name| ToolSchema::function(name, "", serde_json::json!({"type": "object"})))
            .collect::<Vec<_>>();

        apply_runtime_agent_tool_filter(&mut tools, Some(&agent));
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool.function.name.as_str())
            .collect();
        assert_eq!(names, vec!["read_file", "grep", "todo_write"]);
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
    use crate::settings::ApprovalPolicy;
    use deepagent_builtins::{register_guard_hooks_with_bash_full_access, WorkspaceRoot};
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
        let access = crate::run_environment::fs_access_for(sandbox);

        // Compose the BeforeToolUse guards exactly like run_in_session.
        let mut hooks = HookRegistry::new();
        register_guard_hooks_with_bash_full_access(
            &mut hooks,
            WorkspaceRoot::new(root).with_access(access),
            default_bash_allow(),
            matches!(policy, ApprovalPolicy::FullAccess),
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
        let gate = PolicyGate::new(policy, Arc::new(channel))
            .with_classifier(deepagent_builtins::SafetyClassifier::with_defaults());
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
        } else if decision == ApprovalDecision::Allow {
            Outcome::AutoAllow
        } else {
            Outcome::Denied
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
        // Safe shell inspection is auto-approved by the classifier, even when
        // it is not part of the conservative Bash(prefix:*) allow-list.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "bash",
                serde_json::json!({"command": "dir G:\\Code\\Kotlin_code\\demo"})
            )
            .await,
            Outcome::AutoAllow
        );
        // Risky shell still asks; AutoReview is not the same as FullAccess.
        assert_eq!(
            decide(
                p,
                SandboxMode::FullAccess,
                "bash",
                serde_json::json!({"command": "git push origin main"})
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
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Full,
                None,
                None,
                None,
                false,
            )
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
            .build_registry(
                dir.path(),
                deepagent_builtins::FsAccess::Full,
                None,
                None,
                None,
                false,
            )
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

    #[test]
    fn reactive_compaction_keeps_tool_request_with_leading_results() {
        let mut assistant = Message::assistant("");
        assistant.tool_calls = vec![
            ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "one"}),
            },
            ToolCall {
                id: "c2".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "two"}),
            },
        ];
        let messages = vec![
            Message::user("old"),
            assistant,
            Message::tool_result("c1", "one"),
            Message::tool_result("c2", "two"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::user("u3"),
            Message::assistant("a3"),
        ];

        // Naive len-keep would split at index 2 (a tool result). The safe
        // boundary walks back to the assistant request at index 1.
        assert_eq!(pairing_safe_compaction_split(&messages, 8), Some(1));
        let rendered = render_message_for_compaction(&messages[1]);
        assert!(rendered.contains("name=read_file"));
        assert!(rendered.contains("\"path\":\"one\""));
    }
}
