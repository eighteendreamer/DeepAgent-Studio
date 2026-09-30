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

use deepagent_context::{
    CompactionPolicy, ContextPolicy, HeuristicTokenizer, ModelCompactor, TaskSummary, TokenCounter,
};
use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_core::message::{Message, ToolCall};
use deepagent_hooks::{Hook, HookDefinitions, HookPoint, HookRegistry};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelCapabilityResolver, ModelClient, ModelRole};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_persistence::Database;
use deepagent_runtime::{
    tool_ui_metadata, Agent, AgentKernel, ChannelSink, InputLeaseRegistry, InputMode, ModelAgent,
    PromptDecision, ReactiveContextCompactor, RunRequest, RuntimeEvent, RuntimeEventSink,
};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::approval_bridge::{ChannelApprovalGate, PolicyGate};
use crate::chat_service::{ChatService, HarnessRunOverrides};
use crate::context_runtime::{
    build_run_context, collect_invoked_skill_ids_from_events, HookedReactiveContextCompactor,
    RemoteContextFactory, RunContextRequest,
};
use crate::dto::{ApprovalRequestDto, PreflightToolCallDto};
use crate::hook_assembly::{assemble_run_hooks, HookAssemblyRequest, OfficeSkillGuardHook};
use crate::input_runtime::{accept_input_turn, collect_discovered_tools_from_events};
use crate::kernel_runtime::{build_kernel_runtime_config, KernelRuntimeConfigRequest};
use crate::model_runtime::select_run_model;
use crate::office_service::OfficeService;
use crate::plugin_runtime::PluginRuntimeProjection;
use crate::project_map_service::ProjectMapService;
use crate::prompt_gate::{finalize_blocked_user_prompt, submit_user_prompt};
use crate::run_coordinator::RunCoordinator;
use crate::run_environment::RunEnvironment;
use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
use crate::runtime_event_log::{append_runtime_log, spawn_runtime_event_pump};
use crate::settings::SettingsService;
use crate::skill_catalog_reminder::SkillCatalogSendState;
use crate::subagent_runner::{
    collect_runtime_agent_definitions, ChatSubagentRunner, RuntimeAgentDefinition,
};
use crate::tool_manifest::DiscoveredToolSet;
use crate::tool_runtime::{
    build_base_tool_registry, build_main_run_toolset, CommandExecutorFactory,
    MainRunToolsetRequest, RemoteOpsFactory, RuntimeCommandExecutor, ToolRegistryBuildRequest,
};

use crate::chat_service::InvokedSkillMap;
use crate::cost_service::CostService;
use crate::knowledge_service::KnowledgeService;
use crate::mcp_service::McpService;
use crate::plugin_service::PluginService;
use crate::project_service::ProjectService;
use crate::sandboxie_service::SandboxieExecutor;
use crate::skills_service::SkillsService;
use crate::RuntimeBroker;

/// Per-session map of [`DiscoveredToolSet`]s, keyed by session id.
type DiscoveredToolsMap = Arc<std::sync::Mutex<HashMap<String, DiscoveredToolSet>>>;

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
            } => MessageAttachment {
                id: id.clone(),
                kind: "image".into(),
                media_type: Some(media_type.clone()),
                path: Some(path.to_string_lossy().into_owned()),
            },
            InputAttachment::File { id, path } => MessageAttachment {
                id: id.clone(),
                kind: "file".into(),
                media_type: None,
                path: Some(path.to_string_lossy().into_owned()),
            },
            InputAttachment::Text { id, .. } => MessageAttachment {
                id: id.clone(),
                kind: "text".into(),
                media_type: None,
                path: None,
            },
        })
        .collect()
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
    pub(crate) plan_modes: &'a Arc<std::sync::Mutex<HashMap<String, deepagent_builtins::PlanMode>>>,
    pub(crate) subagent_controls: &'a Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pub(crate) tool_results_dir: &'a Path,
    pub(crate) runtime_logs: &'a Option<Arc<RuntimeLogStore>>,
    pub(crate) cost: &'a Option<Arc<CostService>>,
    pub(crate) knowledge: &'a Option<Arc<KnowledgeService>>,
    pub(crate) canvas_model: &'a Option<Arc<crate::canvas_model_gateway::CanvasModelGateway>>,
    pub(crate) skills: &'a Option<Arc<std::sync::Mutex<SkillsService>>>,
    pub(crate) mcp: &'a Option<Arc<McpService>>,
    pub(crate) plugins: &'a Option<Arc<PluginService>>,
    pub(crate) projects: &'a Option<Arc<ProjectService>>,
    pub(crate) office: &'a Option<Arc<OfficeService>>,
    pub(crate) project_map: &'a Option<Arc<ProjectMapService>>,
    pub(crate) skill_catalog_state:
        &'a Arc<std::sync::Mutex<HashMap<String, SkillCatalogSendState>>>,
    pub(crate) discovered_tools: &'a DiscoveredToolsMap,
    pub(crate) invoked_skills: &'a InvokedSkillMap,
    pub(crate) sandboxie_executor: &'a Option<Arc<SandboxieExecutor>>,
    pub(crate) local_command_executor:
        &'a Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>,
    pub(crate) executor_factory: &'a Option<CommandExecutorFactory>,
    pub(crate) runtime_broker: &'a Option<Arc<RuntimeBroker>>,
    pub(crate) remote_context_factory: &'a Option<RemoteContextFactory>,
    pub(crate) remote_ops_factory: &'a Option<RemoteOpsFactory>,
    /// File/image attachments supplied with this turn (structured refs). Empty
    /// for runs without attachments and for the headless/test paths.
    pub(crate) attachments: Vec<deepagent_runtime::InputAttachment>,
}

impl<'a> RunAssembler<'a> {
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
        self,
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
        let (root, session_project) = self.run_location(continue_session)?;
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

        if let Some(session_id) = self
            .maybe_handle_slash_command(prompt, continue_session, &on_event)
            .await?
        {
            return Ok(session_id);
        }

        let run_id = diagnostic_run_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("run_{}", deepagent_core::id::EventId::new()));
        let run_trace = deepagent_tracing::trace_context::TraceContext::new_root();
        let run_trace_id = run_trace.trace_id.to_hex();
        let run_traceparent = run_trace.traceparent();
        let cancellation = self.coordinator.register(run_id.clone(), continue_session);
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
            self.settings,
            self.runtime_logs,
            self.sandboxie_executor,
            &run_id,
            serde_json::to_value(&overrides)?,
        )?;

        let mut model_prompt = self
            .dynamic_command_prompt(prompt)?
            .unwrap_or_else(|| prompt.to_string());
        let mut prompt_to_record = prompt.to_string();

        if let Some(cost) = self.cost {
            cost.check_budget()?;
        }

        if let Some(knowledge) = self.knowledge {
            knowledge.activate_project(&root)?;
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
            continue_session,
            env_mode,
            session_project.as_deref(),
            normalized_input.clone(),
            cancellation.flag(),
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
        let _input_lease = accepted_turn.lease;

        let effective_env_mode = match continue_session {
            Some(_) => match session.state().mode {
                deepagent_core::SessionMode::Remote => Some("remote"),
                _ => None,
            },
            None => env_mode,
        };

        let run_model = select_run_model(
            self.settings,
            self.transport.clone(),
            ModelRole::Chat,
            ModelRole::Reasoner,
            run_config.provider_override(),
            run_config.model_override(),
            run_config.reasoning_effort_override(),
        )?;
        let client = run_model.client;
        let model = run_model.model;
        let thinking_depth = run_model.thinking_depth;
        let fallback_model = run_model.fallback_model;

        let plan = self.plan_mode_for_session(&session_id_str);
        if continue_session.is_none() && initial_plan_mode {
            plan.set(true);
        }

        let restored_discovered = collect_discovered_tools_from_events(&prior_events);
        if !restored_discovered.is_empty() {
            let set = self.discovered_tools_for_session(&session_id_str);
            let mut guard = set.lock().unwrap_or_else(|p| p.into_inner());
            for name in restored_discovered {
                guard.insert(name);
            }
        }

        let tool_search_mode = self.settings.tool_search_mode().unwrap_or_default();
        let tool_search_threshold = self
            .settings
            .tool_search_auto_threshold()
            .unwrap_or(SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS);
        let tool_search_discovered = self.discovered_tools_for_session(&session_id_str);
        let subagent_thinking_depth = thinking_depth;

        let (sink, rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
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

        let (task_runner, task_agent_types) = {
            let mut sub_request = self.base_registry_request(
                &root,
                access,
                None,
                None,
                Some(local_execution_mode),
                matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
            );
            sub_request.project = session_project.clone();
            let sub_registry = Arc::new(build_base_tool_registry(sub_request)?.0);
            let runtime_agents = collect_runtime_agent_definitions(
                [root.clone(), self.workspace.to_path_buf()],
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
                host: self.service.clone(),
                access,
                local_execution_mode,
                bash_full_access: matches!(policy, crate::settings::ApprovalPolicy::FullAccess),
                hooks: subagent_hooks.clone(),
                parent_checkpoint: subagent_parent_checkpoint.clone(),
                parent_approvals: subagent_approvals.clone(),
            };
            (runner, task_agent_types)
        };

        let mut base = self.base_registry_request(
            &root,
            deepagent_builtins::FsAccess::Full,
            effective_env_mode,
            connection_id,
            Some(local_execution_mode),
            true,
        );
        base.project = session_project.clone();
        // Multi-agent control plane: the `plan_execute` tool plans a goal into a
        // DAG and runs it over the SAME sub-agent loop as `task` (one executor).
        let plan_executor: Option<Arc<dyn deepagent_builtins::PlanExecutor>> = Some(Arc::new(
            crate::dag_orchestration::ChatPlanExecutor::new(task_runner.clone()),
        ));
        let toolset = build_main_run_toolset(MainRunToolsetRequest {
            base,
            mcp: self.mcp.as_deref(),
            plugin_projection: plugin_projection.as_ref(),
            task_runner,
            task_agent_types,
            plan_executor,
            plan: plan.clone(),
            skills: self.skills.as_ref(),
            tool_search_mode,
            tool_search_discovered: tool_search_discovered.clone(),
            tool_search_threshold,
        })
        .await?;
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
            settings: self.settings,
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
            bash_allow: self.bash_allow.to_vec(),
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
        let effective_thinking_depth = thinking_depth;
        let model_capability = ModelCapabilityResolver::new().resolve_model_id(&model);
        let context_policy =
            ContextPolicy::for_capability(&model_capability, effective_thinking_depth);

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
        session.append(EventPayload::MessageAppended {
            message: Message::user(&prompt_to_record)
                .with_attachments(message_attachments(&self.attachments)),
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
            mcp_instructions_block: mcp_instructions_block.as_deref(),
            mcp_instructions_entries: Some(&mcp_instructions_entries),
            tool_manifest: &tool_manifest,
            skills: self.skills.as_ref(),
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

        let capture_client = client.clone();
        let capture_model = model.clone();
        let reactive_compactor: Arc<dyn ReactiveContextCompactor> =
            Arc::new(HookedReactiveContextCompactor::new(
                client.clone(),
                model.clone(),
                hooks.clone(),
                session.id(),
            ));
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
        let mut agent = ModelAgent::new(client, model, system_prompt, final_user_prompt, tools)
            .with_thinking_depth(effective_thinking_depth)
            .with_fallback_model(fallback_model)
            .with_reactive_compactor(reactive_compactor)
            .with_history(history)
            .with_response_history(response_history)
            .with_proactive_compaction(proactive_threshold)
            .with_prefire(prefire_start)
            .with_attachment_resolver(Arc::new(crate::attachment_service::FileAttachmentResolver))
            .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME);

        if let Some(knowledge) = self.knowledge {
            agent = agent.with_relevant_memory_provider(Arc::new(
                crate::knowledge_service::KnowledgeMemoryProvider::new(knowledge.clone()),
            ));
        }

        agent = agent.with_todo_reminder_source(Arc::new(
            crate::todo_snapshot_reminder::TodoReminderAdapter::new(todo_store.clone()),
        ));

        if execution_features.stall_detector || crate::stall_classifier::stall_detector_enabled() {
            agent = agent.with_stall_classifier(Arc::new(
                crate::stall_classifier::ModelStallClassifier::new(stall_client, stall_model),
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
        let cancel = cancellation.flag();

        let discovered_before_run: std::collections::HashSet<String> = tool_search_discovered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();

        let verification_plan = crate::completion_plan::discover_verification_plan(&root);
        let (run_result, run_succeeded, terminal_kind, terminal_reason): (
            Result<()>,
            bool,
            Option<String>,
            Option<String>,
        ) = {
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
                    let kind = terminal.kind.label().to_string();
                    let reason = terminal.reason.clone();
                    (
                        terminal.into_completion_result(),
                        succeeded,
                        Some(kind),
                        reason,
                    )
                }
                Err(error) => {
                    let reason = error.to_string();
                    (Err(error), false, Some("failed".to_string()), Some(reason))
                }
            }
        };

        // Project the run terminal into the session stream so replaying the
        // session log alone reconstructs the run outcome (closes the two-log
        // gap; `runs`/`run_events` remain the run-local authority).
        if let Some(kind) = terminal_kind {
            if let Err(error) = session.append(EventPayload::RunTerminal {
                kind,
                reason: terminal_reason,
            }) {
                tracing::warn!(%error, "failed to record RunTerminal event");
            }
        }

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

        AppRunFinalizer::new(
            self.db.clone(),
            self.cost.clone(),
            self.knowledge.clone(),
            self.coordinator.cancellation_map(),
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
                capture_client: Some(capture_client),
                capture_model: Some(capture_model),
            },
        )?;

        drop(agent);
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
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let (root, session_project) = self.run_location(None)?;
        let cancellation = self.coordinator.register(run_id.clone(), None);

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
            None,
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
            None,
            None,
            session_project.as_deref(),
            normalized_input,
            cancellation.flag(),
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
        let _input_lease = accepted_turn.lease;

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
        let publisher = deepagent_runtime::workflow::NodeEventPublisher::new(sink.clone());

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

        let tool_registry = std::sync::Arc::new(ToolRegistry::new());

        let mut agent = deepagent_runtime::workflow::WorkflowAgent::new(
            compiled,
            workflow_request.inputs,
            workflow_request.target_node_id,
            publisher,
        )
        .with_cancel(cancellation.flag());

        if let (Some(client), Some(model_name)) = (wf_client, wf_model_name.clone()) {
            agent = agent.with_model(client, model_name);
        }

        if let Some(gateway) = self.canvas_model.clone() {
            agent = agent.with_canvas_bridge(gateway);
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

        let plan = self.plan_mode_for_session(&session_id);
        let todo_store = deepagent_builtins::TodoStore::new();

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
            granted,
            nested_instructions: None,
        })?;
        let config = kernel_runtime.config;

        cancellation.add_alias(session_id.clone());
        let cancel = cancellation.flag();

        let empty_discovered = std::collections::HashSet::new();
        let empty_toolset: crate::tool_manifest::DiscoveredToolSet =
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let registry = tool_registry;

        let channel_gate =
            ChannelApprovalGate::new(self.coordinator.pending(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> = Arc::new(channel_gate);
        let hooks = deepagent_hooks::HookRegistry::new();

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
            .with_cancellation_flag(cancel);

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
        AppRunFinalizer::new(
            self.db.clone(),
            self.cost.clone(),
            self.knowledge.clone(),
            self.coordinator.cancellation_map(),
        )
        .finalize_after_kernel(
            &mut session,
            AppRunFinalizerRequest {
                session_id: &session_id,
                run_id: &run_id,
                discovered_before_run: &empty_discovered,
                discovered_tools: &empty_toolset,
                usage: agent.cumulative_usage(),
                model_name: finalizer_model_name.as_deref().unwrap_or("workflow"),
                sink: sink.as_ref(),
                run_succeeded,
                capture_client: run_model.as_ref().map(|rm| rm.client.clone()),
                capture_model: wf_model_name,
            },
        )?;

        drop(agent);
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
    fn slash_runtime(&self) -> crate::slash_runtime::SlashRuntime<'a> {
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
            plan_modes: self.plan_modes,
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

    /// Synchronize the plugin runtime projection (if plugins are attached).
    fn sync_plugin_runtime(&self) -> Result<Option<PluginRuntimeProjection>> {
        let Some(plugins) = self.plugins else {
            return Ok(None);
        };
        let projection = plugins.runtime_projection()?;
        if let Some(skills) = self.skills {
            let mut svc = skills
                .lock()
                .map_err(|_| CoreError::other("skills lock poisoned"))?;
            svc.set_plugin_roots(projection.skill_roots.clone())?;
        }
        Ok(Some(projection))
    }

    /// Return the shared plan-mode flag for a session.
    fn plan_mode_for_session(&self, session_id: &str) -> deepagent_builtins::PlanMode {
        let mut map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(session_id.to_string()).or_default().clone()
    }

    /// Return the shared discovered-tools set for a session.
    fn discovered_tools_for_session(&self, session_id: &str) -> DiscoveredToolSet {
        let mut map = self
            .discovered_tools
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())))
            .clone()
    }

    /// Load project-scoped hook definitions from the workspace root.
    fn project_hook_definitions(&self, root: &Path) -> Result<Option<HookDefinitions>> {
        let paths = project_hook_paths(root)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(None);
        }

        let Some(projects) = self.projects else {
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

    /// Wire the office-skill guard hook when an office service is attached.
    fn office_skill_guard_hook(
        &self,
        session_id: &str,
        prior_events: &[deepagent_core::event::Event],
    ) -> Result<Option<OfficeSkillGuardHook>> {
        if self.office.is_none() {
            return Ok(None);
        }
        let Some(skills) = self.skills else {
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

    /// Model-driven context compaction (Phase 2B).
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

        if let Err(e) = session.append(EventPayload::ContextCompacted {
            tokens_before: total as u64,
            tokens_after: tokens_after as u64,
            strategy: "model".to_string(),
        }) {
            tracing::warn!(error = %e, "failed to record ContextCompacted event");
        }

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
                    summary: Some(summary_block.clone()),
                },
            ))
            .await
        {
            tracing::warn!(error = %error, "PostCompact hook failed");
        }
        (compacted, true)
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
        bash_external_safety_gate: bool,
    ) -> ToolRegistryBuildRequest<'a> {
        let local_command_executor = match (self.runtime_broker, self.local_command_executor) {
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
            bash_external_safety_gate,
            bash_allow: self.bash_allow.to_vec(),
            settings: self.settings.clone(),
            executor_factory: self.executor_factory.clone(),
            local_command_executor,
            knowledge: self.knowledge.clone(),
            project_map: self.project_map.clone(),
            office: self.office.clone(),
            remote_ops_factory: self.remote_ops_factory.clone(),
        }
    }
}

fn resolve_run_location(
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

/// Conservative project-hook file search paths.
pub(crate) fn project_hook_paths(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(".claude").join("settings.json"),
        root.join(".claude").join("settings.local.json"),
        root.join(".deepagent").join("settings.json"),
        root.join(".deepagent").join("settings.local.json"),
        root.join(".deepagent").join("hooks.json"),
    ]
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
mod attachment_mapping_tests {
    use super::*;
    use deepagent_runtime::InputAttachment;

    #[test]
    fn maps_input_attachments_to_reference_only_message_attachments() {
        let mapped = message_attachments(&[
            InputAttachment::Image {
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
        assert_eq!(mapped[0].media_type.as_deref(), Some("image/png"));
        assert!(mapped[0]
            .path
            .as_deref()
            .is_some_and(|p| p.ends_with("a.png")));
        assert_eq!(mapped[1].kind, "text");
        assert!(mapped[1].path.is_none());
    }
}
