//! Deterministic `.claude` + `.deepagent` JSON settings overlay.
//!
//! Scalar precedence (low → high): plugin defaults → user → project →
//! project-local → run overrides → managed policy. Within one scope the
//! `.deepagent` file wins over the `.claude` file. Managed policy comes from
//! the platform admin directory (`managed-settings.json` base + alphabetical
//! `managed-settings.d/*.json` drop-ins) and can never be overridden by
//! lower scopes — mirroring Claude Code's policySettings semantics.

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
}

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

pub struct DualConfigLoader {
    workspace: PathBuf,
    user_home: Option<PathBuf>,
    managed_dir: Option<PathBuf>,
    plugin_defaults: serde_json::Value,
    run_overrides: serde_json::Value,
    managed: serde_json::Value,
}

impl DualConfigLoader {
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
            100,
            self.plugin_defaults,
        );

        if let Some(home) = self.user_home {
            load_path(
                &mut value,
                &mut sources,
                &mut layers,
                &mut errors,
                &home.join(".claude/settings.json"),
                200,
            );
            load_path(
                &mut value,
                &mut sources,
                &mut layers,
                &mut errors,
                &home.join(".deepagent/settings.json"),
                210,
            );
        }
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".claude/settings.json"),
            300,
        );
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".deepagent/settings.json"),
            310,
        );
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".claude/settings.local.json"),
            400,
        );
        load_path(
            &mut value,
            &mut sources,
            &mut layers,
            &mut errors,
            &self.workspace.join(".deepagent/settings.local.json"),
            410,
        );

        push_layer(
            &mut value,
            &mut sources,
            &mut layers,
            "run_overrides",
            500,
            self.run_overrides,
        );

        // Managed policy: managed-settings.json is the base, then
        // managed-settings.d/*.json drop-ins merge on top in alphabetical
        // order (later files win) — Claude Code's managed layout.
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

/// Platform admin directory holding `managed-settings.json` (Claude Code
/// parity: `C:\Program Files\ClaudeCode` / `/Library/Application Support/
/// ClaudeCode` / `/etc/claude-code`). `DEEPAGENT_MANAGED_SETTINGS_DIR`
/// overrides for tests and hosted deployments.
fn default_managed_dir() -> Option<PathBuf> {
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
    fn native_file_wins_at_same_scope_and_managed_wins_last() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".claude")).unwrap();
        std::fs::create_dir_all(root.path().join(".deepagent")).unwrap();
        std::fs::write(
            root.path().join(".claude/settings.json"),
            r#"{"model":"claude","nested":{"a":1}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join(".deepagent/settings.json"),
            r#"{"model":"deep","nested":{"b":2}}"#,
        )
        .unwrap();
        let overlay = DualConfigLoader::new(root.path())
            .with_user_home(None)
            .with_managed_dir(None)
            .with_run_overrides(serde_json::json!({"mode":"run"}))
            .with_managed(serde_json::json!({"mode":"managed"}))
            .load();
        assert_eq!(overlay.value["model"], "deep");
        assert_eq!(overlay.value["nested"], serde_json::json!({"a":1,"b":2}));
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

        let overlay = DualConfigLoader::new(root.path())
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
