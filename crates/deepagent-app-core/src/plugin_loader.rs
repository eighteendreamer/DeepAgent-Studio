//! Plugin discovery across built-in, workspace, personal, and marketplace roots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugin::dialect::MarketplaceSource;
use crate::plugin::model::{DiagnosticSeverity, ResolvedPlugin};
use crate::plugin_manifest::{find_plugin_manifest_path, load_plugin_manifest, PluginManifest};
use crate::plugin_security::{is_blocked_plugin_name, is_reserved_plugin_name};

const PLUGIN_CACHE_ORPHAN_MARKER: &str = ".orphaned_at";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PluginOrigin {
    BuiltIn,
    Marketplace,
    Personal,
    Workspace,
    Session,
}

impl PluginOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "builtin",
            Self::Marketplace => "marketplace",
            Self::Personal => "personal",
            Self::Workspace => "workspace",
            Self::Session => "session",
        }
    }

    pub fn default_enabled(self) -> bool {
        match self {
            Self::BuiltIn | Self::Personal | Self::Session => true,
            Self::Marketplace | Self::Workspace => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoots {
    pub session: Vec<PathBuf>,
    pub builtin: PathBuf,
    pub workspace: Option<PathBuf>,
    pub personal: PathBuf,
    pub marketplace_cache: PathBuf,
    pub marketplaces: PathBuf,
}

/// A load-time finding for one plugin.
///
/// Despite the name this channel carries the whole spectrum Agent Plugins
/// §11.3 defines, from "reported and ignored" up to "plugin rejected".
/// [`PluginLoadError::severity`] is what separates them: without it a merely
/// absent declared path reads the same as an unparseable manifest. Prefer
/// constructing these through [`crate::plugin::model::PluginDiagnostic`], which
/// fixes `kind`, `component`, and `severity` together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLoadError {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub message: String,
    /// How much this finding affects usability. Defaults to
    /// [`DiagnosticSeverity::Error`] so a payload predating this field is never
    /// silently downgraded.
    #[serde(default)]
    pub severity: DiagnosticSeverity,
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub id: String,
    pub name: String,
    pub source_key: String,
    pub origin: PluginOrigin,
    pub marketplace: Option<String>,
    pub root: PathBuf,
    pub resolved: Option<ResolvedPlugin>,
    pub available: bool,
    pub overridden_by: Option<String>,
    pub errors: Vec<PluginLoadError>,
}

impl LoadedPlugin {
    pub fn enabled_default(&self) -> bool {
        self.origin.default_enabled() && self.available && self.resolved.is_some()
    }

    /// Legacy component projection while consumers migrate to `ResolvedPlugin`.
    pub fn manifest(&self) -> Option<&PluginManifest> {
        self.resolved.as_ref().map(|plugin| &plugin.manifest)
    }

    pub fn resolved(&self) -> Option<&ResolvedPlugin> {
        self.resolved.as_ref()
    }
}

#[derive(Debug, Clone)]
struct CandidateRoot {
    root: PathBuf,
    origin: PluginOrigin,
    marketplace: Option<String>,
}

pub fn load_plugins(roots: &PluginRoots) -> Vec<LoadedPlugin> {
    let mut candidates = Vec::new();
    candidates.extend(discover_collection(
        &roots.builtin,
        PluginOrigin::BuiltIn,
        None,
    ));
    candidates.extend(discover_marketplace_cache(&roots.marketplace_cache));
    candidates.extend(discover_collection(
        &roots.personal,
        PluginOrigin::Personal,
        None,
    ));
    if let Some(workspace) = &roots.workspace {
        candidates.extend(discover_collection(
            workspace,
            PluginOrigin::Workspace,
            None,
        ));
    }
    for session_root in &roots.session {
        candidates.extend(discover_collection(
            session_root,
            PluginOrigin::Session,
            None,
        ));
    }

    let mut plugins = candidates
        .into_iter()
        .map(load_candidate)
        .collect::<Vec<_>>();

    mark_overrides(&mut plugins);
    plugins.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| b.origin.cmp(&a.origin))
            .then_with(|| a.id.cmp(&b.id))
    });
    plugins
}

fn discover_collection(
    dir: &Path,
    origin: PluginOrigin,
    marketplace: Option<String>,
) -> Vec<CandidateRoot> {
    let mut out = Vec::new();
    if find_plugin_manifest_path(dir).is_some() {
        out.push(CandidateRoot {
            root: dir.to_path_buf(),
            origin,
            marketplace,
        });
        return out;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if matches!(
            name,
            ".staging" | "cache" | "marketplaces" | "data" | "state.json"
        ) {
            continue;
        }
        if is_orphaned_plugin_root(&path) {
            continue;
        }
        if find_plugin_manifest_path(&path).is_some() {
            out.push(CandidateRoot {
                root: path,
                origin,
                marketplace: marketplace.clone(),
            });
        }
    }
    out
}

fn discover_marketplace_cache(cache_root: &Path) -> Vec<CandidateRoot> {
    let mut out = Vec::new();
    let Ok(marketplaces) = std::fs::read_dir(cache_root) else {
        return out;
    };
    for market in marketplaces.flatten() {
        let market_path = market.path();
        if !market_path.is_dir() {
            continue;
        }
        if is_orphaned_plugin_root(&market_path) {
            continue;
        }
        let marketplace = market.file_name().to_string_lossy().trim().to_string();
        if marketplace.is_empty() || marketplace == ".staging" {
            continue;
        }

        if find_plugin_manifest_path(&market_path).is_some() {
            out.push(CandidateRoot {
                root: market_path,
                origin: PluginOrigin::Marketplace,
                marketplace: Some(marketplace),
            });
            continue;
        }

        let Ok(plugin_dirs) = std::fs::read_dir(&market_path) else {
            continue;
        };
        for plugin_dir in plugin_dirs.flatten() {
            let plugin_path = plugin_dir.path();
            if !plugin_path.is_dir() {
                continue;
            }
            if is_orphaned_plugin_root(&plugin_path) {
                continue;
            }
            if find_plugin_manifest_path(&plugin_path).is_some() {
                out.push(CandidateRoot {
                    root: plugin_path,
                    origin: PluginOrigin::Marketplace,
                    marketplace: Some(marketplace.clone()),
                });
                continue;
            }
            let Ok(version_dirs) = std::fs::read_dir(&plugin_path) else {
                continue;
            };
            for version_dir in version_dirs.flatten() {
                let version_path = version_dir.path();
                if version_path.is_dir()
                    && !is_orphaned_plugin_root(&version_path)
                    && find_plugin_manifest_path(&version_path).is_some()
                {
                    out.push(CandidateRoot {
                        root: version_path,
                        origin: PluginOrigin::Marketplace,
                        marketplace: Some(marketplace.clone()),
                    });
                }
            }
        }
    }
    out
}

fn is_orphaned_plugin_root(path: &Path) -> bool {
    path.join(PLUGIN_CACHE_ORPHAN_MARKER).is_file()
}

fn load_candidate(candidate: CandidateRoot) -> LoadedPlugin {
    let fallback_name = candidate
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin")
        .to_string();
    match load_plugin_manifest(&candidate.root) {
        Ok(Some(manifest)) => {
            let source_key = source_key(candidate.origin, candidate.marketplace.as_deref());
            let id = plugin_id(&manifest.name, &source_key);
            let resolved = ResolvedPlugin::from_manifest(
                &candidate.root,
                manifest,
                MarketplaceSource::default(),
            );
            let mut errors = component_errors(&candidate.root, &resolved.manifest, &id);
            errors.extend(
                resolved
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| diagnostic.into_load_error(&id, "plugin")),
            );
            let blocked_by_policy = policy_errors(&resolved.manifest, &id, &candidate.root);
            let available = blocked_by_policy.is_empty();
            errors.extend(blocked_by_policy);
            LoadedPlugin {
                id,
                name: resolved.name().to_string(),
                source_key,
                origin: candidate.origin,
                marketplace: candidate.marketplace,
                root: candidate.root,
                resolved: Some(resolved),
                available,
                overridden_by: None,
                errors,
            }
        }
        Ok(None) => {
            let source_key = source_key(candidate.origin, candidate.marketplace.as_deref());
            let id = plugin_id(&fallback_name, &source_key);
            LoadedPlugin {
                id: id.clone(),
                name: fallback_name,
                source_key,
                origin: candidate.origin,
                marketplace: candidate.marketplace,
                root: candidate.root.clone(),
                resolved: None,
                available: false,
                overridden_by: None,
                errors: vec![PluginLoadError {
                    kind: "manifest-not-found".to_string(),
                    plugin: Some(id),
                    source: candidate.origin.as_str().to_string(),
                    path: Some(candidate.root.display().to_string()),
                    component: Some("manifest".to_string()),
                    message: "plugin manifest not found".to_string(),
                    // §5.1: without a manifest there is no plugin to load.
                    severity: DiagnosticSeverity::Error,
                }],
            }
        }
        Err(error) => {
            let source_key = source_key(candidate.origin, candidate.marketplace.as_deref());
            let id = plugin_id(&fallback_name, &source_key);
            LoadedPlugin {
                id: id.clone(),
                name: fallback_name,
                source_key,
                origin: candidate.origin,
                marketplace: candidate.marketplace,
                root: candidate.root.clone(),
                resolved: None,
                available: false,
                overridden_by: None,
                errors: vec![PluginLoadError {
                    kind: "manifest-parse-error".to_string(),
                    plugin: Some(id),
                    source: candidate.origin.as_str().to_string(),
                    path: find_plugin_manifest_path(&candidate.root)
                        .map(|p| p.display().to_string()),
                    component: Some("manifest".to_string()),
                    message: error.to_string(),
                    // §11.3 rule 2: a manifest schema violation is fatal to the
                    // plugin — no component may be discovered or executed.
                    severity: DiagnosticSeverity::Error,
                }],
            }
        }
    }
}

fn component_errors(root: &Path, manifest: &PluginManifest, id: &str) -> Vec<PluginLoadError> {
    let mut errors = Vec::new();
    append_missing_paths(&mut errors, id, root, "skills", &manifest.paths.skills);
    append_missing_paths(&mut errors, id, root, "commands", &manifest.paths.commands);
    append_missing_paths(&mut errors, id, root, "agents", &manifest.paths.agents);
    append_missing_paths(
        &mut errors,
        id,
        root,
        "output-styles",
        &manifest.paths.output_styles,
    );
    append_missing_paths(
        &mut errors,
        id,
        root,
        "mcp",
        &manifest.paths.mcp_server_paths,
    );
    append_missing_paths(&mut errors, id, root, "hooks", &manifest.paths.hook_paths);
    append_missing_paths(&mut errors, id, root, "apps", &manifest.paths.app_paths);
    errors
}

fn policy_errors(manifest: &PluginManifest, id: &str, root: &Path) -> Vec<PluginLoadError> {
    let mut errors = Vec::new();
    let manifest_path = Some(manifest.manifest_path.display().to_string());
    if is_reserved_plugin_name(&manifest.name) {
        errors.push(PluginLoadError {
            kind: "reserved-name".to_string(),
            plugin: Some(id.to_string()),
            source: "policy".to_string(),
            path: manifest_path.clone(),
            component: Some("manifest".to_string()),
            message: format!("plugin name '{}' is reserved by DeepAgent", manifest.name),
            // Client policy, not a spec rule: the plugin is made unavailable, so
            // this ranks with the fatal tier.
            severity: DiagnosticSeverity::Error,
        });
    }
    if is_blocked_plugin_name(&manifest.name) {
        errors.push(PluginLoadError {
            kind: "blocklist".to_string(),
            plugin: Some(id.to_string()),
            source: "policy".to_string(),
            path: manifest_path.or_else(|| Some(root.display().to_string())),
            component: Some("manifest".to_string()),
            message: format!("plugin name '{}' is blocked by policy", manifest.name),
            severity: DiagnosticSeverity::Error,
        });
    }
    errors
}

fn append_missing_paths(
    errors: &mut Vec<PluginLoadError>,
    id: &str,
    root: &Path,
    component: &str,
    paths: &[PathBuf],
) {
    for path in paths {
        if path.exists() {
            continue;
        }
        let display = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        errors.push(PluginLoadError {
            kind: "path-not-found".to_string(),
            plugin: Some(id.to_string()),
            source: "plugin".to_string(),
            path: Some(path.display().to_string()),
            component: Some(component.to_string()),
            message: format!("declared {component} path does not exist: {display}"),
            // §6.2: an absent component location is not an error. The plugin
            // stays usable, so this is a warning rather than a failure.
            severity: DiagnosticSeverity::Warning,
        });
    }
}

fn mark_overrides(plugins: &mut [LoadedPlugin]) {
    let mut winners: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, plugin) in plugins.iter().enumerate() {
        let key = plugin.name.to_lowercase();
        match winners.get(&key).copied() {
            Some(current) if plugins[current].origin >= plugin.origin => {}
            _ => {
                winners.insert(key, idx);
            }
        }
    }

    for idx in 0..plugins.len() {
        let key = plugins[idx].name.to_lowercase();
        let Some(winner) = winners.get(&key).copied() else {
            continue;
        };
        if winner != idx {
            plugins[idx].available = false;
            plugins[idx].overridden_by = Some(plugins[winner].id.clone());
        }
    }
}

pub fn plugin_id(name: &str, source_key: &str) -> String {
    format!("{name}@{source_key}")
}

pub fn source_key(origin: PluginOrigin, marketplace: Option<&str>) -> String {
    match origin {
        PluginOrigin::Marketplace => marketplace
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("marketplace")
            .to_string(),
        other => other.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn workspace_overrides_builtin_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let builtin = tmp.path().join("builtin");
        let workspace = tmp.path().join("workspace");
        write_manifest(&builtin.join("demo"), "demo");
        write_manifest(&workspace.join("demo"), "demo");

        let plugins = load_plugins(&PluginRoots {
            session: Vec::new(),
            builtin,
            workspace: Some(workspace),
            personal: tmp.path().join("personal"),
            marketplace_cache: tmp.path().join("cache"),
            marketplaces: tmp.path().join("marketplaces"),
        });

        let active = plugins.iter().find(|p| p.available).unwrap();
        assert_eq!(active.origin, PluginOrigin::Workspace);
        assert!(plugins
            .iter()
            .any(|p| p.origin == PluginOrigin::BuiltIn && p.overridden_by.is_some()));
    }

    #[test]
    fn session_overrides_workspace_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let builtin = tmp.path().join("builtin");
        let workspace = tmp.path().join("workspace");
        let session = tmp.path().join("session");
        write_manifest(&builtin.join("demo"), "demo");
        write_manifest(&workspace.join("demo"), "demo");
        write_manifest(&session.join("demo"), "demo");

        let plugins = load_plugins(&PluginRoots {
            session: vec![session],
            builtin,
            workspace: Some(workspace),
            personal: tmp.path().join("personal"),
            marketplace_cache: tmp.path().join("cache"),
            marketplaces: tmp.path().join("marketplaces"),
        });

        let active = plugins.iter().find(|p| p.available).unwrap();
        assert_eq!(active.origin, PluginOrigin::Session);
        assert!(plugins
            .iter()
            .any(|p| p.origin == PluginOrigin::Workspace && p.overridden_by.is_some()));
    }

    #[test]
    fn reserved_plugin_names_are_loaded_as_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let personal = tmp.path().join("personal");
        write_manifest(&personal.join("reserved"), "builtin");

        let plugins = load_plugins(&PluginRoots {
            session: Vec::new(),
            builtin: tmp.path().join("builtin"),
            workspace: None,
            personal,
            marketplace_cache: tmp.path().join("cache"),
            marketplaces: tmp.path().join("marketplaces"),
        });

        let plugin = plugins
            .iter()
            .find(|plugin| plugin.id == "builtin@personal")
            .unwrap();
        assert!(!plugin.available);
        assert!(plugin
            .errors
            .iter()
            .any(|error| error.kind == "reserved-name"));
    }

    #[test]
    fn marketplace_discovery_skips_orphaned_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let active = cache.join("team").join("demo").join("0.2.0");
        let orphan = cache.join("team").join("demo").join("0.1.0");
        write_manifest(&active, "demo");
        write_manifest(&orphan, "demo");
        std::fs::write(orphan.join(PLUGIN_CACHE_ORPHAN_MARKER), "0").unwrap();

        let plugins = load_plugins(&PluginRoots {
            session: Vec::new(),
            builtin: tmp.path().join("builtin"),
            workspace: None,
            personal: tmp.path().join("personal"),
            marketplace_cache: cache,
            marketplaces: tmp.path().join("marketplaces"),
        });

        let marketplace_plugins = plugins
            .iter()
            .filter(|plugin| plugin.origin == PluginOrigin::Marketplace)
            .collect::<Vec<_>>();
        assert_eq!(marketplace_plugins.len(), 1);
        assert_eq!(marketplace_plugins[0].root, active);
    }

    #[test]
    fn discovery_skips_staging_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let personal = tmp.path().join("personal");
        let cache = tmp.path().join("cache");
        write_manifest(&personal.join(".staging").join("demo-stage"), "demo-stage");
        write_manifest(
            &cache.join(".staging").join("team-demo-stage"),
            "team-demo-stage",
        );
        write_manifest(&personal.join("demo"), "demo");

        let plugins = load_plugins(&PluginRoots {
            session: Vec::new(),
            builtin: tmp.path().join("builtin"),
            workspace: None,
            personal,
            marketplace_cache: cache,
            marketplaces: tmp.path().join("marketplaces"),
        });

        assert!(plugins.iter().any(|plugin| plugin.id == "demo@personal"));
        assert!(!plugins
            .iter()
            .any(|plugin| plugin.id == "demo-stage@personal"));
        assert!(!plugins.iter().any(|plugin| plugin.name.contains("stage")));
    }
}
