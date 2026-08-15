//! Manifest discovery across client dialects.
//!
//! A plugin may ship a portable Agent Plugins manifest, a client-specific one,
//! or several at once. This module picks exactly one as authoritative and
//! records which dialect it came from, so nothing downstream has to re-decide.
//!
//! # Discovery order
//!
//! The order mirrors Codex's shipped implementation
//! (`codex-rs/utils/plugins/src/plugin_namespace.rs` plus
//! `DISCOVERABLE_PLUGIN_MANIFEST_PATHS`), with one addition of our own at the
//! end:
//!
//! 1. Root `plugin.json` whose `$schema` names a supported Agent Plugins
//!    version → [`ManifestDialect::AgentPluginV1`]. `.codex-plugin/plugin.json`,
//!    if present, becomes an overlay.
//! 2. Root `plugin.json` whose `$schema` targets an *unsupported* Agent Plugins
//!    version → the plugin is rejected (§5.2). This must not fall through to a
//!    dialect: the package declared an Agent Plugins contract we cannot honor.
//! 3. `.codex-plugin/plugin.json` → [`ManifestDialect::Codex`]
//! 4. `.claude-plugin/plugin.json` → [`ManifestDialect::Claude`]
//! 5. `.cursor-plugin/plugin.json` → [`ManifestDialect::Cursor`]
//! 6. Root `plugin.json` with no Agent Plugins `$schema` →
//!    [`ManifestDialect::DeepAgentLegacy`]
//! 7. Nothing → not a plugin directory, which is not an error.
//!
//! Step 6 exists for compatibility, not conformance. The shipping loader
//! (`plugin_manifest::find_plugin_manifest_path`) accepts a root `plugin.json`
//! unconditionally as its last resort, so dropping that would make an already
//! installed plugin disappear on upgrade. It carries a diagnostic pointing at
//! the portable format instead.
//!
//! # Why a symlinked manifest is refused outright
//!
//! The manifest is the trust root: every containment check downstream is
//! relative to the plugin root that the manifest defines. A symlinked manifest
//! would have to be resolved before the root is known, so there is no safe point
//! at which to bound it. Codex refuses a symlinked root manifest for the same
//! reason; this module extends the refusal to every candidate, since none of
//! them has a legitimate need to be a link.

use std::path::{Path, PathBuf};

use crate::plugin::model::{ComponentKind, PluginDiagnostic};
use crate::plugin::spec::schema::{
    schema_status, SchemaStatus, AGENT_PLUGIN_MANIFEST_RELATIVE_PATH, DISCOVERABLE_MANIFEST_PATHS,
};

/// The manifest flavor a plugin was loaded from.
///
/// Kept on the resolved plugin so the UI can show provenance and so diagnostics
/// can explain why a field was interpreted a particular way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestDialect {
    /// Portable root `plugin.json` (Agent Plugins v1).
    AgentPluginV1,
    /// `.codex-plugin/plugin.json`.
    Codex,
    /// `.claude-plugin/plugin.json`.
    Claude,
    /// `.cursor-plugin/plugin.json`.
    Cursor,
    /// Root `plugin.json` without an Agent Plugins `$schema`, parsed with our
    /// pre-standard superset. Compatibility only.
    DeepAgentLegacy,
}

impl ManifestDialect {
    /// Stable label for DTOs and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentPluginV1 => "agent-plugin-v1",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::DeepAgentLegacy => "deepagent-legacy",
        }
    }

    /// Whether this dialect is the portable standard rather than a
    /// client-specific or legacy shape.
    pub const fn is_portable(self) -> bool {
        matches!(self, Self::AgentPluginV1)
    }
}

/// The manifest chosen for a plugin, with its contents already read.
///
/// Contents are carried along because discovery had to read the file to judge
/// `$schema`; handing them back avoids a second read and the chance of reading a
/// different file than the one that was judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredManifest {
    pub dialect: ManifestDialect,
    pub path: PathBuf,
    pub contents: String,
    /// `.codex-plugin/plugin.json` when the authoritative manifest is the
    /// portable root one. §8 lets a client supplement the portable core; the
    /// overlay may only add presentation metadata and extended component paths,
    /// never replace a portable core field.
    pub overlay: Option<DiscoveredOverlay>,
    /// Findings worth surfacing even though discovery succeeded.
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// A client-specific supplement to a portable manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredOverlay {
    pub path: PathBuf,
    pub contents: String,
}

/// Why a candidate directory was rejected outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The package targets an Agent Plugins version this build does not
    /// implement. §5.2 requires rejecting it and reporting the version.
    UnsupportedSchema { path: PathBuf, schema: String },
    /// The manifest could not be read.
    Unreadable { path: PathBuf, reason: String },
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema { path, schema } => write!(
                f,
                "{} declares an unsupported Agent Plugins version: {schema}",
                path.display()
            ),
            Self::Unreadable { path, reason } => {
                write!(f, "cannot read {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Selects the authoritative manifest for `plugin_root`.
///
/// `Ok(None)` means the directory is not a plugin, which §6.2's spirit treats as
/// ordinary rather than exceptional.
pub fn discover(plugin_root: &Path) -> Result<Option<DiscoveredManifest>, DiscoveryError> {
    let root_manifest = plugin_root.join(AGENT_PLUGIN_MANIFEST_RELATIVE_PATH);

    // A symlinked or non-regular root manifest disqualifies the whole directory
    // rather than falling through to a dialect — see the module docs.
    match std::fs::symlink_metadata(&root_manifest) {
        Ok(metadata) if !metadata.file_type().is_file() => return Ok(None),
        Ok(_) => {
            let contents = read(&root_manifest)?;
            match schema_status(&contents) {
                SchemaStatus::Supported => {
                    return Ok(Some(portable(plugin_root, root_manifest, contents)))
                }
                SchemaStatus::Unsupported => {
                    return Err(DiscoveryError::UnsupportedSchema {
                        schema: declared_schema(&contents),
                        path: root_manifest,
                    })
                }
                // Not an Agent Plugins manifest. Dialects get their turn first,
                // and the legacy fallback below catches it if none matches.
                SchemaStatus::Unrelated => {}
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DiscoveryError::Unreadable {
                path: root_manifest,
                reason: error.to_string(),
            })
        }
    }

    for relative in DISCOVERABLE_MANIFEST_PATHS {
        let path = plugin_root.join(relative);
        if !is_regular_file(&path) {
            continue;
        }
        let dialect = dialect_for(relative);
        let contents = read(&path)?;
        return Ok(Some(DiscoveredManifest {
            dialect,
            path,
            contents,
            overlay: None,
            diagnostics: Vec::new(),
        }));
    }

    // Compatibility fallback: a root `plugin.json` that never claimed to be
    // Agent Plugins. Accepted so existing installs keep working, with a nudge
    // toward the portable format.
    if is_regular_file(&root_manifest) {
        let contents = read(&root_manifest)?;
        return Ok(Some(DiscoveredManifest {
            dialect: ManifestDialect::DeepAgentLegacy,
            path: root_manifest,
            contents,
            overlay: None,
            diagnostics: vec![PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Manifest,
                path: None,
                reason: "root plugin.json has no Agent Plugins `$schema`; parsed with the \
                         pre-standard DeepAgent format. Add the portable `$schema` to make this \
                         plugin loadable by other clients."
                    .to_string(),
            }],
        }));
    }

    Ok(None)
}

/// Builds the portable result, attaching `.codex-plugin/plugin.json` as an
/// overlay when present.
fn portable(plugin_root: &Path, path: PathBuf, contents: String) -> DiscoveredManifest {
    let overlay_path = plugin_root.join(DISCOVERABLE_MANIFEST_PATHS[0]);
    let overlay = if is_regular_file(&overlay_path) {
        std::fs::read_to_string(&overlay_path)
            .ok()
            .map(|contents| DiscoveredOverlay {
                path: overlay_path,
                contents,
            })
    } else {
        None
    };

    DiscoveredManifest {
        dialect: ManifestDialect::AgentPluginV1,
        path,
        contents,
        overlay,
        diagnostics: Vec::new(),
    }
}

fn dialect_for(relative: &str) -> ManifestDialect {
    match relative {
        ".codex-plugin/plugin.json" => ManifestDialect::Codex,
        ".claude-plugin/plugin.json" => ManifestDialect::Claude,
        ".cursor-plugin/plugin.json" => ManifestDialect::Cursor,
        // `DISCOVERABLE_MANIFEST_PATHS` and this mapping are edited together;
        // treating an unmapped entry as Codex would silently misattribute it.
        other => unreachable!("unmapped discoverable manifest path: {other}"),
    }
}

/// A regular file, following symlinks for the type check but refusing the entry
/// itself being a link.
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn read(path: &Path) -> Result<String, DiscoveryError> {
    std::fs::read_to_string(path).map_err(|error| DiscoveryError::Unreadable {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Best-effort read of the declared `$schema`, for error reporting only.
fn declared_schema(contents: &str) -> String {
    crate::plugin::spec::schema::read_schema_value(contents).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::spec::schema::AGENT_PLUGIN_SCHEMA_URI;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, contents).expect("write");
    }

    fn portable_manifest(name: &str) -> String {
        format!(r#"{{"$schema": "{AGENT_PLUGIN_SCHEMA_URI}", "name": "{name}"}}"#)
    }

    #[test]
    fn portable_root_manifest_wins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("plugin.json"), &portable_manifest("demo"));
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "demo"}"#,
        );
        write(
            &root.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "demo"}"#,
        );

        let found = discover(root).expect("discovery ok").expect("a plugin");

        assert_eq!(found.dialect, ManifestDialect::AgentPluginV1);
        assert_eq!(found.path, root.join("plugin.json"));
        assert!(found.diagnostics.is_empty());
    }

    /// §8: the client-specific manifest supplements the portable one.
    #[test]
    fn codex_manifest_becomes_an_overlay_for_a_portable_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("plugin.json"), &portable_manifest("demo"));
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "demo", "interface": {"displayName": "Demo"}}"#,
        );

        let found = discover(root).expect("discovery ok").expect("a plugin");

        let overlay = found.overlay.expect("codex overlay");
        assert_eq!(overlay.path, root.join(".codex-plugin").join("plugin.json"));
        assert!(overlay.contents.contains("displayName"));
    }

    #[test]
    fn portable_root_without_codex_manifest_has_no_overlay() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("plugin.json"), &portable_manifest("demo"));

        let found = discover(root).expect("discovery ok").expect("a plugin");

        assert!(found.overlay.is_none());
    }

    /// §5.2: an unsupported Agent Plugins version must reject the plugin, not
    /// quietly fall back to a dialect manifest that happens to be present.
    #[test]
    fn unsupported_schema_rejects_the_plugin_instead_of_falling_back() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            &root.join("plugin.json"),
            r#"{"$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json",
                "name": "demo"}"#,
        );
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "demo"}"#,
        );

        let error = discover(root).expect_err("must be rejected");

        match error {
            DiscoveryError::UnsupportedSchema { schema, .. } => assert!(schema.contains("2.0.0")),
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn dialect_order_is_codex_then_claude_then_cursor() {
        let cases = [
            (
                vec![".codex-plugin", ".claude-plugin", ".cursor-plugin"],
                ManifestDialect::Codex,
            ),
            (
                vec![".claude-plugin", ".cursor-plugin"],
                ManifestDialect::Claude,
            ),
            (vec![".cursor-plugin"], ManifestDialect::Cursor),
        ];

        for (present, expected) in cases {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path();
            for dir in &present {
                write(&root.join(dir).join("plugin.json"), r#"{"name": "demo"}"#);
            }

            let found = discover(root).expect("discovery ok").expect("a plugin");

            assert_eq!(found.dialect, expected, "with {present:?} present");
            assert!(found.overlay.is_none(), "dialects carry no overlay");
        }
    }

    /// The shipping loader accepts a bare root `plugin.json` as its last resort.
    /// Keeping that keeps already-installed plugins loadable, but it is reported
    /// so the author can move to the portable format.
    #[test]
    fn root_manifest_without_schema_falls_back_to_legacy_with_a_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            &root.join("plugin.json"),
            r#"{"name": "demo", "skills": "./skills"}"#,
        );

        let found = discover(root).expect("discovery ok").expect("a plugin");

        assert_eq!(found.dialect, ManifestDialect::DeepAgentLegacy);
        assert_eq!(found.diagnostics.len(), 1);
        assert!(matches!(
            &found.diagnostics[0],
            PluginDiagnostic::ComponentInvalid { reason, .. } if reason.contains("$schema")
        ));
    }

    /// A dialect manifest outranks the legacy root fallback, matching the
    /// shipping order where `.codex-plugin` is checked before root.
    #[test]
    fn dialect_manifest_outranks_the_legacy_root_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("plugin.json"), r#"{"name": "legacy"}"#);
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "codex"}"#,
        );

        let found = discover(root).expect("discovery ok").expect("a plugin");

        assert_eq!(found.dialect, ManifestDialect::Codex);
        assert!(found.contents.contains("codex"));
    }

    #[test]
    fn directory_without_any_manifest_is_not_a_plugin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("skills")).expect("create dir");

        assert!(discover(tmp.path()).expect("discovery ok").is_none());
    }

    #[test]
    fn root_manifest_as_a_directory_is_not_a_plugin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("plugin.json")).expect("create dir");

        assert!(discover(tmp.path()).expect("discovery ok").is_none());
    }

    /// A symlinked manifest disqualifies the directory outright: it is the trust
    /// root, and there is no safe point at which to bound it. Creating symlinks
    /// needs privileges on Windows, so the test skips when refused.
    #[test]
    fn symlinked_root_manifest_disqualifies_the_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        std::fs::create_dir_all(&root).expect("create root");
        let outside = tmp.path().join("outside.json");
        write(&outside, &portable_manifest("demo"));
        // A dialect manifest is present, proving the refusal is not a fallthrough.
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "demo"}"#,
        );

        if !try_symlink_file(&outside, &root.join("plugin.json")) {
            eprintln!("skipping: platform does not permit creating file symlinks");
            return;
        }

        assert!(
            discover(&root).expect("discovery ok").is_none(),
            "a symlinked manifest must disqualify the directory"
        );
    }

    #[test]
    fn symlinked_dialect_manifest_is_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        std::fs::create_dir_all(root.join(".codex-plugin")).expect("create dir");
        let outside = tmp.path().join("outside.json");
        write(&outside, r#"{"name": "demo"}"#);
        write(
            &root.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "claude"}"#,
        );

        if !try_symlink_file(&outside, &root.join(".codex-plugin").join("plugin.json")) {
            eprintln!("skipping: platform does not permit creating file symlinks");
            return;
        }

        let found = discover(&root).expect("discovery ok").expect("a plugin");

        assert_eq!(
            found.dialect,
            ManifestDialect::Claude,
            "the symlinked codex manifest must be skipped, not used"
        );
    }

    #[test]
    fn dialect_labels_and_portability() {
        assert_eq!(ManifestDialect::AgentPluginV1.as_str(), "agent-plugin-v1");
        assert_eq!(ManifestDialect::Codex.as_str(), "codex");
        assert_eq!(ManifestDialect::Claude.as_str(), "claude");
        assert_eq!(ManifestDialect::Cursor.as_str(), "cursor");
        assert_eq!(
            ManifestDialect::DeepAgentLegacy.as_str(),
            "deepagent-legacy"
        );

        assert!(ManifestDialect::AgentPluginV1.is_portable());
        for dialect in [
            ManifestDialect::Codex,
            ManifestDialect::Claude,
            ManifestDialect::Cursor,
            ManifestDialect::DeepAgentLegacy,
        ] {
            assert!(!dialect.is_portable(), "{dialect:?} is not the standard");
        }
    }

    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }
}
