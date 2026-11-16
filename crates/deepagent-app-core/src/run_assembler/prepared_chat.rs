//! Single-use preparation for the normal chat executor. Admission persists the
//! snapshot from these exact tools; the worker consumes rather than rebuilds it.
use super::*;
use crate::run_capabilities::PreparedCapabilitySnapshot;
use deepagent_tools::ToolRegistry;
use prepared_tools::PreparedMainRunTools;
use std::collections::HashSet;

pub(super) struct PreparedChatEnvironment {
    pub(super) configuration: super::execution_config::ResolvedExecutionConfig,
    pub(super) project_trusted: bool,
    pub(super) plugin_projection: Option<crate::plugin_runtime::PluginRuntimeProjection>,
    pub(super) plugin_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
}

pub(super) struct PreparedChatTools {
    pub(super) tools: PreparedMainRunTools,
    pub(super) plan: deepagent_builtins::PlanMode,
    pub(super) discovered_before_run: HashSet<String>,
    pub(super) discovered: DiscoveredToolSet,
    pub(super) search_mode: deepagent_builtins::ToolSearchMode,
    pub(super) search_threshold: usize,
    pub(super) capabilities: PreparedCapabilitySnapshot,
}

/// Opaque, non-cloneable ownership of the actual main-run registry. Constructed
/// and admitted by ChatService; only the existing run pipeline may consume it.
pub struct PreparedChatRun {
    pub(super) run_id: String,
    pub(super) session_id: String,
    pub(super) root: PathBuf,
    pub(super) session_project: Option<String>,
    // Transient normalized input, never part of durable capability metadata.
    pub(super) input: Option<String>,
    pub(super) contents: PreparedChatContents,
    pub(super) overrides: HarnessRunOverrides,
    pub(super) knowledge: Option<Arc<KnowledgeService>>,
}

pub(super) struct PreparedChatModel {
    pub(super) environment: PreparedChatEnvironment,
    pub(super) tools: PreparedChatTools,
}

pub(super) enum PreparedChatContents {
    Model(Box<PreparedChatModel>),
    Control {
        result: deepagent_intent::CommandResult,
        capabilities: PreparedCapabilitySnapshot,
    },
}

impl PreparedChatRun {
    pub(super) fn validate_input(&self, input: &str) -> Result<()> {
        if self.input.as_deref().is_some_and(|bound| bound != input) {
            return Err(CoreError::invalid(
                "prepared input does not match admission",
            ));
        }
        // The model-only preparation API deliberately leaves text to its
        // caller, but cannot be used to dispatch a different execution domain.
        if self.input.is_none()
            && crate::slash_runtime::parse_builtin_slash_command(input, Some(&self.session_id))?
                .is_some()
        {
            return Err(CoreError::invalid(
                "prepared model cannot switch to control; use admit_prepared_input",
            ));
        }
        Ok(())
    }

    /// Routing metadata and hashes only; not an execution permission grant.
    pub fn capability_snapshot(&self) -> &serde_json::Value {
        match &self.contents {
            PreparedChatContents::Model(model) => &model.tools.capabilities.snapshot,
            PreparedChatContents::Control { capabilities, .. } => &capabilities.snapshot,
        }
    }
}

impl RunAssembler<'_> {
    pub(crate) fn prepare_chat_input(
        self,
        run_id: &str,
        session_id: &str,
        input: Option<&str>,
        overrides: HarnessRunOverrides,
    ) -> Result<PreparedChatRun> {
        let Some(input) = input else {
            return self.prepare_chat_run(run_id, session_id, overrides);
        };
        let session_id = SessionId::from_str(session_id)?.to_string();
        let (root, session_project) = self.run_location(Some(&session_id))?;
        let normalized = deepagent_runtime::InputIngress::normalize(
            Some(session_id.clone()),
            root.clone(),
            input,
            InputMode::Prompt,
            self.attachments.clone(),
        )?;
        let Some(result) = crate::slash_runtime::parse_builtin_slash_command(
            &normalized.effective_text,
            Some(&session_id),
        )?
        else {
            let mut prepared = self.prepare_chat_run(run_id, &session_id, overrides)?;
            prepared.input = Some(normalized.effective_text);
            return Ok(prepared);
        };
        if let deepagent_intent::SlashAction::Resume { session_id: target } = &result.action {
            if SessionId::from_str(target)?.to_string() != session_id {
                return Err(CoreError::invalid(
                    "a prepared control cannot change its thread; use thread/resume",
                ));
            }
        }
        let registry = ToolRegistry::new();
        let capabilities = ResolvedRunCapabilities::resolve(serde_json::json!({
            "executionDomain":"builtin_control", "workspace":std::fs::canonicalize(&root)
                .map_err(|error| CoreError::other(format!("canonicalize control workspace: {error}")))?
        }), &registry, &[], crate::settings::ApprovalPolicy::AlwaysAsk)?.prepare(run_id, &session_id)?;
        Ok(PreparedChatRun {
            run_id: run_id.to_owned(),
            session_id,
            root,
            session_project,
            input: Some(normalized.effective_text),
            contents: PreparedChatContents::Control {
                result,
                capabilities,
            },
            overrides,
            knowledge: self.knowledge,
        })
    }

    pub(crate) async fn run_prepared<F, A>(
        mut self,
        prepared: PreparedChatRun,
        prompt: &str,
        on_event: F,
        on_approval: A,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let run_id = prepared.run_id.clone();
        let session_id = prepared.session_id.clone();
        let overrides = prepared.overrides.clone();
        self.prepared_run = Some(prepared);
        self.run(
            prompt,
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&run_id),
            overrides,
            on_event,
            on_approval,
        )
        .await
    }

    pub(crate) fn prepare_chat_run(
        mut self,
        run_id: &str,
        session_id: &str,
        overrides: HarnessRunOverrides,
    ) -> Result<PreparedChatRun> {
        let session_id = SessionId::from_str(session_id)?.to_string();
        let (root, session_project) = self.run_location(Some(&session_id))?;
        self.knowledge = self
            .knowledge
            .as_ref()
            .map(|knowledge| knowledge.for_project(&root))
            .transpose()?;
        let mut environment = self.prepare_chat_environment(&root, run_id, &overrides)?;
        let session = EventStore::new(self.db)
            .get_session(session_id.parse()?)?
            .ok_or_else(|| CoreError::not_found("prepared chat session"))?;
        let effective_env_mode =
            (session.mode == deepagent_core::SessionMode::Remote).then_some("remote");
        let prior_events = EventStore::new(self.db).load_session(session_id.parse()?)?;
        let tools = self.prepare_chat_tools(
            &root,
            session_project.clone(),
            run_id,
            &session_id,
            effective_env_mode,
            None,
            &prior_events,
            &mut environment,
        )?;
        Ok(PreparedChatRun {
            run_id: run_id.to_owned(),
            session_id,
            root,
            session_project,
            input: None,
            contents: PreparedChatContents::Model(Box::new(PreparedChatModel {
                environment,
                tools,
            })),
            overrides,
            knowledge: self.knowledge,
        })
    }

    pub(super) fn prepare_chat_environment(
        &self,
        root: &Path,
        run_id: &str,
        overrides: &HarnessRunOverrides,
    ) -> Result<PreparedChatEnvironment> {
        let plugin_snapshot = self
            .plugins
            .as_ref()
            .map(|plugins| plugins.ready_snapshot());
        let plugin_projection = plugin_snapshot
            .as_ref()
            .map(|(projection, _)| projection.clone());
        self.sync_plugin_skill_snapshot(plugin_projection.as_ref())?;
        let configuration = self.resolve_execution_config(
            root,
            overrides,
            plugin_projection.as_ref(),
            plugin_snapshot
                .as_ref()
                .map(|(_, registry)| registry.clone()),
            Some(run_id),
        )?;
        Ok(PreparedChatEnvironment {
            project_trusted: configuration.project_trusted,
            configuration,
            plugin_projection,
            plugin_registry: plugin_snapshot.map(|(_, registry)| registry),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_chat_tools(
        &self,
        root: &Path,
        session_project: Option<String>,
        run_id: &str,
        session_id: &str,
        effective_env_mode: Option<&str>,
        connection_id: Option<&str>,
        prior_events: &[deepagent_core::event::Event],
        prepared: &mut PreparedChatEnvironment,
    ) -> Result<PreparedChatTools> {
        let environment = &prepared.configuration.environment;
        let plan = self.plan_mode_for_session(session_id)?;
        plan.is_active()?;
        let (discovered_before_run, discovered) =
            self.service.discovered_tools_for_run(prior_events);
        let search_mode = prepared.configuration.search_mode;
        let search_threshold = prepared.configuration.search_threshold;
        let executor = self.local_executor_for_run(
            root,
            environment.sandbox_mode,
            environment.sandbox_backend,
        )?;
        let mut sub_base = self.base_registry_request(BaseRegistryInputs {
            root,
            access: environment.access,
            env_mode: None,
            connection_id: None,
            local_exec_mode: Some(environment.local_execution_mode),
            force_sandbox_backend: environment.sandbox_backend.is_some(),
            local_command_executor: executor.clone(),
            bash_external_safety_gate: matches!(
                environment.policy,
                crate::settings::ApprovalPolicy::FullAccess
            ),
            session_id,
        });
        sub_base.project = session_project.clone();
        sub_base.question_responder = Arc::new(deepagent_builtins::RecommendedResponder);
        let mut base = self.base_registry_request(BaseRegistryInputs {
            root,
            access: environment.access,
            env_mode: effective_env_mode,
            connection_id,
            local_exec_mode: Some(environment.local_execution_mode),
            force_sandbox_backend: environment.sandbox_backend.is_some(),
            local_command_executor: executor,
            bash_external_safety_gate: true,
            session_id,
        });
        base.project = session_project;
        let tools = self.prepare_main_run_tools(MainRunPreparationRequest {
            base,
            sub_base,
            plugin_projection: prepared.plugin_projection.as_ref(),
            plugin_registry: prepared.plugin_registry.clone(),
            plan: plan.clone(),
            discovered: discovered.clone(),
            search_mode,
            search_threshold,
        })?;
        let mut context = capability_context(CapabilityContextRequest {
            root,
            profile: environment.profile,
            permission_rules: &prepared.configuration.permission_rules,
            sandbox_backend: environment.sandbox_backend,
            backends: self.sandbox_backend_registry.as_deref(),
            tool_search_mode: search_mode,
            tool_search_threshold: search_threshold,
            mcp_lifecycle: &tools.toolset.lifecycle,
        })?;
        // Hash the exact frozen inputs, never a second live service lookup.
        // Skill schemas are constant even when their bodies/enablement change.
        context["skillCatalogHash"] = tools
            .toolset
            .skills
            .as_ref()
            .map(|registry| {
                let skills = registry
                    .catalog()
                    .into_iter()
                    .filter_map(|meta| registry.get(&meta.id))
                    .collect::<Vec<_>>();
                crate::run_capabilities::capability_hash(&skills)
            })
            .transpose()?
            .into();
        context["pluginProjectionHash"] = prepared
            .plugin_projection
            .as_ref()
            .map(crate::run_capabilities::capability_hash)
            .transpose()?
            .into();
        context["runConfigHash"] =
            crate::run_capabilities::capability_hash(&environment.config.value)?.into();
        context["mcpGeneration"] = tools.toolset.mcp_generation.clone().into();
        let policy = environment.policy;
        prepared
            .configuration
            .bind_mcp_generation(tools.toolset.mcp_generation.as_deref())?;
        context["executionConfig"] = prepared.configuration.snapshot.clone();
        let capabilities = ResolvedRunCapabilities::resolve(
            context,
            &tools.toolset.registry,
            &tools.toolset.manifest.tools,
            policy,
        )?
        .prepare(run_id, session_id)?;
        Ok(PreparedChatTools {
            tools,
            plan,
            discovered_before_run,
            discovered,
            search_mode,
            search_threshold,
            capabilities,
        })
    }
}
