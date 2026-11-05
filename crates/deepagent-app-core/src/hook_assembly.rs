use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_builtins::WorkspaceRoot;
use deepagent_core::error::Result;
use deepagent_hooks::{
    DecisionSource, Hook, HookActionExecutor, HookCommandRunner, HookContext, HookData,
    HookDefinitions, HookOutcome, HookPoint, HookRegistry, PermissionRules, PermissionRulesHook,
};
use deepagent_models::{ModelClient, ReasoningEffort};
use deepagent_runtime::RuntimeEventSink;
use deepagent_tools::{PermissionSet, ToolExecutionContext, ToolRegistry};

use crate::chat_service::InvokedSkillMap;
use crate::hook_runtime::{AppHookActionExecutor, ObservableHookRunner};
use crate::plugin_runtime::PluginRuntimeProjection;
use crate::project_service::ProjectService;
use crate::run_config::RunConfigOverlay;
use crate::settings::SettingsService;

pub(crate) struct HookAssemblyRequest<'a> {
    pub(crate) settings: &'a SettingsService,
    pub(crate) run_config: &'a RunConfigOverlay,
    pub(crate) permission_rules: PermissionRules,
    pub(crate) project_hooks: Option<HookDefinitions>,
    pub(crate) plugin_projection: Option<&'a PluginRuntimeProjection>,
    pub(crate) root: &'a Path,
    pub(crate) sink: Arc<dyn RuntimeEventSink>,
    pub(crate) usage_owner: Option<crate::call_usage::RunCallUsageOwner>,
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) mcp: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(crate) registry: &'a ToolRegistry,
    pub(crate) permissions: PermissionSet,
    pub(crate) plan: deepagent_builtins::PlanMode,
    pub(crate) office_skill_guard: Option<Arc<dyn Hook>>,
    pub(crate) access: deepagent_builtins::FsAccess,
    pub(crate) bash_allow: Vec<String>,
    pub(crate) bash_full_access: bool,
    /// Whether the project root is trusted (§6.2). When enforcement is on and
    /// this is `false`, bash/shell escalates to approval.
    pub(crate) is_trusted: bool,
    pub(crate) project_hook_trust: Arc<ProjectHookTrust>,
    pub(crate) runtime_environment: BTreeMap<String, String>,
}

pub(crate) struct HookAssemblyResult {
    pub(crate) hooks: Arc<HookRegistry>,
}

/// Project hooks use the explicit hook grant, not the opt-in shell approval
/// TrustGuard. Keep the run's root fixed, but read grants at each dispatch so
/// revocation affects already registered lifecycle/sub-agent hooks as well.
pub(crate) struct ProjectHookTrust {
    projects: Option<Arc<ProjectService>>,
    root: PathBuf,
}

impl ProjectHookTrust {
    pub(crate) fn new(projects: Option<Arc<ProjectService>>, root: PathBuf) -> Self {
        Self { projects, root }
    }

    pub(crate) fn is_trusted(&self) -> Result<bool> {
        match &self.projects {
            Some(projects) => projects.hooks_trusted(&self.root.to_string_lossy()),
            None => Ok(false),
        }
    }

    fn wrap(self: &Arc<Self>, inner: Arc<dyn Hook>) -> Arc<dyn Hook> {
        Arc::new(TrustedProjectHook {
            trust: self.clone(),
            inner,
        })
    }
}

struct TrustedProjectHook {
    trust: Arc<ProjectHookTrust>,
    inner: Arc<dyn Hook>,
}

#[async_trait]
impl Hook for TrustedProjectHook {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dedup_key(&self) -> String {
        self.inner.dedup_key()
    }

    fn applies(&self, ctx: &HookContext) -> bool {
        self.inner.applies(ctx)
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        self.run_with_context(ctx, ToolExecutionContext::default())
            .await
    }

    async fn run_with_context(
        &self,
        ctx: &HookContext,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        if !self.trust.is_trusted()? {
            tracing::info!(project = %self.trust.root.display(), point = ctx.point.label(),
                "skipping project hook without current trust grant");
            return Ok(HookOutcome::Continue);
        }
        self.inner.run_with_context(ctx, execution).await
    }
}

pub(crate) fn assemble_run_hooks(request: HookAssemblyRequest<'_>) -> Result<HookAssemblyResult> {
    // The effective rules are resolved once by the run assembler and shared
    // with automatic context construction, so hooks and model-visible
    // enrichment cannot observe different permission snapshots.
    let rules = request.permission_rules.clone();

    // One policy assembly, shared hook objects. Capture it BEFORE adding any
    // lifecycle extensions: installing those in an agent-hook would recurse.
    let policy_hooks = Arc::new(assemble_run_policy_hooks(&request, rules));
    let mut hooks = policy_hooks.as_ref().clone();
    let hook_action_executor: Arc<dyn HookActionExecutor> = Arc::new(AppHookActionExecutor {
        client: request.client.clone(),
        model: request.model.clone(),
        reasoning_effort: request.reasoning_effort,
        mcp: request.mcp,
        agent_registry: Arc::new(request.registry.clone()),
        agent_policy: policy_hooks,
        agent_permissions: request.permissions,
        events: Arc::downgrade(&request.sink),
        usage_owner: request.usage_owner,
    });

    // Skill-use bookkeeping belongs to the parent session, not its isolated
    // hook conversation. It is not an authorization policy to clone.
    if let Some(office_skill_guard) = request.office_skill_guard {
        hooks.register(HookPoint::BeforeToolUse, office_skill_guard.clone());
        hooks.register(HookPoint::AfterToolUse, office_skill_guard);
    }
    register_user_hooks(
        &mut hooks,
        request.settings,
        &request.sink,
        &hook_action_executor,
        &request.runtime_environment,
    );
    let overlay_hooks = match request
        .run_config
        .hook_definitions(request.project_hook_trust.is_trusted()?)
    {
        Ok(defs) => defs,
        Err(error) => {
            tracing::warn!(project = %request.root.display(), %error,
                "ignoring malformed run config hooks");
            Default::default()
        }
    };
    register_run_config_hooks(
        &mut hooks,
        overlay_hooks.host,
        request.root,
        &request.sink,
        &hook_action_executor,
        &request.runtime_environment,
    );
    let mut project_hooks = overlay_hooks.project;
    if let Some(defs) = request.project_hooks {
        crate::plugin_runtime::merge_hook_definitions(&mut project_hooks, defs);
    }
    register_plugin_hooks(
        &mut hooks,
        request.plugin_projection,
        &request.sink,
        &hook_action_executor,
        &request.runtime_environment,
    );
    // Register host-authorized sources first. An equivalent project handler
    // must not shadow an enabled user/managed/plugin hook after revocation;
    // forwarding the original key still executes duplicates only once.
    register_project_hooks(
        &mut hooks,
        project_hooks,
        &request.project_hook_trust,
        request.root,
        &request.sink,
        &hook_action_executor,
        &request.runtime_environment,
    );

    Ok(HookAssemblyResult {
        hooks: Arc::new(hooks),
    })
}

/// The run's policy-only projection. Both runtimes execute these same guards;
/// only the parent adds extension callbacks. No policy is reconstructed from
/// hook tool schemas or from a second read of settings inside the child.
fn assemble_run_policy_hooks(
    request: &HookAssemblyRequest<'_>,
    rules: PermissionRules,
) -> HookRegistry {
    let mut hooks = HookRegistry::new();
    hooks.register(
        HookPoint::BeforeToolUse,
        Arc::new(deepagent_builtins::PlanModeHook::new(request.plan.clone())),
    );
    if !rules.is_empty() {
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(PermissionRulesHook::new(rules)),
        );
    }
    deepagent_builtins::register_guard_hooks_with_bash_full_access(
        &mut hooks,
        WorkspaceRoot::new(request.root.to_path_buf()).with_access(request.access),
        request.bash_allow.clone(),
        request.bash_full_access,
    );

    // Opt-in advanced safeguards resolve settings-first, with the matching
    // `DEEPAGENT_*` env var as a force-enable override (power-user / CI).
    let features = request.settings.execution_features();

    // §6.1 LLM command-injection layer (opt-in, defense-in-depth atop the
    // always-on heuristic). Additive `BeforeToolUse` hook: escalates
    // structurally-suspicious commands to approval via a lightweight DeepSeek
    // pass. Fail-open and Ask-only; registered only when enabled so the
    // default build adds no per-command model calls.
    if !request.bash_full_access
        && (features.command_guard || crate::command_guard_llm::llm_command_guard_enabled())
    {
        let classifier: Arc<dyn crate::command_guard_llm::CommandClassifier> =
            Arc::new(crate::command_guard_llm::ModelCommandClassifier::new(
                request.client.clone(),
                request.model.clone(),
            ));
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(crate::command_guard_llm::LlmCommandGuardHook::new(
                classifier,
            )),
        );
        tracing::info!("registered LLM command-injection guard (BeforeToolUse)");
    }

    // §6.2 per-project trust gate (opt-in). While a project is untrusted,
    // escalate bash/shell to approval — even allow-listed commands that would
    // otherwise auto-run wait for the user. Ask-only; once the user trusts the
    // project the gate is not registered.
    if (features.project_trust || crate::trust_service::project_trust_enforced())
        && !request.is_trusted
    {
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(crate::trust_service::TrustGuardHook::new()),
        );
        tracing::info!(
            project = request.root.display().to_string(),
            "project untrusted: registered trust gate (bash → approval)"
        );
    }

    hooks
}

fn register_user_hooks(
    hooks: &mut HookRegistry,
    settings: &SettingsService,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
    runtime_environment: &BTreeMap<String, String>,
) {
    match settings.hook_definitions() {
        Ok(defs) if !defs.is_empty() => {
            let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new(
                sink.clone(),
                runtime_environment.clone(),
            ));
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
    defs: HookDefinitions,
    root: &Path,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
    runtime_environment: &BTreeMap<String, String>,
) {
    if !defs.is_empty() {
        let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new_in_dir(
            sink.clone(),
            root.to_path_buf(),
            runtime_environment.clone(),
        ));
        let n = defs.register_into_with_host(hooks, runner, host.clone());
        tracing::info!(
            count = n,
            project = root.display().to_string(),
            "registered external hooks from run config overlay"
        );
    }
}

fn register_project_hooks(
    hooks: &mut HookRegistry,
    mut defs: HookDefinitions,
    trust: &Arc<ProjectHookTrust>,
    root: &Path,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
    runtime_environment: &BTreeMap<String, String>,
) {
    if !defs.is_empty() {
        for groups in defs.hooks.values_mut() {
            for group in groups {
                for action in &mut group.hooks {
                    action.env.insert(
                        "DEEPAGENT_PROJECT_ROOT".into(),
                        root.to_string_lossy().into_owned(),
                    );
                }
            }
        }
        let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new_in_dir(
            sink.clone(),
            root.to_path_buf(),
            runtime_environment.clone(),
        ));
        let n = defs
            .register_into_with_host_wrapped(hooks, runner, host.clone(), |hook| trust.wrap(hook));
        tracing::info!(
            count = n,
            project = root.display().to_string(),
            "registered trusted project hooks from config layers and hooks.json"
        );
    }
}

fn register_plugin_hooks(
    hooks: &mut HookRegistry,
    plugin_projection: Option<&PluginRuntimeProjection>,
    sink: &Arc<dyn RuntimeEventSink>,
    host: &Arc<dyn HookActionExecutor>,
    runtime_environment: &BTreeMap<String, String>,
) {
    if let Some(projection) = plugin_projection {
        if !projection.hook_definitions.is_empty() {
            let runner: Arc<dyn HookCommandRunner> = Arc::new(ObservableHookRunner::new(
                sink.clone(),
                runtime_environment.clone(),
            ));
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

#[derive(Clone)]
pub(crate) struct OfficeSkillGuardHook {
    invoked_skills: InvokedSkillMap,
    enforce_skills: std::collections::HashSet<String>,
}

impl OfficeSkillGuardHook {
    pub(crate) fn new(
        invoked_skills: InvokedSkillMap,
        enforce_skills: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            invoked_skills,
            enforce_skills,
        }
    }

    pub(crate) fn seed_session(&self, session_id: &str, skills: std::collections::HashSet<String>) {
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

#[cfg(test)]
#[path = "hook_policy_tests.rs"]
mod policy_tests;

#[cfg(test)]
mod project_hook_tests {
    use super::*;
    use deepagent_core::id::SessionId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter(Arc<AtomicUsize>);

    #[async_trait]
    impl Hook for Counter {
        fn name(&self) -> &str {
            "fixture_counter"
        }
        async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(HookOutcome::Continue)
        }
    }

    #[tokio::test]
    async fn project_hook_trust_is_rechecked_for_lifecycle_and_cloned_registries() {
        let db = Arc::new(deepagent_persistence::Database::open_in_memory().unwrap());
        let projects = Arc::new(ProjectService::new(db));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().into_owned();
        projects.add_project(&path).unwrap();
        let trust = Arc::new(ProjectHookTrust::new(
            Some(projects.clone()),
            root.path().to_owned(),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = HookRegistry::new();
        let points = [
            HookPoint::SessionStart,
            HookPoint::SessionEnd,
            HookPoint::BeforeToolUse,
            HookPoint::SubagentStart,
            HookPoint::SubagentStop,
            HookPoint::Stop,
            HookPoint::Notification,
        ];
        for point in points {
            registry.register(point, trust.wrap(Arc::new(Counter(calls.clone()))));
        }
        // Sub-agents and late owners retain this same registry, not a copied
        // boolean grant. Revocation must therefore be visible through clones.
        let child = registry.clone();
        for trusted in [false, true, false] {
            projects.set_hooks_trusted(&path, trusted).unwrap();
            let before = calls.load(Ordering::SeqCst);
            for point in points {
                child
                    .dispatch(&HookContext::new(SessionId::nil(), point, HookData::None))
                    .await
                    .unwrap();
            }
            assert_eq!(
                calls.load(Ordering::SeqCst) - before,
                if trusted { points.len() } else { 0 }
            );
        }
        // No implicit trust from a different active/opened project.
        projects.add_project("other-project").unwrap();
        projects.set_hooks_trusted("other-project", true).unwrap();
        assert!(!trust.is_trusted().unwrap());
    }

    #[tokio::test]
    async fn project_hook_trust_skip_does_not_consume_once_or_duplicate_host_actions() {
        let projects = Arc::new(ProjectService::new(Arc::new(
            deepagent_persistence::Database::open_in_memory().unwrap(),
        )));
        projects.add_project("fixture").unwrap();
        let trust = Arc::new(ProjectHookTrust::new(
            Some(projects.clone()),
            "fixture".into(),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let hook = trust.wrap(deepagent_hooks::OnceHook::new(Arc::new(Counter(
            calls.clone(),
        ))));
        let ctx = HookContext::new(SessionId::nil(), HookPoint::SessionStart, HookData::None);
        hook.run(&ctx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        projects.set_hooks_trusted("fixture", true).unwrap();
        hook.run(&ctx).await.unwrap();
        hook.run(&ctx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut registry = HookRegistry::new();
        registry.register(ctx.point, Arc::new(Counter(calls.clone())));
        registry.register(ctx.point, trust.wrap(Arc::new(Counter(calls.clone()))));
        for trusted in [true, false] {
            projects.set_hooks_trusted("fixture", trusted).unwrap();
            let before = calls.load(Ordering::SeqCst);
            registry.dispatch(&ctx).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst) - before, 1);
        }
    }

    #[tokio::test]
    async fn h240_project_authorization_and_native_matching_both_precede_once_admission() {
        struct Host(Arc<AtomicUsize>);
        #[async_trait]
        impl deepagent_hooks::HookActionExecutor for Host {
            async fn execute(
                &self,
                _: &deepagent_hooks::HookAction,
                _: serde_json::Value,
            ) -> Result<HookOutcome> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(HookOutcome::deny("synthetic project gate"))
            }
        }
        let projects = Arc::new(ProjectService::new(Arc::new(
            deepagent_persistence::Database::open_in_memory().unwrap(),
        )));
        projects.add_project("synthetic-once-project").unwrap();
        let trust = Arc::new(ProjectHookTrust::new(
            Some(projects.clone()),
            "synthetic-once-project".into(),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let definitions = HookDefinitions::parse(
            r#"{"hooks":{"PreToolUse":[{
            "matcher":"write_file","if":"write_file(allowed.txt)","once":true,
            "hooks":[{"type":"mcp_tool","command":"mcp__synthetic__gate"}]}]}}"#,
        )
        .unwrap();
        let ctx = |name, path| {
            HookContext::new(
                SessionId::nil(),
                HookPoint::BeforeToolUse,
                deepagent_hooks::HookData::before_tool(name, serde_json::json!({"path":path})),
            )
        };
        let read = ctx("read_file", "primer");
        let write = ctx("write_file", "allowed.txt");
        let mut registry = HookRegistry::new();
        definitions.register_into_with_host_wrapped(
            &mut registry,
            Arc::new(deepagent_hooks::SystemHookRunner),
            Arc::new(Host(calls.clone())),
            |inner| {
                let outer = trust.wrap(inner);
                assert!(!outer.applies(&read));
                assert!(outer.applies(&write)); // A match is not a trust grant.
                outer
            },
        );
        let cloned = registry.clone();
        assert_eq!(
            cloned.dispatch(&write).await.unwrap(),
            HookOutcome::Continue
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        projects
            .set_hooks_trusted("synthetic-once-project", true)
            .unwrap();
        assert_eq!(cloned.dispatch(&read).await.unwrap(), HookOutcome::Continue);
        assert_eq!(
            cloned.dispatch(&ctx("write_file", "other")).await.unwrap(),
            HookOutcome::Continue
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(registry.dispatch(&write).await.unwrap().is_deny());
        for trusted in [false, true] {
            projects
                .set_hooks_trusted("synthetic-once-project", trusted)
                .unwrap();
            assert_eq!(
                cloned.dispatch(&write).await.unwrap(),
                HookOutcome::Continue
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
