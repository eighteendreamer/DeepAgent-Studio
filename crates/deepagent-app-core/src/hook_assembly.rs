use std::path::Path;
use std::sync::Arc;

use deepagent_builtins::WorkspaceRoot;
use deepagent_core::error::Result;
use deepagent_hooks::{
    Hook, HookActionExecutor, HookCommandRunner, HookDefinitions, HookPoint, HookRegistry,
    PermissionRulesHook,
};
use deepagent_models::{ModelClient, ThinkingDepth};
use deepagent_runtime::RuntimeEventSink;
use deepagent_tools::ToolRegistry;

use crate::hook_runtime::{build_hook_agent_registry, AppHookActionExecutor, ObservableHookRunner};
use crate::plugin_runtime::PluginRuntimeProjection;
use crate::run_config::RunConfigOverlay;
use crate::settings::SettingsService;

pub(crate) struct HookAssemblyRequest<'a> {
    pub(crate) settings: &'a SettingsService,
    pub(crate) run_config: &'a RunConfigOverlay,
    pub(crate) project_hooks: Option<HookDefinitions>,
    pub(crate) plugin_projection: Option<&'a PluginRuntimeProjection>,
    pub(crate) root: &'a Path,
    pub(crate) sink: Arc<dyn RuntimeEventSink>,
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) thinking_depth: ThinkingDepth,
    pub(crate) mcp: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(crate) registry: &'a ToolRegistry,
    pub(crate) plan: deepagent_builtins::PlanMode,
    pub(crate) office_skill_guard: Option<Arc<dyn Hook>>,
    pub(crate) access: deepagent_builtins::FsAccess,
    pub(crate) bash_allow: Vec<String>,
    pub(crate) bash_full_access: bool,
}

pub(crate) struct HookAssemblyResult {
    pub(crate) hooks: Arc<HookRegistry>,
}

pub(crate) fn assemble_run_hooks(request: HookAssemblyRequest<'_>) -> Result<HookAssemblyResult> {
    // Multi-source permission rules: UI settings participate as a user-scope
    // source; every config layer (plugin/user/project/local/run/managed) is
    // set-unioned with deny > ask > allow enforced across the union, and
    // managed layers are supreme (deny/ask from below cannot veto a managed
    // allow, and nothing below can drop a managed deny/ask).
    let ui_rules = request.settings.permission_rules().unwrap_or_default();
    let rules = request.run_config.merged_permission_rules(ui_rules)?;

    let mut hooks = HookRegistry::new();
    let hook_action_executor: Arc<dyn HookActionExecutor> = Arc::new(AppHookActionExecutor {
        client: request.client.clone(),
        model: request.model.clone(),
        thinking_depth: request.thinking_depth,
        mcp: request.mcp,
        agent_registry: Arc::new(build_hook_agent_registry(request.registry)?),
        events: Arc::downgrade(&request.sink),
    });

    hooks.register(
        HookPoint::BeforeToolUse,
        Arc::new(deepagent_builtins::PlanModeHook::new(request.plan)),
    );
    if let Some(office_skill_guard) = request.office_skill_guard {
        hooks.register(HookPoint::BeforeToolUse, office_skill_guard.clone());
        hooks.register(HookPoint::AfterToolUse, office_skill_guard);
    }
    if !rules.is_empty() {
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(PermissionRulesHook::new(rules)),
        );
    }

    register_user_hooks(
        &mut hooks,
        request.settings,
        &request.sink,
        &hook_action_executor,
    );
    register_run_config_hooks(
        &mut hooks,
        request.run_config,
        request.root,
        &request.sink,
        &hook_action_executor,
    )?;
    register_project_hooks(
        &mut hooks,
        request.project_hooks,
        request.root,
        &request.sink,
        &hook_action_executor,
    );
    register_plugin_hooks(
        &mut hooks,
        request.plugin_projection,
        &request.sink,
        &hook_action_executor,
    );

    deepagent_builtins::register_guard_hooks_with_bash_full_access(
        &mut hooks,
        WorkspaceRoot::new(request.root.to_path_buf()).with_access(request.access),
        request.bash_allow,
        request.bash_full_access,
    );

    Ok(HookAssemblyResult {
        hooks: Arc::new(hooks),
    })
}

fn register_user_hooks(
    hooks: &mut HookRegistry,
    settings: &SettingsService,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
) {
    match settings.hook_definitions() {
        Ok(defs) if !defs.is_empty() => {
            let runner: Arc<dyn HookCommandRunner> =
                Arc::new(ObservableHookRunner::new(sink.clone()));
            let n = defs.register_into_with_host(hooks, runner, host.clone());
            tracing::info!(count = n, "registered external hooks from hooks.json");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "ignoring malformed hooks.json");
        }
    }
}

fn register_run_config_hooks(
    hooks: &mut HookRegistry,
    run_config: &RunConfigOverlay,
    root: &Path,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
) -> Result<()> {
    match run_config.hook_definitions() {
        Ok(Some(defs)) if !defs.is_empty() => {
            let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new_in_dir(
                sink.clone(),
                root.to_path_buf(),
            ));
            let n = defs.register_into_with_host(hooks, runner, host.clone());
            tracing::info!(
                count = n,
                project = root.display().to_string(),
                "registered external hooks from run config overlay"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                project = root.display().to_string(),
                error = %e,
                "ignoring malformed run config hooks"
            );
        }
    }
    Ok(())
}

fn register_project_hooks(
    hooks: &mut HookRegistry,
    project_hooks: Option<HookDefinitions>,
    root: &Path,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
) {
    if let Some(defs) = project_hooks.filter(|defs| !defs.is_empty()) {
        let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new_in_dir(
            sink.clone(),
            root.to_path_buf(),
        ));
        let n = defs.register_into_with_host(hooks, runner, host.clone());
        tracing::info!(
            count = n,
            project = root.display().to_string(),
            "registered external hooks from project .deepagent/hooks.json"
        );
    }
}

fn register_plugin_hooks(
    hooks: &mut HookRegistry,
    plugin_projection: Option<&PluginRuntimeProjection>,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
) {
    if let Some(projection) = plugin_projection {
        if !projection.hook_definitions.is_empty() {
            let runner: Arc<dyn HookCommandRunner> =
                Arc::new(ObservableHookRunner::new(sink.clone()));
            let n =
                projection
                    .hook_definitions
                    .register_into_with_host(hooks, runner, host.clone());
            tracing::info!(count = n, "registered external hooks from enabled plugins");
        }
        for error in &projection.errors {
            tracing::warn!(
                plugin = error.plugin_id.as_str(),
                component = error.component.as_str(),
                path = error.path.as_deref().unwrap_or(""),
                message = error.message.as_str(),
                "plugin runtime projection error"
            );
        }
    }
}
