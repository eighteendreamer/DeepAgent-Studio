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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
#[cfg(test)]
use deepagent_core::event::EventPayload;
#[cfg(test)]
use deepagent_core::message::{Message, ToolCall};
#[cfg(test)]
use deepagent_hooks::Hook;
use deepagent_models::transport::HttpTransport;
#[cfg(test)]
use deepagent_models::ToolSchema;
use deepagent_models::{ModelClient, ModelConfig, ModelRole, ThinkingDepth};
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_persistence::Database;
#[cfg(test)]
use deepagent_runtime::{ChannelSink, RuntimeEventSink};
use deepagent_runtime::{InputLeaseRegistry, RuntimeEvent};
#[cfg(test)]
use deepagent_tools::RiskLevel;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::approval_bridge::PendingApprovals;
use crate::context_runtime::RemoteContextFactory;
#[cfg(test)]
use crate::context_runtime::{
    collect_invoked_skill_ids_from_events, collect_invoked_skill_records_from_events,
    invoked_skills_reminder, pairing_safe_compaction_split, plugin_output_styles_prompt,
    render_message_for_compaction,
};
use crate::dto::{ApprovalRequestDto, PreflightToolCallDto};
#[cfg(test)]
use crate::hook_assembly::OfficeSkillGuardHook;
#[cfg(test)]
use crate::input_runtime::{collect_discovered_tools_from_events, conversation_from_events};
use crate::model_runtime::build_model_client;
use crate::office_service::OfficeService;
use crate::project_map_service::ProjectMapService;
use crate::run_assembler::RunAssembler;
use crate::run_coordinator::RunCoordinator;
use crate::runtime_event_log::append_runtime_log;
use crate::settings::SettingsService;
#[cfg(test)]
use crate::subagent_runner::{
    apply_runtime_agent_tool_filter, collect_runtime_agent_definitions, subagent_system_prompt,
    RuntimeAgentDefinition,
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
    build_base_tool_registry, CommandExecutorFactory, RemoteOpsFactory,
    RuntimeCommandExecutor, ToolRegistryBuildRequest,
};

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
    coordinator: Arc<RunCoordinator>,
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

/// Per-session map of [`DiscoveredToolSet`]s, keyed by session id.
type DiscoveredToolsMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, DiscoveredToolSet>>>;

pub(crate) type InvokedSkillMap =
    Arc<std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>>;

/// Per-run machine-facing overrides supplied by CLI/app-server transports.
///
/// These values are merged through the existing run configuration overlay so
/// the desktop path and headless paths still share one permission/model
/// resolution implementation.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HarnessRunOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
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
            db: db.clone(),
            settings,
            transport,
            workspace,
            bash_allow: default_bash_allow(),
            coordinator: Arc::new(RunCoordinator::new(db.clone())),
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

    /// Return the shared database used by sessions, runs, settings, and
    /// approvals. Harness transports use this handle for lifecycle queries
    /// without creating a second persistence boundary.
    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    /// Clone this service with a different workspace root while preserving the
    /// shared runtime wiring and persistence handle.
    pub fn for_workspace(&self, workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        let mut cloned = self.clone();
        cloned.workspace = workspace.clone();
        cloned.tool_results_dir = workspace.join(".deepagent").join("tool_results");
        cloned
    }

    /// Request cancellation of an in-flight run by session id or diagnostic
    /// run id. Both keys point at the same flag while a run is active. Returns
    /// whether a matching in-flight run was found. The run stops at its next
    /// step boundary and ends as cancelled (partial transcript preserved).
    pub fn cancel_session(&self, session_id: &str) -> bool {
        let found = self
            .request_cancel(session_id)
            .map(|request| request.accepted)
            .unwrap_or(false);
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

    /// Durable cancellation API for transports that need to surface storage
    /// failures instead of collapsing them into a `not_found` boolean.
    pub fn request_cancel(
        &self,
        session_id: &str,
    ) -> Result<crate::run_coordinator::CancelRequest> {
        self.coordinator.request_cancel(session_id)
    }

    /// Read the shared coordinator's durable readiness projection.
    pub fn coordinator_readiness(&self) -> Result<crate::run_coordinator::CoordinatorReadiness> {
        self.coordinator.readiness()
    }

    /// Persist that a steering request created a replacement turn.
    pub fn record_continuation(
        &self,
        run_id: &str,
        replaces_turn_id: &str,
        new_turn_id: &str,
    ) -> Result<u64> {
        self.coordinator
            .record_continuation(run_id, replaces_turn_id, new_turn_id)
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
    #[cfg(test)]
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

    /// The shared pending-approvals registry. The UI calls
    /// [`PendingApprovals::resolve_approved`] on this to answer a dialog.
    pub fn pending_approvals(&self) -> PendingApprovals {
        self.coordinator.pending()
    }

    /// Resolve an approval through the durable control projection before
    /// waking the in-process gate. Repeating the same decision is idempotent.
    pub fn resolve_approval(
        &self,
        approval_id: &str,
        approved: bool,
        decided_by: &str,
    ) -> Result<bool> {
        self.coordinator
            .resolve_approval(approval_id, approved, decided_by)
    }

    pub fn resolve_approval_scoped(
        &self,
        approval_id: &str,
        approved: bool,
        scope: Option<&str>,
        decided_by: &str,
    ) -> Result<bool> {
        self.coordinator
            .resolve_approval_scoped(approval_id, approved, scope, decided_by)
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
    #[cfg(test)]
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

    /// List the same base built-in descriptors used by headless runs.
    ///
    /// Main-run-only additions such as `task` require a live sub-agent runner
    /// and are intentionally not fabricated for discovery. MCP/plugin tools
    /// remain owned by their existing services and are added by the run path.
    pub fn tool_descriptors(&self) -> Result<Vec<deepagent_tools::ToolDescriptor>> {
        let profile = self.settings.effective_permission_profile()?;
        let (registry, _) = self.build_registry(
            &self.workspace,
            crate::run_environment::fs_access_for(profile.sandbox_mode),
            None,
            None,
            Some(profile.local_execution_mode),
            matches!(
                profile.approval_policy,
                crate::settings::ApprovalPolicy::FullAccess
            ),
        )?;
        Ok(registry.visible_to(&PermissionSet::developer()))
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
        crate::session_title::generate_session_title(
            &self.db,
            &self.settings,
            self.transport.clone(),
            session_id,
        )
        .await
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
        self.run_in_session_with_overrides(
            prompt,
            continue_session,
            env_mode,
            connection_id,
            preflight_tools,
            preflight_abort_message,
            initial_plan_mode,
            diagnostic_run_id,
            HarnessRunOverrides::default(),
            on_event,
            on_approval,
        )
        .await
    }

    /// Headless/app-server entrypoint that uses the same runtime as Desktop
    /// while allowing transport-scoped model and permission overrides.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_in_session_with_overrides<F, A>(
        &self,
        prompt: &str,
        continue_session: Option<&str>,
        env_mode: Option<&str>,
        connection_id: Option<&str>,
        preflight_tools: Vec<PreflightToolCallDto>,
        preflight_abort_message: Option<String>,
        initial_plan_mode: bool,
        diagnostic_run_id: Option<&str>,
        overrides: HarnessRunOverrides,
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let assembler = self.run_assembler();
        assembler
            .run(
                prompt,
                continue_session,
                env_mode,
                connection_id,
                preflight_tools,
                preflight_abort_message,
                initial_plan_mode,
                diagnostic_run_id,
                overrides,
                on_event,
                on_approval,
            )
            .await
    }

    /// Construct a [`RunAssembler`] borrowing every field the run pipeline
    /// needs. The assembler's [`RunAssembler::run`] method contains the full
    /// assembly logic; this service entry-point is a thin wrapper.
    fn run_assembler(&self) -> RunAssembler<'_> {
        RunAssembler {
            service: self,
            db: &self.db,
            settings: &self.settings,
            transport: &self.transport,
            workspace: &self.workspace,
            coordinator: &self.coordinator,
            bash_allow: &self.bash_allow,
            input_leases: &self.input_leases,
            plan_modes: &self.plan_modes,
            subagent_controls: &self.subagent_controls,
            tool_results_dir: &self.tool_results_dir,
            runtime_logs: &self.runtime_logs,
            cost: &self.cost,
            knowledge: &self.knowledge,
            skills: &self.skills,
            mcp: &self.mcp,
            plugins: &self.plugins,
            projects: &self.projects,
            office: &self.office,
            project_map: &self.project_map,
            skill_catalog_state: &self.skill_catalog_state,
            discovered_tools: &self.discovered_tools,
            invoked_skills: &self.invoked_skills,
            sandboxie_executor: &self.sandboxie_executor,
            local_command_executor: &self.local_command_executor,
            executor_factory: &self.executor_factory,
            runtime_broker: &self.runtime_broker,
            remote_context_factory: &self.remote_context_factory,
            remote_ops_factory: &self.remote_ops_factory,
        }
    }
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
    use std::str::FromStr;

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
            "Encrypted secret record",
            "The DeepSeek API key is stored as authenticated ciphertext in SQLite.",
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
            "Encrypted secret record",
            "The DeepSeek API key is stored as authenticated ciphertext in SQLite.",
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
            HookOutcome::Continue
            | HookOutcome::Modify { .. }
            | HookOutcome::AsyncPending { .. } => return Outcome::AutoAllow,
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
