//! Run assembly pipeline: extracts the mega-method that wires every ChatService
//! dependency into a single agent-kernel run.
//!
//! [`RunAssembler`] holds references to the [`ChatService`](crate::ChatService)
//! fields needed for one run. [`RunAssembler::run`] contains the sequential
//! assembly pipeline (normalize input → slash check → resolve environment →
//! select model → build tools → assemble hooks → build context → construct
//! agent → run kernel → finalize). The chat service entry-point is a thin
//! wrapper that constructs the assembler and delegates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deepagent_context::ContextPolicy;
#[cfg(test)]
use deepagent_context::{ModelCompactor, TaskSummary};
use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_core::message::{Message, ToolCall};
use deepagent_hooks::{Hook, HookDefinitions, HookPoint, HookRegistry};
use deepagent_models::transport::HttpTransport;
#[cfg(test)]
use deepagent_models::ModelClient;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_persistence::Database;
use deepagent_runtime::{
    tool_ui_metadata, Agent, AgentKernel, ChannelSink, InputLeaseRegistry, InputMode, ModelAgent,
    PromptDecision, ReactiveContextCompactor, RunRequest, RuntimeEvent, RuntimeEventSink,
};
#[cfg(test)]
use deepagent_session::Session;
use deepagent_tools::PermissionSet;

use crate::approval_bridge::{ChannelApprovalGate, PolicyGate};
use crate::chat_service::{ChatService, HarnessRunOverrides};
use crate::context_runtime::{
    build_run_context, collect_invoked_skill_ids_from_events, HookedReactiveContextCompactor,
    RemoteContextFactory, RunContextRequest,
};
use crate::dto::{ApprovalRequestDto, PreflightToolCallDto};
use crate::hook_assembly::{
    assemble_run_hooks, HookAssemblyRequest, OfficeSkillGuardHook, ProjectHookTrust,
};
use crate::input_runtime::accept_input_turn;
use crate::kernel_runtime::{build_kernel_runtime_config, KernelRuntimeConfigRequest};
use crate::model_runtime::select_run_model;
use crate::office_service::OfficeService;
use crate::project_map_service::ProjectMapService;
use crate::prompt_gate::{finalize_blocked_user_prompt, submit_user_prompt};
use crate::run_capabilities::{
    capability_context, CapabilityContextRequest, ResolvedRunCapabilities,
};
use crate::run_config::RunConfigOverlay;
use crate::run_coordinator::RunCoordinator;
use crate::run_environment::RunEnvironment;
use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
use crate::runtime_event_log::{append_runtime_log, spawn_runtime_event_pump};
use crate::settings::SettingsService;
use crate::skill_catalog_reminder::SkillCatalogSendState;
use crate::subagent_runner::ChatSubagentRunner;
use crate::tool_manifest::DiscoveredToolSet;
use crate::tool_runtime::{
    build_base_tool_registry, CommandExecutorFactory, RemoteOpsFactory, RuntimeCommandExecutor,
    ToolRegistryBuildRequest,
};

mod prepared_tools;
use prepared_tools::MainRunPreparationRequest;
mod execution_config;
mod prepared_chat;
mod session_end;
pub use prepared_chat::PreparedChatRun;
use prepared_chat::{PreparedChatContents, PreparedChatTools};

use crate::chat_service::InvokedSkillMap;
use crate::cost_service::CostService;
use crate::knowledge_service::KnowledgeService;
use crate::mcp_service::McpService;
use crate::plugin_service::PluginService;
use crate::project_service::ProjectService;
use crate::sandbox_backend::{
    DirectSandboxBackend, SandboxBackendCommandExecutor, SandboxBackendKind,
    SandboxBackendRegistry, SandboxNetworkPolicy,
};
use crate::sandboxie_service::SandboxieExecutor;
use crate::skills_service::SkillsService;
use crate::RuntimeBroker;

/// Map structured input attachments to reference-only message attachments.
fn message_attachments(
    attachments: &[deepagent_runtime::InputAttachment],
) -> Vec<deepagent_core::message::MessageAttachment> {
    use deepagent_core::message::MessageAttachment;
    use deepagent_runtime::InputAttachment;
    attachments
        .iter()
        .map(|att| match att {
            InputAttachment::Image {
                id,
                path,
                media_type,
                sha256,
            } => MessageAttachment {
                id: id.clone(),
                kind: "image".into(),
                media_type: Some(media_type.clone()),
                path: Some(path.to_string_lossy().into_owned()),
                sha256: sha256.clone(),
            },
            InputAttachment::File { id, path } => MessageAttachment {
                id: id.clone(),
                kind: "file".into(),
                sha256: None,
                media_type: None,
                path: Some(path.to_string_lossy().into_owned()),
            },
            InputAttachment::Text { id, .. } => MessageAttachment {
                id: id.clone(),
                kind: "text".into(),
                sha256: None,
                media_type: None,
                path: None,
            },
        })
        .collect()
}

fn todo_reminder_progress_from_events(
    events: &[deepagent_core::event::Event],
) -> Result<Option<(u64, u64)>> {
    for event in events.iter().rev() {
        if let EventPayload::TodoReminderProgress {
            version,
            turns_since_todo_write,
            turns_since_todo_reminder,
        } = &event.payload
        {
            if *version != 1 {
                return Err(CoreError::invalid(format!(
                    "unsupported todo reminder progress version {version}"
                )));
            }
            return Ok(Some((*turns_since_todo_write, *turns_since_todo_reminder)));
        }
    }
    Ok(None)
}

/// Holds references to every [`ChatService`](crate::ChatService) field the run
/// pipeline touches. Constructed by [`ChatService::run_assembler`] and consumed
/// by a single [`RunAssembler::run`] call.
pub(crate) struct RunAssembler<'a> {
    /// Borrowed host service — needed for `ChatSubagentRunner.host` and for
    /// helper methods that remain on `ChatService` (e.g. public surface used
    /// by other call-sites).
    pub(crate) service: &'a ChatService,
    pub(crate) db: &'a Arc<Database>,
    pub(crate) settings: &'a Arc<SettingsService>,
    pub(crate) transport: &'a Arc<dyn HttpTransport>,
    pub(crate) workspace: &'a Path,
    pub(crate) coordinator: &'a Arc<RunCoordinator>,
    pub(crate) bash_allow: &'a [String],
    pub(crate) input_leases: &'a Arc<InputLeaseRegistry>,
    pub(crate) subagent_controls: &'a Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pub(crate) tool_results_dir: &'a Path,
    pub(crate) runtime_logs: &'a Option<Arc<RuntimeLogStore>>,
    pub(crate) cost: &'a Option<Arc<CostService>>,
    /// Owned by this single-use assembler, then bound once to its run project.
    /// The host's UI-facing service is never retargeted by run preparation.
    pub(crate) knowledge: Option<Arc<KnowledgeService>>,
    pub(crate) cron: &'a Option<Arc<crate::CronService>>,
    pub(crate) canvas_model: &'a Option<Arc<crate::canvas_model_gateway::CanvasModelGateway>>,
    pub(crate) workflow_http: &'a Arc<dyn deepagent_builtins::http_request::HttpRequestClient>,
    pub(crate) skills: &'a Option<Arc<std::sync::Mutex<SkillsService>>>,
    pub(crate) mcp: &'a Option<Arc<McpService>>,
    pub(crate) plugins: &'a Option<Arc<PluginService>>,
    pub(crate) projects: &'a Option<Arc<ProjectService>>,
    pub(crate) office: &'a Option<Arc<OfficeService>>,
    pub(crate) project_map: &'a Option<Arc<ProjectMapService>>,
    pub(crate) skill_catalog_state:
        &'a Arc<std::sync::Mutex<HashMap<String, SkillCatalogSendState>>>,
    pub(crate) invoked_skills: &'a InvokedSkillMap,
    /// Host-scoped "one session-memory pass per session" scheduler, borrowed
    /// from `ChatService` so two turns cannot fold the same window twice.
    pub(crate) session_memory_passes: &'a crate::session_memory::SessionMemoryPasses,
    pub(crate) sandboxie_executor: &'a Option<Arc<SandboxieExecutor>>,
    pub(crate) sandbox_backend_registry: &'a Option<Arc<SandboxBackendRegistry>>,
    pub(crate) local_command_executor:
        &'a Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>,
    pub(crate) executor_factory: &'a Option<CommandExecutorFactory>,
    pub(crate) runtime_broker: &'a Option<Arc<RuntimeBroker>>,
    pub(crate) remote_context_factory: &'a Option<RemoteContextFactory>,
    pub(crate) remote_ops_factory: &'a Option<RemoteOpsFactory>,
    /// File/image attachments supplied with this turn (structured refs). Empty
    /// for runs without attachments and for the headless/test paths.
    pub(crate) attachments: Vec<deepagent_runtime::InputAttachment>,
    pub(crate) prepared_run: Option<PreparedChatRun>,
}

// Chat and workflow share this admission boundary. Cleanup cancellation is
// not evidence of a cancelled outcome: the kernel may have committed Failed
// (for example after a durability barrier rejected an event).
pub(crate) fn finish_admitted_run(
    db: &Database,
    run_id: &str,
    session_id: String,
    cancellation: &deepagent_runtime::CancellationTree,
    result: Result<String>,
) -> Result<String> {
    let terminal = deepagent_runtime::KernelTerminal {
        kind: if matches!(&result, Err(CoreError::Persistence(_))) {
            deepagent_runtime::TerminalKind::Failed
        } else if cancellation.is_cancelled() {
            deepagent_runtime::TerminalKind::Cancelled
        } else if result.is_ok() {
            deepagent_runtime::TerminalKind::Succeeded
        } else {
            deepagent_runtime::TerminalKind::Failed
        },
        message: None,
        reason: result.as_ref().err().map(ToString::to_string),
    };
    let retain_error = |error: CoreError| match &result {
        Err(original) => CoreError::Persistence(format!(
            "{original}; outer run finalization also failed: {error}"
        )),
        Ok(_) => error,
    };
    deepagent_runtime::kernel::persist_terminal(db, run_id, &terminal, cancellation)
        .map_err(retain_error)?;
    let committed = deepagent_persistence::run_store::RunStore::new(db)
        .get(run_id)
        .map_err(retain_error)?;
    if committed
        .as_ref()
        .and_then(|run| run.terminal_kind.as_deref())
        == Some("cancelled")
    {
        Ok(session_id)
    } else {
        result
    }
}

impl<'a> RunAssembler<'a> {
    /// Keep the shared skill registry on the same immutable plugin generation
    /// that this run captured for tools, hooks and prompt projection. A plugin
    /// toggle invalidates the ready snapshot synchronously, while the
    /// background preparation callback may publish the empty/next roots later;
    /// synchronizing at the run boundary closes that stale-registry window.
    fn sync_plugin_skill_snapshot(
        &self,
        projection: Option<&crate::plugin_runtime::PluginRuntimeProjection>,
    ) -> Result<()> {
        let (Some(skills), Some(projection)) = (self.skills.as_ref(), projection) else {
            return Ok(());
        };
        let changed = skills
            .lock()
            .map_err(|_| CoreError::other("skills service mutex poisoned"))?
            .sync_plugin_roots(projection.skill_roots.clone())?;
        if changed {
            self.skill_catalog_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
        Ok(())
    }

    /// Attach this turn's structured attachments (builder style).
    pub(crate) fn with_attachments(
        mut self,
        attachments: Vec<deepagent_runtime::InputAttachment>,
    ) -> Self {
        self.attachments = attachments;
        self
    }
    /// Execute the full run assembly pipeline.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run<F, A>(
        mut self,
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
        let run_id = diagnostic_run_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("run_{}", deepagent_core::id::EventId::new()));
        let db = self.db.clone();
        let ending = crate::slash_runtime::parse_builtin_slash_command(prompt, continue_session)?
            .is_some_and(|result| matches!(result.action, deepagent_intent::SlashAction::End));
        let session_id = if let Some(id) = continue_session {
            SessionId::from_str(id)?.to_string()
        } else {
            let (_, project) = self.run_location(None)?;
            let mode = if env_mode == Some("remote") {
                deepagent_core::SessionMode::Remote
            } else {
                deepagent_core::SessionMode::Normal
            };
            deepagent_session::Session::create_in_project(
                &db,
                &SystemClock,
                None,
                mode,
                project.as_deref(),
            )?
            .id()
            .to_string()
        };
        let store = deepagent_persistence::run_store::RunStore::new(&db);
        if ending && self.prepared_run.is_none() && store.get(&run_id)?.is_none() {
            self.prepared_run = Some(self.service.admit_prepared_input(
                &run_id,
                &session_id,
                prompt,
                overrides.clone(),
            )?);
        }
        if let Some(record) = store.get(&run_id)? {
            if record.session_id != session_id || record.finished_at.is_some() {
                return Err(CoreError::invalid("run admission does not match session"));
            }
        } else {
            self.coordinator.admit(&run_id, &session_id)?;
        }
        let cancellation = self.coordinator.take_admission(&run_id)?;
        let result = self
            .run_inner(
                prompt,
                continue_session,
                env_mode,
                connection_id,
                preflight_tools,
                preflight_abort_message,
                initial_plan_mode,
                Some(&run_id),
                overrides,
                on_event,
                on_approval,
                &session_id,
                &cancellation,
            )
            .await;
        // The kernel may already have committed the terminal boundary. Setup
        // failures and cancellations use that same idempotent writer.
        finish_admitted_run(
            &db,
            &run_id,
            session_id,
            &cancellation.cancellation(),
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_inner<F, A>(
        mut self,
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
        bound_session_id: &str,
        cancellation: &crate::run_coordinator::ActiveRunRegistration,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before preparation"));
        }
        let mut prepared_run = self.prepared_run.take();
        if let Some(prepared) = &prepared_run {
            if Some(prepared.run_id.as_str()) != diagnostic_run_id
                || prepared.session_id != bound_session_id
            {
                return Err(CoreError::invalid(
                    "prepared chat identity does not match admission",
                ));
            }
        }
        let (root, session_project) = match &prepared_run {
            Some(prepared) => (prepared.root.clone(), prepared.session_project.clone()),
            None => self.run_location(continue_session)?,
        };
        self.knowledge = self
            .knowledge
            .as_ref()
            .map(|knowledge| knowledge.for_project(&root))
            .transpose()?;
        let normalized_input = deepagent_runtime::InputIngress::normalize(
            continue_session.map(ToOwned::to_owned),
            root.clone(),
            prompt,
            InputMode::Prompt,
            self.attachments.clone(),
        )?;
        let raw_prompt = prompt;
        let effective_input_text = normalized_input.effective_text.clone();
        let prompt = effective_input_text.as_str();

        if let Some(prepared) = &prepared_run {
            prepared.validate_input(prompt)?;
        }

        let mut prepared_run = match prepared_run.take() {
            Some(prepared) => match prepared.contents {
                PreparedChatContents::Control {
                    result,
                    capabilities,
                } => {
                    capabilities.consume(self.db.clone())?;
                    if matches!(result.action, deepagent_intent::SlashAction::End) {
                        return self
                            .execute_session_end(
                                bound_session_id,
                                diagnostic_run_id.ok_or_else(|| {
                                    CoreError::invalid("session end requires a run owner")
                                })?,
                                cancellation,
                                on_event,
                            )
                            .await;
                    }
                    return crate::slash_runtime::execute_builtin_slash_command(
                        &self.slash_runtime(),
                        prompt,
                        Some(bound_session_id),
                        result,
                        &on_event,
                    )
                    .await;
                }
                contents => Some(PreparedChatRun {
                    contents,
                    ..prepared
                }),
            },
            None => None,
        };

        if prepared_run.is_none() {
            if let Some(session_id) = self
                .maybe_handle_slash_command(prompt, continue_session, &on_event)
                .await?
            {
                return Ok(session_id);
            }
        }

        let run_id = diagnostic_run_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("run_{}", deepagent_core::id::EventId::new()));
        let run_trace = deepagent_tracing::trace_context::TraceContext::new_root();
        let run_trace_id = run_trace.trace_id.to_hex();
        let run_traceparent = run_trace.traceparent();
        append_runtime_log(
            self.runtime_logs,
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

        let (mut prepared_environment, prepared_tools) = match prepared_run.take() {
            Some(prepared) => {
                self.knowledge = prepared.knowledge;
                let PreparedChatContents::Model(model) = prepared.contents else {
                    return Err(CoreError::invalid("control escaped prepared dispatch"));
                };
                let model = *model;
                (model.environment, Some(model.tools))
            }
            None => (
                self.prepare_chat_environment(&root, &run_id, &overrides)?,
                None,
            ),
        };
        // The worker uses the admitted environment, never a second settings or
        // MCP lookup that could silently retarget the advertised registry.
        let environment = prepared_environment.configuration.environment.clone();
        let run_config = environment.config.clone();
        let effective_permission_rules =
            prepared_environment.configuration.permission_rules.clone();
        let project_trusted = prepared_environment.project_trusted;
        let plugin_projection = prepared_environment.plugin_projection.clone();
        let RunEnvironment {
            config: _,
            profile: _,
            policy,
            sandbox_mode,
            local_execution_mode,
            sandbox_backend: _,
            access,
        } = environment.clone();

        let mut model_prompt = self
            .dynamic_command_prompt(prompt)?
            .unwrap_or_else(|| prompt.to_string());
        let mut prompt_to_record = prompt.to_string();

        if let Some(cost) = self.cost {
            cost.check_budget()?;
        }

        let clock = SystemClock;
        let coordinator = self.coordinator;
        let runtime_logs_ref = self.runtime_logs.clone();
        let db_handle = self.db.clone();
        let accepted_turn = accept_input_turn(
            &db_handle,
            &clock,
            self.input_leases.clone(),
            self.runtime_logs.as_ref().map(Arc::clone),
            &run_id,
            Some(bound_session_id),
            env_mode,
            session_project.as_deref(),
            normalized_input.clone(),
            cancellation,
            |active_run| {
                let found = coordinator
                    .request_cancel(active_run)
                    .map(|request| request.accepted)
                    .unwrap_or(false);
                append_runtime_log(
                    &runtime_logs_ref,
                    NewRuntimeLogEntry::info("cancel", "cancel_requested")
                        .with_session_id(active_run)
                        .with_source("deepagent-app-core::chat_service")
                        .with_message(if found {
                            "cancel flag set"
                        } else {
                            "cancel requested but no in-flight run found"
                        })
                        .with_data(serde_json::json!({ "found": found })),
                );
                found
            },
        )
        .await?;
        let mut session = accepted_turn.session;
        let history = accepted_turn.history;
        let response_history = accepted_turn.response_history;
        let prior_events = accepted_turn.prior_events;
        let session_id_str = accepted_turn.session_id;
        cancellation.add_alias(session_id_str.clone());
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled during preparation"));
        }

        let effective_env_mode = match continue_session {
            Some(_) => match session.state().mode {
                deepagent_core::SessionMode::Remote => Some("remote"),
                _ => None,
            },
            None => env_mode,
        };

        let plan = self.plan_mode_for_session(&session_id_str)?;
        if continue_session.is_none() && initial_plan_mode {
            plan.set(true)?;
        }
        // Reject unknown/corrupt recovery before issuing a model request.
        plan.is_active()?;

        let prepared = match prepared_tools {
            Some(prepared) => prepared,
            None => {
                let prepared = self.prepare_chat_tools(
                    &root,
                    session_project.clone(),
                    &run_id,
                    &session_id_str,
                    effective_env_mode,
                    connection_id,
                    &prior_events,
                    &mut prepared_environment,
                )?;
                deepagent_persistence::run_store::RunStore::new(self.db).bind_capability_snapshot(
                    &run_id,
                    &prepared.capabilities.snapshot,
                    deepagent_core::clock::Clock::now(&SystemClock).as_millis(),
                )?;
                prepared
            }
        };
        let PreparedChatTools {
            tools: prepared_tools,
            plan,
            discovered_before_run,
            discovered: tool_search_discovered,
            search_mode: tool_search_mode,
            search_threshold: tool_search_threshold,
            capabilities,
        } = prepared;
        let capabilities = Arc::new(capabilities.consume(self.db.clone())?);

        let run_model = prepared_environment
            .configuration
            .model_plan?
            .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?
            .bind(self.settings, self.transport.clone())?;
        let client = run_model.client;
        let model = run_model.model;
        let reasoning_effort = run_model.reasoning_effort;
        let fallback_model = run_model.fallback_model;
        let subagent_reasoning_effort = reasoning_effort;

        let (sink, rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        let call_owner = crate::call_usage::RunCallUsageOwner {
            db: self.db.clone(),
            cost: self.cost.clone(),
            session_id: session_id_str.clone(),
            run_id: run_id.clone(),
            events: Arc::downgrade(&sink),
        };
        let subagent_hooks: Arc<std::sync::OnceLock<Arc<HookRegistry>>> =
            Arc::new(std::sync::OnceLock::new());
        let subagent_parent_checkpoint: Arc<
            std::sync::OnceLock<Arc<deepagent_runtime::CheckpointManager>>,
        > = Arc::new(std::sync::OnceLock::new());
        // Parent-run approval gate, shared with sub-agents so a risky sub-agent
        // tool bubbles to the same UI approval channel instead of auto-denying.
        let subagent_approvals: Arc<std::sync::OnceLock<Arc<dyn deepagent_runtime::ApprovalGate>>> =
            Arc::new(std::sync::OnceLock::new());
        let pump = spawn_runtime_event_pump(
            rx,
            self.runtime_logs.clone(),
            run_id.clone(),
            session_id_str.clone(),
            on_event,
        );

        let task_runner = ChatSubagentRunner {
            db: self.db.clone(),
            parent_run_id: run_id.clone(),
            transcript_root: self.tool_results_dir.join("subagents"),
            client: client.clone(),
            usage_owner: Some(call_owner.clone()),
            model: model.clone(),
            reasoning_effort: subagent_reasoning_effort,
            registry: prepared_tools.sub_registry,
            root: root.clone(),
            assigned_worktree: None,
            tool_search_mode,
            tool_search_auto_threshold: tool_search_threshold,
            parent_discovered_snapshot: prepared_tools.parent_discovered_snapshot,
            agent_definitions: prepared_tools.agent_definitions,
            background: self.subagent_controls.clone(),
            events: Arc::downgrade(&sink),
            skills: self.skills.clone(),
            host: self.service.clone(),
            access,
            local_execution_mode,
            bash_full_access: matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
            hooks: subagent_hooks.clone(),
            parent_checkpoint: subagent_parent_checkpoint.clone(),
            parent_approvals: subagent_approvals.clone(),
        };
        prepared_tools.services.bind(task_runner)?;
        let toolset = prepared_tools.toolset;
        for lifecycle in &toolset.lifecycle {
            sink.emit(RuntimeEvent::McpLifecycle {
                server_id: lifecycle.server_id.clone(),
                status: lifecycle.status.clone(),
                transport: lifecycle.transport.clone(),
                config_hash: lifecycle.config_hash.clone(),
                tool_schema_hash: lifecycle.tool_schema_hash.clone(),
                startup_attempt: lifecycle.startup_attempt,
                degradation_code: lifecycle.degradation_code.clone(),
                reason: lifecycle.reason.clone(),
                tool_count: lifecycle.tool_count,
            });
        }
        let registry = toolset.registry;
        let skill_snapshot = toolset.skills;
        let todo_store = toolset.todo_store;
        let hook_mcp_registry = toolset.hook_mcp_registry;
        let tool_manifest = toolset.manifest;
        let mcp_instructions_block = toolset.mcp_instructions_block;
        // Snapshot the `(server, instructions)` pairs at run start so the
        // rendered delta and the persisted delta event always describe the same
        // run, even though MCP connections only change between runs.
        let mcp_instructions_entries = toolset.mcp_instructions_entries;
        let tools = tool_manifest.tools.clone();
        let granted = PermissionSet::developer();
        append_runtime_log(
            self.runtime_logs,
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

        let channel_gate =
            ChannelApprovalGate::new(self.coordinator.pending(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> = Arc::new(
            PolicyGate::new(policy, Arc::new(channel_gate))
                .with_classifier(deepagent_builtins::SafetyClassifier::with_defaults()),
        );
        let _ = subagent_approvals.set(gate.clone());

        let project_hook_trust =
            Arc::new(ProjectHookTrust::new(self.projects.clone(), root.clone()));
        let project_hooks = match self.project_hook_definitions(&root, &project_hook_trust) {
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
            .office_skill_guard_hook(&session_id_str, &prior_events, skill_snapshot.as_deref())?
            .map(|hook| Arc::new(hook) as Arc<dyn Hook>);
        let hooks = assemble_run_hooks(HookAssemblyRequest {
            settings: self.settings,
            run_config: &run_config,
            permission_rules: effective_permission_rules.clone(),
            project_hooks,
            plugin_projection: plugin_projection.as_ref(),
            root: &root,
            sink: sink.clone(),
            usage_owner: Some(call_owner.clone()),
            client: client.clone(),
            model: model.clone(),
            reasoning_effort,
            mcp: hook_mcp_registry,
            registry: &registry,
            permissions: granted.clone(),
            plan: plan.clone(),
            office_skill_guard,
            access,
            bash_allow: self.bash_allow.to_vec(),
            bash_full_access: matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
            is_trusted: project_trusted,
            project_hook_trust,
            runtime_environment: self
                .runtime_broker
                .as_ref()
                .map(|broker| broker.build_process_environment(Some(&root)))
                .unwrap_or_default(),
        })?
        .hooks;
        // The registry is run-owned, including nested instruction, compaction,
        // and child-lifecycle dispatchers which use the legacy dispatch entry.
        let hooks = Arc::new(hooks.as_ref().clone().with_execution_context(
            deepagent_tools::ToolExecutionContext::from_token_and_flag(
                cancellation.cancellation().token(),
                cancellation.flag(),
            ),
        ));
        let _ = subagent_hooks.set(hooks.clone());
        append_runtime_log(
            self.runtime_logs,
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

        // The Hook owner must observe cancellation and finish its bounded
        // resource cleanup; dropping it here would lose MCP notifications.
        let prompt_decision = submit_user_prompt(
            &registry,
            &hooks,
            session.id(),
            model_prompt.clone(),
            cancellation.flag(),
        )
        .await?;
        let mut prompt_context = None;
        if let PromptDecision::AcceptWithContext {
            contexts,
            truncated,
            ..
        } = &prompt_decision
        {
            prompt_context = Some(crate::prompt_gate::prompt_context_item(
                contexts, *truncated,
            )?);
        }
        match prompt_decision {
            PromptDecision::Accept(effective_prompt)
            | PromptDecision::AcceptWithContext {
                prompt: effective_prompt,
                ..
            } => {
                append_runtime_log(
                    self.runtime_logs,
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
                    self.runtime_logs,
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
                    self.runtime_logs,
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
        let effective_reasoning_effort = reasoning_effort;
        let model_capability = client.capability(&model);
        let requested_output_tokens = client
            .prepare_response_request(deepagent_models::ResponseRequest::from_response_items(
                model.clone(),
                None,
                Vec::new(),
            ))
            .max_output_tokens
            .map(|tokens| tokens as usize);
        let context_policy = ContextPolicy::for_capability_with_output(
            &model_capability,
            effective_reasoning_effort,
            requested_output_tokens,
        );

        // The native model loop owns all context pressure decisions after
        // instructions, user input, attachments and visible tools are assembled.
        let context_compacted = history.first().is_some_and(|message| {
            message
                .content
                .starts_with(crate::input_runtime::COMPACTED_SUMMARY_HEADER)
        });
        let session_id = session.id().to_string();
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before persisting user input"));
        }
        let admitted_attachments = message_attachments(&self.attachments);
        session.append_user_prompt(
            Message::user(&prompt_to_record).with_attachments(admitted_attachments.clone()),
            prompt_context.clone(),
        )?;
        let task = session.create_task(&prompt_to_record)?;
        deepagent_persistence::run_store::RunStore::new(self.db).bind_task(
            &run_id,
            &session_id,
            &task.to_string(),
        )?;
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before preflight results"));
        }
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
                content: Vec::new(),
                call_id: tool.call_id.clone(),
                ok: tool.ok,
                output: tool.output.clone(),
                duration_ms: tool.duration_ms,
            })?;
            let completed_meta = tool_ui_metadata(&tool.name, &tool.arguments, Some(&tool.output));
            sink.emit(RuntimeEvent::ToolCompleted {
                content: Vec::new(),
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
            let result = deepagent_runtime::kernel::persist_terminal_and_notify(
                self.db,
                &run_id,
                &deepagent_runtime::KernelTerminal {
                    kind: deepagent_runtime::TerminalKind::Succeeded,
                    message: Some(abort_message),
                    reason: None,
                },
                &cancellation.cancellation(),
                None,
                sink.as_ref(),
            );
            drop(hooks);
            drop(sink);
            let _ = pump.await;
            return result.map(|_| session_id);
        }

        let run_context_request = RunContextRequest {
            root: &root,
            sandbox_mode,
            permission_rules: &effective_permission_rules,
            plugin_projection: plugin_projection.as_ref(),
            mcp_instructions_block: mcp_instructions_block.as_deref(),
            mcp_instructions_entries: Some(&mcp_instructions_entries),
            tool_manifest: &tool_manifest,
            skills: skill_snapshot.as_deref(),
            settings: self.settings,
            skill_catalog_state: self.skill_catalog_state,
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
        };
        let run_context = {
            let cancel = cancellation.cancellation();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(CoreError::other("cancelled while assembling context")),
                result = build_run_context(run_context_request) => result?,
            }
        };
        let system_manifest = run_context.system_manifest;
        if !system_manifest.loaded_paths.is_empty() {
            let cancel = cancellation.cancellation();
            let context = deepagent_hooks::HookContext::new(
                session.id(),
                HookPoint::InstructionsLoaded,
                deepagent_hooks::HookData::Instructions {
                    paths: system_manifest
                        .loaded_paths
                        .iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect(),
                },
            );
            if let Err(error) = hooks
                .dispatch_with_context(
                    &context,
                    deepagent_tools::ToolExecutionContext::from_token_and_flag(
                        cancel.token(),
                        cancellation.flag(),
                    ),
                )
                .await
            {
                tracing::warn!(error = %error, "InstructionsLoaded hook failed");
            }
        }
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before model request"));
        }
        let system_prompt = run_context.system_prompt;
        let final_user_prompt = run_context.final_user_prompt;
        // The agent publishes the snapshot once its native Surface is ready,
        // then after every mutation and provider calibration.
        let context_usage = run_context.context_usage;

        let capture_client = client.clone();
        let capture_model = model.clone();
        let reactive_compactor: Arc<dyn ReactiveContextCompactor> = Arc::new(
            HookedReactiveContextCompactor::new(
                client.clone(),
                model.clone(),
                hooks.clone(),
                session.id(),
                self.db.clone(),
            )
            .with_call_usage_sink(call_owner.invocation(run_id.clone(), "compaction")),
        );
        let model_name_for_cost = model.clone();

        let autocompact_reserve = self.settings.autocompact_reserve_tokens();
        let autocompact_pct_override = std::env::var("DEEPAGENT_AUTOCOMPACT_PCT_OVERRIDE")
            .ok()
            .and_then(|raw| raw.trim().parse::<f32>().ok());
        let proactive_threshold = context_policy
            .autocompact_threshold_tokens(autocompact_reserve, autocompact_pct_override)
            as u64;
        let prefire_lead_percent = std::env::var("DEEPAGENT_PREFIRE_LEAD_PERCENT")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(10)
            .min(100);
        let prefire_lead_tokens =
            context_policy.effective_context_window() as u64 * prefire_lead_percent / 100;
        let prefire_start = proactive_threshold.saturating_sub(prefire_lead_tokens);
        append_runtime_log(
            self.runtime_logs,
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

        let stall_client = client.clone();
        let stall_model = model.clone();
        let execution_features = self.settings.execution_features();
        let adversarial_bits = (execution_features.adversarial_verify
            || crate::verification_panel::adversarial_verify_enabled())
        .then(|| (client.clone(), model.clone(), prompt_for_model.to_string()));
        let main_usage = call_owner.main_loop();
        let mut agent = ModelAgent::new_with_attachments(
            client,
            model,
            system_prompt,
            final_user_prompt,
            admitted_attachments,
            tools,
        )
        .with_call_usage_sink(main_usage.clone())
        .with_fallible_tool_schema_source(capabilities.clone().schema_source(
            crate::tool_manifest::live_tool_schema_source(
                registry.clone(),
                granted.clone(),
                tool_search_mode,
                tool_search_discovered.clone(),
                tool_search_threshold,
            ),
        ))
        .with_tool_schema_observer(capabilities.wire_observer())
        .with_reasoning_effort(effective_reasoning_effort)
        .with_fallback_model(fallback_model)
        .with_reactive_compactor(reactive_compactor)
        .with_history(history)
        .with_response_history(response_history)
        .with_admitted_input_context(prompt_context)
        .with_surface_persistence(Arc::new(crate::context_runtime::SessionSurfaceStore {
            db: self.db.clone(),
            session_id: session.id(),
        }))
        .with_proactive_compaction(proactive_threshold)
        .with_context_snapshot(context_usage)
        .with_context_policy(
            context_policy.clone(),
            autocompact_reserve,
            autocompact_pct_override,
        )
        .with_prefire(prefire_start)
        .with_attachment_resolver(Arc::new(crate::attachment_service::FileAttachmentResolver))
        .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME);

        if let Some((turns_since_todo_write, turns_since_todo_reminder)) =
            todo_reminder_progress_from_events(&prior_events)?
        {
            agent = agent
                .with_todo_reminder_progress(turns_since_todo_write, turns_since_todo_reminder);
        }

        if let Some(knowledge) = &self.knowledge {
            agent = agent.with_relevant_memory_provider(Arc::new(
                crate::knowledge_service::KnowledgeMemoryProvider::new(knowledge.clone()),
            ));
            // with_response_history already restored intact memory attachments
            // through the shared compaction/snip projection. Historical IDs are
            // telemetry only; they cannot prove that a body remains visible.
        }

        agent = agent.with_todo_reminder_source(Arc::new(
            crate::todo_snapshot_reminder::TodoReminderAdapter::new(todo_store.clone()),
        ));

        if execution_features.stall_detector || crate::stall_classifier::stall_detector_enabled() {
            agent = agent.with_stall_classifier(Arc::new(
                crate::stall_classifier::ModelStallClassifier::new(stall_client, stall_model)
                    .with_call_usage(call_owner.clone()),
            ));
        }

        let verification_policy = self.settings.verification_policy().unwrap_or_default();

        let session_sequence = deepagent_persistence::event_store::EventStore::new(self.db)
            .load_session(session.id())?
            .last()
            .map(|event| event.sequence as i64)
            .unwrap_or(0);

        let kernel_runtime = build_kernel_runtime_config(KernelRuntimeConfigRequest {
            db: self.db.clone(),
            run_id: &run_id,
            session_sequence,
            root: &root,
            tool_results_dir: self.tool_results_dir,
            plan: plan.clone(),
            todo_store: todo_store.clone(),
            verification_policy,
            fire_session_start: continue_session.is_none(),
            granted,
            verification_context: crate::verification_commands::VerificationCommandContext {
                registry: Arc::new(registry.clone()),
                session_id: session.id(),
                hooks: hooks.clone(),
                approvals: gate.clone(),
                cancellation: cancellation.cancellation(),
            },
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

        cancellation.add_alias(session_id.clone());
        let verification_commands = kernel_runtime.verification_commands;
        let verification_plan = crate::completion_plan::discover_verification_plan(
            &root,
            verification_commands.clone(),
        );
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
            .with_cancellation(cancellation.cancellation());
            verification_commands.bind_events(kernel.event_sink())?;
            if let Some(plan) = verification_plan.as_ref() {
                kernel = kernel.with_verification(plan);
            }
            if let Some((av_client, av_model, av_goal)) = adversarial_bits {
                let spawner = Arc::new(
                    crate::verification_panel::ModelSkepticSpawner::new(av_client, av_model)
                        .with_call_usage(call_owner.clone())
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

        // Persist model-invoked history snips so a restart reconstructs the same
        // (snipped) working history instead of resurrecting removed segments.
        let snipped_tags = agent.snipped_tags();
        if !snipped_tags.is_empty() {
            if let Err(error) = session.append(EventPayload::ContextSnipped { tags: snipped_tags })
            {
                tracing::warn!(%error, "failed to record ContextSnipped event");
            }
        }

        // Persist the MCP-instructions delta so a resumed session rebuilds which
        // servers already had their instructions surfaced (`mcpInstructionsDelta`
        // parity). `added` are exactly the servers rendered into this run's
        // prompt; `removed` retracts servers that announced before but are no
        // longer connected. Both `build_run_context` and this bound go through
        // the shared `mcp_instructions_delta` implementation.
        {
            let announced =
                crate::input_runtime::collect_announced_mcp_servers_from_events(&prior_events);
            let (added, removed) =
                crate::input_runtime::mcp_instructions_delta(&mcp_instructions_entries, &announced);
            if !added.is_empty() || !removed.is_empty() {
                if let Err(error) =
                    session.append(EventPayload::McpInstructionsDelta { added, removed })
                {
                    tracing::warn!(%error, "failed to record McpInstructionsDelta event");
                }
            }
        }

        let model_usage = agent.model_usage();
        AppRunFinalizer::new(
            self.db.clone(),
            self.cost.clone(),
            self.knowledge.clone(),
            self.coordinator.cancellation_map(),
            self.session_memory_passes.clone(),
        )
        .finalize_after_kernel(
            &mut session,
            AppRunFinalizerRequest {
                session_id: &session_id,
                run_id: &run_id,
                discovered_before_run: &discovered_before_run,
                discovered_tools: &tool_search_discovered,
                usage: agent.cumulative_usage(),
                side_usage: agent.side_usage(),
                model_name: &model_name_for_cost,
                model_usage: Some(&model_usage),
                booked_calls: Some(main_usage.as_ref()),
                multi_model_label: "multi-model",
                sink: sink.as_ref(),
                run_succeeded,
                capture_client: Some(capture_client),
                capture_model: Some(capture_model),
            },
        )?;

        drop(agent);
        // The completion runner owns the run's hooks/event sender. Release it
        // before draining the pump, just like the kernel/decorator above.
        drop(verification_plan);
        drop(verification_commands);
        drop(hooks);
        drop(sink);
        let _ = pump.await;

        run_result.map(|_| session_id)
    }

    /// Execute a professional-canvas workflow through the same kernel pipeline.
    ///
    /// Skips chat-specific setup (model selection, tool registry, system prompt,
    /// hooks) and constructs a [`WorkflowAgent`] instead of [`ModelAgent`]. The
    /// kernel, event sink, cancellation, persistence and finalizer are shared.
    pub(crate) async fn run_workflow<F, A>(
        self,
        workflow_request: deepagent_runtime::workflow::WorkflowRequest,
        run_id: String,
        session_id: String,
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let store = deepagent_persistence::run_store::RunStore::new(self.db);
        match store.get(&run_id)? {
            Some(record) if record.session_id == session_id && record.finished_at.is_none() => {}
            Some(_) => {
                return Err(CoreError::invalid(
                    "workflow admission does not match session",
                ))
            }
            None => self.coordinator.admit(&run_id, &session_id)?,
        }
        let cancellation = self.coordinator.take_admission(&run_id)?;
        let db = self.db.clone();
        let result = self
            .run_workflow_inner(
                workflow_request,
                run_id.clone(),
                session_id.clone(),
                on_event,
                on_approval,
                &cancellation,
            )
            .await;
        finish_admitted_run(
            &db,
            &run_id,
            session_id,
            &cancellation.cancellation(),
            result,
        )
    }

    async fn run_workflow_inner<F, A>(
        mut self,
        workflow_request: deepagent_runtime::workflow::WorkflowRequest,
        run_id: String,
        session_id: String,
        on_event: F,
        on_approval: A,
        cancellation: &crate::run_coordinator::ActiveRunRegistration,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before workflow preparation"));
        }
        // Validate the caller namespace even for local-only graphs without a
        // model gateway. A missing gateway must never turn invalid scope into default.
        let workspace_id =
            crate::canvas_scope::normalize_workspace_id(workflow_request.workspace_id.as_deref())?;
        let (root, session_project) = self.run_location(Some(&session_id))?;
        self.knowledge = self
            .knowledge
            .as_ref()
            .map(|knowledge| knowledge.for_project(&root))
            .transpose()?;

        // Workflow tools must use the same run-scoped environment and base
        // registry as a normal chat turn.  Constructing an empty registry here
        // made every tool name look unknown and also skipped the configured
        // filesystem/command boundaries.
        let legacy_sandboxie_executor = self
            .sandbox_backend_registry
            .is_none()
            .then_some(self.sandboxie_executor.as_ref())
            .flatten();
        let project_trusted =
            crate::trust_service::TrustService::new(self.db.clone()).is_trusted(&root);
        let run_config = RunConfigOverlay::load(&root, serde_json::json!({}));
        let RunEnvironment {
            config: workflow_config,
            policy,
            sandbox_mode,
            local_execution_mode,
            sandbox_backend,
            access,
            ..
        } = RunEnvironment::resolve(
            run_config,
            self.settings.effective_permission_profile()?,
            project_trusted,
            self.runtime_logs,
            legacy_sandboxie_executor,
            self.sandbox_backend_registry
                .as_ref()
                .map(|registry| registry.default_kind()),
            &run_id,
        )?;
        let run_local_executor =
            self.local_executor_for_run(&root, sandbox_mode, sandbox_backend)?;

        append_runtime_log(
            self.runtime_logs,
            NewRuntimeLogEntry::info("workflow", "workflow_run_requested")
                .with_run_id(&run_id)
                .with_source("deepagent-app-core::chat_service")
                .with_data(serde_json::json!({
                    "node_count": workflow_request.definition.nodes.len(),
                    "edge_count": workflow_request.definition.edges.len(),
                    "input_count": workflow_request.inputs.len(),
                })),
        );

        let clock = SystemClock;
        let normalized_input = deepagent_runtime::InputIngress::normalize(
            Some(session_id.clone()),
            root.clone(),
            format!(
                "[workflow] {} nodes, {} edges",
                workflow_request.definition.nodes.len(),
                workflow_request.definition.edges.len()
            ),
            deepagent_runtime::InputMode::Prompt,
            Vec::new(),
        )?;

        let accepted_turn = accept_input_turn(
            self.db,
            &clock,
            self.input_leases.clone(),
            self.runtime_logs.as_ref().map(Arc::clone),
            &run_id,
            Some(&session_id),
            None,
            session_project.as_deref(),
            normalized_input,
            cancellation,
            |active_run| {
                self.coordinator
                    .request_cancel(active_run)
                    .map(|request| request.accepted)
                    .unwrap_or(false)
            },
        )
        .await?;
        let mut session = accepted_turn.session;
        let session_id = accepted_turn.session_id;
        cancellation.add_alias(session_id.clone());
        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled during workflow preparation"));
        }

        let (sink, rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        let pump = spawn_runtime_event_pump(
            rx,
            self.runtime_logs.clone(),
            run_id.clone(),
            session_id.clone(),
            on_event,
        );

        let compiled = deepagent_runtime::workflow::compile(workflow_request.definition)?;
        let call_owner = crate::call_usage::RunCallUsageOwner {
            db: self.db.clone(),
            cost: self.cost.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            events: Arc::downgrade(&sink),
        };
        let main_usage = call_owner.main_loop();
        let publisher = deepagent_runtime::workflow::NodeEventPublisher::new(sink.clone());
        // Resolve the compiled input contract before binding any physical node
        // capabilities. The same fallible constructor protects direct callers.
        let mut agent = deepagent_runtime::workflow::WorkflowAgent::new(
            compiled,
            workflow_request.inputs,
            workflow_request.target_node_id,
            publisher,
        )?
        .with_cancel(cancellation.flag())
        .with_call_usage_sink(main_usage.clone());

        let run_model = select_run_model(
            self.settings,
            self.transport.clone(),
            ModelRole::Chat,
            ModelRole::Reasoner,
            None,
            None,
            None,
        )
        .ok();
        let wf_client = run_model.as_ref().map(|rm| rm.client.clone());
        let wf_model_name = run_model.as_ref().map(|rm| rm.model.clone());

        let mut tool_request = self.base_registry_request(
            &root,
            access,
            None,
            None,
            Some(local_execution_mode),
            sandbox_backend.is_some(),
            run_local_executor,
            matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
            &session_id,
        );
        tool_request.project = session_project.clone();
        let (mut tool_registry, todo_store) = build_base_tool_registry(tool_request)?;
        // A host broker is not an escape hatch from OS isolation, including the
        // legacy executor path that predates the backend registry. Use the
        // resolved execution mode, not an independent permission-preset mapping.
        let http_enabled = match sandbox_backend {
            Some(SandboxBackendKind::Direct) => true,
            Some(_) => false,
            None => {
                legacy_sandboxie_executor.is_none()
                    || matches!(
                        local_execution_mode,
                        crate::settings::LocalExecutionMode::Direct
                    )
            }
        };
        tool_registry.register(Arc::new(
            deepagent_builtins::http_request::HttpRequestTool::new(
                self.workflow_http.clone(),
                http_enabled,
            ),
        ))?;
        // This binds a capability, not permission to execute. SDK probing and
        // code dispatch stay inside the approved Kernel invocation. An
        // explicitly selected isolated backend is never replaced by the host.
        let code_backend = match (sandbox_backend, self.sandbox_backend_registry.as_ref()) {
            (Some(kind), Some(registry)) => Some(registry.resolve(Some(kind))?.1),
            (Some(SandboxBackendKind::Direct), None) => Some(Arc::new(DirectSandboxBackend::new())
                as Arc<dyn crate::sandbox_backend::SandboxBackend>),
            (None, _)
                if legacy_sandboxie_executor.is_none()
                    || matches!(
                        local_execution_mode,
                        crate::settings::LocalExecutionMode::Direct
                    ) =>
            {
                Some(Arc::new(DirectSandboxBackend::new())
                    as Arc<dyn crate::sandbox_backend::SandboxBackend>)
            }
            _ => None,
        };
        let code_gateway = Arc::new(crate::workflow_code::WorkflowCodeGateway::new(
            self.runtime_broker.clone(),
            code_backend,
        ));
        tool_registry.register(Arc::new(code_gateway.tool()))?;
        let canvas_run_gateway = self
            .canvas_model
            .as_ref()
            .map(|gateway| gateway.scoped_for_run(workspace_id.as_deref()))
            .transpose()?;
        if let Some(gateway) = canvas_run_gateway.as_ref() {
            tool_registry.register(Arc::new(gateway.document_read_tool()))?;
            // Provider-controlled image URLs are deferred by the gateway and
            // consumed only through the canonical kernel tool pipeline.  The
            // signed URL never enters the invocation arguments or events.
            tool_registry.register(Arc::new(gateway.image_fetch_tool()))?;
            tool_registry.register(Arc::new(gateway.audio_fetch_tool(http_enabled)))?;
            tool_registry.register(Arc::new(gateway.video_fetch_tool(http_enabled)))?;
        }
        append_runtime_log(self.runtime_logs, NewRuntimeLogEntry::info("network", "workflow_http_broker_bound")
            .with_run_id(&run_id)
            .with_source("deepagent-app-core::run_assembler")
            .with_data(serde_json::json!({"enabled":http_enabled,"sandbox_backend":sandbox_backend.map(|backend| backend.label()),"legacy_sandboxie":legacy_sandboxie_executor.is_some(),"local_execution_mode":format!("{local_execution_mode:?}"),"policy":"public_pinned_same_origin"})));
        let tool_registry = std::sync::Arc::new(tool_registry);

        agent = agent.with_code_bridge(code_gateway);

        if let (Some(client), Some(model_name)) = (wf_client, wf_model_name.clone()) {
            agent = agent.with_model(client, model_name);
        }

        if let Some(gateway) = canvas_run_gateway {
            agent = agent
                .with_canvas_bridge(gateway.clone())
                .with_artifact_validator(gateway);
        }

        if let Some(knowledge) = self.knowledge.clone() {
            agent = agent.with_knowledge_retriever(std::sync::Arc::new(
                crate::knowledge_service::WorkflowKnowledgeRetriever::new(knowledge),
            ));
        }

        let granted = PermissionSet::developer();
        agent = agent.with_tool_executor(std::sync::Arc::new(
            crate::tool_runtime::WorkflowToolExecutor::new(tool_registry.clone(), granted.clone()),
        ));

        let session_sequence = deepagent_persistence::event_store::EventStore::new(self.db)
            .load_session(session.id())?
            .last()
            .map(|event| event.sequence as i64)
            .unwrap_or(0);

        let channel_gate =
            ChannelApprovalGate::new(self.coordinator.pending(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> =
            Arc::new(PolicyGate::new(policy, Arc::new(channel_gate)));
        let mut hooks = deepagent_hooks::HookRegistry::new();
        let plan = self.plan_mode_for_session(&session_id)?;
        plan.is_active()?;
        hooks.register(
            deepagent_hooks::HookPoint::BeforeToolUse,
            Arc::new(deepagent_builtins::PlanModeHook::new(plan.clone())),
        );
        // Use the existing source merge and rule evaluator. A workflow URL is
        // input, not a grant that may bypass user/project/managed deny rules.
        let rules = workflow_config.merged_permission_rules(self.settings.permission_rules()?)?;
        hooks.register(
            deepagent_hooks::HookPoint::BeforeToolUse,
            Arc::new(deepagent_hooks::PermissionRulesHook::new(rules)),
        );
        let hooks = Arc::new(hooks);
        let kernel_runtime = build_kernel_runtime_config(KernelRuntimeConfigRequest {
            db: self.db.clone(),
            run_id: &run_id,
            session_sequence,
            root: &root,
            tool_results_dir: self.tool_results_dir,
            plan: plan.clone(),
            todo_store,
            verification_policy: crate::settings::VerificationPolicy::default(),
            fire_session_start: true,
            // Only the Agent's canonical deferred calls need the confined
            // code capability. The legacy generic tool-node adapter above
            // retains its original grant; do not broaden its WASM/tool access.
            granted: granted.with(deepagent_tools::Permission::Sandbox),
            verification_context: crate::verification_commands::VerificationCommandContext {
                registry: tool_registry.clone(),
                session_id: session.id(),
                hooks: hooks.clone(),
                approvals: gate.clone(),
                cancellation: cancellation.cancellation(),
            },
            nested_instructions: None,
        })?;
        let mut config = kernel_runtime.config;
        // Nodes need the original bounded HTTP value, not the model preview or
        // injected reminders. Persistence/events retain the normal result policy.
        config.retain_raw_tool_output = true;

        let verification_commands = kernel_runtime.verification_commands;

        if cancellation.cancellation().is_cancelled() {
            return Err(CoreError::other("cancelled before workflow execution"));
        }
        let empty_discovered = std::collections::HashSet::new();
        let empty_toolset: crate::tool_manifest::DiscoveredToolSet =
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let registry = tool_registry;

        let (run_result, run_succeeded): (Result<()>, bool) = {
            let kernel = AgentKernel::<SystemClock>::new(
                self.db.clone(),
                &registry,
                Default::default(),
                config,
                run_id.clone(),
            )
            .with_events(sink.clone())
            .with_approvals(gate)
            .with_hooks(&hooks)
            .with_cancellation(cancellation.cancellation());
            verification_commands.bind_events(kernel.event_sink())?;

            let task = session.create_task("[workflow run]")?;
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

        let finalizer_model_name = wf_model_name.clone();
        let model_usage = agent.model_usage();
        AppRunFinalizer::new(
            self.db.clone(),
            self.cost.clone(),
            self.knowledge.clone(),
            self.coordinator.cancellation_map(),
            self.session_memory_passes.clone(),
        )
        .finalize_after_kernel(
            &mut session,
            AppRunFinalizerRequest {
                session_id: &session_id,
                run_id: &run_id,
                discovered_before_run: &empty_discovered,
                discovered_tools: &empty_toolset,
                usage: agent.cumulative_usage(),
                side_usage: agent.side_usage(),
                model_name: finalizer_model_name.as_deref().unwrap_or("workflow"),
                model_usage: Some(&model_usage),
                booked_calls: Some(main_usage.as_ref()),
                multi_model_label: "workflow",
                sink: sink.as_ref(),
                run_succeeded,
                capture_client: run_model.as_ref().map(|rm| rm.client.clone()),
                capture_model: wf_model_name,
            },
        )?;

        drop(agent);
        drop(verification_commands);
        drop(hooks);
        drop(sink);
        let _ = pump.await;

        run_result.map(|_| session_id)
    }

    // ── Helper methods (moved from ChatService, used only in the run path) ──

    /// Existing sessions keep their persisted project even when the sidebar's
    /// active project has changed. New sessions follow the active selection.
    fn run_location(&self, continue_session: Option<&str>) -> Result<(PathBuf, Option<String>)> {
        resolve_run_location(
            self.db,
            self.projects.as_deref(),
            self.workspace,
            continue_session,
        )
    }

    /// Build a [`crate::slash_runtime::SlashRuntime`] from this assembler's
    /// borrowed fields.
    fn slash_runtime(&self) -> crate::slash_runtime::SlashRuntime<'_> {
        crate::slash_runtime::SlashRuntime {
            db: self.db,
            settings: self.settings,
            workspace: self.workspace,
            cost: self.cost.as_deref(),
            knowledge: self.knowledge.as_deref(),
            mcp: self.mcp.as_deref(),
            projects: self.projects.as_deref(),
            skills: self.skills.as_deref(),
            tool_results_dir: self.tool_results_dir,
            plugins: self.plugins.as_deref(),
        }
    }

    /// Delegate to [`crate::slash_runtime::maybe_handle_slash_command`].
    async fn maybe_handle_slash_command<F>(
        &self,
        prompt: &str,
        continue_session: Option<&str>,
        on_event: &F,
    ) -> Result<Option<String>>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
    {
        crate::slash_runtime::maybe_handle_slash_command(
            &self.slash_runtime(),
            prompt,
            continue_session,
            on_event,
        )
        .await
    }

    /// Delegate to [`crate::slash_runtime::dynamic_command_prompt`].
    fn dynamic_command_prompt(&self, prompt: &str) -> Result<Option<String>> {
        crate::slash_runtime::dynamic_command_prompt(&self.slash_runtime(), prompt)
    }

    /// Bind all consumers to the original session event source, never a cache.
    fn plan_mode_for_session(&self, session_id: &str) -> Result<deepagent_builtins::PlanMode> {
        deepagent_builtins::PlanMode::for_session(self.db.clone(), session_id)
    }

    /// Load the standalone hooks file. Inline settings are loaded only once,
    /// through RunConfigOverlay, preserving their configuration origin.
    fn project_hook_definitions(
        &self,
        root: &Path,
        trust: &ProjectHookTrust,
    ) -> Result<Option<HookDefinitions>> {
        let paths = project_hook_paths(root)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(None);
        }

        if !trust.is_trusted()? {
            tracing::info!(
                project = %root.display(),
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
        Ok(Some(defs))
    }

    /// Wire the office-skill guard hook when an office service is attached.
    fn office_skill_guard_hook(
        &self,
        session_id: &str,
        prior_events: &[deepagent_core::event::Event],
        skills: Option<&deepagent_skills::SkillRegistry>,
    ) -> Result<Option<OfficeSkillGuardHook>> {
        if self.office.is_none() {
            return Ok(None);
        }
        let Some(skills) = skills else {
            return Ok(None);
        };
        let enforce_skills: std::collections::HashSet<String> = {
            ["docx", "xlsx", "pptx", "pdf"]
                .into_iter()
                .filter(|id| skills.contains(id))
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

    /// Assemble the shared [`ToolRegistryBuildRequest`] from the assembler's
    /// borrowed fields.
    fn base_registry_request(
        &self,
        root: &'a Path,
        access: deepagent_builtins::FsAccess,
        env_mode: Option<&'a str>,
        connection_id: Option<&'a str>,
        local_exec_mode: Option<crate::settings::LocalExecutionMode>,
        force_sandbox_backend: bool,
        local_command_executor: Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>,
        bash_external_safety_gate: bool,
        session_id: &str,
    ) -> ToolRegistryBuildRequest<'a> {
        let local_command_executor = match (self.runtime_broker, local_command_executor) {
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
            db: self.db.clone(),
            root,
            project: Some(root.to_string_lossy().into_owned()),
            access,
            env_mode,
            connection_id,
            local_exec_mode,
            force_sandbox_backend,
            bash_external_safety_gate,
            bash_allow: self.bash_allow.to_vec(),
            settings: self.settings.clone(),
            executor_factory: self.executor_factory.clone(),
            local_command_executor,
            knowledge: self.knowledge.clone(),
            cron: self.cron.clone(),
            project_map: self.project_map.clone(),
            office: self.office.clone(),
            remote_ops_factory: self.remote_ops_factory.clone(),
            question_responder: self.service.question_responder(session_id),
        }
    }

    fn local_executor_for_run(
        &self,
        root: &Path,
        sandbox_mode: crate::settings::SandboxMode,
        sandbox_backend: Option<SandboxBackendKind>,
    ) -> Result<Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>> {
        let Some(kind) = sandbox_backend else {
            return Ok(self.local_command_executor.clone());
        };
        let Some(registry) = self.sandbox_backend_registry.as_ref() else {
            // `ChatService::new` remains usable for embedders that do not opt
            // into the backend registry.  Direct is the only unambiguous
            // legacy choice; an isolated backend must be rejected instead of
            // being silently replaced by the host executor.
            if kind == SandboxBackendKind::Direct {
                return Ok(Some(Arc::new(
                    SandboxBackendCommandExecutor::new(
                        Arc::new(DirectSandboxBackend::new()),
                        root,
                        sandbox_mode,
                    )
                    .with_network(SandboxNetworkPolicy::Disabled),
                )));
            }
            return Err(CoreError::invalid(format!(
                "sandbox backend '{}' requested but no backend registry is configured",
                kind.label()
            )));
        };
        let (_, backend) = registry.resolve(Some(kind))?;
        Ok(Some(Arc::new(
            SandboxBackendCommandExecutor::new(backend, root, sandbox_mode)
                .with_network(SandboxNetworkPolicy::Disabled),
        )))
    }
}

/// Resolve the workspace root + project a run belongs to. Shared by the run
/// itself and by [`ChatService::create_session`] so a session minted before the
/// run starts lands in the same project as the run that continues it.
pub(crate) fn resolve_run_location(
    db: &Database,
    projects: Option<&ProjectService>,
    workspace: &Path,
    continue_session: Option<&str>,
) -> Result<(PathBuf, Option<String>)> {
    if let Some(session_id) = continue_session {
        let id = SessionId::from_str(session_id)
            .map_err(|error| CoreError::invalid(format!("bad session id: {error}")))?;
        let record = EventStore::new(db)
            .get_session(id)?
            .ok_or_else(|| CoreError::not_found(format!("session '{session_id}'")))?;
        let root = record
            .project
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.to_path_buf());
        return Ok((root, record.project));
    }

    if let Some(projects) = projects {
        let active = projects.active()?.filter(|path| !path.trim().is_empty());
        let root = active
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.to_path_buf());
        return Ok((root, active));
    }
    let root = workspace.to_path_buf();
    Ok((root.clone(), Some(root.to_string_lossy().into_owned())))
}

/// Standalone hook file; settings hooks belong to the config overlay only.
pub(crate) fn project_hook_paths(root: &Path) -> Vec<PathBuf> {
    vec![root.join(".deepagent").join("hooks.json")]
}

/// The transcript a compaction must summarize, bundled so choosing its source
/// stays a short call.
#[cfg(test)]
pub(crate) struct CompactionTarget<'a> {
    pub(crate) events: &'a [deepagent_core::event::Event],
    pub(crate) history: &'a [Message],
    pub(crate) rendered: &'a [String],
    /// Where the count-based split would cut, used by the model summarizer.
    pub(crate) count_split: usize,
}

/// Choose what replaces the compacted prefix.
///
/// The rolling session note is tried first: it already exists, was written by a
/// pass that read exactly this transcript, and costs no model call. It is used
/// only where it provably covers the prefix being dropped
/// ([`crate::session_memory::compaction_from_note`]); otherwise this behaves as
/// before and asks the model to summarize.
#[cfg(test)]
async fn choose_compaction_summary(
    db: &Arc<Database>,
    session_id: deepagent_core::id::SessionId,
    target: &CompactionTarget<'_>,
    client: &Arc<ModelClient>,
    model: &str,
) -> CompactionChoice {
    let store = crate::session_memory::SessionMemoryStore::new(db.clone());
    let mut choice = match crate::session_memory::compaction_from_note(
        &store,
        session_id,
        target.events,
        target.history,
    ) {
        Some((split, note)) => {
            tracing::debug!(
                split,
                "compacting from the session note without a model call"
            );
            CompactionChoice {
                split,
                summary_block: format!(
                    "Session notes maintained during this conversation:\n{note}"
                ),
                strategy: deepagent_core::compaction_strategy::SESSION_MEMORY,
                usage: deepagent_models::Usage::default(),
            }
        }
        None => {
            let goal = target
                .history
                .first()
                .map(|message| message.content.clone())
                .unwrap_or_default();
            let (summary, usage) = ModelCompactor::new(client.clone(), model.to_string())
                .summarize_accounted(
                    &goal,
                    &TaskSummary::default(),
                    &target.rendered[..target.count_split],
                )
                .await;
            CompactionChoice {
                split: target.count_split,
                summary_block: summary.to_context_block(),
                strategy: deepagent_core::compaction_strategy::MODEL,
                usage,
            }
        }
    };
    // A summary is a lossy rewrite, so the block must also name the log the
    // detail came from — that is the model's only way back (§5.1).
    choice.summary_block = crate::input_runtime::with_transcript_pointer(
        choice.summary_block,
        &db.session_transcript_dir(session_id),
    );
    choice
}

/// What replaced the compacted prefix and what it cost.
#[cfg(test)]
pub(crate) struct CompactionChoice {
    pub(crate) split: usize,
    pub(crate) summary_block: String,
    pub(crate) strategy: &'static str,
    /// Tokens the summarizing call spent; all-zero when the session note carried
    /// the boundary, because that path makes no model call at all.
    pub(crate) usage: deepagent_models::Usage,
}

#[cfg(test)]
mod compaction_source_tests {
    use super::*;
    use crate::session_memory::testing::{scripted_client, unreachable_client, ScriptedTransport};
    use crate::session_memory::{SessionMemoryState, SessionMemoryStore};
    use deepagent_core::clock::{Clock, SystemClock};
    use deepagent_core::event::{Event, EventPayload, Sequence};
    use deepagent_core::id::EventId;

    fn message_event(sequence: Sequence, message: Message) -> Event {
        Event {
            id: EventId::new(),
            session_id: deepagent_core::id::SessionId::new(),
            sequence,
            timestamp: SystemClock.now(),
            payload: EventPayload::MessageAppended { message },
        }
    }

    fn transcript() -> (Vec<Event>, Vec<Message>) {
        let history = vec![
            Message::user("first decision: one vault"),
            Message::assistant("agreed"),
            Message::user("second decision: files hold bodies"),
        ];
        let events = vec![
            message_event(1, Message::user("first decision: one vault")),
            message_event(2, Message::assistant("agreed")),
            message_event(5, Message::user("second decision: files hold bodies")),
        ];
        (events, history)
    }

    #[tokio::test]
    async fn a_covered_note_compacts_without_calling_the_model() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = deepagent_core::id::SessionId::new();
        let (events, history) = transcript();
        let events = crate::session_memory::testing::persist_events(&db, session, &events);
        SessionMemoryStore::new(db.clone())
            .save(
                session,
                &SessionMemoryState {
                    summary: "- goal: one vault\n- files hold bodies".to_string(),
                    last_summarized_sequence: Some(2),
                    tool_cursor_sequence: Some(2),
                    tokens_at_last_extraction: 20_000,
                    initialized: true,
                    updated_at: 1,
                },
                &events[..=2],
            )
            .unwrap();

        // The client would fail loudly if consulted, so success proves the note
        // carried the summary.
        let rendered: Vec<String> = transcript_rendered(&history);
        let target = CompactionTarget {
            events: &events,
            history: &history,
            rendered: &rendered,
            count_split: 2,
        };
        let choice = choose_compaction_summary(
            &db,
            session,
            &target,
            &unreachable_client(),
            "deepseek-chat",
        )
        .await;

        assert_eq!(choice.strategy, "session_memory");
        assert_eq!(
            choice.usage,
            deepagent_models::Usage::default(),
            "a note-based boundary must not spend a model call"
        );
        assert_eq!(
            choice.split, 2,
            "the tail starts at the first uncovered entry"
        );
        assert!(
            choice.summary_block.contains("goal: one vault"),
            "{}",
            choice.summary_block
        );
        assert_names_the_transcript(&choice, &db, session);
    }

    #[tokio::test]
    async fn without_a_note_the_model_summarizer_stays_in_charge_and_reports_cost() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = deepagent_core::id::SessionId::new();
        let (events, history) = transcript();

        let rendered: Vec<String> = transcript_rendered(&history);
        let target = CompactionTarget {
            events: &events,
            history: &history,
            rendered: &rendered,
            count_split: 1,
        };
        let choice = choose_compaction_summary(
            &db,
            session,
            &target,
            &scripted_client(vec![ScriptedTransport::text(
                "{\"goal\":\"ship\",\"completed\":[\"wrote code\"]}",
            )]),
            "deepseek-chat",
        )
        .await;

        assert_eq!(choice.strategy, "model");
        assert_eq!(choice.split, 1, "the count-based split is preserved");
        // The tokens this call spent are what `maybe_compact_history` books.
        assert_eq!(choice.usage.prompt_tokens, 120);
        assert_eq!(choice.usage.completion_tokens, 18);
        assert_names_the_transcript(&choice, &db, session);
    }

    fn transcript_rendered(history: &[Message]) -> Vec<String> {
        history
            .iter()
            .map(|message| format!("{:?}: {}", message.role, message.content))
            .collect()
    }

    /// Either summary source must tell the model where the verbatim log lives.
    fn assert_names_the_transcript(
        choice: &CompactionChoice,
        db: &Database,
        session: deepagent_core::id::SessionId,
    ) {
        let dir = db.session_transcript_dir(session);
        assert!(
            choice.summary_block.contains(&dir.display().to_string()),
            "boundary summary lost the transcript pointer: {}",
            choice.summary_block
        );
    }
}

#[cfg(test)]
mod run_location_tests {
    use super::*;
    use deepagent_core::clock::FixedClock;

    #[test]
    fn existing_session_uses_its_own_project_not_the_active_project() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let projects = ProjectService::new(db.clone());
        projects.add_project("/work/first").unwrap();
        projects.add_project("/work/second").unwrap();
        let session = Session::create_in_project(
            &db,
            &clock,
            Some("first"),
            Default::default(),
            Some("/work/first"),
        )
        .unwrap();
        projects.set_active("/work/second").unwrap();

        assert_eq!(
            resolve_run_location(
                &db,
                Some(&projects),
                Path::new("/default"),
                Some(&session.id().to_string()),
            )
            .unwrap(),
            (
                PathBuf::from("/work/first"),
                Some("/work/first".to_string())
            )
        );
        assert_eq!(
            resolve_run_location(&db, Some(&projects), Path::new("/default"), None).unwrap(),
            (
                PathBuf::from("/work/second"),
                Some("/work/second".to_string())
            )
        );
    }

    #[test]
    fn projectless_session_stays_in_default_workspace_when_project_is_active() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let projects = ProjectService::new(db.clone());
        projects.add_project("/work/project").unwrap();
        let session =
            Session::create_in_project(&db, &clock, None, Default::default(), None).unwrap();
        projects.set_active("/work/project").unwrap();

        assert_eq!(
            resolve_run_location(
                &db,
                Some(&projects),
                Path::new("/default"),
                Some(&session.id().to_string()),
            )
            .unwrap(),
            (PathBuf::from("/default"), None)
        );
        projects.clear_active().unwrap();
        assert_eq!(
            resolve_run_location(&db, Some(&projects), Path::new("/default"), None).unwrap(),
            (PathBuf::from("/default"), None)
        );
    }
}

#[cfg(test)]
mod admission_finalization_tests {
    use super::*;
    use deepagent_persistence::run_store::RunStore;
    use deepagent_runtime::{CancellationTree, KernelTerminal, TerminalKind};

    #[test]
    fn cleanup_cancellation_does_not_replace_durable_failure_or_original_error() {
        for committed in [None, Some("failed"), Some("cancelled")] {
            let db = Database::open_in_memory().unwrap();
            let session = Session::create(&db, &SystemClock, None).unwrap();
            let session_id = session.id().to_string();
            let cancellation = CancellationTree::new();
            RunStore::new(&db)
                .create("fault", &session_id, None, 1)
                .unwrap();
            if let Some(kind) = committed {
                deepagent_runtime::kernel::persist_terminal(
                    &db,
                    "fault",
                    &KernelTerminal {
                        kind: if kind == "failed" {
                            TerminalKind::Failed
                        } else {
                            TerminalKind::Cancelled
                        },
                        message: None,
                        reason: Some("committed outcome".into()),
                    },
                    &cancellation,
                )
                .unwrap();
            }
            cancellation.cancel();
            let result = finish_admitted_run(
                &db,
                "fault",
                session_id.clone(),
                &cancellation,
                Err(CoreError::Persistence(
                    "original event write rejected".into(),
                )),
            );
            if committed == Some("cancelled") {
                assert_eq!(result.unwrap(), session_id);
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("original event write rejected"));
                assert_eq!(
                    RunStore::new(&db)
                        .get("fault")
                        .unwrap()
                        .unwrap()
                        .terminal_kind
                        .as_deref(),
                    Some("failed")
                );
            }
            assert_eq!(
                RunStore::new(&db)
                    .events_after("fault", None)
                    .unwrap()
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn outer_terminal_rejection_keeps_the_original_event_failure() {
        let db = Database::open_in_memory().unwrap();
        let session = Session::create(&db, &SystemClock, None).unwrap();
        let session_id = session.id().to_string();
        RunStore::new(&db)
            .create("fault", &session_id, None, 1)
            .unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_terminal BEFORE INSERT ON run_events
                WHEN NEW.event_type='run_terminal' BEGIN SELECT RAISE(ABORT, 'outer terminal fault'); END;").unwrap();
            Ok(())
        }).unwrap();
        let cancellation = CancellationTree::new();
        cancellation.cancel();
        let error = finish_admitted_run(
            &db,
            "fault",
            session_id,
            &cancellation,
            Err(CoreError::Persistence(
                "original event write rejected".into(),
            )),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("original event write rejected")
                && error.contains("outer terminal fault"),
            "{error}"
        );
        assert!(RunStore::new(&db)
            .get("fault")
            .unwrap()
            .unwrap()
            .finished_at
            .is_none());
    }
}

#[cfg(test)]
mod attachment_mapping_tests {
    use super::*;
    use deepagent_runtime::InputAttachment;

    #[test]
    fn maps_input_attachments_to_reference_only_message_attachments() {
        let mapped = message_attachments(&[
            InputAttachment::Image {
                sha256: Some("a".repeat(64)),
                id: "att_1".into(),
                path: PathBuf::from("/tmp/a.png"),
                media_type: "image/png".into(),
            },
            InputAttachment::Text {
                id: "t1".into(),
                content: "hello".into(),
            },
        ]);
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].kind, "image");
        assert_eq!(mapped[0].id, "att_1");
        assert_eq!(mapped[0].sha256.as_deref(), Some("a".repeat(64).as_str()));
        assert_eq!(mapped[0].media_type.as_deref(), Some("image/png"));
        assert!(mapped[0]
            .path
            .as_deref()
            .is_some_and(|p| p.ends_with("a.png")));
        assert_eq!(mapped[1].kind, "text");
        assert!(mapped[1].path.is_none());
    }
}
