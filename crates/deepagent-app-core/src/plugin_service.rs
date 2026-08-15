//! UI-facing plugin service.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};
use deepagent_mcp::config::{McpConfig, TransportType};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin::dialect::{
    resolve_presentation, InterfaceSource, MarketplaceSource, PortableSource, Presentation,
    PresentationSources,
};
use crate::plugin::model::ResolvedPlugin;
use crate::plugin::spec::schema::{
    AGENT_PLUGIN_MANIFEST_RELATIVE_PATH, DISCOVERABLE_MANIFEST_PATHS,
};
use crate::plugin::spec::{normalize_and_expand, resolve_existing_within, resolve_plugin_relative};
use crate::plugin_dependency::{
    find_reverse_dependents, verify_plugin_dependencies, PluginDependencyOutcome,
};
use crate::plugin_loader::{
    load_plugins, plugin_id, LoadedPlugin, PluginLoadError, PluginOrigin, PluginRoots,
};
use crate::plugin_manifest::{find_plugin_manifest_path, load_plugin_manifest, PluginManifest};
use crate::plugin_marketplace::{
    find_marketplace_manifest_path, load_marketplace_catalog, marketplace_root_from_manifest,
    normalize_marketplace_name, slugify, AddPluginMarketplaceDto, PluginMarketplaceDto,
    PluginMarketplaceEntry, PluginMarketplaceEntryDto, PluginMarketplaceSource,
};
use crate::plugin_runtime::{EnabledPluginRuntimeInput, PluginRuntimeProjection};
use crate::plugin_security::{
    marketplace_source_kind_policy_error, scan_plugin_dir, PluginScanReportDto,
};

const PLUGIN_STATE_SCHEMA_VERSION: u32 = 1;
const PLUGIN_CACHE_ORPHAN_MARKER: &str = ".orphaned_at";
const PLUGIN_CACHE_ORPHAN_GRACE_MILLIS: u128 = 7 * 24 * 60 * 60 * 1000;
const PREPARED_PLUGIN_INSTALL_FILE: &str = ".prepared-install.json";
const GITHUB_API_BASES_ENV: &str = "DEEPAGENT_PLUGIN_GITHUB_API_BASES";
const GITHUB_TOPIC_PREFIX: &str = "https://github.com/topics/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSourceDto {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDependentDto {
    pub id: String,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginExecutionKind {
    HostBacked,
    SkillOnly,
    Subprocess,
    ManagedRuntime,
    DshSidecar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginLifecycleState {
    Discovered,
    Parsed,
    Installed,
    RuntimeReady,
    Executable,
    Verified,
    Incomplete,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHealthStatus {
    Ready,
    NeedsConfiguration,
    NeedsAuthorization,
    ConnectionUnavailable,
    RuntimeUnavailable,
    Incomplete,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginLicenseStatus {
    FirstParty,
    BundledThirdParty,
    MarketplaceOnly,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDto {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    pub source: PluginSourceDto,
    pub origin: String,
    pub dialect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub data_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    pub installed: bool,
    pub enabled: bool,
    pub available: bool,
    pub update_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overridden_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub skill_count: u32,
    pub mcp_server_count: u32,
    pub hook_count: u32,
    pub command_count: u32,
    pub agent_count: u32,
    pub app_count: u32,
    pub output_style_count: u32,
    pub state: PluginLifecycleState,
    pub execution_kind: PluginExecutionKind,
    pub runtime_required: bool,
    pub runtime_available: bool,
    #[serde(default)]
    pub entrypoints: Vec<String>,
    pub has_runtime_payload: bool,
    pub license_status: PluginLicenseStatus,
    pub health_status: PluginHealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_health_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_color: Option<String>,
    #[serde(default)]
    pub required_by: Vec<PluginDependentDto>,
    #[serde(default)]
    pub errors: Vec<PluginLoadError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRuntimeInspectionDto {
    pub plugin_id: String,
    pub execution_kind: PluginExecutionKind,
    pub state: PluginLifecycleState,
    pub runtime_required: bool,
    pub runtime_available: bool,
    #[serde(default)]
    pub entrypoints: Vec<String>,
    pub has_runtime_payload: bool,
    pub health_status: PluginHealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_health_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedPluginInstallDto {
    pub token: String,
    pub marketplace: String,
    pub plugin: String,
    pub plugin_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source_kind: String,
    pub source: String,
    pub content_hash: String,
    pub staging_path: String,
    pub plugin_root: String,
    pub destination_path: String,
    pub scan_report: PluginScanReportDto,
    pub runtime_inspection: PluginRuntimeInspectionDto,
}

impl PluginRuntimeInspectionDto {
    fn from_plugin(plugin: &PluginDto) -> Self {
        Self {
            plugin_id: plugin.id.clone(),
            execution_kind: plugin.execution_kind,
            state: plugin.state,
            runtime_required: plugin.runtime_required,
            runtime_available: plugin.runtime_available,
            entrypoints: plugin.entrypoints.clone(),
            has_runtime_payload: plugin.has_runtime_payload,
            health_status: plugin.health_status,
            last_health_check: plugin.last_health_check.clone(),
            health_error: plugin.health_error.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePluginDraftDto {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginState {
    pub version: Option<String>,
    #[serde(alias = "installPath")]
    pub install_path: String,
    #[serde(alias = "installedAt")]
    pub installed_at: String,
    #[serde(
        default,
        alias = "lastUpdated",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_updated: Option<String>,
    #[serde(
        default,
        alias = "contentHash",
        skip_serializing_if = "Option::is_none"
    )]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MarketplaceState {
    source: String,
    #[serde(default, alias = "gitRef", skip_serializing_if = "Option::is_none")]
    git_ref: Option<String>,
    #[serde(default, alias = "sparsePath", skip_serializing_if = "Option::is_none")]
    sparse_path: Option<String>,
    #[serde(
        default,
        alias = "installLocation",
        skip_serializing_if = "Option::is_none"
    )]
    install_location: Option<String>,
    #[serde(
        default,
        alias = "manifestPath",
        skip_serializing_if = "Option::is_none"
    )]
    manifest_path: Option<String>,
    #[serde(default, alias = "sourceRoot", skip_serializing_if = "Option::is_none")]
    source_root: Option<String>,
    #[serde(
        default,
        alias = "lastUpdated",
        skip_serializing_if = "Option::is_none"
    )]
    last_updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PluginState {
    #[serde(default = "plugin_state_schema_version")]
    version: u32,
    #[serde(default)]
    enabled: BTreeMap<String, bool>,
    #[serde(default)]
    installed: BTreeMap<String, InstalledPluginState>,
    #[serde(default)]
    marketplaces: BTreeMap<String, MarketplaceState>,
    #[serde(default, alias = "healthChecks")]
    health_checks: BTreeMap<String, PluginHealthCheckState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PluginHealthCheckState {
    status: PluginHealthStatus,
    #[serde(alias = "checkedAt", alias = "lastHealthCheck")]
    checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            version: PLUGIN_STATE_SCHEMA_VERSION,
            enabled: BTreeMap::new(),
            installed: BTreeMap::new(),
            marketplaces: BTreeMap::new(),
            health_checks: BTreeMap::new(),
        }
    }
}

pub struct PluginService {
    roots: PluginRoots,
    state_path: PathBuf,
    data_root: PathBuf,
    runtime_cache: Mutex<Option<PluginRuntimeCache>>,
    list_cache: Mutex<Option<PluginListCache>>,
}

struct PluginRuntimeCache {
    projection: PluginRuntimeProjection,
    snapshots: Vec<PathSnapshot>,
}

struct PluginListCache {
    plugins: Vec<PluginDto>,
    snapshots: Vec<PathSnapshot>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PluginComponentCounts {
    skills: u32,
    mcp_servers: u32,
    hooks: u32,
    commands: u32,
    agents: u32,
    apps: u32,
    output_styles: u32,
}

impl PluginComponentCounts {
    fn from_manifest(manifest: Option<&PluginManifest>) -> Self {
        let Some(manifest) = manifest else {
            return Self::default();
        };
        Self {
            skills: count_skills(manifest),
            mcp_servers: count_mcp_servers(manifest),
            hooks: count_hooks(manifest),
            commands: count_commands(manifest),
            agents: count_agents(manifest),
            apps: count_apps(manifest),
            output_styles: count_output_styles(manifest),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSnapshot {
    path: PathBuf,
    exists: bool,
    is_dir: bool,
    len: Option<u64>,
    modified_millis: Option<u128>,
}

impl PluginService {
    /// Build the plugin service with the app installation directory as the
    /// anchor for persistent plugin state and data.
    pub fn new(roots: PluginRoots, install_dir: impl AsRef<Path>) -> Self {
        let plugin_data = install_dir.as_ref().join("plugins");
        Self {
            roots,
            state_path: plugin_data.join("state.json"),
            data_root: plugin_data.join("data"),
            runtime_cache: Mutex::new(None),
            list_cache: Mutex::new(None),
        }
    }

    pub fn roots(&self) -> &PluginRoots {
        &self.roots
    }

    pub fn list(&self) -> Result<Vec<PluginDto>> {
        if let Some(plugins) = self.cached_plugin_list() {
            return Ok(plugins);
        }

        if let Err(error) = self.cleanup_orphaned_cache() {
            tracing::warn!(error = %error, "failed to clean orphaned plugin cache");
        }
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        let plugins = loaded
            .iter()
            .map(|plugin| self.dto_from_loaded(plugin, &loaded, &state, &dependencies))
            .collect::<Vec<_>>();
        let snapshots = self.plugin_list_cache_snapshots(&loaded, &state);
        self.store_plugin_list_cache(plugins.clone(), snapshots);
        Ok(plugins)
    }

    pub fn reload(&self) -> Result<Vec<PluginDto>> {
        self.list()
    }

    pub fn read(&self, id: &str) -> Result<Option<PluginDto>> {
        Ok(self.list()?.into_iter().find(|plugin| plugin.id == id))
    }

    pub fn inspect_plugin_runtime(&self, id: &str) -> Result<Option<PluginRuntimeInspectionDto>> {
        Ok(self
            .read(id)?
            .map(|plugin| PluginRuntimeInspectionDto::from_plugin(&plugin)))
    }

    pub fn check_plugin_health(&self, id: &str) -> Result<Option<PluginRuntimeInspectionDto>> {
        self.invalidate_plugin_caches();
        let Some(plugin) = self.read(id)? else {
            return Ok(None);
        };
        let checked_at = now_string();
        let (health_status, health_error) = self
            .explicit_plugin_health_override(id, &plugin)?
            .unwrap_or((plugin.health_status, plugin.health_error.clone()));
        let mut state = self.load_state()?;
        state.health_checks.insert(
            id.to_string(),
            PluginHealthCheckState {
                status: health_status,
                checked_at: checked_at.clone(),
                error: health_error.clone(),
            },
        );
        self.save_state(&state)?;
        self.invalidate_plugin_caches();

        let mut inspection = PluginRuntimeInspectionDto::from_plugin(&plugin);
        inspection.last_health_check = Some(checked_at);
        inspection.health_status = health_status;
        inspection.health_error = health_error;
        inspection.state = explicit_health_lifecycle_state(inspection.state, health_status);
        Ok(Some(inspection))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<PluginDto> {
        let mut state = self.load_state()?;
        state.enabled.insert(id.to_string(), enabled);
        self.save_state(&state)?;
        self.read(id)?
            .ok_or_else(|| CoreError::not_found(format!("plugin {id}")))
    }

    pub fn create_plugin(&self, draft: CreatePluginDraftDto) -> Result<PluginDto> {
        let display_name = draft.name.trim();
        if display_name.is_empty() {
            return Err(CoreError::invalid("plugin name cannot be empty"));
        }
        std::fs::create_dir_all(&self.roots.personal).map_err(|e| {
            CoreError::Persistence(format!(
                "create personal plugin root {}: {e}",
                self.roots.personal.display()
            ))
        })?;
        let slug = slugify(display_name);
        let target = self.personal_target_dir(draft.directory.as_deref(), &slug)?;
        if target.exists() {
            return Err(CoreError::invalid(format!(
                "plugin directory already exists: {}",
                target.display()
            )));
        }

        std::fs::create_dir_all(target.join(".codex-plugin")).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin manifest dir {}: {e}",
                target.display()
            ))
        })?;
        for dir in [
            "skills",
            "commands",
            "agents",
            "hooks",
            "assets",
            "output-styles",
        ] {
            std::fs::create_dir_all(target.join(dir))
                .map_err(|e| CoreError::Persistence(format!("create plugin subdir {dir}: {e}")))?;
        }

        let description = draft
            .description
            .and_then(trimmed_string)
            .unwrap_or_else(|| "Personal DeepAgent plugin.".to_string());
        let category = draft
            .category
            .and_then(trimmed_string)
            .unwrap_or_else(|| "Developer Tools".to_string());
        let manifest = serde_json::json!({
            "name": slug,
            "version": "0.1.0",
            "description": description,
            "author": { "name": "Personal" },
            "keywords": [],
            "skills": "./skills",
            "commands": "./commands",
            "agents": "./agents",
            "hooks": "./hooks/hooks.json",
            "interface": {
                "displayName": display_name,
                "shortDescription": description,
                "longDescription": description,
                "developerName": "Personal",
                "category": category,
                "capabilities": ["Skill", "Command"],
                "permissions": ["file.read"]
            }
        });
        let manifest_path = target.join(".codex-plugin").join("plugin.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).map_err(CoreError::from)?,
        )
        .map_err(|e| {
            CoreError::Persistence(format!(
                "write plugin manifest {}: {e}",
                manifest_path.display()
            ))
        })?;

        let id = plugin_id(&slug, PluginOrigin::Personal.as_str());
        let mut state = self.load_state()?;
        state.enabled.insert(id.clone(), true);
        self.save_state(&state)?;
        self.read(&id)?
            .ok_or_else(|| CoreError::other(format!("created plugin {id} was not discoverable")))
    }

    pub fn install_from_dir(&self, source_dir: impl AsRef<Path>) -> Result<PluginDto> {
        let source = source_dir.as_ref();
        let report = scan_plugin_dir(source)?;
        if !report.errors.is_empty() {
            return Err(CoreError::invalid(format!(
                "plugin scan failed: {}",
                report.errors.join("; ")
            )));
        }
        let manifest = load_plugin_manifest(source)?
            .ok_or_else(|| CoreError::invalid("plugin manifest not found"))?;
        let name = manifest.name.clone();
        let destination = self.roots.personal.join(&name);

        std::fs::create_dir_all(&self.roots.personal).map_err(|e| {
            CoreError::Persistence(format!(
                "create personal plugin root {}: {e}",
                self.roots.personal.display()
            ))
        })?;

        if same_path(source, &destination) {
            let id = plugin_id(&name, PluginOrigin::Personal.as_str());
            let mut state = self.load_state()?;
            state.enabled.insert(id.clone(), true);
            self.save_state(&state)?;
            return self
                .read(&id)?
                .ok_or_else(|| CoreError::not_found(format!("plugin {id}")));
        }

        commit_plugin_directory(source, &destination, &self.roots.personal, &name)?;
        let content_hash = plugin_directory_content_hash(&destination)?;

        let id = plugin_id(&name, PluginOrigin::Personal.as_str());
        let mut state = self.load_state()?;
        state.enabled.insert(id.clone(), true);
        state.installed.insert(
            id.clone(),
            InstalledPluginState {
                version: manifest.version.clone(),
                install_path: destination.display().to_string(),
                installed_at: now_string(),
                last_updated: None,
                content_hash: Some(content_hash),
            },
        );
        self.save_state(&state)?;
        self.read(&id)?
            .ok_or_else(|| CoreError::not_found(format!("plugin {id}")))
    }

    pub fn uninstall(&self, id: &str, remove_data: bool) -> Result<bool> {
        let Some(plugin) = load_plugins(&self.roots)
            .into_iter()
            .find(|plugin| plugin.id == id)
        else {
            let mut state = self.load_state()?;
            let changed =
                state.enabled.remove(id).is_some() || state.installed.remove(id).is_some();
            if changed {
                self.save_state(&state)?;
            }
            return Ok(false);
        };

        match plugin.origin {
            PluginOrigin::BuiltIn | PluginOrigin::Workspace | PluginOrigin::Session => {
                return Err(CoreError::invalid(format!(
                    "{} plugin cannot be uninstalled; disable it instead",
                    plugin.origin.as_str()
                )));
            }
            PluginOrigin::Personal => safe_remove_dir(&self.roots.personal, &plugin.root)?,
            PluginOrigin::Marketplace => {
                mark_cache_dir_orphaned(&self.roots.marketplace_cache, &plugin.root)?
            }
        }

        let mut state = self.load_state()?;
        state.enabled.remove(id);
        state.installed.remove(id);
        self.save_state(&state)?;

        if remove_data {
            let data_dir = self.data_root.join(sanitize_file_name(id));
            if data_dir.exists() {
                safe_remove_dir(&self.data_root, &data_dir)?;
            }
        }
        Ok(true)
    }

    pub fn list_marketplaces(&self) -> Result<Vec<PluginMarketplaceDto>> {
        let state = self.load_state()?;
        Ok(state
            .marketplaces
            .into_iter()
            .map(|(name, m)| PluginMarketplaceDto {
                name,
                source: m.source,
                git_ref: m.git_ref,
                sparse_path: m.sparse_path,
                install_location: m.install_location,
                last_updated: m.last_updated,
            })
            .collect())
    }

    pub fn add_marketplace(&self, input: AddPluginMarketplaceDto) -> Result<PluginMarketplaceDto> {
        let source = input.source.trim().to_string();
        if source.is_empty() {
            return Err(CoreError::invalid("marketplace source cannot be empty"));
        }
        let name = input
            .name
            .and_then(trimmed_string)
            .map(|s| slugify(&s))
            .unwrap_or_else(|| normalize_marketplace_name(&source));
        let registry_dir = self.roots.marketplaces.join(&name);
        std::fs::create_dir_all(&registry_dir).map_err(|e| {
            CoreError::Persistence(format!(
                "create marketplace dir {}: {e}",
                registry_dir.display()
            ))
        })?;
        let git_ref = input.git_ref.and_then(trimmed_string);
        let sparse_path = input.sparse_path.and_then(trimmed_string);
        let materialized = self.materialize_marketplace(
            &name,
            &source,
            git_ref.as_deref(),
            sparse_path.as_deref(),
        )?;
        let state_item = MarketplaceState {
            source: source.clone(),
            git_ref,
            sparse_path,
            install_location: materialized
                .source_root
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| Some(registry_dir.display().to_string())),
            manifest_path: materialized
                .manifest_path
                .as_ref()
                .map(|path| path.display().to_string()),
            source_root: materialized
                .source_root
                .as_ref()
                .map(|path| path.display().to_string()),
            last_updated: Some(now_string()),
        };
        let mut state = self.load_state()?;
        state.marketplaces.insert(name.clone(), state_item.clone());
        self.save_state(&state)?;
        Ok(PluginMarketplaceDto {
            name,
            source,
            git_ref: state_item.git_ref,
            sparse_path: state_item.sparse_path,
            install_location: state_item.install_location,
            last_updated: state_item.last_updated,
        })
    }

    pub fn remove_marketplace(&self, name: &str) -> Result<bool> {
        let key = slugify(name);
        let marketplace_cache_dir = self.roots.marketplace_cache.join(sanitize_file_name(&key));
        let loaded_marketplace_ids = load_plugins(&self.roots)
            .into_iter()
            .filter(|plugin| {
                plugin.origin == PluginOrigin::Marketplace
                    && plugin.marketplace.as_deref() == Some(key.as_str())
            })
            .map(|plugin| plugin.id)
            .collect::<Vec<_>>();

        let mut state = self.load_state()?;
        let removed = state.marketplaces.remove(&key).is_some();

        let mut plugin_ids = loaded_marketplace_ids;
        plugin_ids.extend(
            state
                .installed
                .iter()
                .filter(|(id, installed)| {
                    plugin_id_source(id).as_deref() == Some(key.as_str())
                        || path_is_under(
                            &PathBuf::from(&installed.install_path),
                            &marketplace_cache_dir,
                        )
                })
                .map(|(id, _)| id.clone()),
        );
        plugin_ids.extend(
            state
                .enabled
                .keys()
                .filter(|id| plugin_id_source(id).as_deref() == Some(key.as_str()))
                .cloned(),
        );
        plugin_ids.sort();
        plugin_ids.dedup();

        let mut changed = removed;
        for id in &plugin_ids {
            changed |= state.enabled.remove(id).is_some();
            changed |= state.installed.remove(id).is_some();
        }

        let registry_dir = self.roots.marketplaces.join(sanitize_file_name(&key));
        if registry_dir.exists() {
            remove_managed_child_dir(&self.roots.marketplaces, &registry_dir)?;
            changed = true;
        }
        if marketplace_cache_dir.exists() {
            remove_managed_child_dir(&self.roots.marketplace_cache, &marketplace_cache_dir)?;
            changed = true;
        }
        for id in &plugin_ids {
            let data_dir = self.data_root.join(sanitize_file_name(id));
            if data_dir.exists() {
                remove_managed_child_dir(&self.data_root, &data_dir)?;
                changed = true;
            }
        }

        if changed {
            self.save_state(&state)?;
        }
        Ok(changed)
    }

    pub fn refresh_marketplace(&self, name: &str) -> Result<PluginMarketplaceDto> {
        let key = slugify(name);
        let mut state = self.load_state()?;
        let Some(item) = state.marketplaces.get_mut(&key) else {
            return Err(CoreError::not_found(format!("marketplace {name}")));
        };
        let materialized = self.materialize_marketplace(
            &key,
            &item.source,
            item.git_ref.as_deref(),
            item.sparse_path.as_deref(),
        )?;
        if let Some(manifest_path) = materialized.manifest_path {
            item.manifest_path = Some(manifest_path.display().to_string());
        }
        if let Some(source_root) = materialized.source_root {
            item.install_location = Some(source_root.display().to_string());
            item.source_root = item.install_location.clone();
        }
        item.last_updated = Some(now_string());
        let dto = PluginMarketplaceDto {
            name: key,
            source: item.source.clone(),
            git_ref: item.git_ref.clone(),
            sparse_path: item.sparse_path.clone(),
            install_location: item.install_location.clone(),
            last_updated: item.last_updated.clone(),
        };
        self.save_state(&state)?;
        Ok(dto)
    }

    pub fn list_marketplace_entries(&self) -> Result<Vec<PluginMarketplaceEntryDto>> {
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let mut entries = Vec::new();
        for (marketplace_name, marketplace) in &state.marketplaces {
            let Some(catalog) = self.load_registered_marketplace(marketplace_name, marketplace)?
            else {
                continue;
            };
            for entry in catalog.entries {
                let id = plugin_id(&entry.name, marketplace_name);
                let loaded_plugin = loaded.iter().find(|plugin| plugin.id == id);
                let installed = loaded_plugin
                    .map(|plugin| plugin.resolved().is_some())
                    .unwrap_or(false);
                let enabled = installed
                    && state.enabled.get(&id).copied().unwrap_or_else(|| {
                        loaded_plugin
                            .map(LoadedPlugin::enabled_default)
                            .unwrap_or(false)
                    });
                let installed_version = state.installed.get(&id).and_then(|item| {
                    item.version.as_deref().or_else(|| {
                        loaded_plugin
                            .and_then(|plugin| plugin.resolved()?.portable.version.as_deref())
                    })
                });
                let update_available =
                    installed && versions_differ(entry.version.as_deref(), installed_version);
                let install_block_reason = marketplace_entry_install_block_reason(&entry);
                let installable = marketplace_entry_installable(&entry);
                let authentication_required =
                    marketplace_authentication_required(entry.policy_authentication.as_deref());
                debug_assert_eq!(installable, install_block_reason.is_none());
                entries.push(PluginMarketplaceEntryDto {
                    marketplace: marketplace_name.clone(),
                    name: entry.name.clone(),
                    display_name: entry
                        .display_name
                        .clone()
                        .unwrap_or_else(|| entry.name.clone()),
                    description: entry
                        .description
                        .clone()
                        .unwrap_or_else(|| "Marketplace plugin".to_string()),
                    version: entry.version.clone(),
                    category: entry.category.clone(),
                    source_kind: entry.source.kind().to_string(),
                    source: entry.source.display().to_string(),
                    installable,
                    install_block_reason,
                    installed,
                    enabled,
                    update_available,
                    policy_installation: entry.policy_installation.clone(),
                    policy_authentication: entry.policy_authentication.clone(),
                    authentication_required,
                    authentication_hint: marketplace_authentication_hint(&entry),
                });
            }
        }
        entries.sort_by(|a, b| {
            a.marketplace
                .cmp(&b.marketplace)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    pub fn scan_marketplace_plugin(
        &self,
        marketplace: &str,
        plugin: &str,
    ) -> Result<PluginScanReportDto> {
        let (marketplace_key, entry) = self.marketplace_entry(marketplace, plugin)?;
        ensure_marketplace_entry_installable(&marketplace_key, &entry)?;
        let materialized = self.materialize_marketplace_plugin_source(&marketplace_key, &entry)?;
        scan_plugin_dir(materialized.plugin_root())
    }

    pub fn prepare_plugin_install(
        &self,
        marketplace: &str,
        plugin: &str,
        authentication_confirmed: bool,
    ) -> Result<PreparedPluginInstallDto> {
        let marketplace_key = slugify(marketplace);
        let (_, entry) = self.marketplace_entry(&marketplace_key, plugin)?;
        ensure_marketplace_entry_installable(&marketplace_key, &entry)?;
        ensure_marketplace_authentication_confirmed(
            &marketplace_key,
            &entry,
            authentication_confirmed,
        )?;

        let staging = self.marketplace_staging_dir(&marketplace_key, &entry.name)?;
        let prepared = (|| {
            let materialized =
                self.materialize_marketplace_plugin_source(&marketplace_key, &entry)?;
            let package_root = staging.join("package");
            copy_dir(materialized.plugin_root(), &package_root)?;
            let plugin_root = resolve_plugin_root(&package_root)?;
            let report = scan_plugin_dir(&plugin_root)?;
            if !report.errors.is_empty() {
                return Err(CoreError::invalid(format!(
                    "plugin scan failed during prepare: {}",
                    report.errors.join("; ")
                )));
            }

            let manifest = load_plugin_manifest(&plugin_root)?
                .ok_or_else(|| CoreError::invalid("plugin manifest not found"))?;
            if manifest.name != entry.name {
                return Err(CoreError::invalid(format!(
                    "marketplace entry '{}' points to plugin manifest '{}'",
                    entry.name, manifest.name
                )));
            }

            let version = manifest
                .version
                .clone()
                .or_else(|| entry.version.clone())
                .unwrap_or_else(|| "0.0.0".to_string());
            let destination =
                self.marketplace_plugin_destination(&marketplace_key, &manifest.name, &version);
            let token = staging
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "prepared plugin staging dir has no token: {}",
                        staging.display()
                    ))
                })?
                .to_string();
            let plugin_id = plugin_id(&manifest.name, &marketplace_key);
            let runtime_inspection = self.runtime_inspection_for_staged_plugin(
                &plugin_id,
                &marketplace_key,
                &plugin_root,
                &manifest,
            );
            let content_hash = plugin_directory_content_hash(&plugin_root)?;
            let metadata = PreparedPluginInstallState {
                schema_version: 1,
                token: token.clone(),
                marketplace: marketplace_key.clone(),
                plugin: manifest.name.clone(),
                plugin_id: plugin_id.clone(),
                plugin_version: Some(version.clone()),
                plugin_root: plugin_root.display().to_string(),
                destination_path: destination.display().to_string(),
                source_kind: entry.source.kind().to_string(),
                source: entry.source.display().to_string(),
                content_hash: content_hash.clone(),
                created_at: now_string(),
            };
            write_prepared_install_metadata(&staging, &metadata)?;

            Ok(PreparedPluginInstallDto {
                token,
                marketplace: marketplace_key,
                plugin: manifest.name,
                plugin_id,
                version: Some(version),
                source_kind: entry.source.kind().to_string(),
                source: entry.source.display().to_string(),
                content_hash,
                staging_path: staging.display().to_string(),
                plugin_root: plugin_root.display().to_string(),
                destination_path: destination.display().to_string(),
                scan_report: report,
                runtime_inspection,
            })
        })();
        if prepared.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        prepared
    }

    pub fn commit_plugin_install(&self, token: &str) -> Result<PluginDto> {
        let staging = self.prepared_install_dir(token)?;
        let result = (|| {
            let metadata = load_prepared_install_metadata(&staging)?;
            if metadata.token != token {
                return Err(CoreError::invalid(format!(
                    "prepared plugin token mismatch: expected {token}, found {}",
                    metadata.token
                )));
            }
            let plugin_root = PathBuf::from(&metadata.plugin_root);
            if !path_is_under(&plugin_root, &staging) {
                return Err(CoreError::invalid(format!(
                    "prepared plugin root escapes staging dir: {}",
                    plugin_root.display()
                )));
            }
            let report = scan_plugin_dir(&plugin_root)?;
            if !report.errors.is_empty() {
                return Err(CoreError::invalid(format!(
                    "plugin scan failed during commit: {}",
                    report.errors.join("; ")
                )));
            }
            let manifest = load_plugin_manifest(&plugin_root)?
                .ok_or_else(|| CoreError::invalid("plugin manifest not found"))?;
            if manifest.name != metadata.plugin {
                return Err(CoreError::invalid(format!(
                    "prepared plugin metadata names '{}' but manifest names '{}'",
                    metadata.plugin, manifest.name
                )));
            }
            if plugin_id(&manifest.name, &metadata.marketplace) != metadata.plugin_id {
                return Err(CoreError::invalid(format!(
                    "prepared plugin id changed for '{}'",
                    manifest.name
                )));
            }
            let content_hash = plugin_directory_content_hash(&plugin_root)?;
            if content_hash != metadata.content_hash {
                return Err(CoreError::invalid(format!(
                    "prepared plugin content hash changed: expected {}, got {}",
                    metadata.content_hash, content_hash
                )));
            }

            let mut stack = vec![metadata.plugin_id.clone()];
            for dependency in &manifest.dependencies {
                let dependency = marketplace_dependency_id(dependency, &metadata.marketplace)?;
                if dependency.marketplace != metadata.marketplace {
                    if self.is_marketplace_dependency_satisfied(&dependency.id)? {
                        continue;
                    }
                    return Err(CoreError::invalid(format!(
                        "plugin '{}' depends on '{}' from marketplace '{}'; install that dependency explicitly before installing this plugin",
                        manifest.name, dependency.name, dependency.marketplace
                    )));
                }
                if self.is_marketplace_dependency_satisfied(&dependency.id)? {
                    continue;
                }
                self.install_from_marketplace_inner(
                    &metadata.marketplace,
                    &dependency.name,
                    true,
                    &mut stack,
                )?;
            }

            let version = manifest
                .version
                .clone()
                .or_else(|| metadata.plugin_version.clone())
                .unwrap_or_else(|| "0.0.0".to_string());
            let destination = self.marketplace_plugin_destination(
                &metadata.marketplace,
                &manifest.name,
                &version,
            );
            if destination.display().to_string() != metadata.destination_path {
                return Err(CoreError::invalid(format!(
                    "prepared plugin destination changed: expected {}, found {}",
                    metadata.destination_path,
                    destination.display()
                )));
            }
            commit_plugin_directory(
                &plugin_root,
                &destination,
                &self.roots.marketplace_cache,
                &format!("{}-{}-{version}", metadata.marketplace, manifest.name),
            )?;
            let plugin_dir = self
                .roots
                .marketplace_cache
                .join(sanitize_file_name(&metadata.marketplace))
                .join(sanitize_file_name(&manifest.name));
            if plugin_dir.exists() {
                mark_stale_marketplace_plugin_versions(
                    &self.roots.marketplace_cache,
                    &plugin_dir,
                    &destination,
                )?;
            }

            let id = plugin_id(&manifest.name, &metadata.marketplace);
            let mut state = self.load_state()?;
            let previous = state.installed.get(&id).cloned();
            state.enabled.entry(id.clone()).or_insert(true);
            state.installed.insert(
                id.clone(),
                InstalledPluginState {
                    version: manifest.version.clone().or(Some(version)),
                    install_path: destination.display().to_string(),
                    installed_at: previous
                        .as_ref()
                        .map(|item| item.installed_at.clone())
                        .unwrap_or_else(now_string),
                    last_updated: previous.as_ref().map(|_| now_string()),
                    content_hash: Some(metadata.content_hash.clone()),
                },
            );
            self.save_state(&state)?;
            self.read(&id)?
                .ok_or_else(|| CoreError::not_found(format!("plugin {id}")))
        })();
        let _ = remove_dir_all_with_retry(&staging);
        result
    }

    pub fn cancel_plugin_install(&self, token: &str) -> Result<bool> {
        let staging = self.prepared_install_dir(token)?;
        if !staging.exists() {
            return Ok(false);
        }
        safe_remove_dir(&self.roots.marketplace_cache.join(".staging"), &staging)?;
        Ok(true)
    }

    pub fn install_from_marketplace(&self, marketplace: &str, plugin: &str) -> Result<PluginDto> {
        self.install_from_marketplace_with_auth(marketplace, plugin, false)
    }

    pub fn install_from_marketplace_with_auth(
        &self,
        marketplace: &str,
        plugin: &str,
        authentication_confirmed: bool,
    ) -> Result<PluginDto> {
        let marketplace_key = slugify(marketplace);
        let mut stack = Vec::new();
        self.install_from_marketplace_inner(
            &marketplace_key,
            plugin,
            authentication_confirmed,
            &mut stack,
        )
    }

    pub fn scan_plugin_update(&self, id: &str) -> Result<PluginScanReportDto> {
        let (marketplace, plugin) = self.marketplace_plugin_for_id(id)?;
        self.scan_marketplace_plugin(&marketplace, &plugin)
    }

    pub fn update_plugin(&self, id: &str) -> Result<PluginDto> {
        self.update_plugin_with_auth(id, false)
    }

    pub fn update_plugin_with_auth(
        &self,
        id: &str,
        authentication_confirmed: bool,
    ) -> Result<PluginDto> {
        let (marketplace, plugin_name) = self.marketplace_plugin_for_id(id)?;
        let before = self.load_state()?;
        let previous = before.installed.get(id).cloned();
        let previous_enabled = before.enabled.get(id).copied();

        let updated = self.install_from_marketplace_with_auth(
            &marketplace,
            &plugin_name,
            authentication_confirmed,
        )?;

        let mut state = self.load_state()?;
        if let Some(installed) = state.installed.get_mut(&updated.id) {
            if let Some(previous) = previous {
                installed.installed_at = previous.installed_at;
            }
            installed.last_updated = Some(now_string());
        }
        if let Some(enabled) = previous_enabled {
            state.enabled.insert(updated.id.clone(), enabled);
        }
        self.save_state(&state)?;
        self.read(&updated.id)?
            .ok_or_else(|| CoreError::not_found(format!("plugin {}", updated.id)))
    }

    pub fn cleanup_orphaned_cache(&self) -> Result<u32> {
        self.cleanup_orphaned_cache_older_than(PLUGIN_CACHE_ORPHAN_GRACE_MILLIS)
    }

    fn cleanup_orphaned_cache_older_than(&self, grace_millis: u128) -> Result<u32> {
        if !self.roots.marketplace_cache.is_dir() {
            return Ok(0);
        }
        let state = self.load_state()?;
        let installed_paths = state
            .installed
            .values()
            .map(|installed| PathBuf::from(&installed.install_path))
            .filter(|path| path_is_under(path, &self.roots.marketplace_cache))
            .collect::<Vec<_>>();
        for path in &installed_paths {
            remove_orphan_marker_if_exists(path)?;
        }
        cleanup_orphaned_cache_dir(
            &self.roots.marketplace_cache,
            &self.roots.marketplace_cache,
            &installed_paths,
            now_millis(),
            grace_millis,
        )
    }

    fn install_from_marketplace_inner(
        &self,
        marketplace_key: &str,
        plugin: &str,
        authentication_confirmed: bool,
        stack: &mut Vec<String>,
    ) -> Result<PluginDto> {
        let (_, entry) = self.marketplace_entry(marketplace_key, plugin)?;
        ensure_marketplace_entry_installable(marketplace_key, &entry)?;
        ensure_marketplace_authentication_confirmed(
            marketplace_key,
            &entry,
            authentication_confirmed,
        )?;
        let id = plugin_id(&entry.name, marketplace_key);
        if stack.iter().any(|item| item == &id) {
            let mut chain = stack.clone();
            chain.push(id);
            return Err(CoreError::invalid(format!(
                "plugin dependency cycle while installing marketplace plugin: {}",
                chain.join(" -> ")
            )));
        }
        stack.push(id.clone());

        let materialized = self.materialize_marketplace_plugin_source(marketplace_key, &entry)?;
        let source = materialized.plugin_root();
        let report = scan_plugin_dir(source)?;
        if !report.errors.is_empty() {
            return Err(CoreError::invalid(format!(
                "plugin scan failed: {}",
                report.errors.join("; ")
            )));
        }

        let manifest = load_plugin_manifest(source)?
            .ok_or_else(|| CoreError::invalid("plugin manifest not found"))?;
        if manifest.name != entry.name {
            return Err(CoreError::invalid(format!(
                "marketplace entry '{}' points to plugin manifest '{}'",
                entry.name, manifest.name
            )));
        }

        for dependency in &manifest.dependencies {
            let dependency = marketplace_dependency_id(dependency, marketplace_key)?;
            if dependency.marketplace != marketplace_key {
                if self.is_marketplace_dependency_satisfied(&dependency.id)? {
                    continue;
                }
                return Err(CoreError::invalid(format!(
                    "plugin '{}' depends on '{}' from marketplace '{}'; install that dependency explicitly before installing this plugin",
                    entry.name, dependency.name, dependency.marketplace
                )));
            }
            if self.is_marketplace_dependency_satisfied(&dependency.id)? {
                continue;
            }
            self.install_from_marketplace_inner(
                marketplace_key,
                &dependency.name,
                authentication_confirmed,
                stack,
            )?;
        }

        let version = manifest
            .version
            .clone()
            .or_else(|| entry.version.clone())
            .unwrap_or_else(|| "0.0.0".to_string());
        let marketplace_dir = self
            .roots
            .marketplace_cache
            .join(sanitize_file_name(marketplace_key));
        let plugin_dir = marketplace_dir.join(sanitize_file_name(&manifest.name));
        let destination = plugin_dir.join(sanitize_file_name(&version));
        commit_plugin_directory(
            source,
            &destination,
            &self.roots.marketplace_cache,
            &format!("{marketplace_key}-{}-{version}", manifest.name),
        )?;
        if plugin_dir.exists() {
            mark_stale_marketplace_plugin_versions(
                &self.roots.marketplace_cache,
                &plugin_dir,
                &destination,
            )?;
        }
        let content_hash = plugin_directory_content_hash(&destination)?;

        let id = plugin_id(&manifest.name, marketplace_key);
        let mut state = self.load_state()?;
        state.enabled.insert(id.clone(), true);
        state.installed.insert(
            id.clone(),
            InstalledPluginState {
                version: manifest.version.clone().or(Some(version)),
                install_path: destination.display().to_string(),
                installed_at: now_string(),
                last_updated: None,
                content_hash: Some(content_hash),
            },
        );
        self.save_state(&state)?;
        stack.pop();
        self.read(&id)?
            .ok_or_else(|| CoreError::not_found(format!("plugin {id}")))
    }

    fn is_marketplace_dependency_satisfied(&self, id: &str) -> Result<bool> {
        Ok(self
            .read(id)?
            .map(|plugin| plugin.installed && plugin.available && plugin.enabled)
            .unwrap_or(false))
    }

    fn marketplace_plugin_for_id(&self, id: &str) -> Result<(String, String)> {
        let Some(plugin) = load_plugins(&self.roots)
            .into_iter()
            .find(|plugin| plugin.id == id)
        else {
            return Err(CoreError::not_found(format!("plugin {id}")));
        };
        if plugin.origin != PluginOrigin::Marketplace {
            return Err(CoreError::invalid(format!(
                "plugin {id} is not a marketplace plugin"
            )));
        }
        let marketplace = plugin
            .marketplace
            .clone()
            .unwrap_or_else(|| plugin.source_key.clone());
        Ok((marketplace, plugin.name))
    }

    pub fn scan_plugin(&self, source_dir: impl AsRef<Path>) -> Result<PluginScanReportDto> {
        scan_plugin_dir(source_dir.as_ref())
    }

    pub fn runtime_projection(&self) -> Result<PluginRuntimeProjection> {
        if let Some(projection) = self.cached_runtime_projection() {
            return Ok(projection);
        }

        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        let mut enabled_plugins = loaded
            .iter()
            .filter(|plugin| {
                self.is_effectively_enabled(plugin, &state)
                    && !dependencies.demoted.contains(&plugin.id)
            })
            .collect::<Vec<_>>();
        enabled_plugins.sort_by(|a, b| {
            plugin_runtime_priority(b.origin)
                .cmp(&plugin_runtime_priority(a.origin))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut inputs = Vec::new();
        for plugin in enabled_plugins {
            let Some(resolved) = plugin.resolved() else {
                continue;
            };
            let data_dir = self.data_root.join(sanitize_file_name(&plugin.id));
            // Agent Plugins §9.1: PLUGIN_DATA is handed to plugin subprocesses,
            // and the client must create that directory and make it writable
            // before launching them. `prepare_runtime_payload` below only
            // creates it for plugins shipping a `runtime.zip`, so without this
            // the directory would be missing for every other plugin and any
            // write from the subprocess would fail.
            std::fs::create_dir_all(&data_dir).map_err(|e| {
                CoreError::Persistence(format!(
                    "create plugin data dir {}: {e}",
                    data_dir.display()
                ))
            })?;
            prepare_runtime_payload(&plugin.root, &data_dir)?;
            inputs.push(EnabledPluginRuntimeInput {
                id: &plugin.id,
                name: &plugin.name,
                source_priority: plugin_runtime_priority(plugin.origin),
                root: &plugin.root,
                data_dir,
                plugin: resolved,
            });
        }
        let projection = PluginRuntimeProjection::from_enabled_plugins(inputs);
        let snapshots = self.runtime_cache_snapshots(&loaded);
        self.store_runtime_projection_cache(projection.clone(), snapshots);
        Ok(projection)
    }

    pub fn list_apps(&self) -> Result<Vec<crate::plugin_runtime::PluginAppEntry>> {
        Ok(self
            .runtime_projection()?
            .app_entries
            .into_iter()
            .filter(|app| host_app_component_is_renderable(&app.component))
            .collect())
    }

    pub fn list_output_styles(&self) -> Result<Vec<crate::plugin_runtime::PluginOutputStyleEntry>> {
        Ok(self.runtime_projection()?.output_styles)
    }

    fn dependency_outcome(
        &self,
        loaded: &[LoadedPlugin],
        state: &PluginState,
    ) -> PluginDependencyOutcome {
        verify_plugin_dependencies(loaded, |plugin| self.is_effectively_enabled(plugin, state))
    }

    fn is_effectively_enabled(&self, plugin: &LoadedPlugin, state: &PluginState) -> bool {
        plugin.available
            && plugin.resolved().is_some()
            && state
                .enabled
                .get(&plugin.id)
                .copied()
                .unwrap_or_else(|| plugin.enabled_default())
    }

    fn plugin_update_available(&self, plugin: &LoadedPlugin, state: &PluginState) -> bool {
        if plugin.origin != PluginOrigin::Marketplace {
            return false;
        }
        let Some(resolved) = plugin.resolved() else {
            return false;
        };
        let Some(marketplace) = plugin.marketplace.as_deref() else {
            return false;
        };
        let Some(marketplace_state) = state.marketplaces.get(marketplace) else {
            return false;
        };
        let Ok(Some(catalog)) = self.load_registered_marketplace(marketplace, marketplace_state)
        else {
            return false;
        };
        let Some(entry) = catalog
            .entries
            .iter()
            .find(|entry| entry.name == plugin.name)
        else {
            return false;
        };
        versions_differ(
            entry.version.as_deref(),
            resolved.portable.version.as_deref(),
        )
    }

    fn plugin_execution_kind(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        counts: PluginComponentCounts,
        has_runtime_payload: bool,
    ) -> PluginExecutionKind {
        if self.bundled_license_bucket(plugin.name.as_str())
            == Some(BundledPluginBucket::FirstParty)
            || manifest.is_some_and(manifest_has_host_backed_app)
        {
            return PluginExecutionKind::HostBacked;
        }
        if has_runtime_payload
            || manifest
                .map(|manifest| {
                    manifest.runtime.node.is_some()
                        || manifest.runtime.python.is_some()
                        || manifest.runtime.java.is_some()
                })
                .unwrap_or(false)
        {
            return PluginExecutionKind::ManagedRuntime;
        }

        if counts.mcp_servers > 0 || counts.hooks > 0 || counts.apps > 0 {
            return PluginExecutionKind::DshSidecar;
        }

        if counts.commands > 0 {
            return PluginExecutionKind::Subprocess;
        }

        if counts.skills > 0 || counts.agents > 0 || counts.output_styles > 0 {
            return PluginExecutionKind::SkillOnly;
        }

        PluginExecutionKind::SkillOnly
    }

    fn runtime_inspection_for_staged_plugin(
        &self,
        plugin_id: &str,
        marketplace: &str,
        plugin_root: &Path,
        manifest: &PluginManifest,
    ) -> PluginRuntimeInspectionDto {
        let resolved = ResolvedPlugin::from_manifest(
            plugin_root,
            manifest.clone(),
            MarketplaceSource::default(),
        );
        let loaded = LoadedPlugin {
            id: plugin_id.to_string(),
            name: manifest.name.clone(),
            source_key: marketplace.to_string(),
            origin: PluginOrigin::Marketplace,
            marketplace: Some(marketplace.to_string()),
            root: plugin_root.to_path_buf(),
            resolved: Some(resolved),
            available: true,
            overridden_by: None,
            errors: Vec::new(),
        };
        let counts = PluginComponentCounts::from_manifest(Some(manifest));
        let entrypoints = self.plugin_entrypoints(&loaded, Some(manifest));
        let has_runtime_payload = plugin_root.join("runtime.zip").is_file();
        let runtime_requirements =
            self.plugin_runtime_requirements(&loaded, Some(manifest), has_runtime_payload);
        let runtime_available = self.runtime_requirements_available(&runtime_requirements);
        let resolved = loaded.resolved.as_ref();
        let health_status = self.plugin_health_status(
            &loaded,
            Some(manifest),
            resolved,
            counts,
            runtime_available,
            &runtime_requirements,
        );
        PluginRuntimeInspectionDto {
            plugin_id: plugin_id.to_string(),
            execution_kind: self.plugin_execution_kind(
                &loaded,
                Some(manifest),
                counts,
                has_runtime_payload,
            ),
            state: self.plugin_lifecycle_state(
                &loaded,
                Some(manifest),
                resolved,
                counts,
                health_status,
                &entrypoints,
            ),
            runtime_required: !runtime_requirements.is_empty(),
            runtime_available,
            entrypoints,
            has_runtime_payload,
            health_status,
            last_health_check: Some(now_string()),
            health_error: self.plugin_health_error(
                &loaded,
                Some(manifest),
                counts,
                &runtime_requirements,
                health_status,
            ),
        }
    }

    fn plugin_runtime_requirements(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        has_runtime_payload: bool,
    ) -> PluginRuntimeNeeds {
        let mut needs = PluginRuntimeNeeds::default();
        if let Some(manifest) = manifest {
            if manifest.runtime.node.is_some() {
                needs.node = true;
            }
            if manifest.runtime.python.is_some() {
                needs.python = true;
            }
            if manifest.runtime.java.is_some() {
                needs.java = true;
            }
        }
        if has_runtime_payload {
            needs.node = true;
        }
        needs.merge_script_requirements(&plugin.root);
        for script in self.plugin_hook_command_scripts(plugin, manifest).scripts {
            needs.merge_script_path(&script);
        }
        needs
    }

    fn runtime_requirements_available(&self, needs: &PluginRuntimeNeeds) -> bool {
        (!needs.node || probe_runtime("node", &["--version"]))
            && (!needs.python || probe_runtime("python", &["--version"]))
            && (!needs.java || probe_runtime("java", &["-version"]))
            && (!needs.shell || probe_shell())
    }

    fn plugin_entrypoints(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
    ) -> Vec<String> {
        let mut entrypoints = Vec::new();
        let Some(manifest) = manifest else {
            return entrypoints;
        };

        if plugin.root.join("runtime.zip").is_file() {
            entrypoints.push(plugin.root.join("runtime.zip").display().to_string());
        }
        for path in &manifest.paths.skills {
            if path.exists() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in &manifest.paths.commands {
            if path.exists() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in &manifest.paths.agents {
            if path.exists() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in &manifest.paths.mcp_server_paths {
            if path.is_file() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in &manifest.paths.hook_paths {
            if path.is_file() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in self
            .plugin_hook_command_scripts(plugin, Some(manifest))
            .scripts
        {
            entrypoints.push(path.display().to_string());
        }
        for path in &manifest.paths.app_paths {
            if path.exists() {
                entrypoints.push(path.display().to_string());
            }
        }
        for path in &manifest.paths.output_styles {
            if path.exists() {
                entrypoints.push(path.display().to_string());
            }
        }
        entrypoints.sort();
        entrypoints.dedup();
        entrypoints
    }

    fn plugin_hook_command_scripts(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
    ) -> HookCommandInspection {
        let Some(manifest) = manifest else {
            return HookCommandInspection::default();
        };
        let data_dir = self.data_root.join(sanitize_file_name(&plugin.id));
        inspect_hook_command_scripts(
            &plugin.root,
            &data_dir,
            manifest.paths.hooks_inline.as_ref(),
            &manifest.paths.hook_paths,
        )
    }

    fn plugin_license_status(&self, plugin: &LoadedPlugin) -> PluginLicenseStatus {
        match self.bundled_license_bucket(plugin.name.as_str()) {
            Some(BundledPluginBucket::FirstParty) => PluginLicenseStatus::FirstParty,
            Some(BundledPluginBucket::BundledThirdParty) => PluginLicenseStatus::BundledThirdParty,
            Some(BundledPluginBucket::MarketplaceOnly) => PluginLicenseStatus::MarketplaceOnly,
            None => PluginLicenseStatus::Unknown,
        }
    }

    fn plugin_health_status(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        resolved: Option<&ResolvedPlugin>,
        counts: PluginComponentCounts,
        runtime_available: bool,
        runtime_requirements: &PluginRuntimeNeeds,
    ) -> PluginHealthStatus {
        if !plugin.available || resolved.is_none() {
            return PluginHealthStatus::Failed;
        }
        if has_fatal_plugin_errors(plugin) {
            return PluginHealthStatus::Failed;
        }
        let execution_kind = manifest
            .map(|manifest| {
                self.plugin_execution_kind(
                    plugin,
                    Some(manifest),
                    counts,
                    plugin.root.join("runtime.zip").is_file(),
                )
            })
            .unwrap_or(PluginExecutionKind::SkillOnly);
        if runtime_requirements.requires_runtime() && !runtime_available {
            return PluginHealthStatus::RuntimeUnavailable;
        }

        if execution_kind == PluginExecutionKind::HostBacked {
            if let Some(manifest) = manifest {
                if !self
                    .host_backed_validation_errors(plugin, manifest, counts)
                    .is_empty()
                {
                    return PluginHealthStatus::Incomplete;
                }
            } else {
                return PluginHealthStatus::Incomplete;
            }
        }
        if manifest.is_some()
            && !self
                .plugin_hook_command_scripts(plugin, manifest)
                .errors
                .is_empty()
        {
            return PluginHealthStatus::Incomplete;
        }

        if manifest.is_some_and(manifest_needs_host_authorization) {
            return PluginHealthStatus::NeedsAuthorization;
        }
        if !missing_credential_env_hints(&plugin.root).is_empty() {
            return PluginHealthStatus::NeedsConfiguration;
        }

        PluginHealthStatus::Ready
    }

    fn plugin_health_error(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        counts: PluginComponentCounts,
        runtime_requirements: &PluginRuntimeNeeds,
        status: PluginHealthStatus,
    ) -> Option<String> {
        if status == PluginHealthStatus::Ready {
            return None;
        }
        let message = match status {
            PluginHealthStatus::NeedsConfiguration => {
                let hints = missing_credential_env_hints(&plugin.root);
                if hints.is_empty() {
                    "plugin configuration is required".to_string()
                } else {
                    format!(
                        "plugin references unconfigured credential environment variables: {}",
                        hints.join(", ")
                    )
                }
            }
            PluginHealthStatus::NeedsAuthorization => {
                "plugin declares an OAuth-backed MCP or connector that still needs host authorization"
                    .to_string()
            }
            PluginHealthStatus::ConnectionUnavailable => {
                "plugin declares a hosted MCP endpoint that is not reachable".to_string()
            }
            PluginHealthStatus::RuntimeUnavailable => {
                format_runtime_unavailable(runtime_requirements)
            }
            PluginHealthStatus::Incomplete => {
                if let Some(manifest) = manifest {
                    if self.plugin_execution_kind(
                        plugin,
                        Some(manifest),
                        counts,
                        plugin.root.join("runtime.zip").is_file(),
                    ) == PluginExecutionKind::HostBacked
                    {
                        let errors = self.host_backed_validation_errors(plugin, manifest, counts);
                        if errors.is_empty() {
                            "host-backed plugin is missing one or more validated host bindings"
                                .to_string()
                        } else {
                            errors.join("; ")
                        }
                    } else {
                        let hook_errors = self.plugin_hook_command_scripts(plugin, Some(manifest));
                        if hook_errors.errors.is_empty() {
                            "plugin is missing one or more required entrypoints".to_string()
                        } else {
                            hook_errors.errors.join("; ")
                        }
                    }
                } else {
                    "plugin manifest could not be resolved".to_string()
                }
            }
            PluginHealthStatus::Failed => {
                if !plugin.available {
                    "plugin is unavailable".to_string()
                } else if manifest.is_none() {
                    "plugin manifest failed to load".to_string()
                } else {
                    "plugin failed health inspection".to_string()
                }
            }
            PluginHealthStatus::Ready | PluginHealthStatus::Unknown => return None,
        };
        Some(message)
    }

    fn explicit_plugin_health_override(
        &self,
        id: &str,
        plugin: &PluginDto,
    ) -> Result<Option<(PluginHealthStatus, Option<String>)>> {
        if plugin.health_status != PluginHealthStatus::Ready {
            return Ok(None);
        }
        let Some(loaded) = load_plugins(&self.roots)
            .into_iter()
            .find(|loaded| loaded.id == id)
        else {
            return Ok(None);
        };
        let Some(resolved) = loaded.resolved.as_ref() else {
            return Ok(None);
        };

        let data_dir = self.data_root.join(sanitize_file_name(&loaded.id));
        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: &loaded.id,
                name: &loaded.name,
                source_priority: plugin_runtime_priority(loaded.origin),
                root: &loaded.root,
                data_dir,
                plugin: resolved,
            }]);

        if let Some(error) = projection
            .errors
            .iter()
            .find(|error| error.component == "mcp")
        {
            return Ok(Some((
                PluginHealthStatus::Failed,
                Some(format!(
                    "plugin MCP runtime projection failed: {}",
                    error.message
                )),
            )));
        }

        let failures = hosted_mcp_connection_failures(&projection.mcp_config);
        if failures.is_empty() {
            Ok(None)
        } else {
            Ok(Some((
                PluginHealthStatus::ConnectionUnavailable,
                Some(format!(
                    "hosted MCP endpoint unavailable: {}",
                    failures.join("; ")
                )),
            )))
        }
    }

    fn host_backed_validation_errors(
        &self,
        _plugin: &LoadedPlugin,
        manifest: &PluginManifest,
        counts: PluginComponentCounts,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        let app_components = host_app_components(manifest);
        let app_component_set = app_components
            .iter()
            .map(|component| host_component_name(component))
            .collect::<BTreeSet<_>>();
        for component in &app_components {
            if !host_app_component_is_renderable(component) {
                errors.push(format!(
                    "host app component '{component}' is not registered in the desktop host registry"
                ));
            }
        }
        if counts.apps > 0 && app_components.is_empty() {
            errors.push(
                "host-backed plugin declares app config but no renderable component was found"
                    .to_string(),
            );
        }
        let command_ids = host_command_ids(manifest);
        if counts.commands > 0 && command_ids.is_empty() {
            errors.push(
                "host-backed plugin declares command entrypoints but no markdown command files were found"
                    .to_string(),
            );
        }
        for command_id in command_ids {
            let Some(binding) = host_command_binding(&command_id) else {
                errors.push(format!(
                    "host command '{command_id}' is not registered in the desktop host command registry"
                ));
                continue;
            };
            if binding.components.is_empty()
                && binding.tauri_commands.is_empty()
                && binding.tool_surfaces.is_empty()
            {
                errors.push(format!(
                    "host command '{command_id}' does not declare a host binding target"
                ));
            }
            for component in &binding.components {
                if !host_app_component_is_renderable(component) {
                    errors.push(format!(
                        "host command '{command_id}' references unregistered host app component '{component}'"
                    ));
                }
            }
            if !binding.components.is_empty()
                && !binding
                    .components
                    .iter()
                    .any(|component| app_component_set.contains(&host_component_name(component)))
            {
                errors.push(format!(
                    "host command '{command_id}' is bound to {} but the manifest does not declare a matching app component",
                    binding.components.join(", ")
                ));
            }
        }
        errors
    }

    fn plugin_lifecycle_state(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        resolved: Option<&ResolvedPlugin>,
        counts: PluginComponentCounts,
        health_status: PluginHealthStatus,
        entrypoints: &[String],
    ) -> PluginLifecycleState {
        if !plugin.available || resolved.is_none() {
            return PluginLifecycleState::Failed;
        }
        if has_fatal_plugin_errors(plugin) {
            return PluginLifecycleState::Failed;
        }
        let Some(manifest) = manifest else {
            return PluginLifecycleState::Discovered;
        };
        if entrypoints.is_empty() {
            return PluginLifecycleState::Parsed;
        }
        match health_status {
            PluginHealthStatus::NeedsConfiguration
            | PluginHealthStatus::NeedsAuthorization
            | PluginHealthStatus::ConnectionUnavailable => PluginLifecycleState::RuntimeReady,
            PluginHealthStatus::RuntimeUnavailable | PluginHealthStatus::Incomplete => {
                PluginLifecycleState::Incomplete
            }
            PluginHealthStatus::Failed => PluginLifecycleState::Failed,
            PluginHealthStatus::Ready | PluginHealthStatus::Unknown => {
                match self.plugin_execution_kind(
                    plugin,
                    Some(manifest),
                    counts,
                    plugin.root.join("runtime.zip").is_file(),
                ) {
                    PluginExecutionKind::HostBacked | PluginExecutionKind::SkillOnly => {
                        PluginLifecycleState::Verified
                    }
                    PluginExecutionKind::Subprocess
                    | PluginExecutionKind::ManagedRuntime
                    | PluginExecutionKind::DshSidecar => PluginLifecycleState::Executable,
                }
            }
        }
    }

    fn bundled_license_bucket(&self, name: &str) -> Option<BundledPluginBucket> {
        bundled_plugin_catalog().bucket_for(name)
    }

    fn dto_from_loaded(
        &self,
        plugin: &LoadedPlugin,
        loaded: &[LoadedPlugin],
        state: &PluginState,
        dependencies: &PluginDependencyOutcome,
    ) -> PluginDto {
        let available = plugin.available && plugin.resolved().is_some();
        let enabled = self.is_effectively_enabled(plugin, state)
            && !dependencies.demoted.contains(&plugin.id);
        let resolved = plugin.resolved();
        let manifest = resolved.map(|plugin| &plugin.manifest);
        let presentation =
            resolved.map(|resolved| self.presentation_for_loaded(plugin, resolved, state));
        let data_dir = self.data_root.join(sanitize_file_name(&plugin.id));
        let counts = PluginComponentCounts::from_manifest(manifest);
        let entrypoints = self.plugin_entrypoints(plugin, manifest);
        let has_runtime_payload = plugin.root.join("runtime.zip").is_file();
        let runtime_requirements =
            self.plugin_runtime_requirements(plugin, manifest, has_runtime_payload);
        let runtime_available = self.runtime_requirements_available(&runtime_requirements);
        let license_status = self.plugin_license_status(plugin);
        let health_status = self.plugin_health_status(
            plugin,
            manifest,
            resolved,
            counts,
            runtime_available,
            &runtime_requirements,
        );
        let health_error = self.plugin_health_error(
            plugin,
            manifest,
            counts,
            &runtime_requirements,
            health_status,
        );
        let last_health_check = state
            .health_checks
            .get(&plugin.id)
            .map(|check| check.checked_at.clone());
        let content_hash = state
            .installed
            .get(&plugin.id)
            .and_then(|installed| installed.content_hash.clone());
        let lifecycle_state = self.plugin_lifecycle_state(
            plugin,
            manifest,
            resolved,
            counts,
            health_status,
            &entrypoints,
        );
        let mut capabilities = manifest
            .map(|m| m.interface.capabilities.clone())
            .unwrap_or_default();
        if capabilities.is_empty() {
            if counts.skills > 0 {
                capabilities.push("Skill".to_string());
            }
            if counts.mcp_servers > 0 {
                capabilities.push("MCP".to_string());
            }
            if counts.hooks > 0 {
                capabilities.push("Hooks".to_string());
            }
            if counts.apps > 0 {
                capabilities.push("App".to_string());
            }
            if counts.output_styles > 0 {
                capabilities.push("Output Style".to_string());
            }
        }

        let mut errors = plugin.errors.clone();
        errors.extend_from_slice(dependencies.errors_for(&plugin.id));
        let required_by = self.reverse_dependents(plugin, loaded, state, dependencies);

        PluginDto {
            id: plugin.id.clone(),
            name: plugin.name.clone(),
            display_name: presentation
                .as_ref()
                .map(|presentation| presentation.display_name.clone())
                .unwrap_or_else(|| plugin.name.clone()),
            description: presentation
                .as_ref()
                .and_then(|presentation| presentation.short_description.clone())
                .or_else(|| resolved.map(|plugin| plugin.short_description()))
                .unwrap_or_else(|| "Plugin failed to load".to_string()),
            long_description: presentation
                .as_ref()
                .and_then(|presentation| presentation.long_description.clone()),
            version: manifest.and_then(|m| m.version.clone()),
            local_version: manifest.and_then(|m| m.version.clone()),
            content_hash,
            developer: presentation
                .as_ref()
                .and_then(|presentation| presentation.developer_name.clone()),
            source: PluginSourceDto {
                kind: plugin.origin.as_str().to_string(),
                name: plugin.source_key.clone(),
                marketplace: plugin.marketplace.clone(),
                path: Some(plugin.root.display().to_string()),
            },
            origin: plugin.origin.as_str().to_string(),
            dialect: resolved
                .map(|plugin| plugin.dialect.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            path: Some(plugin.root.display().to_string()),
            data_dir: data_dir.display().to_string(),
            manifest_path: manifest.map(|m| m.manifest_path.display().to_string()),
            installed: manifest.is_some(),
            enabled,
            available,
            update_available: self.plugin_update_available(plugin, state),
            overridden_by: plugin.overridden_by.clone(),
            category: presentation
                .as_ref()
                .and_then(|presentation| presentation.category.clone()),
            keywords: manifest.map(|m| m.keywords.clone()).unwrap_or_default(),
            capabilities,
            permissions: manifest
                .map(|m| m.interface.permissions.clone())
                .unwrap_or_default(),
            skill_count: counts.skills,
            mcp_server_count: counts.mcp_servers,
            hook_count: counts.hooks,
            command_count: counts.commands,
            agent_count: counts.agents,
            app_count: counts.apps,
            output_style_count: counts.output_styles,
            state: lifecycle_state,
            execution_kind: self.plugin_execution_kind(
                plugin,
                manifest,
                counts,
                has_runtime_payload,
            ),
            runtime_required: !runtime_requirements.is_empty(),
            runtime_available,
            entrypoints,
            has_runtime_payload,
            license_status,
            health_status,
            last_health_check,
            health_error,
            icon_path: manifest
                .and_then(|m| m.interface.composer_icon.as_ref())
                .map(|path| path_string(path)),
            logo_path: manifest
                .and_then(|m| m.interface.logo.as_ref())
                .map(|path| path_string(path)),
            brand_color: manifest.and_then(|m| m.interface.brand_color.clone()),
            required_by,
            errors,
        }
    }

    fn presentation_for_loaded(
        &self,
        plugin: &LoadedPlugin,
        resolved: &ResolvedPlugin,
        state: &PluginState,
    ) -> Presentation {
        let marketplace_entry = plugin
            .marketplace
            .as_deref()
            .and_then(|marketplace_name| {
                state
                    .marketplaces
                    .get(marketplace_name)
                    .map(|state| (marketplace_name, state))
            })
            .and_then(|(marketplace_name, marketplace_state)| {
                self.load_registered_marketplace(marketplace_name, marketplace_state)
                    .ok()
                    .flatten()
            })
            .and_then(|catalog| {
                catalog
                    .entries
                    .into_iter()
                    .find(|entry| entry.name == plugin.name)
            });
        let marketplace_display = marketplace_entry
            .as_ref()
            .and_then(|entry| entry.display_name.as_deref());
        let marketplace_description = marketplace_entry
            .as_ref()
            .and_then(|entry| entry.description.as_deref());
        let marketplace_author = marketplace_entry
            .as_ref()
            .and_then(|entry| entry.author_name.as_deref());
        let marketplace_category = marketplace_entry
            .as_ref()
            .and_then(|entry| entry.category.as_deref());
        let manifest = &resolved.manifest;

        resolve_presentation(PresentationSources {
            interface: InterfaceSource {
                display_name: manifest.interface.display_name.as_deref(),
                short_description: manifest.interface.short_description.as_deref(),
                long_description: manifest.interface.long_description.as_deref(),
                developer_name: manifest.interface.developer_name.as_deref(),
                category: manifest.interface.category.as_deref(),
            },
            marketplace: MarketplaceSource {
                display_name: marketplace_display,
                description: marketplace_description,
                author_name: marketplace_author,
                category: marketplace_category,
            },
            portable: PortableSource {
                name: Some(resolved.name()),
                description: resolved.portable.description.as_deref(),
                author_name: resolved
                    .portable
                    .author
                    .as_ref()
                    .map(|author| author.name.as_str()),
            },
            directory_name: plugin.root.file_name().and_then(|name| name.to_str()),
        })
    }

    fn reverse_dependents(
        &self,
        plugin: &LoadedPlugin,
        loaded: &[LoadedPlugin],
        state: &PluginState,
        dependencies: &PluginDependencyOutcome,
    ) -> Vec<PluginDependentDto> {
        let dependent_ids = find_reverse_dependents(&plugin.id, loaded, |candidate| {
            self.is_effectively_enabled(candidate, state)
                && !dependencies.demoted.contains(&candidate.id)
        });
        dependent_ids
            .into_iter()
            .filter_map(|id| {
                let dependent = loaded.iter().find(|candidate| candidate.id == id)?;
                Some(PluginDependentDto {
                    id: dependent.id.clone(),
                    name: dependent.name.clone(),
                    display_name: dependent
                        .resolved()
                        .map(|plugin| plugin.display_name().to_string())
                        .unwrap_or_else(|| dependent.name.clone()),
                })
            })
            .collect()
    }

    fn load_state(&self) -> Result<PluginState> {
        if !self.state_path.is_file() {
            return Ok(PluginState::default());
        }
        let text = std::fs::read_to_string(&self.state_path).map_err(|e| {
            CoreError::Persistence(format!(
                "read plugin state {}: {e}",
                self.state_path.display()
            ))
        })?;
        let state: PluginState = serde_json::from_str(&text)
            .map_err(|e| CoreError::invalid(format!("parse plugin state: {e}")))?;
        migrate_plugin_state(state)
    }

    fn save_state(&self, state: &PluginState) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Persistence(format!("create plugin state dir {}: {e}", parent.display()))
            })?;
        }
        let mut state = state.clone();
        state.version = PLUGIN_STATE_SCHEMA_VERSION;
        let result = write_file_atomically(
            &self.state_path,
            serde_json::to_string_pretty(&state)
                .map_err(CoreError::from)?
                .as_bytes(),
        );
        if result.is_ok() {
            self.invalidate_plugin_caches();
        }
        result
    }

    fn cached_plugin_list(&self) -> Option<Vec<PluginDto>> {
        let cache = self.list_cache.lock().ok()?;
        let cache = cache.as_ref()?;
        if cache.snapshots.iter().all(PathSnapshot::still_matches) {
            Some(cache.plugins.clone())
        } else {
            None
        }
    }

    fn store_plugin_list_cache(&self, plugins: Vec<PluginDto>, snapshots: Vec<PathSnapshot>) {
        if let Ok(mut cache) = self.list_cache.lock() {
            *cache = Some(PluginListCache { plugins, snapshots });
        }
    }

    fn cached_runtime_projection(&self) -> Option<PluginRuntimeProjection> {
        let cache = self.runtime_cache.lock().ok()?;
        let cache = cache.as_ref()?;
        if cache.snapshots.iter().all(PathSnapshot::still_matches) {
            Some(cache.projection.clone())
        } else {
            None
        }
    }

    fn store_runtime_projection_cache(
        &self,
        projection: PluginRuntimeProjection,
        snapshots: Vec<PathSnapshot>,
    ) {
        if let Ok(mut cache) = self.runtime_cache.lock() {
            *cache = Some(PluginRuntimeCache {
                projection,
                snapshots,
            });
        }
    }

    fn invalidate_runtime_cache(&self) {
        if let Ok(mut cache) = self.runtime_cache.lock() {
            *cache = None;
        }
    }

    fn invalidate_plugin_caches(&self) {
        self.invalidate_runtime_cache();
        if let Ok(mut cache) = self.list_cache.lock() {
            *cache = None;
        }
    }

    fn runtime_cache_snapshots(&self, loaded: &[LoadedPlugin]) -> Vec<PathSnapshot> {
        runtime_watch_paths(&self.roots, &self.state_path, loaded)
            .into_iter()
            .map(PathSnapshot::capture)
            .collect()
    }

    fn plugin_list_cache_snapshots(
        &self,
        loaded: &[LoadedPlugin],
        state: &PluginState,
    ) -> Vec<PathSnapshot> {
        plugin_list_watch_paths(&self.roots, &self.state_path, loaded, state)
            .into_iter()
            .map(PathSnapshot::capture)
            .collect()
    }

    fn personal_target_dir(&self, requested: Option<&str>, fallback_slug: &str) -> Result<PathBuf> {
        let relative = requested
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| fallback_slug.to_string())
            .replace('\\', "/");
        let trimmed = relative
            .trim_start_matches("./")
            .strip_prefix(".deepagent/plugins/")
            .or_else(|| relative.trim_start_matches("./").strip_prefix("plugins/"))
            .unwrap_or_else(|| relative.trim_start_matches("./"));
        let path = Path::new(trimmed);
        if path.is_absolute() || trimmed.contains("..") {
            return Err(CoreError::invalid(
                "plugin directory must stay under personal plugins",
            ));
        }
        Ok(self.roots.personal.join(path))
    }

    fn materialize_marketplace_plugin_source(
        &self,
        marketplace: &str,
        entry: &PluginMarketplaceEntry,
    ) -> Result<MaterializedPluginSource> {
        match &entry.source {
            PluginMarketplaceSource::Local { path, .. } => Ok(MaterializedPluginSource::borrowed(
                resolve_plugin_root(path)?,
            )),
            PluginMarketplaceSource::Git {
                url,
                path,
                ref_name,
                sha,
                ..
            } => self.materialize_marketplace_plugin_in_staging(
                marketplace,
                &entry.name,
                |staging| {
                    materialize_git_plugin(
                        url,
                        path.as_deref(),
                        ref_name.as_deref(),
                        sha.as_deref(),
                        staging,
                    )
                },
            ),
            PluginMarketplaceSource::GitHub {
                repo,
                path,
                ref_name,
                sha,
                ..
            } => self.materialize_marketplace_plugin_in_staging(
                marketplace,
                &entry.name,
                |staging| {
                    materialize_github_plugin(
                        repo,
                        path.as_deref(),
                        ref_name.as_deref(),
                        sha.as_deref(),
                        staging,
                    )
                },
            ),
            PluginMarketplaceSource::GitSubdir {
                url,
                path,
                ref_name,
                sha,
                ..
            } => self.materialize_marketplace_plugin_in_staging(
                marketplace,
                &entry.name,
                |staging| {
                    materialize_git_plugin(
                        url,
                        Some(path),
                        ref_name.as_deref(),
                        sha.as_deref(),
                        staging,
                    )
                },
            ),
            PluginMarketplaceSource::ZipUrl { url, sha256, .. } => self
                .materialize_marketplace_plugin_in_staging(marketplace, &entry.name, |staging| {
                    let archive = staging.join("plugin.zip");
                    download_file(url, &archive)?;
                    if let Some(expected) = sha256.as_deref() {
                        verify_sha256_file(&archive, expected)?;
                    }
                    let unpacked = staging.join("unpacked");
                    extract_zip_into(&archive, &unpacked)?;
                    resolve_plugin_root(&unpacked)
                }),
            PluginMarketplaceSource::Npm {
                package,
                version,
                registry,
                ..
            } => self.materialize_marketplace_plugin_in_staging(
                marketplace,
                &entry.name,
                |staging| {
                    materialize_npm_plugin(
                        package,
                        version.as_deref(),
                        registry.as_deref(),
                        staging,
                    )
                },
            ),
            PluginMarketplaceSource::Unsupported { kind, .. } => Err(CoreError::invalid(format!(
                "marketplace source '{kind}' is not supported"
            ))),
        }
    }

    fn materialize_marketplace_plugin_in_staging(
        &self,
        marketplace: &str,
        plugin: &str,
        materialize: impl FnOnce(&Path) -> Result<PathBuf>,
    ) -> Result<MaterializedPluginSource> {
        let staging = self.marketplace_staging_dir(marketplace, plugin)?;
        match materialize(&staging) {
            Ok(plugin_root) => Ok(MaterializedPluginSource::staged(plugin_root, staging)),
            Err(error) => {
                let _ = remove_dir_all_with_retry(&staging);
                Err(error)
            }
        }
    }

    fn marketplace_staging_dir(&self, marketplace: &str, plugin: &str) -> Result<PathBuf> {
        let base = self.roots.marketplace_cache.join(".staging");
        std::fs::create_dir_all(&base).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin marketplace staging root {}: {e}",
                base.display()
            ))
        })?;
        for attempt in 0..100u32 {
            let dir = base.join(format!(
                "{}-{}-{}-{attempt}",
                sanitize_file_name(marketplace),
                sanitize_file_name(plugin),
                now_string()
            ));
            if dir.exists() {
                continue;
            }
            std::fs::create_dir_all(&dir).map_err(|e| {
                CoreError::Persistence(format!(
                    "create plugin marketplace staging dir {}: {e}",
                    dir.display()
                ))
            })?;
            return Ok(dir);
        }
        Err(CoreError::other(
            "failed to allocate plugin marketplace staging dir",
        ))
    }

    fn prepared_install_dir(&self, token: &str) -> Result<PathBuf> {
        let token = token.trim();
        if token.is_empty()
            || token == "."
            || token == ".."
            || token.contains(['/', '\\'])
            || sanitize_file_name(token) != token
        {
            return Err(CoreError::invalid(format!(
                "invalid prepared plugin install token: {token}"
            )));
        }
        let base = self.roots.marketplace_cache.join(".staging");
        std::fs::create_dir_all(&base).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin marketplace staging root {}: {e}",
                base.display()
            ))
        })?;
        let staging = base.join(token);
        if !path_is_under(&staging, &base) {
            return Err(CoreError::invalid(format!(
                "prepared plugin install token escapes staging root: {token}"
            )));
        }
        Ok(staging)
    }

    fn marketplace_plugin_destination(
        &self,
        marketplace: &str,
        plugin: &str,
        version: &str,
    ) -> PathBuf {
        self.roots
            .marketplace_cache
            .join(sanitize_file_name(marketplace))
            .join(sanitize_file_name(plugin))
            .join(sanitize_file_name(version))
    }

    fn materialize_marketplace(
        &self,
        name: &str,
        source: &str,
        explicit_ref: Option<&str>,
        sparse_path: Option<&str>,
    ) -> Result<MaterializedMarketplace> {
        let registry_dir = self.roots.marketplaces.join(slugify(name));
        std::fs::create_dir_all(&registry_dir).map_err(|e| {
            CoreError::Persistence(format!(
                "create marketplace registry {}: {e}",
                registry_dir.display()
            ))
        })?;

        let source_path = Path::new(source);
        if source_path.exists() {
            let manifest_path = find_marketplace_manifest_path(source_path).ok_or_else(|| {
                CoreError::invalid(format!(
                    "marketplace manifest not found under {}",
                    source_path.display()
                ))
            })?;
            return snapshot_marketplace_manifest(name, &registry_dir, &manifest_path);
        }

        let source = source.trim();
        if source.is_empty() {
            return Ok(MaterializedMarketplace::default());
        }

        if let Some(topic) = github_topic_from_source(source) {
            return materialize_github_topic_marketplace(name, &topic, &registry_dir);
        }

        if source.starts_with("npm:") {
            let package = source.trim_start_matches("npm:").trim();
            if package.is_empty() {
                return Err(CoreError::invalid("npm marketplace source missing package"));
            }
            let checkout = registry_dir.join("source");
            remove_managed_child_dir(&registry_dir, &checkout)?;
            std::fs::create_dir_all(&checkout).map_err(|e| {
                CoreError::Persistence(format!(
                    "create npm marketplace source dir {}: {e}",
                    checkout.display()
                ))
            })?;
            let package_root = materialize_npm_plugin(package, None, None, &checkout)?;
            let source_root = resolve_marketplace_root(&package_root)?;
            let manifest_path = find_marketplace_manifest_path(&source_root).ok_or_else(|| {
                CoreError::invalid(format!(
                    "marketplace manifest not found in npm package {}",
                    package
                ))
            })?;
            return snapshot_marketplace_manifest(name, &registry_dir, &manifest_path);
        }

        if is_http_url(source) && source.ends_with(".json") {
            let snapshot_path = registry_dir.join("marketplace.json");
            download_file(source, &snapshot_path)?;
            let catalog = load_marketplace_catalog(&snapshot_path)?;
            tracing::debug!(
                marketplace = name,
                source = source,
                entries = catalog.entries.len(),
                "downloaded plugin marketplace manifest"
            );
            return Ok(MaterializedMarketplace {
                manifest_path: Some(snapshot_path),
                source_root: None,
            });
        }

        if is_http_url(source) && source.ends_with(".zip") {
            let archive = registry_dir.join("marketplace.zip");
            let unpacked = registry_dir.join("source");
            remove_managed_child_dir(&registry_dir, &unpacked)?;
            download_file(source, &archive)?;
            extract_zip_into(&archive, &unpacked)?;
            let source_root = match sparse_path {
                Some(path) => safe_join_materialized_subdir(&unpacked, path)?,
                None => resolve_marketplace_root(&unpacked)?,
            };
            let manifest_path = find_marketplace_manifest_path(&source_root).ok_or_else(|| {
                CoreError::invalid(format!(
                    "marketplace manifest not found in zip source {source}"
                ))
            })?;
            return snapshot_marketplace_manifest(name, &registry_dir, &manifest_path);
        }

        let (git_source, parsed_ref) = split_source_ref(source);
        let ref_name = explicit_ref.or(parsed_ref.as_deref());
        let git_url = if is_git_url(&git_source) || is_ssh_git_url(&git_source) {
            normalize_git_url(&git_source)
        } else if looks_like_github_shorthand(&git_source) {
            format!("https://github.com/{git_source}.git")
        } else {
            return Ok(MaterializedMarketplace::default());
        };

        let checkout = registry_dir.join("source");
        remove_managed_child_dir(&registry_dir, &checkout)?;
        let source_root = materialize_git_marketplace(&git_url, ref_name, sparse_path, &checkout)?;
        let manifest_path = find_marketplace_manifest_path(&source_root).ok_or_else(|| {
            CoreError::invalid(format!(
                "marketplace manifest not found in materialized source {}",
                git_url
            ))
        })?;
        snapshot_marketplace_manifest(name, &registry_dir, &manifest_path)
    }

    fn load_registered_marketplace(
        &self,
        name: &str,
        state: &MarketplaceState,
    ) -> Result<Option<crate::plugin_marketplace::PluginMarketplaceCatalog>> {
        let snapshot = state
            .manifest_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.roots.marketplaces.join(name).join("marketplace.json"));
        if !snapshot.is_file() {
            return Ok(None);
        }
        let mut catalog = load_marketplace_catalog(&snapshot)?;
        if let Some(source_root) = state.source_root.as_ref().map(PathBuf::from) {
            catalog.root = source_root.clone();
            for entry in &mut catalog.entries {
                if let crate::plugin_marketplace::PluginMarketplaceSource::Local { path, display } =
                    &mut entry.source
                {
                    *path = source_root.join(Path::new(display));
                }
            }
        }
        Ok(Some(catalog))
    }

    fn marketplace_entry(
        &self,
        marketplace: &str,
        plugin: &str,
    ) -> Result<(String, PluginMarketplaceEntry)> {
        let marketplace_key = slugify(marketplace);
        let state = self.load_state()?;
        let marketplace_state = state
            .marketplaces
            .get(&marketplace_key)
            .ok_or_else(|| CoreError::not_found(format!("marketplace {marketplace}")))?;
        let catalog = self
            .load_registered_marketplace(&marketplace_key, marketplace_state)?
            .ok_or_else(|| {
                CoreError::not_found(format!("marketplace manifest for {marketplace}"))
            })?;
        let plugin_key = slugify(plugin);
        let entry = catalog
            .entries
            .into_iter()
            .find(|entry| entry.name == plugin || slugify(&entry.name) == plugin_key)
            .ok_or_else(|| {
                CoreError::not_found(format!("plugin {plugin} in marketplace {marketplace}"))
            })?;
        Ok((marketplace_key, entry))
    }
}

pub(crate) fn prepare_runtime_payload(payload_root: &Path, data_dir: &Path) -> Result<()> {
    let archive_path = payload_root.join("runtime.zip");
    if !archive_path.is_file() {
        return Ok(());
    }
    let runtime_dir = data_dir.join("runtime");
    let marker = runtime_dir.join(".payload-size");
    let archive_size = std::fs::metadata(&archive_path)
        .map_err(|e| CoreError::Persistence(format!("read {}: {e}", archive_path.display())))?
        .len();
    let expected_marker = archive_size.to_string();
    if marker
        .is_file()
        .then(|| std::fs::read_to_string(&marker).ok())
        .flatten()
        .as_deref()
        == Some(expected_marker.as_str())
    {
        return Ok(());
    }

    let file = File::open(&archive_path)
        .map_err(|e| CoreError::Persistence(format!("open {}: {e}", archive_path.display())))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Persistence(format!("read runtime payload: {e}")))?;
    if archive.len() > 100_000 {
        return Err(CoreError::invalid(
            "runtime payload contains too many files",
        ));
    }
    let stage = data_dir.join(".runtime.tmp");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)
        .map_err(|e| CoreError::Persistence(format!("create runtime payload stage: {e}")))?;
    let mut extracted = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| CoreError::Persistence(format!("read runtime payload entry: {e}")))?;
        extracted = extracted.saturating_add(entry.size());
        if extracted > 512 * 1024 * 1024 {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(CoreError::invalid("runtime payload exceeds 512 MiB"));
        }
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| CoreError::invalid("runtime payload contains an unsafe path"))?;
        let output = stage.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|e| CoreError::Persistence(format!("create payload directory: {e}")))?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Persistence(format!("create payload parent: {e}")))?;
            }
            let mut output_file = File::create(&output)
                .map_err(|e| CoreError::Persistence(format!("create payload file: {e}")))?;
            std::io::copy(&mut entry, &mut output_file)
                .map_err(|e| CoreError::Persistence(format!("extract payload file: {e}")))?;
        }
    }
    let _ = std::fs::remove_dir_all(&runtime_dir);
    std::fs::rename(&stage, &runtime_dir)
        .map_err(|e| CoreError::Persistence(format!("activate runtime payload: {e}")))?;
    std::fs::write(marker, archive_size.to_string())
        .map_err(|e| CoreError::Persistence(format!("write runtime payload marker: {e}")))?;
    std::fs::create_dir_all(data_dir.join("workspace"))
        .map_err(|e| CoreError::Persistence(format!("create payload workspace: {e}")))?;
    Ok(())
}

#[derive(Default)]
struct MaterializedMarketplace {
    manifest_path: Option<PathBuf>,
    source_root: Option<PathBuf>,
}

struct MaterializedPluginSource {
    plugin_root: PathBuf,
    cleanup_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedPluginInstallState {
    #[serde(default, rename = "schemaVersion")]
    schema_version: u32,
    token: String,
    marketplace: String,
    plugin: String,
    #[serde(rename = "pluginId")]
    plugin_id: String,
    #[serde(
        default,
        rename = "pluginVersion",
        skip_serializing_if = "Option::is_none"
    )]
    plugin_version: Option<String>,
    #[serde(rename = "pluginRoot")]
    plugin_root: String,
    #[serde(rename = "destinationPath")]
    destination_path: String,
    #[serde(rename = "sourceKind")]
    source_kind: String,
    source: String,
    #[serde(rename = "contentHash")]
    content_hash: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

struct MarketplaceDependency {
    id: String,
    name: String,
    marketplace: String,
}

impl MaterializedPluginSource {
    fn borrowed(plugin_root: PathBuf) -> Self {
        Self {
            plugin_root,
            cleanup_root: None,
        }
    }

    fn staged(plugin_root: PathBuf, cleanup_root: PathBuf) -> Self {
        Self {
            plugin_root,
            cleanup_root: Some(cleanup_root),
        }
    }

    fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }
}

impl Drop for MaterializedPluginSource {
    fn drop(&mut self) {
        if let Some(root) = self.cleanup_root.take() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn write_prepared_install_metadata(
    staging: &Path,
    metadata: &PreparedPluginInstallState,
) -> Result<()> {
    let path = staging.join(PREPARED_PLUGIN_INSTALL_FILE);
    let bytes = serde_json::to_vec_pretty(metadata).map_err(CoreError::from)?;
    write_file_atomically(&path, &bytes)
}

fn load_prepared_install_metadata(staging: &Path) -> Result<PreparedPluginInstallState> {
    let path = staging.join(PREPARED_PLUGIN_INSTALL_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Persistence(format!(
            "read prepared plugin install metadata {}: {e}",
            path.display()
        ))
    })?;
    let metadata: PreparedPluginInstallState = serde_json::from_str(&text)
        .map_err(|e| CoreError::invalid(format!("parse prepared plugin install metadata: {e}")))?;
    if metadata.schema_version != 1 {
        return Err(CoreError::invalid(format!(
            "unsupported prepared plugin install metadata version {}",
            metadata.schema_version
        )));
    }
    Ok(metadata)
}

fn marketplace_source_installable(source: &PluginMarketplaceSource) -> bool {
    !matches!(source, PluginMarketplaceSource::Unsupported { .. })
}

fn marketplace_entry_installable(entry: &PluginMarketplaceEntry) -> bool {
    marketplace_entry_install_block_reason(entry).is_none()
}

fn marketplace_entry_install_block_reason(entry: &PluginMarketplaceEntry) -> Option<String> {
    if !marketplace_source_installable(&entry.source) {
        return Some(format!("unsupported source '{}'", entry.source.kind()));
    }
    if let Some(error) = marketplace_source_kind_policy_error(entry.source.kind()) {
        return Some(error);
    }
    if marketplace_policy_not_available(entry.policy_installation.as_deref()) {
        return Some(format!(
            "marketplace policy marks this plugin as not available for install ({})",
            entry
                .policy_installation
                .as_deref()
                .unwrap_or("NOT_AVAILABLE")
        ));
    }
    None
}

fn marketplace_authentication_required(policy: Option<&str>) -> bool {
    policy
        .map(normalized_policy_value)
        .as_deref()
        .is_some_and(|policy| {
            matches!(
                policy,
                "always" | "on-install" | "required" | "require" | "requires-auth"
            )
        })
}

fn marketplace_authentication_hint(entry: &PluginMarketplaceEntry) -> Option<String> {
    if marketplace_authentication_required(entry.policy_authentication.as_deref()) {
        Some(format!(
            "authentication confirmation required before installing ({})",
            entry
                .policy_authentication
                .as_deref()
                .unwrap_or("ON_INSTALL")
        ))
    } else {
        None
    }
}

fn plugin_state_schema_version() -> u32 {
    PLUGIN_STATE_SCHEMA_VERSION
}

fn migrate_plugin_state(mut state: PluginState) -> Result<PluginState> {
    match state.version {
        0 => {
            state.version = PLUGIN_STATE_SCHEMA_VERSION;
            Ok(state)
        }
        PLUGIN_STATE_SCHEMA_VERSION => Ok(state),
        version if version < PLUGIN_STATE_SCHEMA_VERSION => {
            state.version = PLUGIN_STATE_SCHEMA_VERSION;
            Ok(state)
        }
        version => Err(CoreError::invalid(format!(
            "unsupported plugin state version {version}; this DeepAgent build supports version {PLUGIN_STATE_SCHEMA_VERSION}"
        ))),
    }
}

fn ensure_marketplace_entry_installable(
    marketplace: &str,
    entry: &PluginMarketplaceEntry,
) -> Result<()> {
    if let Some(reason) = marketplace_entry_install_block_reason(entry) {
        return Err(CoreError::invalid(format!(
            "plugin '{}' in marketplace '{}' cannot be installed: {reason}",
            entry.name, marketplace
        )));
    }
    Ok(())
}

fn ensure_marketplace_authentication_confirmed(
    marketplace: &str,
    entry: &PluginMarketplaceEntry,
    authentication_confirmed: bool,
) -> Result<()> {
    if marketplace_authentication_required(entry.policy_authentication.as_deref())
        && !authentication_confirmed
    {
        return Err(CoreError::invalid(format!(
            "plugin '{}' in marketplace '{}' requires authentication confirmation before install ({})",
            entry.name,
            marketplace,
            entry
                .policy_authentication
                .as_deref()
                .unwrap_or("ON_INSTALL")
        )));
    }
    Ok(())
}

fn marketplace_policy_not_available(policy: Option<&str>) -> bool {
    policy
        .map(normalized_policy_value)
        .as_deref()
        .is_some_and(|policy| policy == "not-available")
}

fn normalized_policy_value(policy: &str) -> String {
    policy.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn marketplace_dependency_id(
    raw: &str,
    current_marketplace: &str,
) -> Result<MarketplaceDependency> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::invalid("plugin dependency cannot be empty"));
    }
    let (name, marketplace) = match trimmed.rsplit_once('@') {
        Some((name, marketplace)) if !name.trim().is_empty() && !marketplace.trim().is_empty() => {
            (name.trim().to_string(), slugify(marketplace.trim()))
        }
        Some(_) => {
            return Err(CoreError::invalid(format!(
                "invalid plugin dependency '{trimmed}', expected name or name@marketplace"
            )))
        }
        None => (trimmed.to_string(), current_marketplace.to_string()),
    };
    Ok(MarketplaceDependency {
        id: plugin_id(&name, &marketplace),
        name,
        marketplace,
    })
}

fn materialize_github_topic_marketplace(
    name: &str,
    topic: &str,
    registry_dir: &Path,
) -> Result<MaterializedMarketplace> {
    let scratch = registry_dir.join(".github-api");
    let snapshot_path = registry_dir.join("marketplace.json");
    let result = (|| {
        let search_endpoint = format!(
            "/search/repositories?q={}&sort=updated&order=desc&per_page=100",
            percent_encode_query_value(&format!("topic:{topic}"))
        );
        let search_text = download_github_api_json(&search_endpoint, &scratch, "topic-search")?;
        let search: GitHubSearchRepositoriesResponse = serde_json::from_str(&search_text)
            .map_err(|e| CoreError::invalid(format!("parse GitHub topic response: {e}")))?;
        let mut used_names = BTreeSet::new();
        let mut plugins = Vec::new();
        for repo in search.items {
            let Some(full_name) = trimmed_string(repo.full_name) else {
                continue;
            };
            if full_name.split('/').count() != 2 {
                continue;
            }
            let default_branch = repo
                .default_branch
                .and_then(trimmed_string)
                .unwrap_or_else(|| "main".to_string());
            let branch_endpoint = format!(
                "/repos/{}/branches/{}",
                full_name,
                percent_encode_path_segment(&default_branch)
            );
            let branch_text = download_github_api_json(
                &branch_endpoint,
                &scratch,
                &format!("{}-{default_branch}", full_name.replace('/', "-")),
            )?;
            let branch: GitHubBranchResponse = serde_json::from_str(&branch_text)
                .map_err(|e| CoreError::invalid(format!("parse GitHub branch response: {e}")))?;
            let commit = trimmed_string(branch.commit.sha)
                .filter(|sha| is_git_sha(sha))
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "GitHub repository {full_name} default branch {default_branch} did not expose a valid commit sha"
                    ))
                })?;
            let manifest_metadata =
                github_topic_manifest_metadata(&full_name, &commit, &scratch).unwrap_or(None);
            let repo_name = repo
                .name
                .and_then(trimmed_string)
                .unwrap_or_else(|| full_name.rsplit('/').next().unwrap_or("plugin").to_string());
            let manifest = manifest_metadata
                .as_ref()
                .map(|metadata| &metadata.manifest);
            let mut plugin_name = manifest
                .map(|manifest| manifest.name.clone())
                .unwrap_or_else(|| slugify(&repo_name));
            let manifest_named = manifest.is_some();
            if !used_names.insert(plugin_name.clone()) {
                if manifest_named {
                    continue;
                }
                plugin_name = slugify(&full_name.replace('/', "-"));
                used_names.insert(plugin_name.clone());
            }
            let mut description = repo
                .description
                .and_then(trimmed_string)
                .unwrap_or_else(|| "DSH plugin".to_string());
            if let Some(manifest_description) =
                manifest.and_then(|manifest| trimmed_string(manifest.short_description()))
            {
                description = manifest_description;
            }
            let license = manifest
                .and_then(|manifest| manifest.license.clone())
                .and_then(trimmed_string)
                .or_else(|| {
                    repo.license
                        .and_then(|license| license.spdx_id)
                        .and_then(trimmed_string)
                });
            if let Some(spdx_id) = license
                .as_deref()
                .filter(|_| !description.contains("license:"))
            {
                description = format!("{description} (license: {spdx_id})");
            }
            let display_name = manifest
                .map(PluginManifest::display_name)
                .unwrap_or_else(|| repo_name.clone());
            let version = manifest
                .and_then(|manifest| manifest.version.clone())
                .and_then(trimmed_string)
                .unwrap_or_else(|| format!("git-{}", &commit[..12.min(commit.len())]));
            let category = manifest
                .and_then(|manifest| manifest.interface.category.clone())
                .and_then(trimmed_string)
                .unwrap_or_else(|| "DSH Plugin".to_string());
            let author_name = manifest.and_then(PluginManifest::developer_name);
            let mut source = serde_json::json!({
                "source": "github",
                "repo": full_name,
                "ref": default_branch,
                "sha": commit
            });
            if let Some(metadata) = manifest_metadata.as_ref().filter(|metadata| {
                metadata.manifest_relative_path != AGENT_PLUGIN_MANIFEST_RELATIVE_PATH
            }) {
                if let Some(object) = source.as_object_mut() {
                    object.insert(
                        "manifestPath".to_string(),
                        serde_json::Value::String(metadata.manifest_relative_path.clone()),
                    );
                }
            }
            let mut entry = serde_json::json!({
                "name": plugin_name,
                "displayName": display_name,
                "version": version,
                "description": description,
                "source": source,
                "category": category
            });
            if let Some(author_name) = author_name {
                if let Some(object) = entry.as_object_mut() {
                    object.insert("author".to_string(), serde_json::Value::String(author_name));
                }
            }
            plugins.push(entry);
        }
        let catalog = serde_json::json!({
            "name": name,
            "interface": {
                "displayName": format!("GitHub topic: {topic}")
            },
            "plugins": plugins,
        });
        let bytes = serde_json::to_vec_pretty(&catalog).map_err(CoreError::from)?;
        std::fs::write(&snapshot_path, bytes).map_err(|e| {
            CoreError::Persistence(format!(
                "write GitHub topic marketplace snapshot {}: {e}",
                snapshot_path.display()
            ))
        })?;
        load_marketplace_catalog(&snapshot_path)?;
        Ok(MaterializedMarketplace {
            manifest_path: Some(snapshot_path),
            source_root: None,
        })
    })();
    let _ = remove_dir_all_with_retry(&scratch);
    if result.is_err() {
        let _ = std::fs::remove_file(registry_dir.join("marketplace.json"));
    }
    result
}

fn github_topic_manifest_metadata(
    full_name: &str,
    commit: &str,
    scratch: &Path,
) -> Result<Option<GitHubTopicManifestMetadata>> {
    let mut manifest_paths = Vec::with_capacity(DISCOVERABLE_MANIFEST_PATHS.len() + 1);
    manifest_paths.push(AGENT_PLUGIN_MANIFEST_RELATIVE_PATH);
    manifest_paths.extend(DISCOVERABLE_MANIFEST_PATHS.iter().copied());
    for manifest_relative_path in manifest_paths {
        let endpoint = format!(
            "/repos/{}/contents/{}?ref={}",
            full_name,
            percent_encode_content_path(manifest_relative_path),
            percent_encode_query_value(commit)
        );
        let Ok(text) = download_github_api_json(
            &endpoint,
            scratch,
            &format!(
                "{}-{}",
                full_name.replace('/', "-"),
                manifest_relative_path.replace('/', "-")
            ),
        ) else {
            continue;
        };
        let Ok(contents) = serde_json::from_str::<GitHubContentResponse>(&text) else {
            continue;
        };
        if contents.kind.as_deref() != Some("file")
            || contents.encoding.as_deref() != Some("base64")
        {
            continue;
        }
        let Some(encoded) = contents.content else {
            continue;
        };
        let Ok(bytes) = base64_decode_standard(&encoded) else {
            continue;
        };
        let Ok(manifest_text) = String::from_utf8(bytes) else {
            continue;
        };
        let root = scratch
            .join("manifests")
            .join(sanitize_file_name(full_name));
        let _ = std::fs::remove_dir_all(&root);
        let manifest_path = root.join(manifest_relative_path);
        if let Some(parent) = manifest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Persistence(format!(
                    "create GitHub manifest fixture dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&manifest_path, manifest_text).map_err(|e| {
            CoreError::Persistence(format!(
                "write GitHub manifest probe {}: {e}",
                manifest_path.display()
            ))
        })?;
        if let Some(manifest) = load_plugin_manifest(&root)? {
            return Ok(Some(GitHubTopicManifestMetadata {
                manifest,
                manifest_relative_path: manifest_relative_path.to_string(),
            }));
        }
    }
    Ok(None)
}

struct GitHubTopicManifestMetadata {
    manifest: PluginManifest,
    manifest_relative_path: String,
}

fn materialize_git_plugin(
    url: &str,
    subdir: Option<&str>,
    ref_name: Option<&str>,
    sha: Option<&str>,
    staging: &Path,
) -> Result<PathBuf> {
    let repo_dir = staging.join("repo");
    let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
    if let Some(ref_name) = ref_name {
        args.push("--branch".to_string());
        args.push(ref_name.to_string());
    }
    args.push(url.to_string());
    args.push(repo_dir.display().to_string());
    run_external("git", &args, None)?;

    if let Some(sha) = sha {
        run_external(
            "git",
            &["checkout".to_string(), sha.to_string()],
            Some(&repo_dir),
        )?;
        verify_git_head_matches_sha(&repo_dir, sha)?;
    }

    let root = match subdir {
        Some(subdir) => safe_join_materialized_subdir(&repo_dir, subdir)?,
        None => repo_dir,
    };
    resolve_plugin_root(&root)
}

fn verify_git_head_matches_sha(repo_dir: &Path, expected_sha: &str) -> Result<()> {
    let actual = run_external_output(
        "git",
        &["rev-parse".to_string(), "HEAD".to_string()],
        Some(repo_dir),
    )?;
    let actual = actual.trim().to_ascii_lowercase();
    let expected = expected_sha.trim().to_ascii_lowercase();
    if !expected.is_empty() && actual.starts_with(&expected) {
        return Ok(());
    }
    Err(CoreError::invalid(format!(
        "git checkout verification failed: expected HEAD to match {expected_sha}, got {actual}"
    )))
}

fn materialize_github_plugin(
    repo: &str,
    subdir: Option<&str>,
    ref_name: Option<&str>,
    sha: Option<&str>,
    staging: &Path,
) -> Result<PathBuf> {
    let mut last_error = None;
    #[cfg(test)]
    let skip_clone = std::env::var_os("DEEPAGENT_TEST_GITHUB_SKIP_CLONE")
        .filter(|value| !value.is_empty())
        .is_some();
    #[cfg(not(test))]
    let skip_clone = false;

    if !skip_clone {
        for url in github_clone_url_candidates(repo) {
            let _ = std::fs::remove_dir_all(staging.join("repo"));
            match materialize_git_plugin(&url, subdir, ref_name, sha, staging) {
                Ok(root) => return Ok(root),
                Err(error) => last_error = Some(error),
            }
        }
    }

    match sha {
        Some(sha) => {
            materialize_github_archive_plugin(repo, subdir, sha, staging).map_err(|archive_error| {
                match last_error {
                    Some(clone_error) => CoreError::other(format!(
                    "GitHub clone failed ({clone_error}); archive fallback failed ({archive_error})"
                )),
                    None => archive_error,
                }
            })
        }
        None => Err(last_error.unwrap_or_else(|| {
            CoreError::invalid(
                "GitHub archive fallback requires a pinned commit sha after clone failure",
            )
        })),
    }
}

fn materialize_github_archive_plugin(
    repo: &str,
    subdir: Option<&str>,
    sha: &str,
    staging: &Path,
) -> Result<PathBuf> {
    if !is_git_sha(sha) {
        return Err(CoreError::invalid(format!(
            "GitHub archive fallback requires a valid commit sha, got {sha}"
        )));
    }
    let archive = staging.join("github-archive.zip");
    let url = github_archive_zip_url(repo, sha)?;
    download_file(&url, &archive)?;
    let unpacked = staging.join("github-archive");
    extract_zip_into(&archive, &unpacked)?;
    let checkout_root = github_archive_checkout_root(&unpacked)?;
    let root = match subdir {
        Some(subdir) => safe_join_materialized_subdir(&checkout_root, subdir)?,
        None => checkout_root,
    };
    resolve_plugin_root(&root)
}

fn github_archive_zip_url(repo: &str, sha: &str) -> Result<String> {
    if !is_git_sha(sha) {
        return Err(CoreError::invalid(format!(
            "GitHub archive URL requires a valid commit sha, got {sha}"
        )));
    }
    Ok(format!("https://api.github.com/repos/{repo}/zipball/{sha}"))
}

fn github_archive_checkout_root(unpacked: &Path) -> Result<PathBuf> {
    let mut directories = Vec::new();
    let mut has_files = false;
    for entry in std::fs::read_dir(unpacked).map_err(|e| {
        CoreError::Persistence(format!(
            "read GitHub archive output {}: {e}",
            unpacked.display()
        ))
    })? {
        let entry =
            entry.map_err(|e| CoreError::Persistence(format!("read GitHub archive entry: {e}")))?;
        let file_type = entry.file_type().map_err(|e| {
            CoreError::Persistence(format!(
                "stat GitHub archive entry {}: {e}",
                entry.path().display()
            ))
        })?;
        if file_type.is_dir() {
            directories.push(entry.path());
        } else if file_type.is_file() {
            has_files = true;
        }
    }
    if !has_files && directories.len() == 1 {
        return directories
            .pop()
            .ok_or_else(|| CoreError::invalid("GitHub archive root disappeared"));
    }
    Ok(unpacked.to_path_buf())
}

fn materialize_git_marketplace(
    url: &str,
    ref_name: Option<&str>,
    subdir: Option<&str>,
    checkout: &Path,
) -> Result<PathBuf> {
    let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
    if let Some(ref_name) = ref_name {
        args.push("--branch".to_string());
        args.push(ref_name.to_string());
    }
    args.push(url.to_string());
    args.push(checkout.display().to_string());
    run_external("git", &args, None)?;

    let root = match subdir {
        Some(subdir) => safe_join_materialized_subdir(checkout, subdir)?,
        None => checkout.to_path_buf(),
    };
    resolve_marketplace_root(&root)
}

fn materialize_npm_plugin(
    package: &str,
    version: Option<&str>,
    registry: Option<&str>,
    staging: &Path,
) -> Result<PathBuf> {
    let pack_dir = staging.join("npm-pack");
    std::fs::create_dir_all(&pack_dir).map_err(|e| {
        CoreError::Persistence(format!("create npm pack dir {}: {e}", pack_dir.display()))
    })?;
    let package_spec = match version {
        Some(version) => format!("{package}@{version}"),
        None => package.to_string(),
    };
    let mut args = vec![
        "pack".to_string(),
        package_spec,
        "--pack-destination".to_string(),
        pack_dir.display().to_string(),
    ];
    if let Some(registry) = registry {
        args.push("--registry".to_string());
        args.push(registry.to_string());
    }
    run_external("npm", &args, None)?;

    let archive = find_first_archive(&pack_dir, |path| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("tgz"))
            .unwrap_or(false)
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(".tar.gz"))
                .unwrap_or(false)
    })?
    .ok_or_else(|| CoreError::invalid("npm pack did not produce a .tgz archive"))?;
    let unpacked = staging.join("npm-unpacked");
    extract_targz_into(&archive, &unpacked)?;
    resolve_plugin_root(&unpacked)
}

fn snapshot_marketplace_manifest(
    name: &str,
    registry_dir: &Path,
    manifest_path: &Path,
) -> Result<MaterializedMarketplace> {
    let catalog = load_marketplace_catalog(manifest_path)?;
    let source_root = marketplace_root_from_manifest(manifest_path);
    let snapshot_path = registry_dir.join("marketplace.json");
    if !same_path(manifest_path, &snapshot_path) {
        std::fs::copy(manifest_path, &snapshot_path).map_err(|e| {
            CoreError::Persistence(format!(
                "copy marketplace manifest {} -> {}: {e}",
                manifest_path.display(),
                snapshot_path.display()
            ))
        })?;
    }
    tracing::debug!(
        marketplace = name,
        source = %manifest_path.display(),
        entries = catalog.entries.len(),
        "materialized plugin marketplace"
    );
    Ok(MaterializedMarketplace {
        manifest_path: Some(snapshot_path),
        source_root: Some(source_root),
    })
}

fn resolve_plugin_root(root: &Path) -> Result<PathBuf> {
    if find_plugin_manifest_path(root).is_some() {
        return Ok(root.to_path_buf());
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(root.to_path_buf()),
    };
    let mut manifest_dirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child = entry.path();
        if find_plugin_manifest_path(&child).is_some() {
            manifest_dirs.push(child);
        }
    }
    if manifest_dirs.len() == 1 {
        Ok(manifest_dirs.remove(0))
    } else {
        Ok(root.to_path_buf())
    }
}

fn resolve_marketplace_root(root: &Path) -> Result<PathBuf> {
    if find_marketplace_manifest_path(root).is_some() {
        return Ok(root.to_path_buf());
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(root.to_path_buf()),
    };
    let mut manifest_dirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child = entry.path();
        if find_marketplace_manifest_path(&child).is_some() {
            manifest_dirs.push(child);
        }
    }
    if manifest_dirs.len() == 1 {
        Ok(manifest_dirs.remove(0))
    } else {
        Ok(root.to_path_buf())
    }
}

fn safe_join_materialized_subdir(root: &Path, raw: &str) -> Result<PathBuf> {
    let cleaned = raw.trim().trim_start_matches("./").replace('\\', "/");
    let path = Path::new(&cleaned);
    if cleaned.is_empty() || path.is_absolute() {
        return Err(CoreError::invalid("plugin source subdir must be relative"));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::invalid(
                    "plugin source subdir cannot escape its materialized root",
                ));
            }
        }
    }
    Ok(root.join(path))
}

fn remove_managed_child_dir(base: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    safe_remove_dir(base, target)
}

fn mark_stale_marketplace_plugin_versions(
    base: &Path,
    plugin_dir: &Path,
    active_destination: &Path,
) -> Result<()> {
    let Some(active_name) = active_destination.file_name() else {
        return Err(CoreError::invalid(format!(
            "active plugin cache destination has no file name: {}",
            active_destination.display()
        )));
    };
    let entries = std::fs::read_dir(plugin_dir).map_err(|e| {
        CoreError::Persistence(format!(
            "read marketplace plugin cache {}: {e}",
            plugin_dir.display()
        ))
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        if entry.file_name() == active_name {
            remove_orphan_marker_if_exists(&path)?;
        } else if !is_cache_dir_orphaned(&path) {
            mark_cache_dir_orphaned(base, &path)?;
        }
    }
    Ok(())
}

fn mark_cache_dir_orphaned(base: &Path, target: &Path) -> Result<()> {
    let base = std::fs::canonicalize(base).map_err(|e| {
        CoreError::Persistence(format!("canonicalize cache base {}: {e}", base.display()))
    })?;
    let target = std::fs::canonicalize(target).map_err(|e| {
        CoreError::Persistence(format!(
            "canonicalize plugin cache target {}: {e}",
            target.display()
        ))
    })?;
    if target == base || !target.starts_with(&base) {
        return Err(CoreError::invalid(format!(
            "refusing to mark path outside plugin cache: {}",
            target.display()
        )));
    }
    let marker = target.join(PLUGIN_CACHE_ORPHAN_MARKER);
    std::fs::write(&marker, now_string()).map_err(|e| {
        CoreError::Persistence(format!("write orphan marker {}: {e}", marker.display()))
    })
}

fn is_cache_dir_orphaned(path: &Path) -> bool {
    path.join(PLUGIN_CACHE_ORPHAN_MARKER).is_file()
}

fn remove_orphan_marker_if_exists(path: &Path) -> Result<()> {
    let marker = path.join(PLUGIN_CACHE_ORPHAN_MARKER);
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CoreError::Persistence(format!(
            "remove stale orphan marker {}: {err}",
            marker.display()
        ))),
    }
}

fn cleanup_orphaned_cache_dir(
    base: &Path,
    dir: &Path,
    installed_paths: &[PathBuf],
    now: u128,
    grace_millis: u128,
) -> Result<u32> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(CoreError::Persistence(format!(
                "read plugin cache dir {}: {err}",
                dir.display()
            )));
        }
    };
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        if is_cache_dir_orphaned(&path) {
            if installed_paths
                .iter()
                .any(|installed| same_path(installed, &path))
            {
                remove_orphan_marker_if_exists(&path)?;
            } else if orphan_marker_age_millis(&path, now).is_some_and(|age| age >= grace_millis) {
                remove_managed_child_dir(base, &path)?;
                removed = removed.saturating_add(1);
                continue;
            }
        }

        removed = removed.saturating_add(cleanup_orphaned_cache_dir(
            base,
            &path,
            installed_paths,
            now,
            grace_millis,
        )?);
        remove_empty_managed_dir(base, &path)?;
    }
    Ok(removed)
}

fn orphan_marker_age_millis(path: &Path, now: u128) -> Option<u128> {
    let modified = std::fs::metadata(path.join(PLUGIN_CACHE_ORPHAN_MARKER))
        .ok()?
        .modified()
        .ok()?;
    let modified = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(now.saturating_sub(modified))
}

fn remove_empty_managed_dir(base: &Path, target: &Path) -> Result<()> {
    if target == base || !target.is_dir() {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(target).map_err(|e| {
        CoreError::Persistence(format!("read plugin cache dir {}: {e}", target.display()))
    })?;
    if entries.next().is_none() {
        remove_managed_child_dir(base, target)?;
    }
    Ok(())
}

fn split_source_ref(source: &str) -> (String, Option<String>) {
    if let Some((base, ref_name)) = source.rsplit_once('#') {
        return (base.to_string(), non_empty_ref(ref_name));
    }
    if !source.contains("://") && !is_ssh_git_url(source) {
        if let Some((base, ref_name)) = source.rsplit_once('@') {
            return (base.to_string(), non_empty_ref(ref_name));
        }
    }
    (source.to_string(), None)
}

fn non_empty_ref(ref_name: &str) -> Option<String> {
    let ref_name = ref_name.trim();
    (!ref_name.is_empty()).then(|| ref_name.to_string())
}

fn normalize_git_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.starts_with("https://github.com/") && !url.ends_with(".git") {
        format!("{url}.git")
    } else {
        url.to_string()
    }
}

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn is_git_url(source: &str) -> bool {
    is_http_url(source)
}

fn is_ssh_git_url(source: &str) -> bool {
    source.starts_with("ssh://") || source.starts_with("git@") && source.contains(':')
}

fn looks_like_github_shorthand(source: &str) -> bool {
    let mut segments = source.split('/');
    let owner = segments.next();
    let repo = segments.next();
    let extra = segments.next();
    owner.is_some_and(is_github_shorthand_segment)
        && repo.is_some_and(is_github_shorthand_segment)
        && extra.is_none()
}

fn is_github_shorthand_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[derive(Debug, Deserialize)]
struct GitHubSearchRepositoriesResponse {
    #[serde(default)]
    items: Vec<GitHubRepositoryItem>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepositoryItem {
    #[serde(default)]
    name: Option<String>,
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    license: Option<GitHubRepositoryLicense>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepositoryLicense {
    #[serde(default)]
    spdx_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubBranchResponse {
    commit: GitHubBranchCommit,
}

#[derive(Debug, Deserialize)]
struct GitHubBranchCommit {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitHubContentResponse {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

fn github_topic_from_source(source: &str) -> Option<String> {
    let source = source.trim().trim_end_matches('/');
    let raw = source
        .strip_prefix("github-topic:")
        .or_else(|| source.strip_prefix(GITHUB_TOPIC_PREFIX))?;
    let topic = raw.split(['/', '?', '#']).next().unwrap_or_default().trim();
    is_safe_github_topic(topic).then(|| topic.to_string())
}

fn is_safe_github_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= 50
        && topic
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && topic
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && topic
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
}

fn is_git_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn download_github_api_json(endpoint: &str, scratch: &Path, label: &str) -> Result<String> {
    #[cfg(test)]
    if let Some(fixture) = github_api_fixture_path(endpoint) {
        return std::fs::read_to_string(&fixture).map_err(|e| {
            CoreError::Persistence(format!(
                "read GitHub API fixture {}: {e}",
                fixture.display()
            ))
        });
    }

    std::fs::create_dir_all(scratch).map_err(|e| {
        CoreError::Persistence(format!(
            "create GitHub API scratch dir {}: {e}",
            scratch.display()
        ))
    })?;
    let mut last_error = None;
    for (index, base) in github_api_bases().into_iter().enumerate() {
        let url = github_api_url(&base, endpoint)?;
        let destination = scratch.join(format!("{}-{index}.json", sanitize_file_name(label)));
        match download_file(&url, &destination).and_then(|_| {
            std::fs::read_to_string(&destination).map_err(|e| {
                CoreError::Persistence(format!(
                    "read GitHub API response {}: {e}",
                    destination.display()
                ))
            })
        }) {
            Ok(text) => return Ok(text),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| CoreError::other("GitHub API request had no candidates")))
}

#[cfg(test)]
fn github_api_fixture_path(endpoint: &str) -> Option<PathBuf> {
    std::env::var_os("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|root| root.join(format!("{}.json", github_api_fixture_name(endpoint))))
}

#[cfg(test)]
fn github_api_fixture_name(endpoint: &str) -> String {
    sanitize_file_name(endpoint.trim_start_matches('/'))
}

fn github_api_bases() -> Vec<String> {
    let configured = std::env::var(GITHUB_API_BASES_ENV)
        .ok()
        .map(|value| {
            value
                .split([';', ','])
                .filter_map(|item| trimmed_string(item.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if configured.is_empty() {
        vec!["https://api.github.com".to_string()]
    } else {
        configured
    }
}

fn github_api_url(base: &str, endpoint: &str) -> Result<String> {
    let base = base.trim().trim_end_matches('/');
    if !base.starts_with("https://") {
        return Err(CoreError::invalid("GitHub API base must use https://"));
    }
    let endpoint = endpoint.trim_start_matches('/');
    Ok(format!("{base}/{endpoint}"))
}

fn github_clone_url_candidates(repo: &str) -> Vec<String> {
    let official = format!("https://github.com/{repo}.git");
    github_download_url_candidates(&official)
}

fn github_download_url_candidates(url: &str) -> Vec<String> {
    let mut candidates = vec![url.to_string()];
    if url.starts_with("https://github.com/")
        || url.starts_with("https://raw.githubusercontent.com/")
        || url.starts_with("https://api.github.com/repos/")
    {
        candidates.push(format!("https://gh.llkk.cc/{url}"));
        candidates.push(format!("https://gh-proxy.com/{url}"));
    }
    candidates
}

fn percent_encode_query_value(input: &str) -> String {
    percent_encode_bytes(input.as_bytes(), true)
}

fn percent_encode_path_segment(input: &str) -> String {
    percent_encode_bytes(input.as_bytes(), false)
}

fn percent_encode_content_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_bytes(bytes: &[u8], encode_colon: bool) -> String {
    let mut out = String::new();
    for byte in bytes {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.' | '~')
            || (!encode_colon && ch == ':')
        {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn base64_decode_standard(input: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut len = 0usize;
    for byte in input.bytes().filter(|b| !b.is_ascii_whitespace()) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(CoreError::invalid("invalid base64 data")),
        };
        chunk[len] = value;
        len += 1;
        if len == 4 {
            push_base64_chunk(&mut out, chunk)?;
            len = 0;
        }
    }
    if len != 0 {
        return Err(CoreError::invalid("invalid base64 padding"));
    }
    Ok(out)
}

fn push_base64_chunk(out: &mut Vec<u8>, chunk: [u8; 4]) -> Result<()> {
    if chunk[0] == 64 || chunk[1] == 64 {
        return Err(CoreError::invalid("invalid base64 padding"));
    }
    out.push((chunk[0] << 2) | (chunk[1] >> 4));
    if chunk[2] != 64 {
        out.push(((chunk[1] & 0b1111) << 4) | (chunk[2] >> 2));
    }
    if chunk[3] != 64 {
        out.push(((chunk[2] & 0b11) << 6) | chunk[3]);
    }
    Ok(())
}

fn run_external(program: &str, args: &[String], cwd: Option<&Path>) -> Result<()> {
    run_external_output(program, args, cwd).map(|_| ())
}

fn run_external_output(program: &str, args: &[String], cwd: Option<&Path>) -> Result<String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|e| {
        CoreError::other(format!(
            "failed to start `{}` for plugin marketplace source: {e}",
            external_command_display(program, args)
        ))
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CoreError::other(format!(
        "`{}` failed with status {}{}{}",
        external_command_display(program, args),
        output.status,
        if stdout.trim().is_empty() {
            String::new()
        } else {
            format!("; stdout: {}", truncate_chars(stdout.trim(), 500))
        },
        if stderr.trim().is_empty() {
            String::new()
        } else {
            format!("; stderr: {}", truncate_chars(stderr.trim(), 500))
        }
    )))
}

fn external_command_display(program: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(program.to_string());
    parts.extend(args.iter().map(|arg| {
        if arg.contains(' ') {
            format!("\"{arg}\"")
        } else {
            arg.clone()
        }
    }));
    parts.join(" ")
}

fn truncate_chars(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.to_string();
    }
    let mut out = input.chars().take(cap).collect::<String>();
    out.push_str("...");
    out
}

fn download_file(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CoreError::Persistence(format!("create download parent {}: {e}", parent.display()))
        })?;
    }
    #[cfg(test)]
    if let Some(source) =
        std::env::var_os("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE").filter(|value| !value.is_empty())
    {
        std::fs::copy(PathBuf::from(source), destination).map_err(|e| {
            CoreError::Persistence(format!(
                "copy test plugin download fixture to {}: {e}",
                destination.display()
            ))
        })?;
        return Ok(());
    }
    let mut last_error = None;
    for candidate in github_download_url_candidates(url) {
        match download_file_once(&candidate, destination) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| CoreError::other("download had no URL candidates")))
}

fn download_file_once(url: &str, destination: &Path) -> Result<()> {
    let curl_args = vec![
        "-L".to_string(),
        "-f".to_string(),
        "-o".to_string(),
        destination.display().to_string(),
        url.to_string(),
    ];
    match run_external("curl", &curl_args, None) {
        Ok(()) => Ok(()),
        Err(curl_error) if cfg!(windows) => {
            let ps_args = vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri $args[0] -OutFile $args[1]".to_string(),
                url.to_string(),
                destination.display().to_string(),
            ];
            run_external("powershell", &ps_args, None).map_err(|ps_error| {
                CoreError::other(format!(
                    "download failed with curl ({curl_error}) and powershell ({ps_error})"
                ))
            })
        }
        Err(error) => Err(error),
    }
}

fn verify_sha256_file(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        return Ok(());
    }
    Err(CoreError::invalid(format!(
        "downloaded plugin archive checksum mismatch: expected {}, got {}",
        expected.trim(),
        actual
    )))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .map_err(|e| CoreError::Persistence(format!("open {} for sha256: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| {
            CoreError::Persistence(format!("read {} for sha256: {e}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn plugin_directory_content_hash(root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_plugin_hash_entries(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.relative.cmp(&b.relative).then_with(|| a.kind.cmp(b.kind)));

    let mut hasher = Sha256::new();
    hasher.update(b"deepagent-plugin-dir-v1\0");
    for entry in entries {
        hasher.update(entry.kind.as_bytes());
        hasher.update(b"\0");
        hasher.update(entry.relative.as_bytes());
        hasher.update(b"\0");
        if entry.kind == "file" {
            let metadata = std::fs::metadata(&entry.absolute).map_err(|e| {
                CoreError::Persistence(format!(
                    "stat plugin hash file {}: {e}",
                    entry.absolute.display()
                ))
            })?;
            hasher.update(metadata.len().to_string().as_bytes());
            hasher.update(b"\0");
            let mut file = File::open(&entry.absolute).map_err(|e| {
                CoreError::Persistence(format!(
                    "open plugin hash file {}: {e}",
                    entry.absolute.display()
                ))
            })?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|e| {
                    CoreError::Persistence(format!(
                        "read plugin hash file {}: {e}",
                        entry.absolute.display()
                    ))
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        }
        hasher.update(b"\0");
    }
    Ok(format!("sha256:{}", hex_lower(&hasher.finalize())))
}

struct PluginHashEntry {
    kind: &'static str,
    relative: String,
    absolute: PathBuf,
}

fn collect_plugin_hash_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<PluginHashEntry>,
) -> Result<()> {
    for entry in std::fs::read_dir(current).map_err(|e| {
        CoreError::Persistence(format!("read plugin hash dir {}: {e}", current.display()))
    })? {
        let entry =
            entry.map_err(|e| CoreError::Persistence(format!("read plugin hash entry: {e}")))?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|e| {
            CoreError::Persistence(format!(
                "strip plugin hash prefix {} from {}: {e}",
                root.display(),
                path.display()
            ))
        })?;
        let relative = normalize_hash_relative_path(relative);
        let file_type = entry.file_type().map_err(|e| {
            CoreError::Persistence(format!("stat plugin hash entry {}: {e}", path.display()))
        })?;
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy() == ".git" {
                continue;
            }
            entries.push(PluginHashEntry {
                kind: "dir",
                relative,
                absolute: path.clone(),
            });
            collect_plugin_hash_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            entries.push(PluginHashEntry {
                kind: "file",
                relative,
                absolute: path,
            });
        }
    }
    Ok(())
}

fn normalize_hash_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn extract_zip_into(zip_path: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|e| {
        CoreError::Persistence(format!(
            "create zip destination {}: {e}",
            destination.display()
        ))
    })?;
    let file = File::open(zip_path)
        .map_err(|e| CoreError::Persistence(format!("open zip {}: {e}", zip_path.display())))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::invalid(format!("read zip archive: {e}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| CoreError::invalid(format!("read zip entry {index}: {e}")))?;
        let Some(relative) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let out = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| {
                CoreError::Persistence(format!("create zip dir {}: {e}", out.display()))
            })?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Persistence(format!(
                        "create zip file parent {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            let mut outfile = File::create(&out).map_err(|e| {
                CoreError::Persistence(format!("create zip output {}: {e}", out.display()))
            })?;
            std::io::copy(&mut entry, &mut outfile).map_err(|e| {
                CoreError::Persistence(format!("extract zip output {}: {e}", out.display()))
            })?;
        }
    }
    Ok(())
}

fn extract_targz_into(archive_path: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|e| {
        CoreError::Persistence(format!(
            "create tar destination {}: {e}",
            destination.display()
        ))
    })?;
    let file = File::open(archive_path).map_err(|e| {
        CoreError::Persistence(format!("open tar archive {}: {e}", archive_path.display()))
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| CoreError::invalid(format!("read tar archive: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| CoreError::invalid(format!("read tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| CoreError::invalid(format!("read tar entry path: {e}")))?
            .to_path_buf();
        if tar_path_escapes(&path) {
            continue;
        }
        let out = destination.join(path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Persistence(format!(
                    "create tar output parent {}: {e}",
                    parent.display()
                ))
            })?;
        }
        entry.unpack(&out).map_err(|e| {
            CoreError::Persistence(format!("extract tar output {}: {e}", out.display()))
        })?;
    }
    Ok(())
}

fn tar_path_escapes(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn find_first_archive(dir: &Path, predicate: impl Fn(&Path) -> bool) -> Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| CoreError::Persistence(format!("read archive dir {}: {e}", dir.display())))?
    {
        let entry =
            entry.map_err(|e| CoreError::Persistence(format!("read archive entry: {e}")))?;
        let path = entry.path();
        if entry.file_type().map(|ty| ty.is_file()).unwrap_or(false) && predicate(&path) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PluginRuntimeNeeds {
    node: bool,
    python: bool,
    java: bool,
    shell: bool,
}

impl PluginRuntimeNeeds {
    fn is_empty(&self) -> bool {
        !self.node && !self.python && !self.java && !self.shell
    }

    fn requires_runtime(&self) -> bool {
        !self.is_empty()
    }

    fn merge_script_requirements(&mut self, root: &Path) {
        scan_runtime_script_dirs(root, self);
    }

    fn merge_script_path(&mut self, path: &Path) {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext)
                if matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "js" | "cjs" | "mjs" | "ts" | "tsx" | "jsx"
                ) =>
            {
                self.node = true;
            }
            Some(ext) if ext.eq_ignore_ascii_case("py") => {
                self.python = true;
            }
            Some(ext)
                if matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "sh" | "bash" | "zsh" | "ps1" | "cmd" | "bat"
                ) =>
            {
                self.shell = true;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundledPluginBucket {
    FirstParty,
    BundledThirdParty,
    MarketplaceOnly,
}

#[derive(Debug, Deserialize)]
struct BundledPluginCatalog {
    #[serde(default, rename = "firstParty")]
    first_party: Vec<String>,
    #[serde(default, rename = "bundledThirdParty")]
    bundled_third_party: Vec<BundledPluginCatalogEntry>,
    #[serde(default, rename = "marketplaceOnly")]
    marketplace_only: Vec<BundledPluginCatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct BundledPluginCatalogEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
struct HostPluginRegistry {
    #[serde(default, rename = "builtinComponents")]
    builtin_components: Vec<String>,
    #[serde(default, rename = "tauriComponents")]
    tauri_components: Vec<String>,
    #[serde(default, rename = "commandBindings")]
    command_bindings: Vec<HostCommandBinding>,
}

#[derive(Debug, Clone, Deserialize)]
struct HostCommandBinding {
    command: String,
    #[serde(default)]
    components: Vec<String>,
    #[serde(default, rename = "tauriCommands")]
    tauri_commands: Vec<String>,
    #[serde(default, rename = "toolSurfaces")]
    tool_surfaces: Vec<String>,
}

impl BundledPluginCatalog {
    fn bucket_for(&self, name: &str) -> Option<BundledPluginBucket> {
        if self.first_party.iter().any(|item| item == name) {
            return Some(BundledPluginBucket::FirstParty);
        }
        if self
            .bundled_third_party
            .iter()
            .any(|item| item.name == name)
        {
            return Some(BundledPluginBucket::BundledThirdParty);
        }
        if self.marketplace_only.iter().any(|item| item.name == name) {
            return Some(BundledPluginBucket::MarketplaceOnly);
        }
        None
    }
}

fn bundled_plugin_catalog() -> &'static BundledPluginCatalog {
    static CATALOG: OnceLock<BundledPluginCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src-tauri/bundled-plugins.json"
        ));
        serde_json::from_str(json).unwrap_or_else(|_| BundledPluginCatalog {
            first_party: Vec::new(),
            bundled_third_party: Vec::new(),
            marketplace_only: Vec::new(),
        })
    })
}

fn host_plugin_registry() -> &'static HostPluginRegistry {
    static REGISTRY: OnceLock<HostPluginRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src-tauri/host-plugin-registry.json"
        ));
        serde_json::from_str(json).unwrap_or_else(|_| HostPluginRegistry {
            builtin_components: Vec::new(),
            tauri_components: Vec::new(),
            command_bindings: Vec::new(),
        })
    })
}

fn host_app_component_is_renderable(component: &str) -> bool {
    let component = component.trim().to_ascii_lowercase();
    if let Some(name) = component.strip_prefix("builtin:") {
        return host_plugin_registry()
            .builtin_components
            .iter()
            .any(|registered| registered == name);
    }
    if let Some(name) = component.strip_prefix("tauri:") {
        return host_plugin_registry()
            .tauri_components
            .iter()
            .any(|registered| registered == name);
    }
    true
}

fn host_component_name(component: &str) -> String {
    let component = component.trim().to_ascii_lowercase();
    component
        .strip_prefix("builtin:")
        .or_else(|| component.strip_prefix("tauri:"))
        .unwrap_or(&component)
        .to_string()
}

fn host_command_binding(command: &str) -> Option<&'static HostCommandBinding> {
    host_plugin_registry()
        .command_bindings
        .iter()
        .find(|binding| binding.command == command)
}

fn host_app_components(manifest: &PluginManifest) -> Vec<String> {
    let mut components = Vec::new();
    for value in manifest.paths.app_paths.iter().filter_map(read_json_file) {
        collect_host_app_components(&value, &mut components);
    }
    components.sort();
    components.dedup();
    components
}

fn host_command_ids(manifest: &PluginManifest) -> Vec<String> {
    let mut command_ids = Vec::new();
    for path in &manifest.paths.commands {
        if path.is_file() {
            push_command_id(path, &mut command_ids);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_markdown_file(&path) {
                push_command_id(&path, &mut command_ids);
            }
        }
    }
    command_ids.sort();
    command_ids.dedup();
    command_ids
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct HookCommandInspection {
    scripts: Vec<PathBuf>,
    errors: Vec<String>,
}

fn inspect_hook_command_scripts(
    plugin_root: &Path,
    data_dir: &Path,
    hooks_inline: Option<&serde_json::Value>,
    hook_paths: &[PathBuf],
) -> HookCommandInspection {
    let mut inspection = HookCommandInspection::default();
    for value in hook_paths.iter().filter_map(read_json_file) {
        collect_hook_command_scripts(plugin_root, data_dir, &value, &mut inspection);
    }
    if let Some(value) = hooks_inline {
        collect_hook_command_scripts(plugin_root, data_dir, value, &mut inspection);
    }
    inspection.scripts.sort();
    inspection.scripts.dedup();
    inspection.errors.sort();
    inspection.errors.dedup();
    inspection
}

fn collect_hook_command_scripts(
    plugin_root: &Path,
    data_dir: &Path,
    value: &serde_json::Value,
    inspection: &mut HookCommandInspection,
) {
    match value {
        serde_json::Value::Object(map) => {
            if map
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind == "command")
            {
                match map.get("command").and_then(serde_json::Value::as_str) {
                    Some(command) if !command.trim().is_empty() => {
                        inspect_hook_command(plugin_root, data_dir, command, inspection);
                    }
                    _ => inspection
                        .errors
                        .push("hook command entry is missing a non-empty command".to_string()),
                }
            }
            for value in map.values() {
                collect_hook_command_scripts(plugin_root, data_dir, value, inspection);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_hook_command_scripts(plugin_root, data_dir, value, inspection);
            }
        }
        _ => {}
    }
}

fn inspect_hook_command(
    plugin_root: &Path,
    data_dir: &Path,
    command: &str,
    inspection: &mut HookCommandInspection,
) {
    let plugin_root =
        std::fs::canonicalize(plugin_root).unwrap_or_else(|_| plugin_root.to_path_buf());
    let root = plugin_root.display().to_string();
    let data = data_dir.display().to_string();
    let expanded = normalize_and_expand(command, &root, &data);
    for token in shell_like_tokens(&expanded) {
        let Some(candidate) = hook_command_script_candidate(&plugin_root, &token) else {
            continue;
        };
        match candidate {
            Ok(path) => inspection.scripts.push(path),
            Err(error) => inspection.errors.push(format!(
                "hook command '{command}' references invalid script: {error}"
            )),
        }
    }
}

fn hook_command_script_candidate(root: &Path, token: &str) -> Option<Result<PathBuf>> {
    let trimmed = token.trim_matches(['"', '\'', '`', ';']);
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("./") || trimmed.starts_with(".\\") {
        let declared = resolve_plugin_relative(root, trimmed).map_err(|error| {
            CoreError::invalid(format!("hook command path '{trimmed}' is invalid: {error}"))
        });
        return Some(declared.and_then(|path| resolve_hook_script_path(root, &path, trimmed)));
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() && path_is_under(&path, root) {
        return Some(resolve_hook_script_path(root, &path, trimmed));
    }
    if token_looks_like_script_path(trimmed) {
        return Some(Err(CoreError::invalid(format!(
            "hook command script path '{trimmed}' must start with `./` or `${{PLUGIN_ROOT}}/` and stay inside the plugin root"
        ))));
    }
    None
}

fn token_looks_like_script_path(token: &str) -> bool {
    (token.contains('/') || token.contains('\\')) && is_script_file(&PathBuf::from(token))
}

fn resolve_hook_script_path(root: &Path, path: &Path, display: &str) -> Result<PathBuf> {
    let path = resolve_existing_within(root, path).map_err(|error| {
        CoreError::invalid(format!(
            "hook command path '{display}' is unavailable: {error}"
        ))
    })?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(CoreError::invalid(format!(
            "hook command path '{}' is not a file",
            path.display()
        )))
    }
}

fn shell_like_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn push_command_id(path: &Path, out: &mut Vec<String>) {
    if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
        let stem = stem.trim();
        if !stem.is_empty() {
            out.push(stem.to_string());
        }
    }
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "mdx"))
        .unwrap_or(false)
}

fn collect_host_app_components(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "component" {
                    if let Some(component) = value.as_str() {
                        let component = component.trim();
                        if component.starts_with("builtin:") || component.starts_with("tauri:") {
                            out.push(component.to_string());
                        }
                    }
                    continue;
                }
                collect_host_app_components(value, out);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items {
                collect_host_app_components(value, out);
            }
        }
        _ => {}
    }
}

fn scan_runtime_script_dirs(root: &Path, needs: &mut PluginRuntimeNeeds) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let is_script_dir = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| matches!(name, "scripts" | "bin"))
            .unwrap_or(false);
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path.clone());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if !is_script_file(&path) && !is_script_dir {
                continue;
            }
            match path.extension().and_then(|ext| ext.to_str()) {
                Some(ext)
                    if matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "js" | "cjs" | "mjs" | "ts" | "tsx" | "jsx"
                    ) =>
                {
                    needs.node = true;
                }
                Some(ext) if matches!(ext.to_ascii_lowercase().as_str(), "py") => {
                    needs.python = true;
                }
                Some(ext)
                    if matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "sh" | "bash" | "zsh" | "ps1" | "cmd" | "bat"
                    ) =>
                {
                    needs.shell = true;
                }
                _ => {}
            }
        }
    }
}

fn is_script_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "js" | "cjs"
                    | "mjs"
                    | "ts"
                    | "tsx"
                    | "jsx"
                    | "py"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "ps1"
                    | "cmd"
                    | "bat"
            )
        })
        .unwrap_or(false)
}

fn probe_runtime(program: &str, args: &[&str]) -> bool {
    runtime_probe_candidates(program)
        .into_iter()
        .any(|candidate| {
            Command::new(candidate)
                .args(args)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
}

fn runtime_probe_candidates(program: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(PathBuf::from(program));
    if let Some(env_key) = runtime_env_key(program) {
        if let Some(path) = std::env::var_os(env_key).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            if !candidates.iter().any(|candidate| candidate == &path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

fn runtime_env_key(program: &str) -> Option<&'static str> {
    match program.to_ascii_lowercase().as_str() {
        "node" | "node.exe" => Some("DEEPAGENT_NODE"),
        "python" | "python.exe" | "python3" => Some("DEEPAGENT_PYTHON"),
        "java" | "java.exe" => Some("DEEPAGENT_JAVA"),
        _ => None,
    }
}

fn probe_shell() -> bool {
    let candidates: [(&str, &[&str]); 4] = [
        (
            "pwsh",
            &[
                "-NoProfile",
                "-Command",
                "$PSVersionTable.PSVersion.ToString()",
            ],
        ),
        (
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "$PSVersionTable.PSVersion.ToString()",
            ],
        ),
        ("bash", &["-lc", "true"]),
        ("sh", &["-lc", "true"]),
    ];
    candidates.iter().any(|(program, args)| {
        Command::new(program)
            .args(*args)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    })
}

fn format_runtime_unavailable(needs: &PluginRuntimeNeeds) -> String {
    let mut parts = Vec::new();
    if needs.node {
        parts.push("node");
    }
    if needs.python {
        parts.push("python");
    }
    if needs.java {
        parts.push("java");
    }
    if needs.shell {
        parts.push("shell");
    }
    if parts.is_empty() {
        "runtime is unavailable".to_string()
    } else {
        format!("runtime unavailable: {}", parts.join(", "))
    }
}

fn explicit_health_lifecycle_state(
    current: PluginLifecycleState,
    health_status: PluginHealthStatus,
) -> PluginLifecycleState {
    match health_status {
        PluginHealthStatus::NeedsConfiguration
        | PluginHealthStatus::NeedsAuthorization
        | PluginHealthStatus::ConnectionUnavailable => PluginLifecycleState::RuntimeReady,
        PluginHealthStatus::RuntimeUnavailable | PluginHealthStatus::Incomplete => {
            PluginLifecycleState::Incomplete
        }
        PluginHealthStatus::Failed => PluginLifecycleState::Failed,
        PluginHealthStatus::Ready | PluginHealthStatus::Unknown => current,
    }
}

fn hosted_mcp_connection_failures(config: &McpConfig) -> Vec<String> {
    config
        .servers
        .iter()
        .filter_map(|(name, server)| hosted_mcp_url(server).map(|url| (name.as_str(), url)))
        .filter_map(|(name, url)| probe_hosted_mcp_url(name, url).err())
        .collect()
}

fn hosted_mcp_url(server: &deepagent_mcp::config::McpServerConfig) -> Option<&str> {
    match server.effective_type().ok()? {
        TransportType::Http | TransportType::Sse | TransportType::Ws => server.url.as_deref(),
        TransportType::Stdio => None,
    }
}

fn probe_hosted_mcp_url(name: &str, url: &str) -> std::result::Result<(), String> {
    let (host, port) = parse_url_host_port(url).map_err(|error| format!("{name}: {error}"))?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("{name}: resolve {host}:{port}: {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("{name}: no socket addresses for {host}:{port}"));
    }

    let timeout = std::time::Duration::from_millis(250);
    let mut last_error = None;
    for address in addresses {
        match std::net::TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(format!(
        "{name}: connect {host}:{port}: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unreachable".to_string())
    ))
}

fn parse_url_host_port(url: &str) -> std::result::Result<(String, u16), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("invalid MCP url '{url}'"))?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => 443,
        "http" | "ws" => 80,
        other => return Err(format!("unsupported MCP url scheme '{other}'")),
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return Err(format!("invalid MCP url '{url}'"));
    }

    if let Some(after_bracket) = authority.strip_prefix('[') {
        let (host, suffix) = after_bracket
            .split_once(']')
            .ok_or_else(|| format!("invalid bracketed host in '{url}'"))?;
        let port = suffix
            .strip_prefix(':')
            .map(parse_port)
            .transpose()?
            .unwrap_or(default_port);
        return Ok((host.to_string(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => {
            (host, parse_port(port)?)
        }
        _ => (authority, default_port),
    };
    if host.trim().is_empty() {
        Err(format!("invalid MCP url host in '{url}'"))
    } else {
        Ok((host.to_string(), port))
    }
}

fn parse_port(port: &str) -> std::result::Result<u16, String> {
    port.parse::<u16>()
        .map_err(|error| format!("invalid MCP url port '{port}': {error}"))
}

fn has_fatal_plugin_errors(plugin: &LoadedPlugin) -> bool {
    plugin
        .errors
        .iter()
        .any(|error| error.severity == crate::plugin::model::DiagnosticSeverity::Error)
        || plugin
            .errors
            .iter()
            .any(|error| matches!(error.kind.as_str(), "manifest-parse-error" | "blocklist"))
}

fn manifest_has_host_backed_app(manifest: &PluginManifest) -> bool {
    manifest
        .paths
        .app_paths
        .iter()
        .filter_map(read_json_file)
        .any(|value| {
            json_contains_text_value(&value, "component", |component| {
                component.starts_with("builtin:") || component.starts_with("tauri:")
            })
        })
}

fn manifest_needs_host_authorization(manifest: &PluginManifest) -> bool {
    manifest_has_oauth_mcp(manifest) || manifest_has_connector_app(manifest)
}

fn manifest_has_oauth_mcp(manifest: &PluginManifest) -> bool {
    manifest
        .paths
        .mcp_server_paths
        .iter()
        .filter_map(read_json_file)
        .any(|value| json_contains_key(&value, |key| key.to_ascii_lowercase().contains("oauth")))
        || manifest
            .paths
            .mcp_servers_inline
            .as_ref()
            .is_some_and(|value| {
                json_contains_key(value, |key| key.to_ascii_lowercase().contains("oauth"))
            })
}

fn manifest_has_connector_app(manifest: &PluginManifest) -> bool {
    manifest
        .paths
        .app_paths
        .iter()
        .filter_map(read_json_file)
        .any(|value| {
            value
                .get("apps")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|apps| {
                    apps.values().any(|app| {
                        app.as_object().is_some_and(|object| {
                            object
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|id| !id.trim().is_empty())
                                && !object.contains_key("component")
                        })
                    })
                })
        })
}

fn read_json_file(path: &PathBuf) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn json_contains_key(value: &serde_json::Value, predicate: impl Fn(&str) -> bool + Copy) -> bool {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| predicate(key) || json_contains_key(value, predicate)),
        serde_json::Value::Array(items) => items
            .iter()
            .any(|value| json_contains_key(value, predicate)),
        _ => false,
    }
}

fn json_contains_text_value(
    value: &serde_json::Value,
    key_name: &str,
    predicate: impl Fn(&str) -> bool + Copy,
) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            if key == key_name {
                value.as_str().is_some_and(predicate)
            } else {
                json_contains_text_value(value, key_name, predicate)
            }
        }),
        serde_json::Value::Array(items) => items
            .iter()
            .any(|value| json_contains_text_value(value, key_name, predicate)),
        _ => false,
    }
}

fn credential_env_hints(root: &Path) -> Vec<String> {
    let mut hints = std::collections::BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited_files = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if should_scan_plugin_subdir(&path) {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() || !should_scan_credential_file(&path) {
                continue;
            }
            visited_files += 1;
            if visited_files > 256 {
                return hints.into_iter().collect();
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > 256 * 1024 {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            collect_credential_tokens(&text, &mut hints);
        }
    }
    hints.into_iter().collect()
}

fn missing_credential_env_hints(root: &Path) -> Vec<String> {
    credential_env_hints(root)
        .into_iter()
        .filter(|name| std::env::var_os(name).is_none())
        .collect()
}

fn should_scan_plugin_subdir(path: &Path) -> bool {
    !path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | "node_modules" | "target" | "dist" | "build" | "runtime"
            )
        })
}

fn should_scan_credential_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "toml"
                    | "sh"
                    | "ps1"
                    | "py"
                    | "js"
                    | "ts"
                    | "cjs"
                    | "mjs"
            )
        })
        .unwrap_or(false)
}

fn collect_credential_tokens(text: &str, out: &mut std::collections::BTreeSet<String>) {
    for token in text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim_matches('_');
        if token.len() < 8 || token.len() > 80 {
            continue;
        }
        if token.chars().any(|ch| ch.is_ascii_lowercase()) {
            continue;
        }
        if is_credential_env_name(token) {
            out.insert(token.to_string());
        }
    }
}

fn is_credential_env_name(token: &str) -> bool {
    token.ends_with("_API_KEY")
        || token.ends_with("_ACCESS_TOKEN")
        || token.ends_with("_AUTH_TOKEN")
        || token.ends_with("_CLIENT_SECRET")
        || token.ends_with("_SECRET")
}

fn count_skills(manifest: &PluginManifest) -> u32 {
    manifest
        .paths
        .skills
        .iter()
        .map(|path| count_skill_root(path))
        .sum()
}

fn count_skill_root(path: &Path) -> u32 {
    if path.join("SKILL.md").is_file() {
        return 1;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .count() as u32
}

fn count_mcp_servers(manifest: &PluginManifest) -> u32 {
    let path_count = manifest
        .paths
        .mcp_server_paths
        .iter()
        .filter(|path| path.is_file())
        .count() as u32;
    path_count + inline_object_count(manifest.paths.mcp_servers_inline.as_ref())
}

fn count_hooks(manifest: &PluginManifest) -> u32 {
    let path_count = manifest
        .paths
        .hook_paths
        .iter()
        .filter(|path| path.is_file())
        .count() as u32;
    path_count + inline_object_count(manifest.paths.hooks_inline.as_ref())
}

fn count_commands(manifest: &PluginManifest) -> u32 {
    count_markdown_like(&manifest.paths.commands)
}

fn count_agents(manifest: &PluginManifest) -> u32 {
    count_markdown_like(&manifest.paths.agents)
}

fn count_apps(manifest: &PluginManifest) -> u32 {
    manifest
        .paths
        .app_paths
        .iter()
        .filter(|path| path.exists())
        .count() as u32
}

fn count_output_styles(manifest: &PluginManifest) -> u32 {
    count_markdown_like(&manifest.paths.output_styles)
}

fn count_markdown_like(paths: &[PathBuf]) -> u32 {
    let mut count = 0;
    for path in paths {
        if path.is_file() {
            count += 1;
            continue;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            continue;
        };
        count += entries
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| matches!(ext, "md" | "mdx"))
                    .unwrap_or(false)
            })
            .count() as u32;
    }
    count
}

fn inline_object_count(value: Option<&serde_json::Value>) -> u32 {
    value
        .and_then(|value| value.as_object())
        .map(|object| object.len() as u32)
        .unwrap_or_default()
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|e| {
        CoreError::Persistence(format!(
            "create plugin destination {}: {e}",
            destination.display()
        ))
    })?;
    for entry in std::fs::read_dir(source).map_err(|e| {
        CoreError::Persistence(format!("read plugin source {}: {e}", source.display()))
    })? {
        let entry = entry.map_err(|e| CoreError::Persistence(format!("read plugin entry: {e}")))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let ty = entry.file_type().map_err(|e| {
            CoreError::Persistence(format!("stat plugin entry {}: {e}", from.display()))
        })?;
        if ty.is_dir() {
            if entry.file_name().to_string_lossy() == ".git" {
                continue;
            }
            copy_dir(&from, &to)?;
        } else if ty.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Persistence(format!("create plugin file parent: {e}"))
                })?;
            }
            std::fs::copy(&from, &to).map_err(|e| {
                CoreError::Persistence(format!(
                    "copy plugin file {} -> {}: {e}",
                    from.display(),
                    to.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn commit_plugin_directory(
    source: &Path,
    destination: &Path,
    install_root: &Path,
    label: &str,
) -> Result<()> {
    let stage = create_plugin_staging_dir(install_root, label)?;
    let result = (|| {
        copy_dir(source, &stage)?;
        load_plugin_manifest(&stage)?
            .ok_or_else(|| CoreError::invalid("staged plugin manifest not found"))?;
        replace_plugin_dir(&stage, destination, install_root)
    })();
    if result.is_err() {
        let _ = remove_dir_all_with_retry(&stage);
    }
    result
}

fn create_plugin_staging_dir(install_root: &Path, label: &str) -> Result<PathBuf> {
    let stage = unique_plugin_staging_path(install_root, label)?;
    std::fs::create_dir_all(&stage).map_err(|e| {
        CoreError::Persistence(format!(
            "create plugin staging dir {}: {e}",
            stage.display()
        ))
    })?;
    Ok(stage)
}

fn unique_plugin_staging_path(install_root: &Path, label: &str) -> Result<PathBuf> {
    let base = install_root.join(".staging");
    std::fs::create_dir_all(&base).map_err(|e| {
        CoreError::Persistence(format!(
            "create plugin staging root {}: {e}",
            base.display()
        ))
    })?;
    let label = sanitize_file_name(label);
    for attempt in 0..100u32 {
        let path = base.join(format!(
            "{label}-{}-{}-{attempt}",
            std::process::id(),
            now_string()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(CoreError::other(format!(
        "failed to allocate plugin staging dir under {}",
        base.display()
    )))
}

fn remove_dir_all_with_retry(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut last_error = None;
    for attempt in 0..5 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    Err(last_error.unwrap())
}

fn replace_plugin_dir(stage: &Path, destination: &Path, install_root: &Path) -> Result<()> {
    ensure_replace_dir_is_managed(stage, destination, install_root)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin destination parent {}: {e}",
                parent.display()
            ))
        })?;
    }

    if !destination.exists() {
        return std::fs::rename(stage, destination).map_err(|e| {
            CoreError::Persistence(format!(
                "activate plugin directory {} -> {}: {e}",
                stage.display(),
                destination.display()
            ))
        });
    }

    let backup = unique_plugin_staging_path(install_root, "rollback")?;
    std::fs::rename(destination, &backup).map_err(|e| {
        CoreError::Persistence(format!(
            "backup existing plugin directory {} -> {}: {e}",
            destination.display(),
            backup.display()
        ))
    })?;

    match std::fs::rename(stage, destination) {
        Ok(()) => {
            if let Err(error) = std::fs::remove_dir_all(&backup) {
                tracing::warn!(
                    path = %backup.display(),
                    error = %error,
                    "failed to remove replaced plugin backup"
                );
            }
            Ok(())
        }
        Err(activate_err) => {
            let restore = std::fs::rename(&backup, destination);
            let _ = std::fs::remove_dir_all(stage);
            match restore {
                Ok(()) => Err(CoreError::Persistence(format!(
                    "activate plugin directory {} -> {} failed and previous version was restored: {activate_err}",
                    stage.display(),
                    destination.display()
                ))),
                Err(restore_err) => Err(CoreError::Persistence(format!(
                    "activate plugin directory {} -> {} failed ({activate_err}); restore from {} also failed: {restore_err}",
                    stage.display(),
                    destination.display(),
                    backup.display()
                ))),
            }
        }
    }
}

fn ensure_replace_dir_is_managed(
    stage: &Path,
    destination: &Path,
    install_root: &Path,
) -> Result<()> {
    std::fs::create_dir_all(install_root).map_err(|e| {
        CoreError::Persistence(format!(
            "create plugin install root {}: {e}",
            install_root.display()
        ))
    })?;
    let root = std::fs::canonicalize(install_root).map_err(|e| {
        CoreError::Persistence(format!(
            "canonicalize plugin install root {}: {e}",
            install_root.display()
        ))
    })?;
    let stage = std::fs::canonicalize(stage).map_err(|e| {
        CoreError::Persistence(format!(
            "canonicalize plugin staging dir {}: {e}",
            stage.display()
        ))
    })?;
    if stage == root || !stage.starts_with(&root) {
        return Err(CoreError::invalid(format!(
            "refusing to activate plugin staging dir outside install root: {}",
            stage.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin destination parent {}: {e}",
                parent.display()
            ))
        })?;
        let parent = std::fs::canonicalize(parent).map_err(|e| {
            CoreError::Persistence(format!(
                "canonicalize plugin destination parent {}: {e}",
                parent.display()
            ))
        })?;
        if !parent.starts_with(&root) {
            return Err(CoreError::invalid(format!(
                "refusing to activate plugin outside install root: {}",
                destination.display()
            )));
        }
    }
    if destination.exists() {
        let destination = std::fs::canonicalize(destination).map_err(|e| {
            CoreError::Persistence(format!(
                "canonicalize plugin destination {}: {e}",
                destination.display()
            ))
        })?;
        if destination == root || !destination.starts_with(&root) {
            return Err(CoreError::invalid(format!(
                "refusing to replace plugin outside install root: {}",
                destination.display()
            )));
        }
    }
    Ok(())
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::invalid(format!("state path has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent).map_err(|e| {
        CoreError::Persistence(format!("create state parent {}: {e}", parent.display()))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("state.json");
    for attempt in 0..100u32 {
        let tmp = parent.join(format!(
            ".{file_name}.tmp-{}-{}-{attempt}",
            std::process::id(),
            now_string()
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(CoreError::Persistence(format!(
                    "create temp state file {}: {err}",
                    tmp.display()
                )));
            }
        };
        let result = file
            .write_all(contents)
            .and_then(|_| file.sync_all())
            .map_err(|e| {
                CoreError::Persistence(format!("write temp state file {}: {e}", tmp.display()))
            });
        drop(file);
        if let Err(error) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        return replace_file(&tmp, path);
    }

    Err(CoreError::other(format!(
        "failed to allocate temp state file for {}",
        path.display()
    )))
}

fn replace_file(source: &Path, target: &Path) -> Result<()> {
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(rename_err) if target.exists() => {
            std::fs::remove_file(target).map_err(|remove_err| {
                CoreError::Persistence(format!(
                    "replace state file {} failed after rename error ({rename_err}); remove target failed: {remove_err}",
                    target.display()
                ))
            })?;
            std::fs::rename(source, target).map_err(|second_err| {
                CoreError::Persistence(format!(
                    "replace state file {} with {} failed: {second_err}",
                    target.display(),
                    source.display()
                ))
            })
        }
        Err(err) => {
            let _ = std::fs::remove_file(source);
            Err(CoreError::Persistence(format!(
                "replace state file {} with {} failed: {err}",
                target.display(),
                source.display()
            )))
        }
    }
}

fn safe_remove_dir(base: &Path, target: &Path) -> Result<()> {
    let base = std::fs::canonicalize(base).map_err(|e| {
        CoreError::Persistence(format!("canonicalize base {}: {e}", base.display()))
    })?;
    let target = std::fs::canonicalize(target).map_err(|e| {
        CoreError::Persistence(format!("canonicalize target {}: {e}", target.display()))
    })?;
    if target == base || !target.starts_with(&base) {
        return Err(CoreError::invalid(format!(
            "refusing to remove path outside plugin root: {}",
            target.display()
        )));
    }
    std::fs::remove_dir_all(&target).map_err(|e| {
        CoreError::Persistence(format!("remove plugin directory {}: {e}", target.display()))
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn path_is_under(path: &Path, base: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(base)) {
        (Ok(path), Ok(base)) => path == base || path.starts_with(&base),
        _ => path == base || path.starts_with(base),
    }
}

fn plugin_id_source(id: &str) -> Option<String> {
    id.rsplit_once('@')
        .map(|(_, source)| source.trim())
        .filter(|source| !source.is_empty())
        .map(str::to_string)
}

fn plugin_runtime_priority(origin: PluginOrigin) -> u8 {
    match origin {
        PluginOrigin::BuiltIn => 10,
        PluginOrigin::Marketplace => 20,
        PluginOrigin::Personal => 30,
        PluginOrigin::Workspace => 40,
        PluginOrigin::Session => 50,
    }
}

fn sanitize_file_name(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn trimmed_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn versions_differ(remote: Option<&str>, local: Option<&str>) -> bool {
    match (version_ref(remote), version_ref(local)) {
        (Some(remote), Some(local)) => remote != local,
        (Some(_), None) => true,
        _ => false,
    }
}

fn version_ref(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn now_string() -> String {
    now_millis().to_string()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

impl PathSnapshot {
    fn capture(path: PathBuf) -> Self {
        match std::fs::metadata(&path) {
            Ok(metadata) => Self {
                path,
                exists: true,
                is_dir: metadata.is_dir(),
                len: Some(metadata.len()),
                modified_millis: metadata.modified().ok().and_then(system_time_millis),
            },
            Err(_) => Self {
                path,
                exists: false,
                is_dir: false,
                len: None,
                modified_millis: None,
            },
        }
    }

    fn still_matches(&self) -> bool {
        Self::capture(self.path.clone()) == *self
    }
}

fn system_time_millis(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn runtime_watch_paths(
    roots: &PluginRoots,
    state_path: &Path,
    loaded: &[LoadedPlugin],
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_watch_path(&mut paths, state_path.to_path_buf());
    push_unique_watch_path(&mut paths, roots.builtin.clone());
    push_unique_watch_path(&mut paths, roots.personal.clone());
    push_unique_watch_path(&mut paths, roots.marketplace_cache.clone());
    push_unique_watch_path(&mut paths, roots.marketplaces.clone());
    if let Some(workspace) = &roots.workspace {
        push_unique_watch_path(&mut paths, workspace.clone());
    }
    for session_root in &roots.session {
        push_unique_watch_path(&mut paths, session_root.clone());
    }

    for plugin in loaded {
        push_unique_watch_path(&mut paths, plugin.root.clone());
        if let Some(manifest) = plugin.manifest() {
            push_unique_watch_path(&mut paths, manifest.manifest_path.clone());
            for path in &manifest.paths.skills {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.commands {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.agents {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.app_paths {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.mcp_server_paths {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.hook_paths {
                push_unique_watch_path(&mut paths, path.clone());
            }
            for path in &manifest.paths.output_styles {
                push_runtime_tree_watch_paths(&mut paths, path);
            }
        }
    }

    paths
}

fn plugin_list_watch_paths(
    roots: &PluginRoots,
    state_path: &Path,
    loaded: &[LoadedPlugin],
    state: &PluginState,
) -> Vec<PathBuf> {
    let mut paths = runtime_watch_paths(roots, state_path, loaded);
    for (name, marketplace) in &state.marketplaces {
        let snapshot = marketplace
            .manifest_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| roots.marketplaces.join(name).join("marketplace.json"));
        push_unique_watch_path(&mut paths, snapshot);
        if let Some(source_root) = marketplace.source_root.as_ref().map(PathBuf::from) {
            push_unique_watch_path(&mut paths, source_root);
        }
        if let Some(install_location) = marketplace.install_location.as_ref().map(PathBuf::from) {
            push_unique_watch_path(&mut paths, install_location);
        }
    }

    for plugin in loaded {
        if let Some(manifest) = plugin.manifest() {
            for path in &manifest.paths.skills {
                push_runtime_tree_watch_paths(&mut paths, path);
            }
            for path in &manifest.paths.commands {
                push_runtime_tree_watch_paths(&mut paths, path);
            }
            for path in &manifest.paths.output_styles {
                push_runtime_tree_watch_paths(&mut paths, path);
            }
        }
    }

    paths
}

fn push_runtime_tree_watch_paths(paths: &mut Vec<PathBuf>, path: &Path) {
    push_unique_watch_path(paths, path.to_path_buf());
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            push_runtime_tree_watch_paths(paths, &child);
        } else if file_type.is_file() {
            push_unique_watch_path(paths, child);
        }
    }
}

fn push_unique_watch_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn roots(tmp: &Path) -> PluginRoots {
        PluginRoots {
            session: Vec::new(),
            builtin: tmp.join("builtin"),
            workspace: None,
            personal: tmp.join("personal"),
            marketplace_cache: tmp.join("cache"),
            marketplaces: tmp.join("marketplaces"),
        }
    }

    fn write_plugin(root: &Path, name: &str) {
        write_plugin_with_dependencies(root, name, &[]);
    }

    fn write_plugin_with_dependencies(root: &Path, name: &str, dependencies: &[&str]) {
        write_plugin_with_version_and_dependencies(root, name, "0.1.0", dependencies);
    }

    fn write_plugin_with_version(root: &Path, name: &str, version: &str) {
        write_plugin_with_version_and_dependencies(root, name, version, &[]);
    }

    fn write_plugin_zip(archive: &Path, root_name: &str, plugin_name: &str, version: &str) {
        write_plugin_zip_with_payload(archive, root_name, plugin_name, version, &[]);
    }

    fn write_plugin_zip_with_payload(
        archive: &Path,
        root_name: &str,
        plugin_name: &str,
        version: &str,
        extra_files: &[(&str, &[u8])],
    ) {
        if let Some(parent) = archive.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let file = File::create(archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        let manifest = serde_json::json!({
            "name": plugin_name,
            "version": version,
            "skills": "skills",
        })
        .to_string();

        zip.start_file(format!("{root_name}/.codex-plugin/plugin.json"), opts)
            .unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        zip.start_file(format!("{root_name}/skills/SKILL.md"), opts)
            .unwrap();
        zip.write_all(b"---\nname: demo\n---\nDemo skill").unwrap();
        for (relative, contents) in extra_files {
            zip.start_file(format!("{root_name}/{relative}"), opts)
                .unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap();
    }

    fn write_github_contents_manifest_fixture(
        fixtures: &Path,
        full_name: &str,
        manifest_relative_path: &str,
        commit: &str,
        manifest: serde_json::Value,
    ) {
        let endpoint = format!(
            "/repos/{}/contents/{}?ref={}",
            full_name,
            percent_encode_content_path(manifest_relative_path),
            percent_encode_query_value(commit)
        );
        let encoded = base64_encode_test(serde_json::to_string(&manifest).unwrap().as_bytes());
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(&endpoint))),
            serde_json::json!({
                "type": "file",
                "encoding": "base64",
                "content": encoded,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn base64_encode_test(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = *chunk.get(1).unwrap_or(&0);
            let b2 = *chunk.get(2).unwrap_or(&0);
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn git_available() -> bool {
        run_external("git", &["--version".to_string()], None).is_ok()
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        run_external_output(
            "git",
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
            Some(cwd),
        )
        .unwrap()
    }

    fn write_git_plugin_repo(root: &Path) -> (String, String) {
        std::fs::create_dir_all(root).unwrap();
        run_git(root, &["init"]);
        run_git(root, &["config", "user.email", "plugin-test@example.com"]);
        run_git(root, &["config", "user.name", "Plugin Test"]);
        run_git(root, &["checkout", "-b", "main"]);
        write_plugin_with_version(root, "git-demo", "0.1.0");
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "v1"]);
        let first = run_git(root, &["rev-parse", "HEAD"]).trim().to_string();

        write_plugin_with_version(root, "git-demo", "0.2.0");
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "v2"]);
        let second = run_git(root, &["rev-parse", "HEAD"]).trim().to_string();
        (first, second)
    }

    fn write_plugin_with_version_and_dependencies(
        root: &Path,
        name: &str,
        version: &str,
        dependencies: &[&str],
    ) {
        std::fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::fs::write(
            root.join("skills").join("SKILL.md"),
            "---\nname: demo\n---\nDemo skill",
        )
        .unwrap();
        let manifest = serde_json::json!({
            "name": name,
            "version": version,
            "skills": "skills",
            "dependencies": dependencies,
        });
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn write_plugin_with_app_and_output_style(root: &Path, name: &str) {
        write_plugin_with_app_component_and_output_style(root, name, &format!("builtin:{name}"));
    }

    fn write_plugin_with_app_component_and_output_style(root: &Path, name: &str, component: &str) {
        write_plugin_with_version_and_dependencies(root, name, "0.1.0", &[]);
        std::fs::write(
            root.join(".app.json"),
            serde_json::json!({
                "apps": [
                    {
                        "id": format!("{name}-panel"),
                        "title": name,
                        "description": format!("Open the {name} panel"),
                        "placement": "right-sidebar",
                        "component": component,
                        "icon": "folder",
                        "category": "Developer Tools"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("output-styles")).unwrap();
        std::fs::write(
            root.join("output-styles").join("concise.md"),
            format!("# Concise {name}\n\nKeep replies short and concrete."),
        )
        .unwrap();
    }

    fn write_plugin_with_command_and_app_component(
        root: &Path,
        name: &str,
        command: &str,
        component: &str,
    ) {
        write_plugin_with_version_and_dependencies(root, name, "0.1.0", &[]);
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(
            root.join("commands").join(format!("{command}.md")),
            format!("---\ndescription: {name} workflow\n---\nRun {command} with ${{ARGUMENTS}}"),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "name": name,
            "version": "0.1.0",
            "skills": "skills",
            "commands": "commands",
        });
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join(".app.json"),
            serde_json::json!({
                "apps": [
                    {
                        "id": format!("{name}-panel"),
                        "title": name,
                        "description": format!("Open the {name} panel"),
                        "placement": "right-sidebar",
                        "component": component,
                        "icon": "folder",
                        "category": "Developer Tools"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn write_plugin_with_connector_app(
        root: &Path,
        name: &str,
        provider: &str,
        connector_id: &str,
    ) {
        write_plugin(root, name);
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::json!({
                "name": name,
                "version": "0.1.0",
                "skills": "skills",
                "apps": ".app.json",
            })
            .to_string(),
        )
        .unwrap();
        let mut apps = serde_json::Map::new();
        apps.insert(
            provider.to_string(),
            serde_json::json!({
                "id": connector_id
            }),
        );
        std::fs::write(
            root.join(".app.json"),
            serde_json::json!({
                "apps": apps
            })
            .to_string(),
        )
        .unwrap();
    }

    fn write_plugin_with_hosted_mcp(root: &Path, name: &str, url: &str, oauth: bool) {
        write_plugin(root, name);
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::json!({
                "name": name,
                "version": "0.1.0",
                "skills": "skills",
                "mcpServers": ".mcp.json",
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join(".mcp.json"),
            if oauth {
                serde_json::json!({
                    "mcpServers": {
                        "hosted": {
                            "type": "http",
                            "url": url,
                            "oauth_resource": url
                        }
                    }
                })
            } else {
                serde_json::json!({
                    "mcpServers": {
                        "hosted": {
                            "type": "http",
                            "url": url
                        }
                    }
                })
            }
            .to_string(),
        )
        .unwrap();
    }

    fn write_plugin_with_command(root: &Path, name: &str) {
        write_plugin_with_version_and_dependencies(root, name, "0.1.0", &[]);
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(
            root.join("commands").join("inspect.md"),
            "---\ndescription: Inspect plugin state\n---\nInspect $ARGUMENTS",
        )
        .unwrap();
        let manifest = serde_json::json!({
            "name": name,
            "version": "0.1.0",
            "skills": "skills",
            "commands": "commands",
        });
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn write_manifest_only_plugin(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        let manifest = serde_json::json!({
            "name": name,
            "version": "0.1.0",
        });
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn manifest_only_plugin_stops_at_parsed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_manifest_only_plugin(&roots.builtin.join("empty"), "empty");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("empty@builtin").unwrap().unwrap();

        assert!(plugin.installed);
        assert_eq!(plugin.state, PluginLifecycleState::Parsed);
        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert!(plugin.entrypoints.is_empty());
        assert!(!plugin.runtime_required);
    }

    #[test]
    fn host_backed_plugin_reports_missing_host_component() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_app_and_output_style(&roots.builtin.join("host-demo"), "host-demo");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("host-demo@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        assert_eq!(plugin.state, PluginLifecycleState::Incomplete);
        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("builtin:host-demo"));
    }

    #[test]
    fn host_backed_plugin_with_registered_app_component_can_be_verified() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_app_component_and_output_style(
            &roots.builtin.join("host-demo"),
            "host-demo",
            "builtin:browser",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("host-demo@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert_eq!(plugin.state, PluginLifecycleState::Verified);
        assert!(plugin.health_error.is_none());
    }

    #[test]
    fn host_backed_plugin_with_registered_command_binding_can_be_verified() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_command_and_app_component(
            &roots.builtin.join("browser"),
            "browser",
            "inspect",
            "builtin:browser",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("browser@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert_eq!(plugin.state, PluginLifecycleState::Verified);
        assert_eq!(plugin.command_count, 1);
        assert!(plugin
            .entrypoints
            .iter()
            .any(|entry| Path::new(entry).ends_with("commands")));
    }

    #[test]
    fn host_backed_plugin_with_unregistered_host_component_stays_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_command_and_app_component(
            &roots.builtin.join("computer-use"),
            "computer-use",
            "control",
            "builtin:computer-use",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("computer-use@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("host app component"));
    }

    #[test]
    fn connector_only_app_requires_host_authorization_without_becoming_host_backed() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("connector-demo");
        write_plugin_with_connector_app(
            &root,
            "connector-demo",
            "design-provider",
            "connector_test_123",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("connector-demo@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::DshSidecar);
        assert_eq!(plugin.health_status, PluginHealthStatus::NeedsAuthorization);
        assert_eq!(plugin.state, PluginLifecycleState::RuntimeReady);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("authorization"));

        let projection = svc.runtime_projection().unwrap();
        assert!(projection.app_entries.is_empty());
        assert_eq!(projection.connector_entries.len(), 1);
        assert_eq!(projection.connector_entries[0].provider, "design-provider");
        assert_eq!(projection.connector_entries[0].id, "connector_test_123");
    }

    #[test]
    fn check_plugin_health_persists_latest_result_for_later_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("connector-demo");
        write_plugin_with_connector_app(
            &root,
            "connector-demo",
            "design-provider",
            "connector_test_123",
        );
        let app_data = tmp.path().join("app-data");
        let svc = PluginService::new(roots.clone(), &app_data);

        let before = svc
            .inspect_plugin_runtime("connector-demo@builtin")
            .unwrap()
            .unwrap();
        assert_eq!(before.health_status, PluginHealthStatus::NeedsAuthorization);
        assert!(before.last_health_check.is_none());

        let checked = svc
            .check_plugin_health("connector-demo@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(
            checked.health_status,
            PluginHealthStatus::NeedsAuthorization
        );
        let checked_at = checked
            .last_health_check
            .clone()
            .expect("explicit check records timestamp");
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("authorization"));

        let state = svc.load_state().unwrap();
        let persisted = state
            .health_checks
            .get("connector-demo@builtin")
            .expect("health check persisted");
        assert_eq!(persisted.status, PluginHealthStatus::NeedsAuthorization);
        assert_eq!(persisted.checked_at, checked_at);
        assert!(persisted
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("authorization"));

        let reloaded = PluginService::new(roots, &app_data);
        let after = reloaded.read("connector-demo@builtin").unwrap().unwrap();
        assert_eq!(
            after.last_health_check.as_deref(),
            Some(checked_at.as_str())
        );
        assert_eq!(after.health_status, PluginHealthStatus::NeedsAuthorization);
    }

    #[test]
    fn check_plugin_health_missing_plugin_does_not_pollute_state() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = PluginService::new(roots(tmp.path()), tmp.path().join("app-data"));

        let checked = svc.check_plugin_health("missing@builtin").unwrap();

        assert!(checked.is_none());
        let state = svc.load_state().unwrap();
        assert!(state.health_checks.is_empty());
    }

    #[test]
    fn check_plugin_health_marks_unreachable_hosted_mcp_without_oauth_as_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("hosted-demo");
        write_plugin_with_hosted_mcp(&root, "hosted-demo", "https://127.0.0.1:9/mcp", false);
        let app_data = tmp.path().join("app-data");
        let svc = PluginService::new(roots.clone(), &app_data);

        let before = svc.read("hosted-demo@builtin").unwrap().unwrap();
        assert_eq!(before.health_status, PluginHealthStatus::Ready);

        let checked = svc
            .check_plugin_health("hosted-demo@builtin")
            .unwrap()
            .unwrap();
        assert_eq!(
            checked.health_status,
            PluginHealthStatus::ConnectionUnavailable
        );
        assert_eq!(checked.state, PluginLifecycleState::RuntimeReady);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("hosted MCP endpoint unavailable"));

        let state = svc.load_state().unwrap();
        let persisted = state.health_checks.get("hosted-demo@builtin").unwrap();
        assert_eq!(persisted.status, PluginHealthStatus::ConnectionUnavailable);
        assert!(persisted
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("hosted MCP endpoint unavailable"));

        let reloaded = PluginService::new(roots, &app_data);
        let after = reloaded.read("hosted-demo@builtin").unwrap().unwrap();
        assert_eq!(after.health_status, PluginHealthStatus::Ready);
        assert!(after.last_health_check.as_deref().is_some());
    }

    #[test]
    fn check_plugin_health_keeps_oauth_mcp_in_needs_authorization() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("oauth-demo");
        write_plugin_with_hosted_mcp(&root, "oauth-demo", "https://127.0.0.1:9/mcp", true);
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("oauth-demo@builtin")
            .unwrap()
            .unwrap();
        assert_eq!(
            checked.health_status,
            PluginHealthStatus::NeedsAuthorization
        );
        assert_eq!(checked.state, PluginLifecycleState::RuntimeReady);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("authorization"));
    }

    #[test]
    fn script_payload_marks_runtime_requirement_without_name_hardcoding() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::create_dir_all(root.join("skills").join("demo").join("scripts")).unwrap();
        std::fs::write(
            root.join("skills")
                .join("demo")
                .join("scripts")
                .join("analyze.py"),
            "print('ok')\n",
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("analysis-plugin@builtin").unwrap().unwrap();

        assert!(plugin.runtime_required);
        assert!(plugin
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint.ends_with("skills")));
    }

    #[test]
    fn hook_command_script_is_validated_and_reported_as_entrypoint() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("hook-plugin");
        write_plugin(&root, "hook-plugin");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts").join("post.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(
            root.join("hooks.json"),
            r#"{
              "hooks": {
                "PostToolUse": [
                  {
                    "matcher": "Write|Edit",
                    "hooks": [
                      { "type": "command", "command": "./scripts/post.sh" }
                    ]
                  }
                ]
              }
            }"#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("hook-plugin@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::DshSidecar);
        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert!(plugin.runtime_required);
        assert!(plugin
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint.ends_with("scripts\\post.sh")
                || entrypoint.ends_with("scripts/post.sh")));
    }

    #[test]
    fn bundled_figma_hook_command_script_resolves_without_errors() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src-tauri/resources/plugins/figma");
        if !root.is_dir() {
            eprintln!("skipping: bundled figma plugin is not present");
            return;
        }
        let inspection = inspect_hook_command_scripts(
            &root,
            &tempfile::tempdir().unwrap().path().join("plugin-data"),
            None,
            &[root.join("hooks.json")],
        );

        assert!(inspection.errors.is_empty(), "{inspection:?}");
        assert!(inspection
            .scripts
            .iter()
            .any(|script| script.ends_with("scripts/post_write_figma_parity_check.sh")));
    }

    #[test]
    fn missing_hook_command_script_keeps_plugin_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("hook-plugin");
        write_plugin(&root, "hook-plugin");
        std::fs::write(
            root.join("hooks.json"),
            r#"{
              "hooks": {
                "PostToolUse": [
                  {
                    "hooks": [
                      { "type": "command", "command": "./scripts/missing.sh" }
                    ]
                  }
                ]
              }
            }"#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("hook-plugin@builtin").unwrap().unwrap();

        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert_eq!(plugin.state, PluginLifecycleState::Incomplete);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("missing.sh"));
    }

    #[test]
    fn escaping_hook_command_script_keeps_plugin_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("hook-plugin");
        write_plugin(&root, "hook-plugin");
        std::fs::write(
            root.join("hooks.json"),
            r#"{
              "hooks": {
                "PostToolUse": [
                  {
                    "hooks": [
                      { "type": "command", "command": "../outside.sh" }
                    ]
                  }
                ]
              }
            }"#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("hook-plugin@builtin").unwrap().unwrap();

        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("../outside.sh"));
    }

    #[test]
    fn runtime_probe_candidates_prefer_local_then_managed_env() {
        let candidates = runtime_probe_candidates("node");

        assert_eq!(candidates.first(), Some(&PathBuf::from("node")));
        if let Some(managed) = std::env::var_os("DEEPAGENT_NODE").filter(|value| !value.is_empty())
        {
            assert!(candidates.contains(&PathBuf::from(managed)));
        }
    }

    #[test]
    fn all_detected_plugin_credentials_must_be_configured() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY");
        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_BETA_API_KEY");
        std::env::set_var("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY", "configured");

        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("credentialed-plugin");
        write_plugin(&root, "credentialed-plugin");
        std::fs::write(
            root.join("README.md"),
            "Requires DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY and DEEPAGENT_TEST_PLUGIN_BETA_API_KEY.",
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("credentialed-plugin@builtin").unwrap().unwrap();

        assert_eq!(plugin.health_status, PluginHealthStatus::NeedsConfiguration);
        let error = plugin.health_error.unwrap_or_default();
        assert!(!error.contains("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY"));
        assert!(error.contains("DEEPAGENT_TEST_PLUGIN_BETA_API_KEY"));

        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY");
        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_BETA_API_KEY");
    }

    #[test]
    fn plugin_is_configured_when_all_detected_credentials_are_present() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY", "configured");
        std::env::set_var("DEEPAGENT_TEST_PLUGIN_BETA_API_KEY", "configured");

        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("credentialed-plugin");
        write_plugin(&root, "credentialed-plugin");
        std::fs::write(
            root.join("README.md"),
            "Requires DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY and DEEPAGENT_TEST_PLUGIN_BETA_API_KEY.",
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("credentialed-plugin@builtin").unwrap().unwrap();

        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert_eq!(plugin.state, PluginLifecycleState::Verified);

        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_ALPHA_API_KEY");
        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_BETA_API_KEY");
    }

    #[test]
    fn create_plugin_writes_manifest_and_lists_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = PluginService::new(roots(tmp.path()), tmp.path().join("app-data"));

        let plugin = svc
            .create_plugin(CreatePluginDraftDto {
                name: "My Helper".to_string(),
                description: Some("Demo plugin".to_string()),
                directory: None,
                category: None,
            })
            .unwrap();

        assert_eq!(plugin.id, "my-helper@personal");
        assert!(plugin.enabled);
        assert!(Path::new(plugin.manifest_path.as_deref().unwrap()).is_file());
    }

    #[test]
    fn install_from_dir_commits_complete_directory_via_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let source_v1 = tmp.path().join("source-v1");
        let source_v2 = tmp.path().join("source-v2");
        write_plugin_with_version(&source_v1, "demo", "0.1.0");
        write_plugin_with_version(&source_v2, "demo", "0.2.0");
        std::fs::write(source_v2.join("README.md"), "complete package marker").unwrap();
        std::fs::write(source_v2.join("LICENSE"), "Apache-2.0").unwrap();
        std::fs::create_dir_all(source_v2.join("assets")).unwrap();
        std::fs::write(source_v2.join("assets").join("icon.png"), b"png").unwrap();
        std::fs::create_dir_all(source_v2.join("commands")).unwrap();
        std::fs::write(
            source_v2.join("commands").join("inspect.md"),
            "---\ndescription: Inspect complete package\n---\nInspect $ARGUMENTS",
        )
        .unwrap();
        std::fs::create_dir_all(source_v2.join("scripts")).unwrap();
        std::fs::write(
            source_v2.join("scripts").join("run.sh"),
            "#!/bin/sh\nexit 0\n",
        )
        .unwrap();
        std::fs::create_dir_all(source_v2.join("agents")).unwrap();
        std::fs::write(
            source_v2.join("agents").join("reviewer.md"),
            "---\ndescription: Review changes\n---\nReview the package",
        )
        .unwrap();
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let installed = svc.install_from_dir(&source_v1).unwrap();
        assert_eq!(installed.id, "demo@personal");
        assert_eq!(installed.version.as_deref(), Some("0.1.0"));

        let updated = svc.install_from_dir(&source_v2).unwrap();

        assert_eq!(updated.id, "demo@personal");
        assert_eq!(updated.version.as_deref(), Some("0.2.0"));
        assert!(roots.personal.join("demo").join("README.md").is_file());
        assert!(roots.personal.join("demo").join("LICENSE").is_file());
        assert!(roots
            .personal
            .join("demo")
            .join("assets")
            .join("icon.png")
            .is_file());
        assert!(roots
            .personal
            .join("demo")
            .join("commands")
            .join("inspect.md")
            .is_file());
        assert!(roots
            .personal
            .join("demo")
            .join("scripts")
            .join("run.sh")
            .is_file());
        assert!(roots
            .personal
            .join("demo")
            .join("agents")
            .join("reviewer.md")
            .is_file());
        assert!(!roots.personal.join(".staging").join("demo").exists());
        let staged_entries = std::fs::read_dir(roots.personal.join(".staging"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(staged_entries, 0);
    }

    #[cfg(windows)]
    #[test]
    fn replace_plugin_dir_restores_previous_version_and_cleans_staging_after_lock_release() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("install-root");
        let destination = install_root.join("demo");
        let stage = install_root.join(".staging").join("demo");
        write_plugin_with_version(&destination, "demo", "0.1.0");
        write_plugin_with_version(&stage, "demo", "0.2.0");

        let lock_path = stage.join(".codex-plugin").join("plugin.json");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(lock);
        });

        let err = replace_plugin_dir(&stage, &destination, &install_root).unwrap_err();
        release.join().unwrap();

        assert!(err.to_string().contains("restored"));
        assert!(destination
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
        let manifest =
            std::fs::read_to_string(destination.join(".codex-plugin").join("plugin.json")).unwrap();
        assert!(manifest.contains("\"0.1.0\""));
        assert!(!manifest.contains("\"0.2.0\""));
        assert!(!stage.exists());
    }

    #[test]
    fn save_state_replaces_existing_file_without_temp_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = PluginService::new(roots(tmp.path()), tmp.path().join("app-data"));
        let mut state = PluginState::default();
        state.enabled.insert("demo@builtin".to_string(), true);
        svc.save_state(&state).unwrap();

        state.enabled.insert("demo@builtin".to_string(), false);
        state.installed.insert(
            "demo@builtin".to_string(),
            InstalledPluginState {
                version: Some("1.0.0".to_string()),
                install_path: tmp
                    .path()
                    .join("builtin")
                    .join("demo")
                    .display()
                    .to_string(),
                installed_at: "first".to_string(),
                last_updated: None,
                content_hash: None,
            },
        );
        svc.save_state(&state).unwrap();

        let loaded = svc.load_state().unwrap();
        assert_eq!(loaded.enabled.get("demo@builtin"), Some(&false));
        assert_eq!(
            loaded
                .installed
                .get("demo@builtin")
                .and_then(|item| item.version.as_deref()),
            Some("1.0.0")
        );

        let state_dir = svc.state_path.parent().unwrap();
        let leftovers = std::fs::read_dir(state_dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.contains(".tmp-"))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "state temp files should be cleaned up: {leftovers:?}"
        );
    }

    #[test]
    fn load_state_migrates_legacy_missing_version_and_camel_case_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = PluginService::new(roots(tmp.path()), tmp.path().join("app-data"));
        let state_path = svc.state_path.clone();
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(
            &state_path,
            serde_json::json!({
                "enabled": {
                    "demo@builtin": true
                },
                "installed": {
                    "demo@team": {
                        "version": "0.1.0",
                        "installPath": tmp.path().join("cache/team/demo/0.1.0").display().to_string(),
                        "installedAt": "2026-07-22T10:00:00Z",
                        "lastUpdated": "2026-07-22T11:00:00Z"
                    }
                },
                "marketplaces": {
                    "team": {
                        "source": tmp.path().join("team-marketplace").display().to_string(),
                        "gitRef": "main",
                        "sparsePath": "plugins",
                        "installLocation": tmp.path().join("marketplaces/team").display().to_string(),
                        "manifestPath": tmp.path().join("marketplaces/team/marketplace.json").display().to_string(),
                        "sourceRoot": tmp.path().join("marketplaces/team").display().to_string(),
                        "lastUpdated": "2026-07-22T12:00:00Z"
                    }
                },
                "healthChecks": {
                    "demo@team": {
                        "status": "needs_authorization",
                        "checkedAt": "2026-07-22T13:00:00Z",
                        "error": "needs OAuth authorization"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let state = svc.load_state().unwrap();

        assert_eq!(state.version, PLUGIN_STATE_SCHEMA_VERSION);
        assert_eq!(state.enabled.get("demo@builtin"), Some(&true));
        let installed = state.installed.get("demo@team").unwrap();
        assert_eq!(installed.version.as_deref(), Some("0.1.0"));
        assert_eq!(installed.installed_at, "2026-07-22T10:00:00Z");
        assert_eq!(installed.content_hash, None);
        assert_eq!(
            installed.last_updated.as_deref(),
            Some("2026-07-22T11:00:00Z")
        );
        let marketplace = state.marketplaces.get("team").unwrap();
        assert_eq!(marketplace.git_ref.as_deref(), Some("main"));
        assert_eq!(marketplace.sparse_path.as_deref(), Some("plugins"));
        let health = state.health_checks.get("demo@team").unwrap();
        assert_eq!(health.status, PluginHealthStatus::NeedsAuthorization);
        assert_eq!(health.checked_at, "2026-07-22T13:00:00Z");
        assert_eq!(health.error.as_deref(), Some("needs OAuth authorization"));

        svc.save_state(&state).unwrap();
        let rewritten = std::fs::read_to_string(&state_path).unwrap();
        assert!(rewritten.contains("\"version\": 1"));
        assert!(rewritten.contains("install_path"));
        assert!(rewritten.contains("installed_at"));
        assert!(rewritten.contains("health_checks"));
    }

    #[test]
    fn load_state_rejects_future_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = PluginService::new(roots(tmp.path()), tmp.path().join("app-data"));
        let state_path = svc.state_path.clone();
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(&state_path, r#"{"version":999,"enabled":{}}"#).unwrap();

        let err = svc.load_state().unwrap_err();

        assert!(err
            .to_string()
            .contains("unsupported plugin state version 999"));
    }

    #[test]
    fn plugin_list_cache_refreshes_when_manifest_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let plugin_root = roots.builtin.join("demo");
        write_plugin_with_version(&plugin_root, "demo", "0.1.0");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let initial = svc.read("demo@builtin").unwrap().unwrap();
        assert_eq!(initial.version.as_deref(), Some("0.1.0"));

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_plugin_with_version(&plugin_root, "demo", "0.2.0");

        let refreshed = svc.read("demo@builtin").unwrap().unwrap();
        assert_eq!(refreshed.version.as_deref(), Some("0.2.0"));
    }

    /// Agent Plugins §9.1 puts two obligations on `PLUGIN_DATA`: the client must
    /// create the directory before launching a plugin subprocess, and must
    /// preserve its contents across plugin updates.
    ///
    /// The creation half regressed silently before this test existed:
    /// `prepare_runtime_payload` only creates the directory for plugins shipping
    /// a `runtime.zip`, so every other plugin was handed a `PLUGIN_DATA` path
    /// that did not exist.
    #[test]
    fn plugin_data_dir_is_created_and_survives_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let source = tmp.path().join("source").join("demo");
        write_plugin_with_version(&source, "demo", "0.1.0");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let installed = svc.install_from_dir(&source).unwrap();
        let data_dir = svc.data_root.join(sanitize_file_name(&installed.id));

        // The projection is what hands PLUGIN_DATA to subprocesses, so the
        // directory must exist once it has run — with no runtime.zip involved.
        svc.runtime_projection().unwrap();
        assert!(
            data_dir.is_dir(),
            "PLUGIN_DATA must exist before a subprocess is launched: {}",
            data_dir.display()
        );

        // Plugin state that must outlive an update.
        std::fs::write(data_dir.join("state.json"), r#"{"runs":7}"#).unwrap();

        write_plugin_with_version(&source, "demo", "0.2.0");
        let updated = svc.install_from_dir(&source).unwrap();
        assert_eq!(updated.version.as_deref(), Some("0.2.0"));
        assert_eq!(updated.id, installed.id, "the id must be stable");

        svc.runtime_projection().unwrap();
        assert_eq!(
            std::fs::read_to_string(data_dir.join("state.json")).unwrap(),
            r#"{"runs":7}"#,
            "§9.1 requires PLUGIN_DATA contents to survive a plugin update"
        );
    }

    #[test]
    fn runtime_projection_respects_enable_disable_state() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let initial = svc.runtime_projection().unwrap();
        assert_eq!(
            initial.skill_roots,
            vec![roots.builtin.join("demo").join("skills")]
        );

        let disabled = svc.set_enabled("demo@builtin", false).unwrap();
        assert!(!disabled.enabled);
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
    }

    #[test]
    fn policy_blocked_plugins_do_not_project_runtime_capabilities() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin(&roots.personal.join("reserved"), "builtin");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let listed = svc.read("builtin@personal").unwrap().unwrap();

        assert!(listed.installed);
        assert!(!listed.available);
        assert!(!listed.enabled);
        assert!(listed
            .errors
            .iter()
            .any(|error| error.kind == "reserved-name"));
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
    }

    #[test]
    fn runtime_projection_projects_apps_and_output_styles() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_app_component_and_output_style(
            &roots.builtin.join("demo"),
            "demo",
            "builtin:browser",
        );
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let plugin = svc.read("demo@builtin").unwrap().unwrap();
        assert_eq!(plugin.app_count, 1);
        assert_eq!(plugin.output_style_count, 1);

        let apps = svc.list_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].plugin_id, "demo@builtin");
        assert_eq!(apps[0].component, "builtin:browser");

        let styles = svc.list_output_styles().unwrap();
        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].plugin_id, "demo@builtin");
        assert_eq!(styles[0].name, "demo:concise");
    }

    #[test]
    fn list_apps_filters_unregistered_builtin_components() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_app_component_and_output_style(
            &roots.builtin.join("known"),
            "known",
            "builtin:browser",
        );
        write_plugin_with_app_component_and_output_style(
            &roots.builtin.join("unknown"),
            "unknown",
            "builtin:computer-use",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let apps = svc.list_apps().unwrap();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].plugin_id, "known@builtin");
        let unknown = svc.read("unknown@builtin").unwrap().unwrap();
        assert_eq!(unknown.health_status, PluginHealthStatus::Incomplete);
        assert!(unknown
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("builtin:computer-use"));
    }

    #[test]
    fn runtime_projection_cache_refreshes_when_watched_runtime_file_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_app_and_output_style(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        let style_path = roots
            .builtin
            .join("demo")
            .join("output-styles")
            .join("concise.md");

        let initial = svc.list_output_styles().unwrap();
        assert!(initial[0].prompt.contains("Keep replies short"));

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            &style_path,
            "# Concise demo\n\nUse the edited runtime style.",
        )
        .unwrap();

        let refreshed = svc.list_output_styles().unwrap();
        assert!(refreshed[0].prompt.contains("edited runtime style"));
    }

    #[test]
    fn runtime_projection_projects_command_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_command(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let plugin = svc.read("demo@builtin").unwrap().unwrap();
        assert_eq!(plugin.command_count, 1);

        let projection = svc.runtime_projection().unwrap();
        assert_eq!(projection.command_roots.len(), 1);
        assert_eq!(projection.command_roots[0].plugin_id, "demo@builtin");
        assert_eq!(
            projection.command_roots[0].path,
            roots.builtin.join("demo").join("commands")
        );
    }

    #[test]
    fn workspace_plugins_are_inert_until_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let mut roots = roots(tmp.path());
        let workspace = tmp.path().join("workspace");
        roots.workspace = Some(workspace.clone());
        write_plugin(&workspace.join("team"), "team");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let listed = svc.read("team@workspace").unwrap().unwrap();
        assert!(!listed.enabled);
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());

        let enabled = svc.set_enabled("team@workspace", true).unwrap();
        assert!(enabled.enabled);
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![workspace.join("team").join("skills")]
        );
    }

    #[test]
    fn session_plugins_are_enabled_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let mut roots = roots(tmp.path());
        let session = tmp.path().join("session");
        roots.session = vec![session.clone()];
        write_plugin(&session.join("temp"), "temp");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let listed = svc.read("temp@session").unwrap().unwrap();
        assert!(listed.enabled);
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![session.join("temp").join("skills")]
        );
    }

    #[test]
    fn dependency_missing_demotes_plugin_from_runtime_projection() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_dependencies(&roots.personal.join("worker"), "worker", &["helper"]);
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let listed = svc.read("worker@personal").unwrap().unwrap();
        assert!(!listed.enabled);
        assert!(listed
            .errors
            .iter()
            .any(|err| err.kind == "dependency-unsatisfied"
                && err.message.contains("helper@personal is not-found")));
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
    }

    #[test]
    fn disabled_dependency_demotes_dependent_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_dependencies(&roots.personal.join("worker"), "worker", &["helper"]);
        write_plugin(&roots.personal.join("helper"), "helper");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        svc.set_enabled("helper@personal", false).unwrap();
        let listed = svc.read("worker@personal").unwrap().unwrap();

        assert!(!listed.enabled);
        assert!(listed
            .errors
            .iter()
            .any(|err| err.kind == "dependency-unsatisfied"
                && err.message.contains("helper@personal is not-enabled")));
    }

    #[test]
    fn dto_reports_enabled_reverse_dependents() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_dependencies(&roots.personal.join("worker"), "worker", &["helper"]);
        write_plugin(&roots.personal.join("helper"), "helper");
        write_plugin_with_dependencies(&roots.personal.join("disabled"), "disabled", &["helper"]);
        let svc = PluginService::new(roots, tmp.path().join("app-data"));
        svc.set_enabled("disabled@personal", false).unwrap();

        let helper = svc.read("helper@personal").unwrap().unwrap();

        assert_eq!(helper.required_by.len(), 1);
        assert_eq!(helper.required_by[0].id, "worker@personal");
        assert_eq!(helper.required_by[0].display_name, "worker");
    }

    #[test]
    fn dependency_cycle_demotes_cycle_members() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_dependencies(&roots.personal.join("alpha"), "alpha", &["beta"]);
        write_plugin_with_dependencies(&roots.personal.join("beta"), "beta", &["alpha"]);
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let alpha = svc.read("alpha@personal").unwrap().unwrap();
        let beta = svc.read("beta@personal").unwrap().unwrap();

        assert!(!alpha.enabled);
        assert!(!beta.enabled);
        assert!(alpha
            .errors
            .iter()
            .any(|err| err.kind == "dependency-cycle"));
        assert!(beta.errors.iter().any(|err| err.kind == "dependency-cycle"));
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
    }

    #[test]
    fn local_marketplace_entry_installs_to_versioned_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_source, "demo");
        std::fs::write(
            plugin_source.join(".codex-plugin").join("plugin.json"),
            serde_json::json!({
                "name": "demo",
                "version": "0.1.0",
                "description": "Manifest description",
                "author": { "name": "Manifest Author" },
                "skills": "skills"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "displayName": "Demo Catalog",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "author": "Catalog Author",
                  "source": { "source": "local", "path": "./plugins/demo" },
                  "category": "Developer Tools"
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        let marketplace = svc
            .add_marketplace(AddPluginMarketplaceDto {
                name: Some("team".to_string()),
                source: marketplace_root.display().to_string(),
                git_ref: None,
                sparse_path: None,
            })
            .unwrap();
        assert_eq!(marketplace.name, "team");

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "demo");
        assert!(entries[0].installable);
        assert!(!entries[0].installed);
        assert!(
            svc.scan_marketplace_plugin("team", "demo")
                .unwrap()
                .manifest_ok
        );

        let installed = svc.install_from_marketplace("team", "demo").unwrap();
        assert_eq!(installed.id, "demo@team");
        assert_eq!(installed.origin, "marketplace");
        assert_eq!(installed.display_name, "Demo Catalog");
        assert_eq!(installed.description, "Demo marketplace plugin");
        assert_eq!(installed.developer.as_deref(), Some("Catalog Author"));
        assert_eq!(installed.category.as_deref(), Some("Developer Tools"));
        assert!(installed.enabled);

        let cached = roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0");
        assert!(cached.join(".codex-plugin").join("plugin.json").is_file());
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![cached.join("skills")]
        );
    }

    #[test]
    fn prepared_marketplace_install_commits_after_user_confirmation() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin_with_version(&plugin_source, "demo", "0.1.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let prepared = svc.prepare_plugin_install("team", "demo", false).unwrap();

        assert_eq!(prepared.plugin_id, "demo@team");
        assert!(prepared.scan_report.manifest_ok);
        assert!(prepared.content_hash.starts_with("sha256:"));
        assert_eq!(
            prepared.runtime_inspection.execution_kind,
            PluginExecutionKind::SkillOnly
        );
        assert!(Path::new(&prepared.staging_path).is_dir());
        assert!(svc.read("demo@team").unwrap().is_none());

        let installed = svc.commit_plugin_install(&prepared.token).unwrap();

        assert_eq!(installed.id, "demo@team");
        assert_eq!(installed.version.as_deref(), Some("0.1.0"));
        assert_eq!(
            installed.content_hash.as_deref(),
            Some(prepared.content_hash.as_str())
        );
        let state = svc.load_state().unwrap();
        assert_eq!(
            state
                .installed
                .get("demo@team")
                .and_then(|installed| installed.content_hash.as_deref()),
            Some(prepared.content_hash.as_str())
        );
        assert!(!Path::new(&prepared.staging_path).exists());
        assert!(roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0")
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
    }

    #[test]
    fn prepared_marketplace_commit_rejects_changed_staged_content_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin_with_version(&plugin_source, "demo", "0.1.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let prepared = svc.prepare_plugin_install("team", "demo", false).unwrap();
        std::fs::write(
            Path::new(&prepared.plugin_root).join("scripts-added-after-prepare.sh"),
            "echo tampered",
        )
        .unwrap();

        let err = svc.commit_plugin_install(&prepared.token).unwrap_err();

        assert!(err.to_string().contains("content hash changed"));
        assert!(!Path::new(&prepared.staging_path).exists());
        assert!(!roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0")
            .exists());
        let state = svc.load_state().unwrap();
        assert!(!state.installed.contains_key("demo@team"));
    }

    #[test]
    fn prepared_marketplace_install_cancel_removes_staging_without_installing() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        write_plugin_with_version(
            &marketplace_root.join("plugins").join("demo"),
            "demo",
            "0.1.0",
        );
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots, tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();
        let prepared = svc.prepare_plugin_install("team", "demo", false).unwrap();

        assert!(svc.cancel_plugin_install(&prepared.token).unwrap());

        assert!(!Path::new(&prepared.staging_path).exists());
        assert!(svc.read("demo@team").unwrap().is_none());
        assert!(svc.commit_plugin_install(&prepared.token).is_err());
    }

    #[test]
    fn prepared_marketplace_commit_failure_preserves_installed_version() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin_with_version(&plugin_source, "demo", "0.1.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();
        let installed = svc.install_from_marketplace("team", "demo").unwrap();
        assert_eq!(installed.version.as_deref(), Some("0.1.0"));

        write_plugin_with_version(&plugin_source, "demo", "0.2.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.2.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();
        svc.refresh_marketplace("team").unwrap();
        let prepared = svc.prepare_plugin_install("team", "demo", false).unwrap();
        std::fs::write(
            Path::new(&prepared.plugin_root)
                .join(".codex-plugin")
                .join("plugin.json"),
            "{not valid json",
        )
        .unwrap();

        let err = svc.commit_plugin_install(&prepared.token).unwrap_err();

        assert!(err.to_string().contains("commit"));
        assert!(!Path::new(&prepared.staging_path).exists());
        let current = svc.read("demo@team").unwrap().unwrap();
        assert_eq!(current.version.as_deref(), Some("0.1.0"));
        assert!(roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0")
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
        assert!(!roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.2.0")
            .exists());
    }

    #[test]
    fn remove_marketplace_cleans_cached_plugins_state_and_data() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_source, "demo");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();
        let installed = svc.install_from_marketplace("team", "demo").unwrap();
        let data_dir = svc.data_root.join(sanitize_file_name(&installed.id));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("state.json"), "{}").unwrap();

        let cached = roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0");
        assert!(cached.join(".codex-plugin").join("plugin.json").is_file());
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![cached.join("skills")]
        );

        assert!(svc.remove_marketplace("team").unwrap());

        assert!(svc.list_marketplaces().unwrap().is_empty());
        assert!(svc.list_marketplace_entries().unwrap().is_empty());
        assert!(svc.read("demo@team").unwrap().is_none());
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
        assert!(!roots.marketplaces.join("team").exists());
        assert!(!roots.marketplace_cache.join("team").exists());
        assert!(!data_dir.exists());

        let state = svc.load_state().unwrap();
        assert!(!state.enabled.contains_key("demo@team"));
        assert!(!state.installed.contains_key("demo@team"));
    }

    #[test]
    fn uninstall_marketplace_plugin_orphans_cached_version_until_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        write_plugin(&marketplace_root.join("plugins").join("demo"), "demo");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();
        let installed = svc.install_from_marketplace("team", "demo").unwrap();
        let cached = roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0");
        assert_eq!(installed.id, "demo@team");
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![cached.join("skills")]
        );

        assert!(svc.uninstall("demo@team", false).unwrap());

        assert!(svc.read("demo@team").unwrap().is_none());
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
        assert!(cached.join(PLUGIN_CACHE_ORPHAN_MARKER).is_file());
        assert_eq!(svc.cleanup_orphaned_cache_older_than(0).unwrap(), 1);
        assert!(!cached.exists());
    }

    #[test]
    fn marketplace_plugin_update_refreshes_versioned_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin_with_version(&plugin_source, "demo", "0.1.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();
        let installed = svc.install_from_marketplace("team", "demo").unwrap();
        assert!(!installed.update_available);
        svc.set_enabled("demo@team", false).unwrap();
        let original_state = svc.load_state().unwrap();
        let original_installed_at = original_state.installed["demo@team"].installed_at.clone();

        write_plugin_with_version(&plugin_source, "demo", "0.2.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.2.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();
        svc.refresh_marketplace("team").unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert!(entries[0].update_available);
        assert!(svc.read("demo@team").unwrap().unwrap().update_available);

        let updated = svc.update_plugin("demo@team").unwrap();
        assert_eq!(updated.version.as_deref(), Some("0.2.0"));
        assert!(!updated.enabled, "update should preserve disabled state");
        assert!(!updated.update_available);

        let state = svc.load_state().unwrap();
        let installed_state = &state.installed["demo@team"];
        assert_eq!(installed_state.version.as_deref(), Some("0.2.0"));
        assert_eq!(installed_state.installed_at, original_installed_at);
        assert!(installed_state.last_updated.is_some());
        let old_version = roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.1.0");
        let new_version = roots
            .marketplace_cache
            .join("team")
            .join("demo")
            .join("0.2.0");
        assert!(old_version.join(PLUGIN_CACHE_ORPHAN_MARKER).is_file());
        assert!(new_version
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
        assert!(svc.runtime_projection().unwrap().skill_roots.is_empty());
        assert_eq!(
            svc.list()
                .unwrap()
                .into_iter()
                .filter(|plugin| plugin.id == "demo@team")
                .count(),
            1
        );
        svc.set_enabled("demo@team", true).unwrap();
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![new_version.join("skills")]
        );

        assert_eq!(svc.cleanup_orphaned_cache_older_than(0).unwrap(), 1);
        assert!(!old_version.exists());
        assert!(new_version
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
    }

    #[test]
    fn remote_marketplace_entries_are_installable_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("remote-marketplace");
        std::fs::create_dir_all(&marketplace_root).unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "git-plugin",
                  "source": {
                    "source": "git",
                    "url": "https://example.com/team/plugins.git",
                    "path": "plugins/git-plugin"
                  }
                },
                {
                  "name": "npm-plugin",
                  "source": {
                    "source": "npm",
                    "package": "@scope/plugin",
                    "version": "1.2.3"
                  }
                },
                {
                  "name": "zip-plugin",
                  "source": {
                    "source": "zip-url",
                    "url": "https://example.com/plugin.zip",
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots, tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("remote".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| entry.installable));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "git-plugin" && entry.source_kind == "git"));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "npm-plugin" && entry.source_kind == "npm"));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "zip-plugin" && entry.source_kind == "zip-url"));
    }

    #[test]
    fn git_materialization_verifies_pinned_head_commit() {
        if !git_available() {
            eprintln!("skipping: git is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let staging = tmp.path().join("staging");
        let (first, _second) = write_git_plugin_repo(&source);

        let plugin_root = materialize_git_plugin(
            &source.display().to_string(),
            None,
            Some("main"),
            Some(&first[..12]),
            &staging,
        )
        .unwrap();

        let manifest = load_plugin_manifest(&plugin_root).unwrap().unwrap();
        assert_eq!(manifest.name, "git-demo");
        assert_eq!(manifest.version.as_deref(), Some("0.1.0"));
        let head = run_git(&staging.join("repo"), &["rev-parse", "HEAD"]);
        assert!(head.trim().starts_with(&first));
    }

    #[test]
    fn git_head_verification_rejects_unexpected_commit() {
        if !git_available() {
            eprintln!("skipping: git is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let (first, second) = write_git_plugin_repo(&source);
        assert_ne!(first, second);

        let err = verify_git_head_matches_sha(&source, &first[..12]).unwrap_err();

        assert!(err.to_string().contains("checkout verification failed"));
        assert!(err.to_string().contains(&second[..12]));
    }

    #[test]
    fn github_archive_fallback_installs_complete_directory_payload() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let archive = tmp.path().join("fixtures").join("archive-plugin.zip");
        let old_skip_clone = std::env::var_os("DEEPAGENT_TEST_GITHUB_SKIP_CLONE");
        let old_download_source = std::env::var_os("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE");
        std::env::set_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE", "1");
        std::env::set_var(
            "DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE",
            archive.display().to_string(),
        );

        write_plugin_zip_with_payload(
            &archive,
            "deepseek-ai-archive-demo-0123456",
            "archive-demo",
            "0.1.0",
            &[
                ("scripts/run.sh", b"#!/usr/bin/env sh\necho ok\n"),
                ("assets/logo.txt", b"asset payload"),
                ("README.md", b"# Archive Demo\n"),
                ("LICENSE", b"MIT\n"),
                ("tests/smoke.txt", b"smoke"),
            ],
        );
        let marketplace_root = tmp.path().join("remote-marketplace");
        std::fs::create_dir_all(&marketplace_root).unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            format!(
                r#"{{
                  "name": "remote",
                  "plugins": [
                    {{
                      "name": "archive-demo",
                      "version": "0.1.0",
                      "source": {{
                        "source": "github",
                        "repo": "deepseek-ai/archive-demo",
                        "sha": "{sha}"
                      }}
                    }}
                  ]
                }}"#
            ),
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("remote".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let prepared = svc
            .prepare_plugin_install("remote", "archive-demo", false)
            .unwrap();
        let prepared_root = PathBuf::from(&prepared.plugin_root);
        assert!(prepared_root.join("scripts").join("run.sh").is_file());
        assert!(prepared_root.join("assets").join("logo.txt").is_file());
        assert!(prepared_root.join("README.md").is_file());
        assert!(prepared_root.join("LICENSE").is_file());
        assert!(prepared_root.join("tests").join("smoke.txt").is_file());

        let installed = svc.commit_plugin_install(&prepared.token).unwrap();
        assert_eq!(installed.id, "archive-demo@remote");
        let cached = roots
            .marketplace_cache
            .join("remote")
            .join("archive-demo")
            .join("0.1.0");
        assert!(cached.join("scripts").join("run.sh").is_file());
        assert!(cached.join("assets").join("logo.txt").is_file());
        assert!(cached.join("README.md").is_file());
        assert!(cached.join("LICENSE").is_file());
        assert!(cached.join("tests").join("smoke.txt").is_file());

        let snapshot =
            std::fs::read_to_string(roots.marketplaces.join("remote").join("marketplace.json"))
                .unwrap();
        assert!(snapshot.contains("deepseek-ai/archive-demo"));
        assert!(!snapshot.contains("gh.llkk.cc"));
        assert!(!snapshot.contains("gh-proxy.com"));

        match old_skip_clone {
            Some(value) => std::env::set_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE", value),
            None => std::env::remove_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE"),
        }
        match old_download_source {
            Some(path) => std::env::set_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE", path),
            None => std::env::remove_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE"),
        }
    }

    #[test]
    fn github_archive_fallback_failure_cleans_staging_without_installing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        let bad_archive = tmp.path().join("fixtures").join("bad-archive.zip");
        let old_skip_clone = std::env::var_os("DEEPAGENT_TEST_GITHUB_SKIP_CLONE");
        let old_download_source = std::env::var_os("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE");
        if let Some(parent) = bad_archive.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&bad_archive, b"not a zip").unwrap();
        std::env::set_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE", "1");
        std::env::set_var(
            "DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE",
            bad_archive.display().to_string(),
        );

        let marketplace_root = tmp.path().join("remote-marketplace");
        std::fs::create_dir_all(&marketplace_root).unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            format!(
                r#"{{
                  "name": "remote",
                  "plugins": [
                    {{
                      "name": "archive-demo",
                      "version": "0.1.0",
                      "source": {{
                        "source": "github",
                        "repo": "deepseek-ai/archive-demo",
                        "sha": "{sha}"
                      }}
                    }}
                  ]
                }}"#
            ),
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("remote".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let err = svc
            .prepare_plugin_install("remote", "archive-demo", false)
            .unwrap_err();

        assert!(err.to_string().contains("read zip archive"));
        assert!(!roots
            .marketplace_cache
            .join("remote")
            .join("archive-demo")
            .join("0.1.0")
            .exists());
        let staging_root = roots.marketplace_cache.join(".staging");
        assert!(
            std::fs::read_dir(&staging_root)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
            "failed GitHub archive materialization must not leave staging behind"
        );
        let state = svc.load_state().unwrap();
        assert!(!state.installed.contains_key("archive-demo@remote"));
        assert!(!state.enabled.contains_key("archive-demo@remote"));

        match old_skip_clone {
            Some(value) => std::env::set_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE", value),
            None => std::env::remove_var("DEEPAGENT_TEST_GITHUB_SKIP_CLONE"),
        }
        match old_download_source {
            Some(path) => std::env::set_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE", path),
            None => std::env::remove_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE"),
        }
    }

    #[test]
    fn dsh_github_topic_refresh_writes_metadata_snapshot_without_plugin_download() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let fixtures = tmp.path().join("github-api");
        std::fs::create_dir_all(&fixtures).unwrap();
        let old_fixtures = std::env::var_os("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR");
        std::env::set_var(
            "DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR",
            fixtures.display().to_string(),
        );

        let search_endpoint =
            "/search/repositories?q=topic%3Adsh-plugin&sort=updated&order=desc&per_page=100";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(search_endpoint))),
            r#"{
              "items": [
                {
                  "name": "dsh-demo",
                  "full_name": "deepseek-ai/dsh-demo",
                  "description": "Demo DSH plugin",
                  "default_branch": "main",
                  "license": { "spdx_id": "MIT" }
                }
              ]
            }"#,
        )
        .unwrap();
        let branch_endpoint = "/repos/deepseek-ai/dsh-demo/branches/main";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(branch_endpoint))),
            r#"{
              "commit": {
                "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        let marketplace = svc
            .add_marketplace(AddPluginMarketplaceDto {
                name: Some("dsh".to_string()),
                source: "https://github.com/topics/dsh-plugin".to_string(),
                git_ref: None,
                sparse_path: None,
            })
            .unwrap();

        assert_eq!(marketplace.name, "dsh");
        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.marketplace, "dsh");
        assert_eq!(entry.name, "dsh-demo");
        assert_eq!(entry.display_name, "dsh-demo");
        assert_eq!(entry.description, "Demo DSH plugin (license: MIT)");
        assert_eq!(entry.version.as_deref(), Some("git-aaaaaaaaaaaa"));
        assert_eq!(entry.source_kind, "github");
        assert!(entry.source.contains("deepseek-ai/dsh-demo@main"));
        assert!(entry.installable);
        assert!(!entry.installed);

        let snapshot =
            std::fs::read_to_string(roots.marketplaces.join("dsh").join("marketplace.json"))
                .unwrap();
        assert!(snapshot.contains(r#""repo": "deepseek-ai/dsh-demo""#));
        assert!(snapshot.contains(r#""sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa""#));
        assert!(!snapshot.contains("gh.llkk.cc"));
        assert!(!snapshot.contains("gh-proxy.com"));
        assert!(!roots
            .marketplace_cache
            .join("dsh")
            .join("dsh-demo")
            .exists());

        match old_fixtures {
            Some(value) => std::env::set_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR", value),
            None => std::env::remove_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR"),
        }
    }

    #[test]
    fn dsh_github_topic_refresh_prefers_real_manifest_metadata() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let fixtures = tmp.path().join("github-api");
        std::fs::create_dir_all(&fixtures).unwrap();
        let old_fixtures = std::env::var_os("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR");
        std::env::set_var(
            "DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR",
            fixtures.display().to_string(),
        );

        let search_endpoint =
            "/search/repositories?q=topic%3Adsh-plugin&sort=updated&order=desc&per_page=100";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(search_endpoint))),
            r#"{
              "items": [
                {
                  "name": "repo-shell",
                  "full_name": "deepseek-ai/repo-shell",
                  "description": "Repo fallback description",
                  "default_branch": "main",
                  "license": { "spdx_id": "MIT" }
                }
              ]
            }"#,
        )
        .unwrap();
        let branch_endpoint = "/repos/deepseek-ai/repo-shell/branches/main";
        let commit = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(branch_endpoint))),
            format!(r#"{{ "commit": {{ "sha": "{commit}" }} }}"#),
        )
        .unwrap();
        write_github_contents_manifest_fixture(
            &fixtures,
            "deepseek-ai/repo-shell",
            ".codex-plugin/plugin.json",
            commit,
            serde_json::json!({
                "name": "manifest-plugin",
                "version": "2.3.4",
                "description": "Manifest description",
                "license": "Apache-2.0",
                "author": { "name": "Manifest Author" },
                "interface": {
                    "displayName": "Manifest Plugin",
                    "category": "Science"
                },
                "skills": "skills"
            }),
        );

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("dsh".to_string()),
            source: "https://github.com/topics/dsh-plugin".to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "manifest-plugin");
        assert_eq!(entry.display_name, "Manifest Plugin");
        assert_eq!(
            entry.description,
            "Manifest description (license: Apache-2.0)"
        );
        assert_eq!(entry.version.as_deref(), Some("2.3.4"));
        assert_eq!(entry.category.as_deref(), Some("Science"));
        assert_eq!(entry.source_kind, "github");
        assert!(entry.installable);

        let snapshot =
            std::fs::read_to_string(roots.marketplaces.join("dsh").join("marketplace.json"))
                .unwrap();
        assert!(snapshot.contains(r#""name": "manifest-plugin""#));
        assert!(snapshot.contains(r#""manifestPath": ".codex-plugin/plugin.json""#));
        assert!(snapshot.contains(r#""sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb""#));
        assert!(!snapshot.contains("gh.llkk.cc"));
        assert!(!snapshot.contains("gh-proxy.com"));

        match old_fixtures {
            Some(value) => std::env::set_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR", value),
            None => std::env::remove_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR"),
        }
    }

    #[test]
    fn dsh_github_topic_refresh_failure_cleans_scratch_and_state() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let fixtures = tmp.path().join("github-api");
        std::fs::create_dir_all(&fixtures).unwrap();
        let old_fixtures = std::env::var_os("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR");
        std::env::set_var(
            "DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR",
            fixtures.display().to_string(),
        );

        let search_endpoint =
            "/search/repositories?q=topic%3Adsh-plugin&sort=updated&order=desc&per_page=100";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(search_endpoint))),
            r#"{
              "items": [
                {
                  "name": "dsh-demo",
                  "full_name": "deepseek-ai/dsh-demo",
                  "default_branch": "main"
                }
              ]
            }"#,
        )
        .unwrap();
        let branch_endpoint = "/repos/deepseek-ai/dsh-demo/branches/main";
        std::fs::write(
            fixtures.join(format!("{}.json", github_api_fixture_name(branch_endpoint))),
            r#"{
              "commit": {
                "sha": "not-a-commit"
              }
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        let err = svc
            .add_marketplace(AddPluginMarketplaceDto {
                name: Some("dsh".to_string()),
                source: "https://github.com/topics/dsh-plugin".to_string(),
                git_ref: None,
                sparse_path: None,
            })
            .unwrap_err();

        assert!(err.to_string().contains("valid commit sha"));
        assert!(svc.list_marketplaces().unwrap().is_empty());
        assert!(!roots
            .marketplaces
            .join("dsh")
            .join("marketplace.json")
            .exists());
        assert!(!roots.marketplaces.join("dsh").join(".github-api").exists());

        match old_fixtures {
            Some(value) => std::env::set_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR", value),
            None => std::env::remove_var("DEEPAGENT_TEST_GITHUB_API_FIXTURE_DIR"),
        }
    }

    #[test]
    fn github_download_candidates_include_domestic_mirrors_after_official_url() {
        let candidates = github_download_url_candidates(
            "https://github.com/deepseek-ai/dsh-demo/archive/main.zip",
        );

        assert_eq!(
            candidates,
            vec![
                "https://github.com/deepseek-ai/dsh-demo/archive/main.zip",
                "https://gh.llkk.cc/https://github.com/deepseek-ai/dsh-demo/archive/main.zip",
                "https://gh-proxy.com/https://github.com/deepseek-ai/dsh-demo/archive/main.zip",
            ]
        );
    }

    #[test]
    fn github_topic_source_rejects_unsafe_topic_names() {
        assert_eq!(
            github_topic_from_source("https://github.com/topics/dsh-plugin"),
            Some("dsh-plugin".to_string())
        );
        assert_eq!(
            github_topic_from_source("github-topic:dsh-plugin"),
            Some("dsh-plugin".to_string())
        );
        assert_eq!(
            github_topic_from_source("https://github.com/topics/../secret"),
            None
        );
        assert_eq!(
            github_topic_from_source("https://github.com/topics/DSH-Plugin"),
            None
        );
    }

    #[test]
    fn zip_url_marketplace_entry_requires_sha256_to_install() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("remote-marketplace");
        std::fs::create_dir_all(&marketplace_root).unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "zip-plugin",
                  "source": {
                    "source": "zip-url",
                    "url": "https://example.com/plugin.zip"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots, tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("remote".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_kind, "zip-url");
        assert!(!entries[0].installable);
        assert!(entries[0].source.contains("missing sha256"));

        let err = svc
            .scan_marketplace_plugin("remote", "zip-plugin")
            .unwrap_err();
        assert!(err.to_string().contains("unsupported source 'zip-url'"));
    }

    #[test]
    fn zip_url_checksum_failure_cleans_staging_without_installing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let archive = tmp.path().join("fixtures").join("zip-plugin.zip");
        let old_download_source = std::env::var_os("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE");

        write_plugin_zip(&archive, "zip-plugin", "zip-plugin", "0.1.0");
        std::env::set_var(
            "DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE",
            archive.display().to_string(),
        );

        let marketplace_root = tmp.path().join("remote-marketplace");
        std::fs::create_dir_all(&marketplace_root).unwrap();
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "zip-plugin",
                  "version": "0.1.0",
                  "source": {
                    "source": "zip-url",
                    "url": "https://example.com/zip-plugin.zip",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("remote".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let err = svc
            .prepare_plugin_install("remote", "zip-plugin", false)
            .unwrap_err();

        assert!(err.to_string().contains("checksum mismatch"));
        assert!(!roots
            .marketplace_cache
            .join("remote")
            .join("zip-plugin")
            .join("0.1.0")
            .exists());
        let staging_root = roots.marketplace_cache.join(".staging");
        assert!(
            std::fs::read_dir(&staging_root)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
            "failed zip-url materialization must not leave staging behind"
        );
        let state = svc.load_state().unwrap();
        assert!(!state.installed.contains_key("zip-plugin@remote"));
        assert!(!state.enabled.contains_key("zip-plugin@remote"));

        match old_download_source {
            Some(path) => std::env::set_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE", path),
            None => std::env::remove_var("DEEPAGENT_TEST_PLUGIN_DOWNLOAD_SOURCE"),
        }
    }

    #[test]
    fn marketplace_policy_not_available_blocks_install() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_source, "demo");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" },
                  "policy": { "installation": "NOT_AVAILABLE" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].installable);
        assert!(entries[0]
            .install_block_reason
            .as_deref()
            .unwrap()
            .contains("not available for install"));
        assert_eq!(
            entries[0].policy_installation.as_deref(),
            Some("NOT_AVAILABLE")
        );

        let scan_err = svc.scan_marketplace_plugin("team", "demo").unwrap_err();
        assert!(scan_err.to_string().contains("not available for install"));
        let install_err = svc.install_from_marketplace("team", "demo").unwrap_err();
        assert!(install_err
            .to_string()
            .contains("not available for install"));
        assert!(!roots.marketplace_cache.join("team").join("demo").exists());
        assert!(svc.read("demo@team").unwrap().is_none());
    }

    #[test]
    fn marketplace_authentication_policy_requires_confirmation_for_install_and_update() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin_with_version(&plugin_source, "demo", "0.1.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" },
                  "policy": { "authentication": "ON_INSTALL" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let entries = svc.list_marketplace_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].installable);
        assert!(entries[0].authentication_required);
        assert!(entries[0]
            .authentication_hint
            .as_deref()
            .unwrap()
            .contains("authentication confirmation required"));
        assert!(svc.scan_marketplace_plugin("team", "demo").is_ok());

        let install_err = svc.install_from_marketplace("team", "demo").unwrap_err();
        assert!(install_err
            .to_string()
            .contains("requires authentication confirmation"));
        assert!(!roots.marketplace_cache.join("team").join("demo").exists());

        let installed = svc
            .install_from_marketplace_with_auth("team", "demo", true)
            .unwrap();
        assert_eq!(installed.id, "demo@team");
        assert_eq!(installed.version.as_deref(), Some("0.1.0"));

        write_plugin_with_version(&plugin_source, "demo", "0.2.0");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.2.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" },
                  "policy": { "authentication": "ON_INSTALL" }
                }
              ]
            }"#,
        )
        .unwrap();
        svc.refresh_marketplace("team").unwrap();
        assert!(svc.read("demo@team").unwrap().unwrap().update_available);

        let update_err = svc.update_plugin("demo@team").unwrap_err();
        assert!(update_err
            .to_string()
            .contains("requires authentication confirmation"));

        let updated = svc.update_plugin_with_auth("demo@team", true).unwrap();
        assert_eq!(updated.version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn marketplace_plugin_reinstall_keeps_user_data_and_single_runtime_projection_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        let plugin_source = marketplace_root.join("plugins").join("demo");
        write_plugin(&plugin_source, "demo");
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
                  "source": { "source": "local", "path": "./plugins/demo" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let first = svc.install_from_marketplace("team", "demo").unwrap();
        assert_eq!(first.id, "demo@team");
        let data_dir = svc.data_root.join(sanitize_file_name(&first.id));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("state.json"), r#"{"runs":1}"#).unwrap();

        let second = svc.install_from_marketplace("team", "demo").unwrap();
        assert_eq!(second.id, "demo@team");
        assert_eq!(second.version.as_deref(), Some("0.1.0"));
        assert_eq!(
            std::fs::read_to_string(data_dir.join("state.json")).unwrap(),
            r#"{"runs":1}"#
        );
        assert_eq!(
            svc.list()
                .unwrap()
                .into_iter()
                .filter(|plugin| plugin.id == "demo@team")
                .count(),
            1
        );
        assert_eq!(
            svc.runtime_projection().unwrap().skill_roots,
            vec![roots
                .marketplace_cache
                .join("team")
                .join("demo")
                .join("0.1.0")
                .join("skills")]
        );
    }

    #[test]
    fn marketplace_install_installs_same_marketplace_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let marketplace_root = tmp.path().join("team-marketplace");
        write_plugin(&marketplace_root.join("plugins").join("helper"), "helper");
        write_plugin_with_dependencies(
            &marketplace_root.join("plugins").join("worker"),
            "worker",
            &["helper"],
        );
        std::fs::write(
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "worker",
                  "version": "0.1.0",
                  "description": "Worker plugin",
                  "source": { "source": "local", "path": "./plugins/worker" }
                },
                {
                  "name": "helper",
                  "version": "0.1.0",
                  "description": "Helper plugin",
                  "source": { "source": "local", "path": "./plugins/helper" }
                }
              ]
            }"#,
        )
        .unwrap();

        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));
        svc.add_marketplace(AddPluginMarketplaceDto {
            name: Some("team".to_string()),
            source: marketplace_root.display().to_string(),
            git_ref: None,
            sparse_path: None,
        })
        .unwrap();

        let worker = svc.install_from_marketplace("team", "worker").unwrap();
        let helper = svc.read("helper@team").unwrap().unwrap();

        assert_eq!(worker.id, "worker@team");
        assert!(worker.enabled);
        assert!(helper.enabled);
        assert_eq!(helper.origin, "marketplace");
        let projection = svc.runtime_projection().unwrap();
        assert!(projection.skill_roots.contains(
            &roots
                .marketplace_cache
                .join("team")
                .join("helper")
                .join("0.1.0")
                .join("skills")
        ));
        assert!(projection.skill_roots.contains(
            &roots
                .marketplace_cache
                .join("team")
                .join("worker")
                .join("0.1.0")
                .join("skills")
        ));
    }
}
