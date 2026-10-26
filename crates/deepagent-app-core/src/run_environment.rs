use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};

use crate::run_config::RunConfigOverlay;
use crate::runtime_event_log::append_runtime_log;
use crate::sandbox_backend::SandboxBackendKind;
use crate::sandboxie_service::SandboxieExecutor;
use crate::settings::{
    ApprovalPolicy, EffectivePermissionProfile, LocalExecutionMode, SandboxMode,
};

#[derive(Debug, Clone)]
pub(crate) struct RunEnvironment {
    pub(crate) config: RunConfigOverlay,
    pub(crate) profile: EffectivePermissionProfile,
    pub(crate) policy: ApprovalPolicy,
    pub(crate) sandbox_mode: SandboxMode,
    pub(crate) local_execution_mode: LocalExecutionMode,
    pub(crate) sandbox_backend: Option<SandboxBackendKind>,
    pub(crate) access: deepagent_builtins::FsAccess,
}

impl RunEnvironment {
    pub(crate) fn resolve(
        config: RunConfigOverlay,
        base_profile: EffectivePermissionProfile,
        project_trusted: bool,
        runtime_logs: &Option<Arc<RuntimeLogStore>>,
        sandboxie_executor: Option<&Arc<SandboxieExecutor>>,
        default_sandbox_backend: Option<SandboxBackendKind>,
        run_id: &str,
    ) -> Result<Self> {
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
                    "project_permissions_trusted": project_trusted,
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

        let profile = config.apply_permission_profile(base_profile, project_trusted)?;
        let policy = profile.approval_policy;
        let sandbox_mode = profile.sandbox_mode;
        let local_execution_mode = profile.local_execution_mode;
        let sandbox_backend = match config.value.get("sandbox_backend") {
            None => default_sandbox_backend,
            Some(serde_json::Value::String(value)) => {
                Some(SandboxBackendKind::parse(value).ok_or_else(|| {
                    deepagent_core::error::CoreError::invalid(format!(
                        "unsupported sandbox backend in run configuration: {value}"
                    ))
                })?)
            }
            Some(_) => {
                return Err(deepagent_core::error::CoreError::invalid(
                    "sandbox_backend in run configuration must be a string",
                ));
            }
        };
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
                    "sandbox_backend": sandbox_backend.map(SandboxBackendKind::label),
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
            sandbox_backend,
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
    use deepagent_builtins::{WorkspaceRoot, WriteFileTool};
    use deepagent_tools::Tool;

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

    #[tokio::test]
    async fn permission_managed_files_constrain_explicit_grants_at_actual_file_tool() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let admin = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".deepagent")).unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir_all(admin.path().join("managed-settings.d")).unwrap();
        let full = r#"{"permission_preset":"full_access","sandbox_mode":"full_access","approval_policy":"full_access"}"#;
        std::fs::write(root.path().join(".deepagent/settings.json"), full).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), full).unwrap();
        std::fs::write(
            admin.path().join("managed-settings.json"),
            r#"{"sandboxMode":"read_only"}"#,
        )
        .unwrap();
        std::fs::write(
            admin.path().join("managed-settings.d/10-approval.json"),
            r#"{"approvalPolicy":"always_ask"}"#,
        )
        .unwrap();
        for managed in [false, true] {
            let loaded = deepagent_context::DualConfigLoader::new(root.path())
                .with_user_home(Some(home.path().to_owned()))
                .with_managed_dir(managed.then(|| admin.path().to_owned()))
                .with_run_overrides(serde_json::json!({"permission_profile":"full_access"}))
                .load();
            assert!(loaded.errors.is_empty());
            assert_eq!(
                loaded
                    .layers
                    .iter()
                    .filter(|layer| layer.is_managed())
                    .count(),
                if managed { 3 } else { 1 }
            ); // Includes the programmatic managed layer.
            let config = RunConfigOverlay {
                value: loaded.value,
                sources: loaded.sources,
                layers: loaded.layers,
                errors: loaded.errors,
            };
            let environment = RunEnvironment::resolve(
                config,
                crate::settings::PermissionPreset::Default.to_effective_profile(),
                false,
                &None,
                None,
                None,
                "permission-fixture",
            )
            .unwrap();
            let target = root.path().join(format!("managed-{managed}.txt"));
            let tool =
                WriteFileTool::new(WorkspaceRoot::new(root.path()).with_access(environment.access));
            let output = tool
                .invoke(serde_json::json!({"path":target,"content":"managed fixture"}))
                .await
                .unwrap();
            assert_eq!(output.ok, !managed);
            assert_eq!(target.exists(), !managed);
            if managed {
                assert_eq!(environment.policy, ApprovalPolicy::AlwaysAsk);
                assert_eq!(environment.sandbox_mode, SandboxMode::ReadOnly);
                assert_eq!(
                    environment.local_execution_mode,
                    LocalExecutionMode::SandboxiePreferred
                );
            } else {
                assert_eq!(std::fs::read_to_string(target).unwrap(), "managed fixture");
            }
        }
    }
}
