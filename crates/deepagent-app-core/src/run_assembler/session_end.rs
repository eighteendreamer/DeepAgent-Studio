//! Explicit lifecycle control over the existing admission, kernel and ledgers.
use super::*;
use deepagent_core::clock::Clock;
use deepagent_hooks::{HookAction, HookActionExecutor, HookContext, HookData, HookOutcome};
use deepagent_runtime::{AgentDecision, Observation, RuntimeConfig};
use deepagent_tools::ToolExecutionContext;

struct SessionEndAgent {
    db: Arc<Database>,
    session_id: SessionId,
    run_id: String,
    hooks: Arc<HookRegistry>,
    execution: ToolExecutionContext,
}

#[async_trait::async_trait]
impl Agent for SessionEndAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        // Publish before dispatch: a lost process/receipt cannot replay cleanup
        // side effects. A crash in this gap may miss callbacks, not duplicate them.
        if EventStore::new(&self.db).end_session_for_run(
            self.session_id,
            &self.run_id,
            Some("explicit_end"),
            SystemClock.now(),
        )? {
            self.hooks
                .dispatch_with_context(
                    &HookContext::new(self.session_id, HookPoint::SessionEnd, HookData::None),
                    self.execution.clone(),
                )
                .await?;
        }
        Ok(AgentDecision::Complete(
            "会话已结束；继续请新建或 Fork。".to_owned(),
        ))
    }
}

/// Uses precisely the existing host action executor/policy when a configured
/// model/MCP hook actually needs it. Command/HTTP-only closing never reads a key
/// or selects/samples the main chat model.
struct EndHookHost {
    service: ChatService,
    session_id: String,
    run_id: String,
    sink: Arc<dyn RuntimeEventSink>,
    executor: tokio::sync::OnceCell<Arc<dyn HookActionExecutor>>,
}

#[async_trait::async_trait]
impl HookActionExecutor for EndHookHost {
    async fn execute(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
    ) -> Result<HookOutcome> {
        self.execute_with_context(action, payload, ToolExecutionContext::default())
            .await
    }

    async fn execute_with_context(
        &self,
        action: &HookAction,
        payload: serde_json::Value,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        let host = self
            .executor
            .get_or_try_init(|| async {
                self.service.run_assembler().end_hook_executor(
                    &self.session_id,
                    &self.run_id,
                    self.sink.clone(),
                )
            })
            .await?;
        host.execute_with_context(action, payload, execution).await
    }
}

impl RunAssembler<'_> {
    pub(super) async fn execute_session_end<F>(
        self,
        session_id: &str,
        run_id: &str,
        cancellation: &crate::run_coordinator::ActiveRunRegistration,
        on_event: F,
    ) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
    {
        let id = session_id.parse()?;
        let store = deepagent_persistence::run_store::RunStore::new(self.db);
        store.validate_session_end_publication(session_id, Some(run_id))?;
        // A retry after publication is read-only: no new messages/tasks/hooks.
        if EventStore::new(self.db)
            .get_session(id)?
            .ok_or_else(|| CoreError::not_found("session end"))?
            .ended_at
            .is_some()
        {
            // A no-op still owns an admitted turn. Use the existing canonical
            // terminal+notification transaction (as other admission-only
            // paths do), so SDK/Desktop consumers do not wait forever.
            let (channel, receiver) = ChannelSink::new();
            let sink: Arc<dyn RuntimeEventSink> = Arc::new(channel);
            let pump = spawn_runtime_event_pump(
                receiver,
                self.runtime_logs.clone(),
                run_id.to_owned(),
                session_id.to_owned(),
                on_event,
            );
            let result = deepagent_runtime::kernel::persist_terminal_and_notify(
                self.db,
                run_id,
                &deepagent_runtime::KernelTerminal {
                    kind: deepagent_runtime::TerminalKind::Succeeded,
                    message: Some("会话已结束；继续请新建或 Fork。".to_owned()),
                    reason: None,
                },
                &cancellation.cancellation(),
                None,
                sink.as_ref(),
            );
            drop(sink);
            let _ = pump.await;
            result?;
            return Ok(session_id.to_owned());
        }
        let (root, _) = self.run_location(Some(session_id))?;
        let mut session = deepagent_session::Session::recover(self.db, &SystemClock, id)?;
        session.append(EventPayload::MessageAppended {
            message: Message::user("/end"),
        })?;
        let task = session.create_task("/end")?;
        let registry = deepagent_tools::ToolRegistry::new();
        let config = RuntimeConfig {
            max_steps: Some(1),
            fire_session_start: false,
            task_timeout: Some(ToolExecutionContext::DEFAULT_TIMEOUT),
            ..Default::default()
        };
        let (channel, receiver) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(channel);
        let pump = spawn_runtime_event_pump(
            receiver,
            self.runtime_logs.clone(),
            run_id.to_owned(),
            session_id.to_owned(),
            on_event,
        );
        let kernel: AgentKernel<'_, SystemClock> = AgentKernel::new(
            self.db.clone(),
            &registry,
            Default::default(),
            config,
            run_id,
        )
        .with_events(sink.clone())
        .with_cancellation(cancellation.cancellation());
        let durable_sink = kernel.event_sink();
        let result = async {
            let host: Arc<dyn HookActionExecutor> = Arc::new(EndHookHost {
                service: self.service.clone(),
                session_id: session_id.to_owned(),
                run_id: run_id.to_owned(),
                sink: durable_sink.clone(),
                executor: tokio::sync::OnceCell::new(),
            });
            let trust = Arc::new(ProjectHookTrust::new(self.projects.clone(), root.clone()));
            let run_config = RunConfigOverlay::load(&root, serde_json::json!({}));
            let projection = self
                .plugins
                .as_ref()
                .map(|plugins| plugins.ready_snapshot().0);
            let mut hooks = HookRegistry::new();
            crate::hook_assembly::register_extension_hooks(
                &mut hooks,
                crate::hook_assembly::ExtensionHookRequest {
                    settings: self.settings,
                    run_config: &run_config,
                    project_hooks: self.project_hook_definitions(&root, &trust)?,
                    plugin_projection: projection.as_ref(),
                    root: &root,
                    sink: durable_sink.clone(),
                    project_hook_trust: trust,
                    runtime_environment: self
                        .runtime_broker
                        .as_ref()
                        .map(|broker| broker.build_process_environment(Some(&root)))
                        .unwrap_or_default(),
                },
                &host,
            )?;
            let control = cancellation.cancellation();
            let mut agent = SessionEndAgent {
                db: self.db.clone(),
                session_id: id,
                run_id: run_id.to_owned(),
                hooks: Arc::new(hooks),
                execution: ToolExecutionContext::from_token_and_flag(
                    control.token(),
                    control.legacy_flag(),
                ),
            };
            // One deterministic Agent over the production kernel, not a second loop.
            kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await
                .and_then(|terminal| terminal.into_completion_result())
        }
        .await;
        drop(durable_sink);
        drop(kernel);
        drop(sink);
        let _ = pump.await;
        result.map(|_| session_id.to_owned())
    }

    fn end_hook_executor(
        &self,
        session_id: &str,
        run_id: &str,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<Arc<dyn HookActionExecutor>> {
        let (root, project) = self.run_location(Some(session_id))?;
        let mut prepared =
            self.prepare_chat_environment(&root, run_id, &HarnessRunOverrides::default())?;
        let events = EventStore::new(self.db).load_session(session_id.parse()?)?;
        let tools = self.prepare_chat_tools(
            &root,
            project,
            run_id,
            session_id,
            None,
            None,
            &events,
            &mut prepared,
        )?;
        let environment = &prepared.configuration.environment;
        let (client, model, reasoning_effort) = crate::model_runtime::build_model_client(
            self.settings,
            self.transport.clone(),
            ModelRole::Chat,
        )?;
        let usage_owner = Some(crate::call_usage::RunCallUsageOwner {
            db: self.db.clone(),
            cost: self.cost.clone(),
            session_id: session_id.to_owned(),
            run_id: run_id.to_owned(),
            events: Arc::downgrade(&sink),
        });
        Ok(
            crate::hook_assembly::assemble_hook_executor(&HookAssemblyRequest {
                settings: self.settings,
                run_config: &environment.config,
                permission_rules: prepared.configuration.permission_rules.clone(),
                project_hooks: None,
                plugin_projection: prepared.plugin_projection.as_ref(),
                root: &root,
                sink,
                usage_owner,
                client,
                model,
                reasoning_effort,
                mcp: tools.tools.toolset.hook_mcp_registry,
                registry: &tools.tools.toolset.registry,
                permissions: PermissionSet::developer(),
                plan: tools.plan,
                office_skill_guard: None,
                access: environment.access,
                bash_allow: self.bash_allow.to_vec(),
                bash_full_access: matches!(
                    environment.policy,
                    crate::settings::ApprovalPolicy::FullAccess
                ),
                is_trusted: prepared.project_trusted,
                project_hook_trust: Arc::new(ProjectHookTrust::new(
                    self.projects.clone(),
                    root.clone(),
                )),
                runtime_environment: Default::default(),
            })
            .1,
        )
    }
}
