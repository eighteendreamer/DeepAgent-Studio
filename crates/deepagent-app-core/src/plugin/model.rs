//! Structured load-time findings and their severity, per Agent Plugins
//! Specification 1.0.0 §11.3.
//!
//! §11.3 defines three failure tiers:
//!
//! 1. **Fatal to the plugin** — an invalid `plugin.json`. The client must
//!    reject the plugin and must not discover or execute any component.
//! 2. **Fatal to one component type** — a fixed location present but of the
//!    wrong filesystem kind, or an `mcp.json` that fails its top-level
//!    requirements. Other component types keep loading.
//! 3. **Fatal to one entry** — one skill or one MCP server. Sibling entries and
//!    other component types keep loading.
//!
//! Plus two explicitly non-fatal cases that must be *reported and ignored*
//! rather than silently dropped: unknown top-level manifest fields (§5.2) and a
//! non-object `extensions` field (§8.1).
//!
//! # Why this extends `PluginLoadError` instead of adding a parallel channel
//!
//! The shipping [`PluginLoadError`] channel already carries both tiers without
//! distinguishing them: `manifest-parse-error` makes a plugin unusable, while
//! `path-not-found` merely notes a declared path that is absent. The UI renders
//! both as "errors", which overstates the second. Adding a second array would
//! duplicate that data instead of fixing it, so [`DiagnosticSeverity`] is
//! attached to the existing channel and [`PluginDiagnostic`] becomes the
//! type-safe way to construct entries — no hand-written `kind` strings at call
//! sites.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugin_loader::PluginLoadError;

/// How much a load-time finding affects usability (§11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Reported and ignored; the plugin and all its components still load.
    Info,
    /// A component type or a single entry was skipped; the rest still loads.
    Warning,
    /// The plugin is unusable.
    Error,
}

impl DiagnosticSeverity {
    /// Stable string label for the wire contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// `PluginLoadError::severity` defaults to [`DiagnosticSeverity::Error`].
///
/// Deserializing a payload written before severity existed must not silently
/// downgrade a real failure into a note, so the conservative value wins.
impl Default for DiagnosticSeverity {
    fn default() -> Self {
        Self::Error
    }
}

/// Which component type a finding concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// The manifest itself.
    Manifest,
    /// Agent Skills (§7.1).
    Skills,
    /// MCP servers (§7.2).
    Mcp,
    /// Slash commands — a client extension, outside v1.
    Commands,
    /// Subagents — a client extension, outside v1.
    Agents,
    /// Lifecycle hooks — a client extension, outside v1.
    Hooks,
    /// Output styles — a client extension, outside v1.
    OutputStyles,
    /// Sidebar apps — a DeepAgent extension.
    Apps,
}

impl ComponentKind {
    /// Stable label, matching the strings the shipping loader already emits in
    /// `PluginLoadError::component`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Skills => "skills",
            Self::Mcp => "mcp",
            Self::Commands => "commands",
            Self::Agents => "agents",
            Self::Hooks => "hooks",
            Self::OutputStyles => "output-styles",
            Self::Apps => "apps",
        }
    }
}

/// A structured load-time finding.
///
/// Construct these instead of assembling [`PluginLoadError`] by hand: the
/// variant fixes the `kind` string, the severity, and the component together,
/// so a new call site cannot invent an inconsistent combination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginDiagnostic {
    /// An unknown top-level manifest field. Non-fatal: report and ignore
    /// (§5.2).
    UnknownManifestField { field: String },
    /// `extensions` was present but not an object. Non-fatal: report and ignore
    /// the field, keep loading components (§8.1).
    ExtensionsNotObject,
    /// One skill directory was skipped; siblings keep loading (§7.1).
    SkillSkipped { path: PathBuf, reason: String },
    /// MCP is disabled for this plugin, but other component types keep loading
    /// (§7.2.2 rule 2).
    McpDisabled { reason: String },
    /// One MCP server entry was skipped; siblings keep loading (§7.2.2 rules
    /// 3–4).
    McpServerSkipped { server: String, reason: String },
    /// A component type is present but unusable — wrong filesystem kind, failed
    /// containment, or unparseable (§6.2, §11.3 rule 3).
    ComponentInvalid {
        component: ComponentKind,
        path: Option<PathBuf>,
        reason: String,
    },
    /// A hook event name that maps to no internal lifecycle point. Recorded
    /// rather than dropped, so an unsupported hook is visible instead of
    /// silently inert.
    HookEventUnmapped { event: String },
}

impl PluginDiagnostic {
    /// Stable machine-readable discriminator carried as `PluginLoadError::kind`.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::UnknownManifestField { .. } => "unknown-manifest-field",
            Self::ExtensionsNotObject => "extensions-not-object",
            Self::SkillSkipped { .. } => "skill-skipped",
            Self::McpDisabled { .. } => "mcp-disabled",
            Self::McpServerSkipped { .. } => "mcp-server-skipped",
            Self::ComponentInvalid { .. } => "component-invalid",
            Self::HookEventUnmapped { .. } => "hook-event-unmapped",
        }
    }

    /// Severity per §11.3. None of these variants is fatal to the plugin:
    /// a fatal manifest violation is returned as an error instead of recorded
    /// here, because §11.3 forbids discovering components of a rejected plugin.
    pub const fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::UnknownManifestField { .. } | Self::ExtensionsNotObject => {
                DiagnosticSeverity::Info
            }
            Self::SkillSkipped { .. }
            | Self::McpDisabled { .. }
            | Self::McpServerSkipped { .. }
            | Self::ComponentInvalid { .. }
            | Self::HookEventUnmapped { .. } => DiagnosticSeverity::Warning,
        }
    }

    /// Which component type the finding concerns.
    pub const fn component(&self) -> ComponentKind {
        match self {
            Self::UnknownManifestField { .. } | Self::ExtensionsNotObject => {
                ComponentKind::Manifest
            }
            Self::SkillSkipped { .. } => ComponentKind::Skills,
            Self::McpDisabled { .. } | Self::McpServerSkipped { .. } => ComponentKind::Mcp,
            Self::ComponentInvalid { component, .. } => *component,
            Self::HookEventUnmapped { .. } => ComponentKind::Hooks,
        }
    }

    /// Human-readable explanation shown in the UI.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownManifestField { field } => {
                format!("ignoring unknown manifest field: {field}")
            }
            Self::ExtensionsNotObject => "ignoring `extensions`: expected an object".to_string(),
            Self::SkillSkipped { path, reason } => {
                format!("skipped skill {}: {reason}", display(path))
            }
            Self::McpDisabled { reason } => {
                format!("MCP servers disabled for this plugin: {reason}")
            }
            Self::McpServerSkipped { server, reason } => {
                format!("skipped MCP server '{server}': {reason}")
            }
            Self::ComponentInvalid {
                component,
                path,
                reason,
            } => match path {
                Some(path) => format!(
                    "{} component unusable at {}: {reason}",
                    component.as_str(),
                    display(path)
                ),
                None => format!("{} component unusable: {reason}", component.as_str()),
            },
            Self::HookEventUnmapped { event } => {
                format!("hook event '{event}' is not supported and was skipped")
            }
        }
    }

    /// The filesystem path the finding concerns, when it has one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::SkillSkipped { path, .. } => Some(path.as_path()),
            Self::ComponentInvalid { path, .. } => path.as_deref(),
            _ => None,
        }
    }

    /// Projects the finding onto the shipping load-error channel.
    ///
    /// `source` matches the existing convention in `plugin_loader`: the plugin
    /// origin for package-provided problems, or `"policy"` for client-imposed
    /// ones.
    pub fn into_load_error(self, plugin_id: &str, source: &str) -> PluginLoadError {
        PluginLoadError {
            kind: self.kind().to_string(),
            plugin: Some(plugin_id.to_string()),
            source: source.to_string(),
            path: self.path().map(|path| path.display().to_string()),
            component: Some(self.component().as_str().to_string()),
            message: self.message(),
            severity: self.severity(),
        }
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_fatal_manifest_findings_are_info() {
        assert_eq!(
            PluginDiagnostic::UnknownManifestField {
                field: "extra".into()
            }
            .severity(),
            DiagnosticSeverity::Info
        );
        assert_eq!(
            PluginDiagnostic::ExtensionsNotObject.severity(),
            DiagnosticSeverity::Info
        );
    }

    /// Skipping a component or entry degrades the plugin but does not reject
    /// it, so these must not be reported at error severity.
    #[test]
    fn skipped_components_are_warnings() {
        let cases = [
            PluginDiagnostic::SkillSkipped {
                path: PathBuf::from("/p/skills/a"),
                reason: "missing SKILL.md".into(),
            },
            PluginDiagnostic::McpDisabled {
                reason: "schema version mismatch".into(),
            },
            PluginDiagnostic::McpServerSkipped {
                server: "db".into(),
                reason: "unknown transport".into(),
            },
            PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Skills,
                path: None,
                reason: "not a directory".into(),
            },
            PluginDiagnostic::HookEventUnmapped {
                event: "Weird".into(),
            },
        ];
        for case in cases {
            assert_eq!(
                case.severity(),
                DiagnosticSeverity::Warning,
                "{case:?} should be a warning"
            );
        }
    }

    #[test]
    fn severity_default_is_conservative() {
        // A payload written before severity existed must not downgrade a real
        // failure into a note.
        assert_eq!(DiagnosticSeverity::default(), DiagnosticSeverity::Error);
    }

    #[test]
    fn severity_orders_info_below_warning_below_error() {
        assert!(DiagnosticSeverity::Info < DiagnosticSeverity::Warning);
        assert!(DiagnosticSeverity::Warning < DiagnosticSeverity::Error);
    }

    #[test]
    fn severity_labels_are_snake_case() {
        assert_eq!(DiagnosticSeverity::Info.as_str(), "info");
        assert_eq!(DiagnosticSeverity::Warning.as_str(), "warning");
        assert_eq!(DiagnosticSeverity::Error.as_str(), "error");
        assert_eq!(
            serde_json::to_string(&DiagnosticSeverity::Warning).unwrap(),
            "\"warning\""
        );
    }

    /// The labels must match what the shipping loader already emits, so the
    /// frontend keeps grouping by component without a translation table.
    #[test]
    fn component_labels_match_existing_loader_strings() {
        assert_eq!(ComponentKind::Manifest.as_str(), "manifest");
        assert_eq!(ComponentKind::Skills.as_str(), "skills");
        assert_eq!(ComponentKind::Mcp.as_str(), "mcp");
        assert_eq!(ComponentKind::Commands.as_str(), "commands");
        assert_eq!(ComponentKind::Agents.as_str(), "agents");
        assert_eq!(ComponentKind::Hooks.as_str(), "hooks");
        assert_eq!(ComponentKind::OutputStyles.as_str(), "output-styles");
        assert_eq!(ComponentKind::Apps.as_str(), "apps");
    }

    #[test]
    fn kinds_are_unique_and_stable() {
        let kinds = [
            PluginDiagnostic::UnknownManifestField { field: "x".into() }.kind(),
            PluginDiagnostic::ExtensionsNotObject.kind(),
            PluginDiagnostic::SkillSkipped {
                path: PathBuf::new(),
                reason: String::new(),
            }
            .kind(),
            PluginDiagnostic::McpDisabled {
                reason: String::new(),
            }
            .kind(),
            PluginDiagnostic::McpServerSkipped {
                server: String::new(),
                reason: String::new(),
            }
            .kind(),
            PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Mcp,
                path: None,
                reason: String::new(),
            }
            .kind(),
            PluginDiagnostic::HookEventUnmapped {
                event: String::new(),
            }
            .kind(),
        ];
        let mut unique = kinds.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), kinds.len(), "diagnostic kinds must be unique");
    }

    #[test]
    fn projects_onto_load_error_channel() {
        let diagnostic = PluginDiagnostic::SkillSkipped {
            path: PathBuf::from("/p/skills/broken"),
            reason: "SKILL.md is not a regular file".into(),
        };
        let error = diagnostic.into_load_error("demo@personal", "plugin");

        assert_eq!(error.kind, "skill-skipped");
        assert_eq!(error.plugin.as_deref(), Some("demo@personal"));
        assert_eq!(error.source, "plugin");
        assert_eq!(error.component.as_deref(), Some("skills"));
        assert_eq!(error.severity, DiagnosticSeverity::Warning);
        assert!(error.path.is_some());
        assert!(error.message.contains("SKILL.md is not a regular file"));
    }

    #[test]
    fn messages_name_the_offending_entry() {
        assert!(PluginDiagnostic::McpServerSkipped {
            server: "database".into(),
            reason: "unknown field `port`".into(),
        }
        .message()
        .contains("database"));

        assert!(PluginDiagnostic::UnknownManifestField {
            field: "mcpServers".into(),
        }
        .message()
        .contains("mcpServers"));

        assert!(PluginDiagnostic::HookEventUnmapped {
            event: "PreCommit".into(),
        }
        .message()
        .contains("PreCommit"));
    }
}
