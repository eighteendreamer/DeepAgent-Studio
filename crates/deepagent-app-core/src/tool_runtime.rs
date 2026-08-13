use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolRegistry;

use crate::knowledge_service::KnowledgeService;
use crate::mcp_runtime::attach_mcp_tools;
use crate::mcp_service::McpService;
use crate::office_service::OfficeService;
use crate::plugin_runtime::PluginRuntimeProjection;
use crate::project_map_service::ProjectMapService;
use crate::settings::{LocalExecutionMode, SettingsService};
use crate::tool_manifest::{prepare_tool_manifest, DiscoveredToolSet, ToolManifest};

pub(crate) type CommandExecutorFactory =
    Arc<dyn Fn(String) -> Arc<dyn deepagent_builtins::bash_tool::CommandExecutor> + Send + Sync>;

pub(crate) type RemoteOpsFactory =
    Arc<dyn Fn(String) -> Arc<dyn deepagent_builtins::RemoteOpsBackend> + Send + Sync>;

/// Adds the current project's resolved runtime environment to an existing
/// command executor. The wrapped executor still owns sandboxing, cancellation
/// and output capture; this type only supplies the process environment.
pub(crate) struct RuntimeCommandExecutor {
    inner: Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>,
    broker: Arc<crate::RuntimeBroker>,
    project_root: PathBuf,
}

impl RuntimeCommandExecutor {
    pub(crate) fn new(
        inner: Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>,
        broker: Arc<crate::RuntimeBroker>,
        project_root: &Path,
    ) -> Self {
        Self {
            inner,
            broker,
            project_root: project_root.to_path_buf(),
        }
    }

    fn environment(&self, extra: &[(String, String)]) -> Vec<(String, String)> {
        let mut environment = self
            .broker
            .build_process_environment(Some(&self.project_root))
            .into_iter()
            .collect::<Vec<_>>();
        for (key, value) in extra {
            if let Some(existing) = environment.iter_mut().find(|(name, _)| name == key) {
                existing.1 = value.clone();
            } else {
                environment.push((key.clone(), value.clone()));
            }
        }
        environment
    }
}

#[async_trait]
impl deepagent_builtins::bash_tool::CommandExecutor for RuntimeCommandExecutor {
    async fn run(
        &self,
        command: &str,
        cwd: &str,
    ) -> Result<deepagent_builtins::bash_tool::CommandOutcome> {
        self.run_with_options(
            command,
            cwd,
            deepagent_builtins::bash_tool::CommandShell::Auto,
        )
        .await
    }

    async fn run_with_options(
        &self,
        command: &str,
        cwd: &str,
        shell: deepagent_builtins::bash_tool::CommandShell,
    ) -> Result<deepagent_builtins::bash_tool::CommandOutcome> {
        let environment = self.environment(&[]);
        self.inner
            .run_with_environment(command, cwd, shell, &environment)
            .await
    }

    async fn run_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: deepagent_builtins::bash_tool::CommandShell,
        environment: &[(String, String)],
    ) -> Result<deepagent_builtins::bash_tool::CommandOutcome> {
        let environment = self.environment(environment);
        self.inner
            .run_with_environment(command, cwd, shell, &environment)
            .await
    }

    async fn run_controlled(
        &self,
        command: &str,
        cwd: &str,
        shell: deepagent_builtins::bash_tool::CommandShell,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Result<deepagent_builtins::bash_tool::CommandOutcome> {
        let environment = self.environment(&[]);
        self.inner
            .run_controlled_with_environment(command, cwd, shell, cancel, timeout, &environment)
            .await
    }

    async fn run_controlled_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: deepagent_builtins::bash_tool::CommandShell,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
        environment: &[(String, String)],
    ) -> Result<deepagent_builtins::bash_tool::CommandOutcome> {
        let environment = self.environment(environment);
        self.inner
            .run_controlled_with_environment(command, cwd, shell, cancel, timeout, &environment)
            .await
    }
}

pub(crate) struct ToolRegistryBuildRequest<'a> {
    pub(crate) root: &'a Path,
    pub(crate) access: deepagent_builtins::FsAccess,
    pub(crate) env_mode: Option<&'a str>,
    pub(crate) connection_id: Option<&'a str>,
    pub(crate) local_exec_mode: Option<LocalExecutionMode>,
    pub(crate) bash_external_safety_gate: bool,
    pub(crate) bash_allow: Vec<String>,
    pub(crate) settings: Arc<SettingsService>,
    pub(crate) executor_factory: Option<CommandExecutorFactory>,
    pub(crate) local_command_executor:
        Option<Arc<dyn deepagent_builtins::bash_tool::CommandExecutor>>,
    pub(crate) knowledge: Option<Arc<KnowledgeService>>,
    pub(crate) project_map: Option<Arc<ProjectMapService>>,
    pub(crate) office: Option<Arc<OfficeService>>,
    pub(crate) remote_ops_factory: Option<RemoteOpsFactory>,
}

pub(crate) fn build_base_tool_registry(
    request: ToolRegistryBuildRequest<'_>,
) -> Result<(ToolRegistry, deepagent_builtins::TodoStore)> {
    use deepagent_builtins::{
        register_builtins, AskUserQuestionTool, BuiltinConfig, DeclineResponder, WorkspaceRoot,
    };

    let mut registry = ToolRegistry::new();
    let mut config = BuiltinConfig::new(
        WorkspaceRoot::new(request.root.to_path_buf()).with_access(request.access),
        request.bash_allow,
    )
    .with_bash_external_safety_gate(request.bash_external_safety_gate);

    if let (Some("remote"), Some(factory), Some(conn_id)) = (
        request.env_mode,
        request.executor_factory.as_ref(),
        request.connection_id,
    ) {
        config = config.with_command_executor(factory(conn_id.to_string()));
    } else if request.env_mode != Some("remote") {
        let use_sandbox = !matches!(request.local_exec_mode, Some(LocalExecutionMode::Direct));
        if use_sandbox {
            if let Some(executor) = request.local_command_executor {
                config = config.with_command_executor(executor);
            }
        }
    }

    let todo_store = register_builtins(&mut registry, config)?;
    register_web_tools(&mut registry, &request.settings)?;

    registry.register(Arc::new(AskUserQuestionTool::new(DeclineResponder)))?;
    register_knowledge_search(&mut registry, request.knowledge);
    // Shared lazy code-index builder: the first code_map_*/codegraph_* call that
    // finds the index missing builds it once (grok-build code_nav.rs parity),
    // so the model isn't stuck being told to use a map it can't create.
    let code_index = CodeIndexAutoBuild::new(request.root.to_path_buf());
    register_project_map_tools(
        &mut registry,
        request.project_map,
        request.root,
        &code_index,
    )?;
    register_codegraph_tools(&mut registry, request.root, &code_index)?;
    register_office_tools(&mut registry, request.office)?;
    register_remote_ops_tools(
        &mut registry,
        request.env_mode,
        request.connection_id,
        request.remote_ops_factory,
    )?;

    Ok((registry, todo_store))
}

pub(crate) fn register_skill_tool(
    registry: &mut ToolRegistry,
    skills: Option<&Arc<Mutex<crate::skills_service::SkillsService>>>,
) -> Result<()> {
    let Some(skills) = skills else {
        return Ok(());
    };
    let registry_snapshot = {
        let svc = skills
            .lock()
            .map_err(|_| CoreError::invalid("skills service mutex poisoned"))?;
        Arc::new(svc.manager().registry().clone())
    };
    registry.register(Arc::new(deepagent_builtins::SkillTool::new(
        registry_snapshot,
    )))?;
    Ok(())
}

/// Register the `task` sub-agent tool into the MAIN run's registry only.
///
/// Kept out of the base/sub-agent registry so a sub-agent cannot spawn further
/// sub-agents (Claude-Code parity, Req 4.6). The `runner` and `agent_types`
/// are assembled by [`ChatService`] because they depend on live run state
/// (session, event sink, checkpoints); this helper owns the registration so
/// the main-run tool set is wired through `tool_runtime` like the base tools.
pub(crate) fn register_task_tool<R>(
    registry: &mut ToolRegistry,
    runner: R,
    agent_types: Vec<deepagent_builtins::TaskAgentType>,
) -> Result<()>
where
    R: deepagent_builtins::SubagentRunner + 'static,
{
    registry.register(Arc::new(
        deepagent_builtins::TaskTool::new_with_agent_types(runner, agent_types),
    ))?;
    Ok(())
}

/// Register the `knowledge_write` tool into the MAIN run's registry only
/// (sub-agents get `knowledge_search` but not write). No-op when no
/// [`KnowledgeService`] is attached.
pub(crate) fn register_knowledge_write_tool(
    registry: &mut ToolRegistry,
    knowledge: Option<&Arc<KnowledgeService>>,
) -> Result<()> {
    let Some(knowledge) = knowledge else {
        return Ok(());
    };
    use deepagent_builtins::KnowledgeWriteTool;
    let backend = crate::knowledge_service::KnowledgeServiceBackend::new(knowledge.clone());
    registry.register(Arc::new(KnowledgeWriteTool::new(backend)))?;
    Ok(())
}

/// Register the `enter_plan_mode` / `exit_plan_mode` toggles over the shared
/// [`PlanMode`](deepagent_builtins::PlanMode) flag for the current session.
/// Main-run only — sub-agents inherit the parent's plan-mode decision.
pub(crate) fn register_plan_mode_tools(
    registry: &mut ToolRegistry,
    plan: &deepagent_builtins::PlanMode,
) -> Result<()> {
    registry.register(Arc::new(deepagent_builtins::EnterPlanModeTool::new(
        plan.clone(),
    )))?;
    registry.register(Arc::new(deepagent_builtins::ExitPlanModeTool::new(
        plan.clone(),
    )))?;
    Ok(())
}

/// Everything the MAIN run needs on top of the base toolset (see
/// [`MainRunToolsetRequest`]). Assembled by [`build_main_run_toolset`], the
/// single entry point for main-run tool wiring.
pub(crate) struct MainRunToolset {
    pub(crate) registry: ToolRegistry,
    pub(crate) todo_store: deepagent_builtins::TodoStore,
    pub(crate) manifest: ToolManifest,
    /// MCP registry handle for MCP-typed hooks (None when MCP is disabled or
    /// no server connected).
    pub(crate) hook_mcp_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
}

/// Inputs for [`build_main_run_toolset`]. `base` describes the shared
/// built-in registry (same builder the sub-agent registries use); the rest
/// are the MAIN-run-only additions.
pub(crate) struct MainRunToolsetRequest<'a, R>
where
    R: deepagent_builtins::SubagentRunner + 'static,
{
    pub(crate) base: ToolRegistryBuildRequest<'a>,
    pub(crate) mcp: Option<&'a McpService>,
    pub(crate) plugin_projection: Option<&'a PluginRuntimeProjection>,
    pub(crate) task_runner: R,
    pub(crate) task_agent_types: Vec<deepagent_builtins::TaskAgentType>,
    pub(crate) plan: deepagent_builtins::PlanMode,
    pub(crate) skills: Option<&'a Arc<Mutex<crate::skills_service::SkillsService>>>,
    pub(crate) tool_search_mode: deepagent_builtins::ToolSearchMode,
    pub(crate) tool_search_discovered: DiscoveredToolSet,
    pub(crate) tool_search_threshold: usize,
}

/// Single entry point for the MAIN run's tool wiring (kernel-refactor Phase
/// A). Fixed order — base built-ins → MCP adapters → `task` →
/// `knowledge_write` → plan-mode toggles → `skill` → tool-search manifest
/// snapshot — matching the pre-consolidation `run_in_session` sequence
/// byte-for-byte. The manifest MUST be prepared last so deferred-tool
/// snapshots cover every registered tool.
pub(crate) async fn build_main_run_toolset<R>(
    request: MainRunToolsetRequest<'_, R>,
) -> Result<MainRunToolset>
where
    R: deepagent_builtins::SubagentRunner + 'static,
{
    let knowledge = request.base.knowledge.clone();
    let (mut registry, todo_store) = build_base_tool_registry(request.base)?;
    let mcp_runtime =
        attach_mcp_tools(&mut registry, request.mcp, request.plugin_projection).await?;
    register_task_tool(&mut registry, request.task_runner, request.task_agent_types)?;
    register_knowledge_write_tool(&mut registry, knowledge.as_ref())?;
    register_plan_mode_tools(&mut registry, &request.plan)?;
    register_skill_tool(&mut registry, request.skills)?;
    // History-snip tool (Claude Code HISTORY_SNIP): lets the model free
    // context by dropping clearly-finished earlier segments. Registered
    // unconditionally; the ModelAgent applies the removal to the live window.
    let _ = registry.register(Arc::new(deepagent_builtins::SnipHistoryTool::new()));
    let manifest = prepare_tool_manifest(
        &mut registry,
        request.tool_search_mode,
        request.tool_search_discovered,
        request.tool_search_threshold,
    )?;
    Ok(MainRunToolset {
        registry,
        todo_store,
        manifest,
        hook_mcp_registry: mcp_runtime.hook_registry,
    })
}

fn register_knowledge_search(
    registry: &mut ToolRegistry,
    knowledge: Option<Arc<KnowledgeService>>,
) {
    if let Some(knowledge) = knowledge {
        use deepagent_builtins::KnowledgeSearchTool;
        let backend = crate::knowledge_service::KnowledgeServiceBackend::new(knowledge);
        let _ = registry.register(Arc::new(KnowledgeSearchTool::new(backend)));
    }
}

fn register_project_map_tools(
    registry: &mut ToolRegistry,
    project_map: Option<Arc<ProjectMapService>>,
    root: &Path,
    auto: &CodeIndexAutoBuild,
) -> Result<()> {
    let Some(project_map) = project_map else {
        return Ok(());
    };
    use deepagent_builtins::{
        CodeMapImpactTool, CodeMapNeighborsTool, CodeMapOverviewTool, CodeMapRefreshTool,
        CodeMapSearchTool,
    };
    let backend = ProjectMapToolBackend {
        service: project_map,
        root: root.to_path_buf(),
        auto: auto.clone(),
    };
    registry.register(Arc::new(CodeMapOverviewTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeMapSearchTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeMapNeighborsTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeMapImpactTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeMapRefreshTool::new(backend)))?;
    Ok(())
}

fn register_codegraph_tools(
    registry: &mut ToolRegistry,
    root: &Path,
    auto: &CodeIndexAutoBuild,
) -> Result<()> {
    use deepagent_builtins::{
        CodeGraphCalleesTool, CodeGraphCallersTool, CodeGraphExploreTool, CodeGraphImpactTool,
        CodeGraphLocateTool, CodeGraphNodeTool, CodeGraphSearchTool,
    };
    let backend = CodeGraphToolBackend::new(root.to_path_buf(), auto.clone());
    registry.register(Arc::new(CodeGraphSearchTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphExploreTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphCallersTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphCalleesTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphImpactTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphNodeTool::new(backend.clone())))?;
    registry.register(Arc::new(CodeGraphLocateTool::new(backend)))?;
    Ok(())
}

#[cfg(feature = "runtimes")]
fn register_office_tools(
    registry: &mut ToolRegistry,
    office: Option<Arc<OfficeService>>,
) -> Result<()> {
    let Some(office) = office else {
        return Ok(());
    };
    use deepagent_builtins::{OfficeDocxCreateTool, OfficeReadTool, OfficeXlsxCreateTool};
    let backend = OfficeToolBackend { service: office };
    registry.register(Arc::new(OfficeReadTool::new(backend.clone())))?;
    registry.register(Arc::new(OfficeDocxCreateTool::new(backend.clone())))?;
    registry.register(Arc::new(OfficeXlsxCreateTool::new(backend)))?;
    Ok(())
}

#[cfg(not(feature = "runtimes"))]
fn register_office_tools(
    _registry: &mut ToolRegistry,
    _office: Option<Arc<OfficeService>>,
) -> Result<()> {
    Ok(())
}

fn register_remote_ops_tools(
    registry: &mut ToolRegistry,
    env_mode: Option<&str>,
    connection_id: Option<&str>,
    remote_ops_factory: Option<RemoteOpsFactory>,
) -> Result<()> {
    let (Some("remote"), Some(factory), Some(conn_id)) =
        (env_mode, remote_ops_factory.as_ref(), connection_id)
    else {
        return Ok(());
    };
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
    Ok(())
}

#[cfg(feature = "web")]
fn register_web_tools(registry: &mut ToolRegistry, settings: &SettingsService) -> Result<()> {
    use crate::settings::WebSearchProvider;
    use deepagent_builtins::{ReqwestWebClient, WebFetchTool, WebSearchTool};

    registry.register(Arc::new(WebFetchTool::new(ReqwestWebClient::new())))?;
    let web_settings = settings.web_search_settings()?;
    if !web_settings.enabled {
        return Ok(());
    }

    let anysearch = if web_settings.anysearch_enabled {
        anysearch_config(settings, web_settings.anysearch_base_url.clone())
    } else {
        None
    };
    let searxng_url = match web_settings.provider {
        // DeepSeekFirst is executed by the model provider's native Responses
        // `web_search` tool. Keep a local descriptor registered so the runtime
        // advertises the capability, but never call the legacy Anthropic route.
        WebSearchProvider::DeepSeekFirst => configured_searxng_url(web_settings.searxng_url),
        WebSearchProvider::Searxng => configured_searxng_url(web_settings.searxng_url),
        WebSearchProvider::DuckDuckGo => None,
    };
    registry.register(Arc::new(WebSearchTool::new(
        ReqwestWebClient::with_search_chain(anysearch, searxng_url),
    )))?;
    Ok(())
}

#[cfg(not(feature = "web"))]
fn register_web_tools(_registry: &mut ToolRegistry, _settings: &SettingsService) -> Result<()> {
    Ok(())
}

#[cfg(feature = "web")]
fn configured_searxng_url(setting: Option<String>) -> Option<String> {
    setting
        .or_else(|| std::env::var("DEEPAGENT_SEARXNG_URL").ok())
        .or_else(|| std::env::var("SEARXNG_URL").ok())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "web")]
fn anysearch_config(
    settings: &SettingsService,
    base_url: Option<String>,
) -> Option<deepagent_builtins::AnySearchConfig> {
    use deepagent_builtins::AnySearchConfig;

    let api_key = settings.anysearch_api_key().ok().flatten()?;
    if api_key.trim().is_empty() {
        return None;
    }
    Some(AnySearchConfig::new(
        Some(api_key),
        base_url.unwrap_or_else(|| "https://api.anysearch.com".to_string()),
    ))
}

#[derive(Clone)]
struct ProjectMapToolBackend {
    service: Arc<ProjectMapService>,
    root: PathBuf,
    auto: CodeIndexAutoBuild,
}

#[async_trait]
impl deepagent_builtins::ProjectMapBackend for ProjectMapToolBackend {
    async fn overview(&self) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        serde_json::to_value(self.service.overview(&self.root)).map_err(Into::into)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        let hits = self.service.search(&self.root, query, limit)?;
        Ok(serde_json::json!({
            "count": hits.len(),
            "hits": hits,
        }))
    }

    async fn neighbors(&self, node_id: &str) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        serde_json::to_value(self.service.neighbors(&self.root, node_id)?).map_err(Into::into)
    }

    async fn impact(&self, target: &str) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        serde_json::to_value(self.service.impact(&self.root, target)?).map_err(Into::into)
    }

    async fn refresh(&self) -> Result<serde_json::Value> {
        // Explicit rebuild: run the same deep refresh the UI button uses, off the
        // async runtime (tree-sitter parse + SQLite writes are blocking work).
        let service = self.service.clone();
        let root = self.root.clone();
        let dto = tokio::task::spawn_blocking(move || service.refresh_deep(&root))
            .await
            .map_err(|e| CoreError::other(format!("code_map_refresh task join failed: {e}")))??;
        // A fresh build now exists; let lazy callers take the fast path.
        self.auto.mark_done();
        serde_json::to_value(dto).map_err(Into::into)
    }
}

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
        if !overwrite && Path::new(out_path).exists() {
            return Err(CoreError::invalid(
                "file already exists - pass overwrite=true to replace it",
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
        if !overwrite && Path::new(out_path).exists() {
            return Err(CoreError::invalid(
                "file already exists - pass overwrite=true to replace it",
            ));
        }
        let parsed = parse_office_sheets(&sheets)?;
        self.service.create_xlsx(&parsed, out_path)?;
        Ok(serde_json::json!({ "path": out_path, "ok": true }))
    }
}

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

/// Lazily builds the code index (codegraph.db + the projected UA
/// knowledge-graph.json) the first time a `code_map_*` / `codegraph_*` tool is
/// used on a project whose index is missing.
///
/// The system prompt tells the model to prefer the project map, so the map must
/// build itself on first use instead of returning "not indexed" with no way for
/// the model to fix it. Mirrors grok-build's `code_nav.rs` "first lazy spawn of
/// codebase index". Best-effort and at most one attempt per run: a failure is
/// logged and the tool falls back to its normal missing payload.
#[derive(Clone)]
struct CodeIndexAutoBuild {
    root: PathBuf,
    /// Set once an index exists (built here or already present) so subsequent
    /// tool calls take a lock-free fast path.
    done: Arc<AtomicBool>,
    /// Serializes concurrent first-use tool calls so we build at most once.
    building: Arc<tokio::sync::Mutex<()>>,
}

impl CodeIndexAutoBuild {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            done: Arc::new(AtomicBool::new(false)),
            building: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Mark the index as present without building (e.g. after an explicit
    /// `code_map_refresh`), so lazy callers skip straight to querying.
    fn mark_done(&self) {
        self.done.store(true, Ordering::Relaxed);
    }

    async fn ensure_built(&self) {
        if self.done.load(Ordering::Relaxed) {
            return;
        }
        let _guard = self.building.lock().await;
        if self.done.load(Ordering::Relaxed) {
            return;
        }
        let root = self.root.clone();
        let outcome = tokio::task::spawn_blocking(move || -> Result<bool> {
            let mut graph = deepagent_codegraph::CodeGraph::open(&root)?;
            if graph.has_existing_index() {
                return Ok(false);
            }
            let stats = graph.index_all()?;
            // Keep the human-facing panel graph consistent with the AI index so a
            // model-triggered build also lights up the Project Map panel.
            let ua = root.join(".understand-anything/knowledge-graph.json");
            let _ = graph.project_ua_json(&ua);
            Ok(stats.files_indexed > 0)
        })
        .await;
        // Attempt at most once per run regardless of outcome: on failure the
        // tool returns its normal missing payload rather than rebuilding each call.
        self.done.store(true, Ordering::Relaxed);
        match outcome {
            Ok(Ok(built)) => tracing::info!(
                target: "deepagent::codegraph",
                root = %self.root.display(),
                built,
                "lazy code-index ensure complete"
            ),
            Ok(Err(err)) => tracing::warn!(
                target: "deepagent::codegraph",
                root = %self.root.display(),
                error = %err,
                "lazy code-index build failed"
            ),
            Err(err) => tracing::warn!(
                target: "deepagent::codegraph",
                root = %self.root.display(),
                error = %err,
                "lazy code-index build task join failed"
            ),
        }
    }
}

#[derive(Clone)]
struct CodeGraphToolBackend {
    root: PathBuf,
    auto: CodeIndexAutoBuild,
    graph: Arc<Mutex<Option<deepagent_codegraph::CodeGraph>>>,
}

#[async_trait]
impl deepagent_builtins::CodeGraphBackend for CodeGraphToolBackend {
    async fn search(
        &self,
        query: &str,
        kind: Option<String>,
        limit: usize,
    ) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
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
        self.auto.ensure_built().await;
        self.with_graph(|graph| {
            serde_json::to_value(graph.explore(symbols, parse_explore_budget(&budget))?)
                .map_err(Into::into)
        })
    }

    async fn callers(&self, symbol: &str, limit: usize) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        self.with_graph(|graph| {
            serde_json::to_value(graph.callers(symbol, limit)?).map_err(Into::into)
        })
    }

    async fn callees(&self, symbol: &str, limit: usize) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        self.with_graph(|graph| {
            serde_json::to_value(graph.callees(symbol, limit)?).map_err(Into::into)
        })
    }

    async fn impact(&self, symbol: &str, depth: usize) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        self.with_graph(|graph| {
            serde_json::to_value(graph.impact(symbol, depth)?).map_err(Into::into)
        })
    }

    async fn node(&self, target: &str) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        self.with_graph(|graph| serde_json::to_value(graph.node(target)?).map_err(Into::into))
    }

    async fn node_at_location(&self, file: &str, line: u32) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
        self.with_graph(|graph| {
            serde_json::to_value(graph.store().node_at_location(file, line)?).map_err(Into::into)
        })
    }

    async fn locate(&self, text: &str) -> Result<serde_json::Value> {
        self.auto.ensure_built().await;
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
    fn new(root: PathBuf, auto: CodeIndexAutoBuild) -> Self {
        Self {
            root,
            auto,
            graph: Arc::new(Mutex::new(None)),
        }
    }

    fn with_graph<F>(&self, f: F) -> Result<serde_json::Value>
    where
        F: FnOnce(&deepagent_codegraph::CodeGraph) -> Result<serde_json::Value>,
    {
        let mut cached = self.graph.lock().map_err(|_| {
            CoreError::Persistence("code graph connection cache lock poisoned".to_string())
        })?;
        // (Re)open when there is no cached handle, or when the cached handle was
        // opened before an index existed — a lazy/explicit build may have just
        // populated codegraph.db through a separate connection, and a fresh open
        // picks up those committed rows.
        let needs_open = match cached.as_ref() {
            None => true,
            Some(graph) => !graph.has_existing_index(),
        };
        if needs_open {
            *cached = Some(deepagent_codegraph::CodeGraph::open(&self.root)?);
        }
        let graph = cached
            .as_ref()
            .ok_or_else(|| CoreError::other("code graph connection cache was not initialized"))?;
        if !graph.has_existing_index() {
            return Ok(codegraph_not_indexed());
        }
        f(graph)
    }
}

fn codegraph_not_indexed() -> serde_json::Value {
    serde_json::json!({
        "indexed": false,
        "message": "Code graph index is unavailable (auto-build may have failed or is still running). Call code_map_refresh to (re)build it, then retry, or open the Project Map panel and click Refresh.",
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
