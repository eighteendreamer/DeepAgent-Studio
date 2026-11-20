//! Read-only projection of the same resolution consumed by run preparation.
//! This is a local, versioned host contract, not a provider request schema.
use super::*;
use crate::model_runtime::{resolve_run_model, RunModelPlan};
use crate::plugin_runtime::PluginRuntimeProjection;
use serde_json::{json, Value};

pub(super) struct ResolvedExecutionConfig {
    pub(super) environment: RunEnvironment,
    pub(super) permission_rules: deepagent_hooks::PermissionRules,
    pub(super) browser_requires_approval: bool,
    pub(super) model_plan: Result<Option<RunModelPlan>>,
    pub(super) snapshot: Value,
    pub(super) project_trusted: bool,
    pub(super) search_mode: deepagent_builtins::ToolSearchMode,
    pub(super) search_threshold: usize,
}

impl ResolvedExecutionConfig {
    /// The tool builder may observe a newer host publication. Bind the config
    /// identity to the generation it actually froze, before admission commits.
    pub(super) fn bind_mcp_generation(&mut self, generation: Option<&str>) -> Result<()> {
        self.snapshot["mcpGeneration"] = generation.into();
        self.snapshot
            .as_object_mut()
            .unwrap()
            .remove("configSnapshotId");
        self.snapshot["configSnapshotId"] =
            crate::run_capabilities::capability_hash(&self.snapshot)?.into();
        Ok(())
    }
}

impl RunAssembler<'_> {
    pub(crate) fn read_execution_config(&self, overrides: &HarnessRunOverrides) -> Result<Value> {
        let plugin_snapshot = self
            .plugins
            .as_ref()
            .map(|plugins| plugins.ready_snapshot());
        Ok(self
            .resolve_execution_config(
                self.workspace,
                overrides,
                plugin_snapshot.as_ref().map(|(projection, _)| projection),
                plugin_snapshot
                    .as_ref()
                    .map(|(_, registry)| registry.clone()),
                None,
            )?
            .snapshot)
    }

    pub(super) fn resolve_execution_config(
        &self,
        root: &Path,
        overrides: &HarnessRunOverrides,
        plugin_projection: Option<&PluginRuntimeProjection>,
        plugin_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
        run_id: Option<&str>,
    ) -> Result<ResolvedExecutionConfig> {
        // One committed Settings document supplies the coupled policy/model
        // defaults. Never call independent getters that straddle a writer.
        let settings = self.settings.load()?;
        let config = RunConfigOverlay::load(root, serde_json::to_value(overrides)?);
        let trusted = crate::trust_service::TrustService::new(self.db.clone()).is_trusted(root);
        let legacy_executor = run_id.and_then(|_| {
            self.sandbox_backend_registry
                .is_none()
                .then_some(self.sandboxie_executor.as_ref())
                .flatten()
        });
        let environment = RunEnvironment::resolve(
            config,
            SettingsService::permission_profile_from_settings(settings.as_ref()),
            trusted,
            if run_id.is_some() {
                self.runtime_logs
            } else {
                &None
            },
            legacy_executor,
            self.sandbox_backend_registry
                .as_ref()
                .map(|registry| registry.default_kind()),
            run_id.unwrap_or_default(),
        )?;
        let config = &environment.config;
        let permission_rules = config.merged_permission_rules(
            settings
                .as_ref()
                .map(|s| s.permission_rules.clone())
                .unwrap_or_default(),
        )?;
        let model_plan = settings
            .as_ref()
            .map(|loaded| {
                resolve_run_model(
                    self.db,
                    loaded,
                    config.provider_override(),
                    config.model_override(),
                    config.reasoning_effort_override(),
                )
            })
            .transpose();
        let model = model_plan.as_ref().ok().and_then(Option::as_ref);
        let status = match &model_plan {
            Ok(Some(_)) => "resolved",
            Ok(None) => "uninitialized",
            Err(_) => "invalid",
        };
        let profile = environment.profile;
        let search_mode = settings
            .as_ref()
            .map(|s| s.tool_search_mode)
            .unwrap_or_default();
        let search_threshold = settings
            .as_ref()
            .and_then(|s| s.tool_search_auto_threshold_chars)
            .unwrap_or(SettingsService::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS);
        let root = std::fs::canonicalize(root)
            .map_err(|_| CoreError::invalid("configuration workspace is unavailable"))?;
        let plugin_hash = plugin_projection
            .map(crate::run_capabilities::capability_hash)
            .transpose()?;
        let mcp_generation = self
            .mcp
            .as_ref()
            .map(|mcp| {
                crate::mcp_runtime::runtime_snapshot(mcp, plugin_registry)
                    .map(|(_, _, generation)| generation)
            })
            .transpose()?;
        let sources: Vec<_> = config
            .layers
            .iter()
            .map(|layer| {
                json!({
                    "scope":format!("{:?}",layer.scope()), "precedence":layer.precedence,
                    "permissionsEligible": !layer.is_project_scoped() || trusted,
                })
            })
            .collect();
        // Raw settings, rules, paths, overlay errors and plugin definitions are
        // never serialized. The digest is comparison metadata, not a grant.
        let input_hash = crate::run_capabilities::capability_hash(&json!({
            "workspace":root, "settings":settings, "overlay":config.value,
            "layers":config.layers, "permissionRules":permission_rules,
        }))?;
        let snapshot = json!({
            "configSnapshotVersion":1, "resolutionStatus":status,
            "credentialChecked":false, "inputHash":input_hash,
            "provider":model.map(|plan| &plan.provider_id),
            "protocol":model.map(|plan| &plan.protocol),
            "providerRevision":model.map(|plan| plan.provider_revision),
            "modelConfigHash":model.map(|plan| &plan.config_hash),
            "model":model.map(|plan| &plan.model),
            "reasoningEffort":model.map(|plan| plan.reasoning_effort),
            "modelOverrideApplied":model.map(|plan| plan.override_applied),
            "permissionProfile":{
                "approvalPolicy":profile.approval_policy.label(),
                "sandboxMode":profile.sandbox_mode.label(),
                "localExecutionMode":format!("{:?}",profile.local_execution_mode),
                "networkAlwaysAsk":profile.network_always_ask,
            },
            "sandboxBackend":environment.sandbox_backend.map(crate::SandboxBackendKind::label),
            "projectTrusted":trusted, "sources":sources, "loadErrorCount":config.errors.len(),
            "pluginProjectionHash":plugin_hash, "mcpGeneration":mcp_generation,
            "toolSearch":{"mode":search_mode.label(),"threshold":search_threshold},
            "capabilitiesScope":"tool/list uses the admitted run snapshot",
        });
        let mut resolved = ResolvedExecutionConfig {
            environment,
            permission_rules,
            browser_requires_approval: settings
                .as_ref()
                .is_some_and(|s| s.browser.require_approval),
            model_plan,
            snapshot,
            project_trusted: trusted,
            search_mode,
            search_threshold,
        };
        resolved.bind_mcp_generation(mcp_generation.as_deref())?;
        Ok(resolved)
    }
}
