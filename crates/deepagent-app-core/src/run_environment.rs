use std::path::Path;
use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};

use crate::run_config::RunConfigOverlay;
use crate::runtime_event_log::append_runtime_log;
use crate::sandboxie_service::SandboxieExecutor;
use crate::settings::{
    ApprovalPolicy, EffectivePermissionProfile, LocalExecutionMode, SandboxMode, SettingsService,
};

#[derive(Debug, Clone)]
pub(crate) struct RunEnvironment {
    pub(crate) config: RunConfigOverlay,
    pub(crate) profile: EffectivePermissionProfile,
    pub(crate) policy: ApprovalPolicy,
    pub(crate) sandbox_mode: SandboxMode,
    pub(crate) local_execution_mode: LocalExecutionMode,
    pub(crate) access: deepagent_builtins::FsAccess,
}

impl RunEnvironment {
    pub(crate) fn resolve(
        root: &Path,
        settings: &SettingsService,
        runtime_logs: &Option<Arc<RuntimeLogStore>>,
        sandboxie_executor: &Option<Arc<SandboxieExecutor>>,
        run_id: &str,
    ) -> Result<Self> {
        let config = RunConfigOverlay::load(root);
        // Surface the resolved managed-policy directory so a missing managed
        // layer is diagnosable at a glance (manual acceptance M-09: the
        // DEEPAGENT_MANAGED_SETTINGS_DIR env var did not propagate to the app
        // process, so managed-settings.json was silently absent). `sources`
        // lists what actually loaded; `managed_dir`/`managed_dir_exists` show
        // where we looked.
        let managed_dir = deepagent_context::default_managed_dir();
        let managed_dir_exists = managed_dir
            .as_ref()
            .map(|dir| dir.exists())
            .unwrap_or(false);
        append_runtime_log(
            runtime_logs,
            NewRuntimeLogEntry::info("config", "run_config_overlay_loaded")
                .with_run_id(run_id)
                .with_source("deepagent-app-core::run_environment")
                .with_message("run configuration overlay loaded")
                .with_data(serde_json::json!({
                    "sources": config.sources.clone(),
                    "errors": config.errors.clone(),
                    "managed_dir": managed_dir
                        .as_ref()
                        .map(|dir| dir.to_string_lossy().to_string()),
                    "managed_dir_exists": managed_dir_exists,
                    "has_permissions": config.value.get("permissions").is_some()
                        || config.value.get("permission_rules").is_some(),
                    "has_hooks": config.value.get("hooks").is_some()
                        || config.value.get("hooks_json").is_some()
                        || config.value.get("hooksJson").is_some(),
                    "has_approval_policy": config.value.get("approval_policy").is_some()
                        || config.value.get("approvalPolicy").is_some(),
                    "has_sandbox_mode": config.value.get("sandbox_mode").is_some()
                        || config.value.get("sandboxMode").is_some(),
                })),
        );

        let profile = config.apply_permission_profile(settings.effective_permission_profile()?)?;
        let policy = profile.approval_policy;
        let sandbox_mode = profile.sandbox_mode;
        let local_execution_mode = profile.local_execution_mode;
        let access = fs_access_for(sandbox_mode);
        append_runtime_log(
            runtime_logs,
            NewRuntimeLogEntry::info("permission", "effective_profile")
                .with_run_id(run_id)
                .with_source("deepagent-app-core::run_environment")
                .with_message("effective permission profile resolved")
                .with_data(serde_json::json!({
                    "approval_policy": policy.label(),
                    "sandbox_mode": sandbox_mode.label(),
                    "local_execution_mode": format!("{local_execution_mode:?}"),
                    "fs_access": format!("{access:?}"),
                })),
        );
        if let Some(sandboxie) = sandboxie_executor {
            sandboxie.set_sandbox_mode(sandbox_mode);
        }

        Ok(Self {
            config,
            profile,
            policy,
            sandbox_mode,
            local_execution_mode,
            access,
        })
    }
}

/// Map app sandbox mode to built-in filesystem access used by file tools and
/// path guard.
pub(crate) fn fs_access_for(mode: SandboxMode) -> deepagent_builtins::FsAccess {
    use deepagent_builtins::FsAccess;
    match mode {
        SandboxMode::ReadOnly => FsAccess::ReadOnly,
        SandboxMode::WorkspaceWrite => FsAccess::Workspace,
        SandboxMode::FullAccess => FsAccess::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_access_tracks_sandbox_mode() {
        assert_eq!(
            fs_access_for(SandboxMode::ReadOnly),
            deepagent_builtins::FsAccess::ReadOnly
        );
        assert_eq!(
            fs_access_for(SandboxMode::WorkspaceWrite),
            deepagent_builtins::FsAccess::Workspace
        );
        assert_eq!(
            fs_access_for(SandboxMode::FullAccess),
            deepagent_builtins::FsAccess::Full
        );
    }
}
