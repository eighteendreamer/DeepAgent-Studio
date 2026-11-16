//! Deterministic DeepAgent JSON settings overlay.
//!
//! Scalar precedence (low → high): plugin defaults → user → project →
//! project-local → run overrides → managed policy. Only `.deepagent`
//! settings participate; other clients' configuration is never imported.
//! Managed policy comes from
//! the platform admin directory (`managed-settings.json` base + alphabetical
//! `managed-settings.d/*.json` drop-ins) and can never be overridden by
//! lower scopes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSource {
    pub path: String,
    pub precedence: u16,
}

/// One merged-in configuration layer, preserved verbatim so callers can apply
/// aggregation semantics (e.g. permission-rule set union) that a plain
/// last-write-wins deep merge would destroy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigLayer {
    pub path: String,
    pub precedence: u16,
    pub value: serde_json::Value,
}

impl ConfigLayer {
    /// Whether this layer comes from managed (admin) policy — the tier that
    /// can never be overridden by lower scopes.
    pub fn is_managed(&self) -> bool {
        self.precedence >= MANAGED_PRECEDENCE
    }

    /// Return the trust scope represented by this loader layer.
    ///
    /// Hook and execution policy consumers must use the source scope rather
    /// than the merged value: a project-local layer is not equivalent to a
    /// user or managed layer merely because it has higher precedence.
    pub fn scope(&self) -> ConfigScope {
        match self.precedence {
            PLUGIN_PRECEDENCE => ConfigScope::Plugin,
            USER_DEEPAGENT_PRECEDENCE => ConfigScope::User,
            PROJECT_DEEPAGENT_PRECEDENCE => ConfigScope::Project,
            PROJECT_LOCAL_DEEPAGENT_PRECEDENCE => ConfigScope::ProjectLocal,
            RUN_PRECEDENCE => ConfigScope::Run,
            precedence if precedence >= MANAGED_PRECEDENCE => ConfigScope::Managed,
            _ => ConfigScope::Unknown,
        }
    }

    /// Whether this layer is supplied by the opened project rather than by
    /// the user, host, managed policy, or an explicit per-run override.
    pub fn is_project_scoped(&self) -> bool {
        matches!(
            self.scope(),
            ConfigScope::Project | ConfigScope::ProjectLocal
        )
    }
}

/// Origin of a merged configuration layer.  Keeping this on the layer lets
/// execution consumers apply trust before interpreting commands/hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Plugin,
    User,
    Project,
    ProjectLocal,
    Run,
    Managed,
    Unknown,
}

pub const PLUGIN_PRECEDENCE: u16 = 100;
pub const USER_DEEPAGENT_PRECEDENCE: u16 = 210;
pub const PROJECT_DEEPAGENT_PRECEDENCE: u16 = 310;
pub const PROJECT_LOCAL_DEEPAGENT_PRECEDENCE: u16 = 410;
pub const RUN_PRECEDENCE: u16 = 500;
/// Precedence at or above which a layer counts as managed policy.
pub const MANAGED_PRECEDENCE: u16 = 600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigOverlay {
    pub value: serde_json::Value,
    pub sources: Vec<ConfigSource>,
    /// Per-source raw values in ascending precedence order (same order they
    /// were merged into `value`).
    #[serde(default)]
    pub layers: Vec<ConfigLayer>,
    pub errors: Vec<String>,
}

pub struct ConfigLoader {
    workspace: PathBuf,
    user_home: Option<PathBuf>,
    managed_dir: Option<PathBuf>,
    plugin_defaults: serde_json::Value,
    run_overrides: serde_json::Value,
    managed: serde_json::Value,
}

impl ConfigLoader {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            user_home: home_dir(),
            managed_dir: default_managed_dir(),
            plugin_defaults: serde_json::json!({}),
            run_overrides: serde_json::json!({}),
            managed: serde_json::json!({}),
        }
    }

    pub fn with_user_home(mut self, home: Option<PathBuf>) -> Self {
        self.user_home = home;
        self
    }

    /// Override the managed policy directory (tests / hosted deployments).
    /// `None` disables filesystem managed policy entirely.
    pub fn with_managed_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.managed_dir = dir;
        self
    }

    pub fn with_plugin_defaults(mut self, value: serde_json::Value) -> Self {
        self.plugin_defaults = value;
        self
    }

    pub fn with_run_overrides(mut self, value: serde_json::Value) -> Self {
        self.run_overrides = value;
        self
    }

    pub fn with_managed(mut self, value: serde_json::Value) -> Self {
        self.managed = value;
        self
    }

    pub fn load(self) -> ConfigOverlay {
        let mut value = serde_json::json!({});
        let mut sources = Vec::new();
        let mut layers = Vec::new();
        let mut errors = Vec::new();

        push_layer(
            &mut value,
            &mut sources,
            &mut layers,
            "plugin_defaults",
            PLUGIN_PRECEDENCE,
            self.plugin_defaults,
        );

        if let Some(home) = self.user_home {
            load_path(
                &mut value,
                &mut sources,
                &mut layers,
                &mut errors,
                &home.join(".deepagent/settings.json"),
                USER_DEEPAGENT_PRECEDENCE,
            );
        }
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".deepagent/settings.json"),
            PROJECT_DEEPAGENT_PRECEDENCE,
        );
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".deepagent/settings.local.json"),
            PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
        );

        push_layer(
            &mut value,
            &mut sources,
            &mut layers,
            "run_overrides",
            RUN_PRECEDENCE,
            self.run_overrides,
        );

        // Managed policy: managed-settings.json is the base, then
        // managed-settings.d/*.json drop-ins merge on top in alphabetical
        // order (later files win), under DeepAgent's own admin directory.
        if let Some(dir) = &self.managed_dir {
            load_path(
                &mut value,
                &mut sources,
                &mut layers,
                &mut errors,
                &dir.join("managed-settings.json"),
                MANAGED_PRECEDENCE,
            );
            let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(dir.join("managed-settings.d"))
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
                        .collect()
                })
                .unwrap_or_default();
            drop_ins.sort();
            for (index, drop_in) in drop_ins.iter().enumerate() {
                load_path(
                    &mut value,
                    &mut sources,
                    &mut layers,
                    &mut errors,
                    drop_in,
                    MANAGED_PRECEDENCE
                        + 1
                        + index.min(usize::from(u16::MAX - MANAGED_PRECEDENCE - 2)) as u16,
                );
            }
        }
        // Programmatic managed overrides (hosted policy) merge last of all.
        push_layer(
            &mut value,
            &mut sources,
            &mut layers,
            "managed",
            u16::MAX,
            self.managed,
        );

        ConfigOverlay {
            value,
            sources,
            layers,
            errors,
        }
    }
}

/// DeepAgent's platform admin directory holding `managed-settings.json`.
/// `DEEPAGENT_MANAGED_SETTINGS_DIR` overrides for tests and hosted deployments.
pub fn default_managed_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DEEPAGENT_MANAGED_SETTINGS_DIR") {
        let dir = PathBuf::from(dir);
        return (!dir.as_os_str().is_empty()).then_some(dir);
    }
    Some(PathBuf::from(if cfg!(windows) {
        "C:\\Program Files\\DeepAgent"
    } else if cfg!(target_os = "macos") {
        "/Library/Application Support/DeepAgent"
    } else {
        "/etc/deepagent"
    }))
}

fn push_layer(
    target: &mut serde_json::Value,
    sources: &mut Vec<ConfigSource>,
    layers: &mut Vec<ConfigLayer>,
    label: &str,
    precedence: u16,
    value: serde_json::Value,
) {
    merge_value(target, value.clone());
    sources.push(ConfigSource {
        path: label.into(),
        precedence,
    });
    layers.push(ConfigLayer {
        path: label.into(),
        precedence,
        value,
    });
}

fn load_path(
    target: &mut serde_json::Value,
    sources: &mut Vec<ConfigSource>,
    layers: &mut Vec<ConfigLayer>,
    errors: &mut Vec<String>,
    path: &Path,
    precedence: u16,
) {
    if !path.exists() {
        return;
    }
    match std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw).map_err(|error| error.to_string())
        }) {
        Ok(value) => {
            merge_value(target, value.clone());
            sources.push(ConfigSource {
                path: path.display().to_string(),
                precedence,
            });
            layers.push(ConfigLayer {
                path: path.display().to_string(),
                precedence,
                value,
            });
        }
        Err(error) => errors.push(format!("{}: {error}", path.display())),
    }
}

fn merge_value(target: &mut serde_json::Value, source: serde_json::Value) {
    match (target, source) {
        (serde_json::Value::Object(target), serde_json::Value::Object(source)) => {
            for (key, value) in source {
                merge_value(target.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_only_native_configuration_sources() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for base in [home.path(), root.path()] {
            std::fs::create_dir_all(base.join(".deepagent")).unwrap();
            std::fs::write(
                base.join(".deepagent/settings.json"),
                r#"{"model":"deepseek-flash","native":true}"#,
            )
            .unwrap();
        }
        std::fs::write(
            root.path().join(".deepagent/settings.local.json"),
            r#"{"local":true}"#,
        )
        .unwrap();
        let overlay = ConfigLoader::new(root.path())
            .with_user_home(Some(home.path().to_owned()))
            .with_managed_dir(None)
            .load();
        assert!(overlay.errors.is_empty());
        assert_eq!(overlay.value["native"], true);
        assert_eq!(overlay.value["model"], "deepseek-flash");
        assert_eq!(overlay.value["local"], true);
        let sources: Vec<_> = overlay
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        assert_eq!(
            sources,
            vec![
                "plugin_defaults".to_owned(),
                home.path()
                    .join(".deepagent/settings.json")
                    .display()
                    .to_string(),
                root.path()
                    .join(".deepagent/settings.json")
                    .display()
                    .to_string(),
                root.path()
                    .join(".deepagent/settings.local.json")
                    .display()
                    .to_string(),
                "run_overrides".to_owned(),
                "managed".to_owned(),
            ]
        );
    }

    #[test]
    fn malformed_native_settings_report_their_source() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for base in [home.path(), root.path()] {
            std::fs::create_dir_all(base.join(".deepagent")).unwrap();
            std::fs::write(base.join(".deepagent/settings.json"), "not JSON").unwrap();
        }
        let overlay = ConfigLoader::new(root.path())
            .with_user_home(Some(home.path().to_owned()))
            .with_managed_dir(None)
            .load();
        assert_eq!(overlay.errors.len(), 2);
        assert!(overlay
            .errors
            .iter()
            .all(|error| error.contains(".deepagent")));
    }

    #[test]
    fn config_layer_scope_preserves_trust_origin() {
        let cases = [
            (PLUGIN_PRECEDENCE, ConfigScope::Plugin, false),
            (USER_DEEPAGENT_PRECEDENCE, ConfigScope::User, false),
            (PROJECT_DEEPAGENT_PRECEDENCE, ConfigScope::Project, true),
            (
                PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
                ConfigScope::ProjectLocal,
                true,
            ),
            (RUN_PRECEDENCE, ConfigScope::Run, false),
            (MANAGED_PRECEDENCE, ConfigScope::Managed, false),
            (200, ConfigScope::Unknown, false),
            (300, ConfigScope::Unknown, false),
            (400, ConfigScope::Unknown, false),
        ];
        for (precedence, scope, project_scoped) in cases {
            let layer = ConfigLayer {
                path: "test".into(),
                precedence,
                value: serde_json::json!({}),
            };
            assert_eq!(layer.scope(), scope);
            assert_eq!(layer.is_project_scoped(), project_scoped);
        }
    }

    #[test]
    fn native_settings_and_explicit_overrides_keep_their_precedence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".deepagent")).unwrap();
        std::fs::write(
            root.path().join(".deepagent/settings.json"),
            r#"{"model":"deep","nested":{"b":2}}"#,
        )
        .unwrap();
        let overlay = ConfigLoader::new(root.path())
            .with_user_home(None)
            .with_managed_dir(None)
            .with_run_overrides(serde_json::json!({"mode":"run"}))
            .with_managed(serde_json::json!({"mode":"managed"}))
            .load();
        assert_eq!(overlay.value["model"], "deep");
        assert_eq!(overlay.value["nested"], serde_json::json!({"b":2}));
        assert_eq!(overlay.value["mode"], "managed");
    }

    #[test]
    fn managed_dir_base_and_drop_ins_win_over_all_lower_scopes() {
        let root = tempfile::tempdir().unwrap();
        let managed = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".deepagent")).unwrap();
        std::fs::write(
            root.path().join(".deepagent/settings.json"),
            r#"{"model":"project","telemetry":true}"#,
        )
        .unwrap();
        std::fs::write(
            managed.path().join("managed-settings.json"),
            r#"{"model":"managed-base","locked":1}"#,
        )
        .unwrap();
        std::fs::create_dir_all(managed.path().join("managed-settings.d")).unwrap();
        std::fs::write(
            managed.path().join("managed-settings.d/10-policy.json"),
            r#"{"model":"managed-dropin"}"#,
        )
        .unwrap();

        let overlay = ConfigLoader::new(root.path())
            .with_user_home(None)
            .with_managed_dir(Some(managed.path().to_path_buf()))
            .load();

        // Drop-in overrides the managed base; both override the project scope.
        assert_eq!(overlay.value["model"], "managed-dropin");
        assert_eq!(overlay.value["locked"], 1);
        assert_eq!(overlay.value["telemetry"], true);
        // Layers preserve per-source values with managed tiers flagged.
        let managed_layers: Vec<_> = overlay.layers.iter().filter(|l| l.is_managed()).collect();
        assert_eq!(managed_layers.len(), 3); // base + drop-in + programmatic
        assert!(overlay
            .layers
            .iter()
            .any(|l| !l.is_managed() && l.value["model"] == "project"));
    }
}
