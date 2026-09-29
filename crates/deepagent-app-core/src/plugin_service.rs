//! UI-facing plugin service.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};
use deepagent_mcp::config::{McpConfig, McpServerConfig, TransportType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin::dialect::{
    resolve_presentation, InterfaceSource, MarketplaceSource, PortableSource, Presentation,
    PresentationSources,
};
use crate::plugin::model::ResolvedPlugin;
use crate::plugin::spec::{normalize_and_expand, resolve_existing_within, resolve_plugin_relative};
use crate::plugin_dependency::{
    find_reverse_dependents, verify_plugin_dependencies, PluginDependencyOutcome,
};
use crate::plugin_loader::{
    load_plugins, plugin_id, LoadedPlugin, PluginLoadError, PluginOrigin, PluginRoots,
};
use crate::plugin_manifest::{load_plugin_manifest, PluginManifest};
use crate::plugin_runtime::{
    load_plugin_app_entries, load_plugin_output_style_entries, EnabledPluginRuntimeInput,
    PluginRuntimeProjection,
};
use crate::plugin_security::{scan_plugin_dir, PluginScanReportDto};

const PLUGIN_STATE_SCHEMA_VERSION: u32 = 1;
const MCP_SIDECAR_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSourceDto {
    pub kind: String,
    pub name: String,
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
    McpSidecar,
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
    pub plugin_id: String,
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

/// Metadata needed by the plugin catalog. Runtime and diagnostic details are
/// fetched through `read_plugin` only after a plugin is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSummaryDto {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub installed: bool,
    pub enabled: bool,
    pub available: bool,
    pub skill_count: u32,
    pub mcp_server_count: u32,
    pub hook_count: u32,
    pub output_style_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_color: Option<String>,
    #[serde(default)]
    pub required_by: Vec<PluginDependentDto>,
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
struct PluginState {
    #[serde(default = "plugin_state_schema_version")]
    version: u32,
    #[serde(default)]
    enabled: BTreeMap<String, bool>,
    #[serde(default)]
    installed: BTreeMap<String, InstalledPluginState>,
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
    summary_cache: Mutex<Option<PluginSummaryCache>>,
}

struct PluginRuntimeCache {
    projection: PluginRuntimeProjection,
    snapshots: Vec<PathSnapshot>,
}

struct PluginListCache {
    plugins: Vec<PluginDto>,
    snapshots: Vec<PathSnapshot>,
}

struct PluginSummaryCache {
    plugins: Vec<PluginSummaryDto>,
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

fn merge_runtime_requirements_from_mcp_value(
    needs: &mut PluginRuntimeNeeds,
    value: Option<&serde_json::Value>,
) {
    let Some(value) = value else {
        return;
    };
    let Ok(text) = serde_json::to_string(value) else {
        return;
    };
    let Ok(config) = McpConfig::parse(&text) else {
        return;
    };
    merge_runtime_requirements_from_mcp_config(needs, &config);
}

fn merge_runtime_requirements_from_mcp_config(needs: &mut PluginRuntimeNeeds, config: &McpConfig) {
    for server in config.servers.values() {
        if !matches!(server.effective_type(), Ok(TransportType::Stdio)) {
            continue;
        }
        let Some(command) = server.command.as_deref() else {
            continue;
        };
        match mcp_command_runtime_requirement(command) {
            Some("node") => needs.node = true,
            Some("python") => needs.python = true,
            Some("java") => needs.java = true,
            _ => {}
        }
    }
}

fn mcp_command_runtime_requirement(command: &str) -> Option<&'static str> {
    let stem = Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_ascii_lowercase())?;
    match stem.as_str() {
        "node" | "npm" | "npx" | "pnpm" | "yarn" | "bun" => Some("node"),
        "python" | "python3" | "py" | "pip" | "pip3" | "uv" | "uvx" => Some("python"),
        "java" => Some("java"),
        _ => None,
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
        let service = Self {
            roots,
            state_path: plugin_data.join("state.json"),
            data_root: plugin_data.join("data"),
            runtime_cache: Mutex::new(None),
            list_cache: Mutex::new(None),
            summary_cache: Mutex::new(None),
        };
        service.reclaim_decommissioned_marketplace_storage(&plugin_data);
        service
    }

    /// The marketplace feature is gone, but a build that predates its removal
    /// leaves `plugins/marketplaces/` and `plugins/cache/` behind with no UI
    /// path able to delete them. Reclaim both once, at construction.
    fn reclaim_decommissioned_marketplace_storage(&self, plugin_data: &Path) {
        for name in ["marketplaces", "cache"] {
            let dir = plugin_data.join(name);
            if !dir.exists() {
                continue;
            }
            if let Err(error) = safe_remove_dir(plugin_data, &dir) {
                tracing::warn!(
                    path = %dir.display(),
                    error = %error,
                    "failed to reclaim decommissioned marketplace storage"
                );
            }
        }
    }

    pub fn roots(&self) -> &PluginRoots {
        &self.roots
    }

    pub fn list(&self) -> Result<Vec<PluginDto>> {
        if let Some(plugins) = self.cached_plugin_list() {
            return Ok(plugins);
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

    pub fn list_summaries(&self) -> Result<Vec<PluginSummaryDto>> {
        if let Some(plugins) = self.cached_plugin_summaries() {
            return Ok(plugins);
        }
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        let plugins = loaded
            .iter()
            .map(|plugin| self.summary_from_loaded(plugin, &loaded, &state, &dependencies))
            .collect::<Vec<_>>();
        let snapshots = self.plugin_list_cache_snapshots(&loaded, &state);
        self.store_plugin_summary_cache(plugins.clone(), snapshots);
        Ok(plugins)
    }

    pub fn reload(&self) -> Result<Vec<PluginDto>> {
        self.list()
    }

    pub fn read(&self, id: &str) -> Result<Option<PluginDto>> {
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        Ok(loaded
            .iter()
            .find(|plugin| plugin.id == id)
            .map(|plugin| self.dto_from_loaded(plugin, &loaded, &state, &dependencies)))
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
        let (checked_at, runtime_available, lifecycle_state, health_status, health_error) =
            self.evaluate_and_persist_plugin_health(id, &plugin)?;
        self.invalidate_plugin_caches();

        let mut inspection = PluginRuntimeInspectionDto::from_plugin(&plugin);
        inspection.runtime_available = runtime_available;
        inspection.last_health_check = Some(checked_at);
        inspection.health_status = health_status;
        inspection.health_error = health_error;
        inspection.state = lifecycle_state;
        Ok(Some(inspection))
    }

    fn evaluate_and_persist_plugin_health(
        &self,
        id: &str,
        plugin: &PluginDto,
    ) -> Result<(
        String,
        bool,
        PluginLifecycleState,
        PluginHealthStatus,
        Option<String>,
    )> {
        let checked_at = now_string();
        let (runtime_available, lifecycle_state, health_status, health_error) =
            self.evaluate_plugin_health(id, plugin)?;
        let (checked_at, health_status, health_error) =
            self.persist_plugin_health_check(id, checked_at, health_status, health_error)?;
        Ok((
            checked_at,
            runtime_available,
            explicit_health_lifecycle_state(lifecycle_state, health_status),
            health_status,
            health_error,
        ))
    }

    fn evaluate_plugin_health(
        &self,
        id: &str,
        plugin: &PluginDto,
    ) -> Result<(
        bool,
        PluginLifecycleState,
        PluginHealthStatus,
        Option<String>,
    )> {
        let Some(loaded) = load_plugins(&self.roots)
            .into_iter()
            .find(|loaded| loaded.id == id)
        else {
            return Ok((
                plugin.runtime_available,
                plugin.state,
                plugin.health_status,
                plugin.health_error.clone(),
            ));
        };
        let resolved = loaded.resolved();
        let manifest = resolved.map(|plugin| &plugin.manifest);
        let counts = PluginComponentCounts::from_manifest(manifest);
        let entrypoints = self.plugin_entrypoints(&loaded, manifest);
        let has_runtime_payload = loaded.root.join("runtime.zip").is_file();
        let runtime_requirements =
            self.plugin_runtime_requirements(&loaded, manifest, has_runtime_payload);
        let runtime_available = self.runtime_requirements_available(&runtime_requirements);
        let health_status = self.plugin_health_status(
            &loaded,
            manifest,
            resolved,
            counts,
            runtime_available,
            &runtime_requirements,
        );
        let health_error = self.plugin_health_error(
            &loaded,
            manifest,
            counts,
            &runtime_requirements,
            health_status,
        );
        let lifecycle_state = self.plugin_lifecycle_state(
            &loaded,
            manifest,
            resolved,
            counts,
            health_status,
            &entrypoints,
        );
        if health_status != PluginHealthStatus::Ready {
            return Ok((
                runtime_available,
                lifecycle_state,
                health_status,
                health_error,
            ));
        }

        let mut evaluated_plugin = plugin.clone();
        evaluated_plugin.runtime_available = runtime_available;
        evaluated_plugin.health_status = health_status;
        evaluated_plugin.health_error = health_error.clone();
        evaluated_plugin.state = lifecycle_state;
        let (health_status, health_error) = self
            .explicit_plugin_health_override(id, &evaluated_plugin)?
            .unwrap_or((health_status, health_error));
        Ok((
            runtime_available,
            lifecycle_state,
            health_status,
            health_error,
        ))
    }

    fn persist_plugin_health_check(
        &self,
        id: &str,
        checked_at: String,
        health_status: PluginHealthStatus,
        health_error: Option<String>,
    ) -> Result<(String, PluginHealthStatus, Option<String>)> {
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
        Ok((checked_at, health_status, health_error))
    }

    fn refresh_plugin_runtime_and_health_after_install(&self, id: &str) -> Result<()> {
        self.invalidate_plugin_caches();
        let data_dir = self.data_root.join(sanitize_file_name(id));
        std::fs::create_dir_all(&data_dir).map_err(|e| {
            CoreError::Persistence(format!(
                "create plugin data dir {} after install: {e}",
                data_dir.display()
            ))
        })?;

        if let Err(error) = self.runtime_projection() {
            self.persist_plugin_health_check(
                id,
                now_string(),
                PluginHealthStatus::Failed,
                Some(format!(
                    "plugin runtime projection refresh failed after install: {error}"
                )),
            )?;
            self.invalidate_plugin_caches();
            return Ok(());
        }

        self.invalidate_plugin_caches();
        let Some(plugin) = self.read(id)? else {
            return Err(CoreError::not_found(format!("plugin {id}")));
        };
        let result = self.evaluate_and_persist_plugin_health(id, &plugin);
        if let Err(error) = result {
            self.persist_plugin_health_check(
                id,
                now_string(),
                PluginHealthStatus::Failed,
                Some(format!("plugin health check failed after install: {error}")),
            )?;
        }
        self.invalidate_plugin_caches();
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<PluginDto> {
        let mut state = self.load_state()?;
        state.enabled.insert(id.to_string(), enabled);
        self.save_state(&state)?;
        self.invalidate_plugin_caches();
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
        self.invalidate_plugin_caches();
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
            state.enabled.entry(id.clone()).or_insert(true);
            let content_hash = plugin_directory_content_hash(source).ok();
            let previous = state.installed.get(&id).cloned();
            let should_refresh_health = mark_plugin_health_stale_for_install(
                &mut state,
                &id,
                previous.as_ref(),
                manifest.version.as_deref(),
                content_hash.as_deref(),
            );
            self.save_state(&state)?;
            self.invalidate_plugin_caches();
            if should_refresh_health {
                if let Err(error) = self.refresh_plugin_runtime_and_health_after_install(&id) {
                    tracing::warn!(plugin_id = %id, error = %error, "failed to refresh plugin health after install");
                }
            }
            return self
                .read(&id)?
                .ok_or_else(|| CoreError::not_found(format!("plugin {id}")));
        }

        commit_plugin_directory(source, &destination, &self.roots.personal, &name)?;
        let content_hash = plugin_directory_content_hash(&destination)?;

        let id = plugin_id(&name, PluginOrigin::Personal.as_str());
        let mut state = self.load_state()?;
        let previous = state.installed.get(&id).cloned();
        let should_refresh_health = mark_plugin_health_stale_for_install(
            &mut state,
            &id,
            previous.as_ref(),
            manifest.version.as_deref(),
            Some(content_hash.as_str()),
        );
        state.enabled.entry(id.clone()).or_insert(true);
        state.installed.insert(
            id.clone(),
            InstalledPluginState {
                version: manifest.version.clone(),
                install_path: destination.display().to_string(),
                installed_at: previous
                    .as_ref()
                    .map(|installed| installed.installed_at.clone())
                    .unwrap_or_else(now_string),
                last_updated: previous.as_ref().map(|_| now_string()),
                content_hash: Some(content_hash),
            },
        );
        self.save_state(&state)?;
        self.invalidate_plugin_caches();
        if should_refresh_health {
            if let Err(error) = self.refresh_plugin_runtime_and_health_after_install(&id) {
                tracing::warn!(plugin_id = %id, error = %error, "failed to refresh plugin health after install");
            }
        }
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
                self.invalidate_plugin_caches();
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
        }

        let mut state = self.load_state()?;
        state.enabled.remove(id);
        state.installed.remove(id);
        self.save_state(&state)?;
        self.invalidate_plugin_caches();

        if remove_data {
            let data_dir = self.data_root.join(sanitize_file_name(id));
            if data_dir.exists() {
                safe_remove_dir(&self.data_root, &data_dir)?;
            }
        }
        Ok(true)
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
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        let mut enabled_plugins = loaded
            .into_iter()
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
        Ok(enabled_plugins
            .iter()
            .filter_map(|plugin| {
                plugin
                    .resolved()
                    .map(|resolved| (plugin, &resolved.manifest))
            })
            .flat_map(|(plugin, manifest)| {
                load_plugin_app_entries(&plugin.id, &plugin.name, manifest)
            })
            .filter(|app| host_app_component_is_renderable(&app.component))
            .collect())
    }

    pub fn list_output_styles(&self) -> Result<Vec<crate::plugin_runtime::PluginOutputStyleEntry>> {
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        Ok(loaded
            .iter()
            .filter(|plugin| {
                self.is_effectively_enabled(plugin, &state)
                    && !dependencies.demoted.contains(&plugin.id)
            })
            .filter_map(|plugin| {
                plugin
                    .resolved()
                    .map(|resolved| (plugin, &resolved.manifest))
            })
            .flat_map(|(plugin, manifest)| {
                load_plugin_output_style_entries(&plugin.id, &plugin.name, manifest)
            })
            .collect())
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
            return PluginExecutionKind::McpSidecar;
        }

        if counts.commands > 0 {
            return PluginExecutionKind::Subprocess;
        }

        if counts.skills > 0 || counts.agents > 0 || counts.output_styles > 0 {
            return PluginExecutionKind::SkillOnly;
        }

        PluginExecutionKind::SkillOnly
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
            merge_runtime_requirements_from_mcp_value(
                &mut needs,
                manifest.paths.mcp_servers_inline.as_ref(),
            );
            for path in &manifest.paths.mcp_server_paths {
                if !path.is_file() {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(path) else {
                    continue;
                };
                let Ok(config) = McpConfig::parse(&text) else {
                    continue;
                };
                merge_runtime_requirements_from_mcp_config(&mut needs, &config);
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
            && (!needs.python || python_runtime_candidate().is_some())
            && missing_python_imports(needs).is_empty()
            && (!needs.java || probe_runtime("java", &["-version"]))
            && (!needs.shell || probe_shell())
            && missing_command_probes(needs).is_empty()
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
            for entrypoint in runtime_payload_declared_entrypoints(&plugin.root) {
                entrypoints.push(format!("runtime.zip!{}", entrypoint.display()));
            }
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
        if !runtime_payload_errors(&plugin.root).is_empty() {
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

    fn plugin_lightweight_health_status(
        &self,
        plugin: &LoadedPlugin,
        manifest: Option<&PluginManifest>,
        resolved: Option<&ResolvedPlugin>,
        counts: PluginComponentCounts,
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
        if !runtime_payload_errors(&plugin.root).is_empty() {
            return PluginHealthStatus::Incomplete;
        }

        if manifest.is_some_and(manifest_needs_host_authorization) {
            return PluginHealthStatus::NeedsAuthorization;
        }
        if !missing_credential_env_hints(&plugin.root).is_empty() {
            return PluginHealthStatus::NeedsConfiguration;
        }
        if runtime_requirements.requires_runtime() {
            return PluginHealthStatus::Unknown;
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
                        let runtime_errors = runtime_payload_errors(&plugin.root);
                        if !runtime_errors.is_empty() {
                            runtime_errors.join("; ")
                        } else if hook_errors.errors.is_empty() {
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
                data_dir: data_dir.clone(),
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
        if !failures.is_empty() {
            return Ok(Some((
                PluginHealthStatus::ConnectionUnavailable,
                Some(format!(
                    "hosted MCP endpoint unavailable: {}",
                    failures.join("; ")
                )),
            )));
        }

        if !credentials_satisfy_documented_auth(&loaded.root) {
            let auth_failures = documented_auth_command_failures(&loaded.root);
            if let Some(failure) = auth_failures.first() {
                return Ok(Some(match failure.kind {
                    CommandProbeFailureKind::Unavailable => (
                        PluginHealthStatus::RuntimeUnavailable,
                        Some(format!(
                            "plugin authentication health check command is unavailable: {}",
                            failure.probe.display()
                        )),
                    ),
                    CommandProbeFailureKind::Rejected => (
                        PluginHealthStatus::NeedsConfiguration,
                        Some(format!(
                            "plugin authentication health check failed: {}",
                            failure.probe.display()
                        )),
                    ),
                }));
            }
        }

        if plugin.execution_kind == PluginExecutionKind::McpSidecar {
            if let Err(error) = verify_plugin_data_writable(&data_dir) {
                return Ok(Some((PluginHealthStatus::Incomplete, Some(error))));
            }
            if let Some(failure) = mcp_sidecar_health_failure(&projection.mcp_config) {
                return Ok(Some(failure));
            }
        }

        if plugin.has_runtime_payload {
            let data_dir = self.data_root.join(sanitize_file_name(&loaded.id));
            prepare_runtime_payload(&loaded.root, &data_dir)?;
            if let Some(error) = runtime_payload_health_error(&loaded.root, &data_dir) {
                return Ok(Some((PluginHealthStatus::Incomplete, Some(error))));
            }
        }

        Ok(None)
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
                if !host_command_component_target_is_registered(component) {
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
            PluginHealthStatus::Unknown => PluginLifecycleState::Installed,
            PluginHealthStatus::Ready => match self.plugin_execution_kind(
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
                | PluginExecutionKind::McpSidecar => PluginLifecycleState::Executable,
            },
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
        let license_status = self.plugin_license_status(plugin);
        let persisted_health = state.health_checks.get(&plugin.id);
        let runtime_available =
            lightweight_runtime_available(&runtime_requirements, persisted_health);
        let computed_health_status = self.plugin_lightweight_health_status(
            plugin,
            manifest,
            resolved,
            counts,
            &runtime_requirements,
        );
        let computed_health_error = self.plugin_health_error(
            plugin,
            manifest,
            counts,
            &runtime_requirements,
            computed_health_status,
        );
        let last_health_check = persisted_health.map(|check| check.checked_at.clone());
        let (health_status, health_error) =
            if plugin.available && resolved.is_some() && !has_fatal_plugin_errors(plugin) {
                persisted_health
                    .map(|check| (check.status, check.error.clone()))
                    .unwrap_or((computed_health_status, computed_health_error))
            } else {
                (computed_health_status, computed_health_error)
            };
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
        let capabilities = plugin_capabilities(manifest, counts);

        let mut errors = plugin.errors.clone();
        errors.extend_from_slice(dependencies.errors_for(&plugin.id));
        let required_by = self.reverse_dependents(plugin, loaded, state, dependencies);

        PluginDto {
            id: plugin.id.clone(),
            plugin_id: plugin.id.clone(),
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

    fn summary_from_loaded(
        &self,
        plugin: &LoadedPlugin,
        loaded: &[LoadedPlugin],
        state: &PluginState,
        dependencies: &PluginDependencyOutcome,
    ) -> PluginSummaryDto {
        let resolved = plugin.resolved();
        let manifest = resolved.map(|plugin| &plugin.manifest);
        let presentation =
            resolved.map(|resolved| self.presentation_for_loaded(plugin, resolved, state));
        let counts = PluginComponentCounts::from_manifest(manifest);
        PluginSummaryDto {
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
            developer: presentation
                .as_ref()
                .and_then(|presentation| presentation.developer_name.clone()),
            origin: plugin.origin.as_str().to_string(),
            category: presentation
                .as_ref()
                .and_then(|presentation| presentation.category.clone()),
            keywords: manifest.map(|m| m.keywords.clone()).unwrap_or_default(),
            capabilities: plugin_capabilities(manifest, counts),
            installed: manifest.is_some(),
            enabled: self.is_effectively_enabled(plugin, state)
                && !dependencies.demoted.contains(&plugin.id),
            available: plugin.available && resolved.is_some(),
            skill_count: counts.skills,
            mcp_server_count: counts.mcp_servers,
            hook_count: counts.hooks,
            output_style_count: counts.output_styles,
            icon_path: manifest
                .and_then(|m| m.interface.composer_icon.as_ref())
                .map(|path| path_string(path)),
            logo_path: manifest
                .and_then(|m| m.interface.logo.as_ref())
                .map(|path| path_string(path)),
            brand_color: manifest.and_then(|m| m.interface.brand_color.clone()),
            required_by: self.reverse_dependents(plugin, loaded, state, dependencies),
        }
    }

    fn presentation_for_loaded(
        &self,
        plugin: &LoadedPlugin,
        resolved: &ResolvedPlugin,
        _state: &PluginState,
    ) -> Presentation {
        let manifest = &resolved.manifest;

        resolve_presentation(PresentationSources {
            interface: InterfaceSource {
                display_name: manifest.interface.display_name.as_deref(),
                short_description: manifest.interface.short_description.as_deref(),
                long_description: manifest.interface.long_description.as_deref(),
                developer_name: manifest.interface.developer_name.as_deref(),
                category: manifest.interface.category.as_deref(),
            },
            marketplace: MarketplaceSource::default(),
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

    fn cached_plugin_summaries(&self) -> Option<Vec<PluginSummaryDto>> {
        let cache = self.summary_cache.lock().ok()?;
        let cache = cache.as_ref()?;
        if cache.snapshots.iter().all(PathSnapshot::still_matches) {
            Some(cache.plugins.clone())
        } else {
            None
        }
    }

    fn store_plugin_summary_cache(
        &self,
        plugins: Vec<PluginSummaryDto>,
        snapshots: Vec<PathSnapshot>,
    ) {
        if let Ok(mut cache) = self.summary_cache.lock() {
            *cache = Some(PluginSummaryCache { plugins, snapshots });
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
        if let Ok(mut cache) = self.summary_cache.lock() {
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
}

/// Extract a plugin's `runtime.zip` payload into `data_dir/runtime`, skipping
/// the work when the marker file already matches the archive size.
///
/// Public so the desktop entry point can pre-warm the extraction off the
/// startup path (a background thread); the MCP/plugin assembly paths still call
/// it synchronously and rely on the marker to make the repeat call a no-op.
pub fn prepare_runtime_payload(payload_root: &Path, data_dir: &Path) -> Result<()> {
    let archive_path = payload_root.join("runtime.zip");
    if !archive_path.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(data_dir)
        .map_err(|e| CoreError::Persistence(format!("create plugin data dir: {e}")))?;
    std::fs::create_dir_all(data_dir.join("workspace"))
        .map_err(|e| CoreError::Persistence(format!("create payload workspace: {e}")))?;
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
    Ok(())
}

/// Extract every enabled plugin's `runtime.zip` payload into its
/// `PLUGIN_DATA/runtime` directory, using the same id → directory mapping as
/// [`PluginService::runtime_projection`].
///
/// Intended for the desktop entry point to call on a background thread at
/// startup, so the first session after an upgrade does not stall on the
/// synchronous extraction of a large payload (e.g. `wedecode`'s ~30 MB zip).
/// The per-payload marker file makes a later synchronous call a no-op. A
/// failure for one plugin is logged and does not abort the others; the session
/// path still surfaces genuine errors through the normal MCP/plugin channel.
pub fn prepare_runtime_payloads(roots: &PluginRoots, data_root: &Path) {
    for plugin in load_plugins(roots) {
        if !plugin.enabled_default() {
            continue;
        }
        if !plugin.root.join("runtime.zip").is_file() {
            continue;
        }
        let data_dir = data_root.join(sanitize_file_name(&plugin.id));
        if let Err(error) = prepare_runtime_payload(&plugin.root, &data_dir) {
            tracing::warn!(
                plugin = plugin.id.as_str(),
                root = %plugin.root.display(),
                error = %error,
                "runtime payload pre-warm failed"
            );
        }
    }
}

fn runtime_payload_declared_entrypoints(plugin_root: &Path) -> Vec<PathBuf> {
    runtime_payload_inspection(plugin_root).entrypoints
}

fn runtime_payload_errors(plugin_root: &Path) -> Vec<String> {
    runtime_payload_inspection(plugin_root).errors
}

fn runtime_payload_health_error(plugin_root: &Path, data_dir: &Path) -> Option<String> {
    let inspection = runtime_payload_inspection(plugin_root);
    if let Some(error) = inspection.errors.first() {
        return Some(error.clone());
    }
    if inspection.entrypoints.is_empty() {
        return None;
    }

    let runtime_dir = data_dir.join("runtime");
    if !runtime_dir.is_dir() {
        return Some(format!(
            "runtime payload was not extracted to {}",
            runtime_dir.display()
        ));
    }
    let workspace_dir = data_dir.join("workspace");
    if !workspace_dir.is_dir() {
        return Some(format!(
            "runtime payload workspace is missing: {}",
            workspace_dir.display()
        ));
    }
    if let Err(error) = verify_plugin_data_writable(data_dir) {
        return Some(error);
    }

    for entrypoint in inspection.entrypoints {
        let runtime_entry = runtime_dir.join(&entrypoint);
        if !runtime_entry.is_file() {
            return Some(format!(
                "runtime payload entrypoint '{}' was not extracted",
                entrypoint.display()
            ));
        }
        if is_node_entrypoint(&runtime_entry) && probe_runtime("node", &["--version"]) {
            let output = Command::new("node")
                .arg("--check")
                .arg(&runtime_entry)
                .current_dir(&workspace_dir)
                .env("PLUGIN_ROOT", plugin_root)
                .env("PLUGIN_DATA", data_dir)
                .env("DEEPAGENT_PLUGIN_ROOT", plugin_root)
                .env("DEEPAGENT_PLUGIN_DATA", data_dir)
                .output();
            match output {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Some(format!(
                        "runtime payload entrypoint '{}' failed node syntax check: {}",
                        entrypoint.display(),
                        stderr.trim()
                    ));
                }
                Err(error) => {
                    return Some(format!(
                        "runtime payload entrypoint '{}' could not be checked with node: {error}",
                        entrypoint.display()
                    ));
                }
            }
        }
    }
    None
}

fn mcp_sidecar_health_failure(
    mcp_config: &McpConfig,
) -> Option<(PluginHealthStatus, Option<String>)> {
    mcp_sidecar_health_failure_inner(mcp_config, true)
}

fn mcp_sidecar_health_failure_inner(
    mcp_config: &McpConfig,
    probe_sidecar: bool,
) -> Option<(PluginHealthStatus, Option<String>)> {
    for (server_name, server) in &mcp_config.servers {
        let transport = match server.effective_type() {
            Ok(transport) => transport,
            Err(error) => {
                return Some((
                    PluginHealthStatus::Failed,
                    Some(format!("MCP sidecar '{server_name}' is invalid: {error}")),
                ));
            }
        };
        if transport != TransportType::Stdio {
            continue;
        }
        let Some(command) = server
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Some((
                PluginHealthStatus::Failed,
                Some(format!("MCP sidecar '{server_name}' is missing a command")),
            ));
        };
        let cwd = server.cwd.as_deref();
        if let Some(cwd) = cwd.filter(|path| !path.is_dir()) {
            return Some((
                PluginHealthStatus::Incomplete,
                Some(format!(
                    "MCP sidecar '{server_name}' cwd does not exist: {}",
                    cwd.display()
                )),
            ));
        }
        let resolved_command = match resolve_sidecar_command(command, cwd) {
            Ok(command) => command,
            Err(failure) => {
                return Some((
                    failure.status,
                    Some(format!("MCP sidecar '{server_name}' {}", failure.message)),
                ));
            }
        };
        if let Some(failure) = sidecar_script_health_failure(
            server_name,
            command,
            &resolved_command,
            &server.args,
            cwd,
        ) {
            return Some(failure);
        }
        if probe_sidecar {
            if let Err(error) = probe_mcp_sidecar(server.clone()) {
                return Some((
                    PluginHealthStatus::Failed,
                    Some(format!(
                        "MCP sidecar '{server_name}' failed initialize/tools handshake: {error}"
                    )),
                ));
            }
        }
    }
    None
}

fn probe_mcp_sidecar(server: McpServerConfig) -> std::result::Result<usize, String> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("create MCP probe runtime: {e}"))?;
        runtime.block_on(async move {
            let transport = deepagent_mcp::connect_transport(&server)
                .map_err(|e| format!("connect transport: {e}"))?;
            let client = deepagent_mcp::McpClient::new(transport);
            match tokio::time::timeout(
                MCP_SIDECAR_PROBE_TIMEOUT,
                client.initialize("deepagent-plugin-health"),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    let _ = client.close().await;
                    return Err(format!("initialize: {error}"));
                }
                Err(error) => {
                    let _ = client.close().await;
                    return Err(format!("initialize timed out: {error}"));
                }
            }
            let tools =
                match tokio::time::timeout(MCP_SIDECAR_PROBE_TIMEOUT, client.list_tools()).await {
                    Ok(Ok(tools)) => tools,
                    Ok(Err(error)) => {
                        let _ = client.close().await;
                        return Err(format!("tools/list: {error}"));
                    }
                    Err(error) => {
                        let _ = client.close().await;
                        return Err(format!("tools/list timed out: {error}"));
                    }
                };
            let _ = client.close().await;
            Ok(tools.len())
        })
    })
    .join()
    .map_err(|_| "MCP probe thread panicked".to_string())?
}

#[derive(Debug)]
struct SidecarHealthFailure {
    status: PluginHealthStatus,
    message: String,
}

fn resolve_sidecar_command(
    command: &str,
    cwd: Option<&Path>,
) -> std::result::Result<PathBuf, SidecarHealthFailure> {
    if command_has_path_separator(command) {
        let path = PathBuf::from(command);
        let resolved = if path.is_absolute() {
            path
        } else {
            cwd.unwrap_or_else(|| Path::new(".")).join(path)
        };
        if resolved.is_file() {
            return Ok(resolved);
        }
        return Err(SidecarHealthFailure {
            status: PluginHealthStatus::Incomplete,
            message: format!("command path is missing: {}", resolved.display()),
        });
    }

    if let Some(path) = find_command_on_path(command) {
        return Ok(path);
    }
    for env_key in external_command_env_keys(command) {
        if let Some(path) = std::env::var_os(env_key).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(path);
            }
        }
    }

    Err(SidecarHealthFailure {
        status: PluginHealthStatus::RuntimeUnavailable,
        message: format!("command is not available on PATH: {command}"),
    })
}

fn sidecar_script_health_failure(
    server_name: &str,
    command: &str,
    resolved_command: &Path,
    args: &[String],
    cwd: Option<&Path>,
) -> Option<(PluginHealthStatus, Option<String>)> {
    let entrypoint = sidecar_script_entrypoint(command, resolved_command, args, cwd)?;
    let entrypoint = match entrypoint {
        Ok(entrypoint) => entrypoint,
        Err(error) => {
            return Some((
                PluginHealthStatus::Incomplete,
                Some(format!("MCP sidecar '{server_name}' {error}")),
            ));
        }
    };
    if is_node_entrypoint(&entrypoint) {
        return node_sidecar_syntax_failure(server_name, &entrypoint, resolved_command);
    }
    if entrypoint
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
    {
        return python_sidecar_syntax_failure(server_name, &entrypoint, resolved_command);
    }
    None
}

fn sidecar_script_entrypoint(
    command: &str,
    resolved_command: &Path,
    args: &[String],
    cwd: Option<&Path>,
) -> Option<std::result::Result<PathBuf, String>> {
    if is_node_program(command) || is_node_program_path(resolved_command) {
        return Some(resolve_sidecar_arg_script(args, cwd, is_node_entrypoint));
    }
    if is_python_program(command) || is_python_program_path(resolved_command) {
        return Some(resolve_sidecar_arg_script(args, cwd, |path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
        }));
    }
    if is_node_entrypoint(resolved_command)
        || resolved_command
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
    {
        return Some(Ok(resolved_command.to_path_buf()));
    }
    None
}

fn resolve_sidecar_arg_script(
    args: &[String],
    cwd: Option<&Path>,
    predicate: impl Fn(&Path) -> bool,
) -> std::result::Result<PathBuf, String> {
    for arg in args {
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        let path = PathBuf::from(arg);
        if !predicate(&path) {
            continue;
        }
        let resolved = if path.is_absolute() {
            path
        } else {
            cwd.unwrap_or_else(|| Path::new(".")).join(path)
        };
        if resolved.is_file() {
            return Ok(resolved);
        }
        return Err(format!(
            "script entrypoint is missing: {}",
            resolved.display()
        ));
    }
    Err("does not declare a script entrypoint argument".to_string())
}

fn node_sidecar_syntax_failure(
    server_name: &str,
    entrypoint: &Path,
    node_command: &Path,
) -> Option<(PluginHealthStatus, Option<String>)> {
    let output = Command::new(node_command)
        .arg("--check")
        .arg(entrypoint)
        .output();
    match output {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some((
            PluginHealthStatus::Incomplete,
            Some(format!(
                "MCP sidecar '{server_name}' node entrypoint '{}' failed syntax check: {}",
                entrypoint.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        )),
        Err(error) => Some((
            PluginHealthStatus::RuntimeUnavailable,
            Some(format!(
                "MCP sidecar '{server_name}' node command could not run syntax check: {error}"
            )),
        )),
    }
}

fn python_sidecar_syntax_failure(
    server_name: &str,
    entrypoint: &Path,
    python_command: &Path,
) -> Option<(PluginHealthStatus, Option<String>)> {
    let output = Command::new(python_command)
        .arg("-m")
        .arg("py_compile")
        .arg(entrypoint)
        .output();
    match output {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some((
            PluginHealthStatus::Incomplete,
            Some(format!(
                "MCP sidecar '{server_name}' python entrypoint '{}' failed syntax check: {}",
                entrypoint.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        )),
        Err(error) => Some((
            PluginHealthStatus::RuntimeUnavailable,
            Some(format!(
                "MCP sidecar '{server_name}' python command could not run syntax check: {error}"
            )),
        )),
    }
}

fn command_has_path_separator(command: &str) -> bool {
    command.contains('/') || command.contains('\\')
}

fn find_command_on_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let candidates = command_name_candidates(command);
    for dir in std::env::split_paths(&path) {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn command_name_candidates(command: &str) -> Vec<String> {
    let mut candidates = vec![command.to_string()];
    #[cfg(windows)]
    {
        if Path::new(command).extension().is_none() {
            let pathext =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
            for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
                candidates.push(format!("{command}{}", ext.trim().to_ascii_lowercase()));
                candidates.push(format!("{command}{}", ext.trim().to_ascii_uppercase()));
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn is_node_program(command: &str) -> bool {
    matches!(
        Path::new(command)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_ascii_lowercase())
            .as_deref(),
        Some("node")
    )
}

fn is_node_program_path(path: &Path) -> bool {
    is_node_program(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    )
}

fn is_python_program(command: &str) -> bool {
    matches!(
        Path::new(command)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_ascii_lowercase())
            .as_deref(),
        Some("python" | "python3")
    )
}

fn is_python_program_path(path: &Path) -> bool {
    is_python_program(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    )
}

fn verify_plugin_data_writable(data_dir: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("create plugin data dir {}: {e}", data_dir.display()))?;
    let probe = data_dir.join(".deepagent-healthcheck.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("PLUGIN_DATA is not writable at {}: {e}", data_dir.display()))?;
    file.write_all(b"ok")
        .map_err(|e| format!("write PLUGIN_DATA health probe {}: {e}", probe.display()))?;
    drop(file);
    std::fs::remove_file(&probe)
        .map_err(|e| format!("remove PLUGIN_DATA health probe {}: {e}", probe.display()))?;
    Ok(())
}

fn is_node_entrypoint(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "js" | "cjs" | "mjs"))
        .unwrap_or(false)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RuntimePayloadInspection {
    entrypoints: Vec<PathBuf>,
    errors: Vec<String>,
}

fn runtime_payload_inspection(plugin_root: &Path) -> RuntimePayloadInspection {
    let archive_path = plugin_root.join("runtime.zip");
    if !archive_path.is_file() {
        return RuntimePayloadInspection::default();
    }
    let mut inspection = RuntimePayloadInspection::default();
    let file = match File::open(&archive_path) {
        Ok(file) => file,
        Err(error) => {
            inspection.errors.push(format!(
                "runtime payload cannot be opened at {}: {error}",
                archive_path.display()
            ));
            return inspection;
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            inspection
                .errors
                .push(format!("runtime payload zip cannot be read: {error}"));
            return inspection;
        }
    };
    let Some(package_json_entry) = find_runtime_payload_package_json(&mut archive) else {
        inspection
            .errors
            .push("runtime payload does not declare a package.json entrypoint".to_string());
        return inspection;
    };
    let package_text = match archive.by_name(&package_json_entry) {
        Ok(mut entry) => {
            let mut text = String::new();
            if let Err(error) = entry.read_to_string(&mut text) {
                inspection.errors.push(format!(
                    "runtime payload package.json cannot be read: {error}"
                ));
                return inspection;
            }
            text
        }
        Err(error) => {
            inspection.errors.push(format!(
                "runtime payload package.json cannot be opened: {error}"
            ));
            return inspection;
        }
    };
    let package: serde_json::Value = match serde_json::from_str(&package_text) {
        Ok(package) => package,
        Err(error) => {
            inspection.errors.push(format!(
                "runtime payload package.json cannot be parsed: {error}"
            ));
            return inspection;
        }
    };
    let package_dir = package_json_entry
        .strip_suffix("package.json")
        .unwrap_or_default();
    for declared in package_json_declared_entrypoints(&package) {
        match resolve_zip_package_entrypoint(package_dir, &declared) {
            Some(entrypoint) if archive.by_name(&entrypoint).is_ok() => {
                inspection.entrypoints.push(PathBuf::from(entrypoint));
            }
            Some(entrypoint) => inspection.errors.push(format!(
                "runtime payload package.json declares missing entrypoint '{entrypoint}'"
            )),
            None => inspection.errors.push(format!(
                "runtime payload package.json declares unsafe entrypoint '{declared}'"
            )),
        }
    }
    inspection.entrypoints.sort();
    inspection.entrypoints.dedup();
    inspection.errors.sort();
    inspection.errors.dedup();
    if inspection.entrypoints.is_empty() && inspection.errors.is_empty() {
        inspection.errors.push(
            "runtime payload package.json does not declare a bin or main entrypoint".to_string(),
        );
    }
    inspection
}

fn find_runtime_payload_package_json(archive: &mut zip::ZipArchive<File>) -> Option<String> {
    if archive.by_name("package.json").is_ok() {
        return Some("package.json".to_string());
    }
    let mut candidates = Vec::new();
    let limit = archive.len().min(2048);
    for index in 0..limit {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = normalize_zip_entry_name(entry.name());
        if name.ends_with("package.json") && !name.contains("/node_modules/") {
            candidates.push(name);
        }
    }
    candidates.sort_by_key(|name| name.matches('/').count());
    candidates.into_iter().next()
}

fn package_json_declared_entrypoints(package: &serde_json::Value) -> Vec<String> {
    let mut entrypoints = Vec::new();
    match package.get("bin") {
        Some(serde_json::Value::String(path)) => entrypoints.push(path.clone()),
        Some(serde_json::Value::Object(map)) => {
            for value in map.values() {
                if let Some(path) = value.as_str() {
                    entrypoints.push(path.to_string());
                }
            }
        }
        _ => {}
    }
    if entrypoints.is_empty() {
        if let Some(main) = package.get("main").and_then(serde_json::Value::as_str) {
            entrypoints.push(main.to_string());
        }
    }
    entrypoints.sort();
    entrypoints.dedup();
    entrypoints
}

fn resolve_zip_package_entrypoint(package_dir: &str, declared: &str) -> Option<String> {
    let declared = declared.trim().replace('\\', "/");
    if declared.is_empty() || declared.starts_with('/') || declared.contains('\0') {
        return None;
    }
    let mut components = Vec::new();
    for component in declared.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            item => components.push(item),
        }
    }
    if components.is_empty() {
        return None;
    }
    let mut normalized = String::new();
    if !package_dir.is_empty() {
        normalized.push_str(package_dir);
    }
    normalized.push_str(&components.join("/"));
    Some(normalized)
}

fn normalize_zip_entry_name(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PluginRuntimeNeeds {
    node: bool,
    python: bool,
    python_imports: BTreeSet<String>,
    java: bool,
    shell: bool,
    command_probes: BTreeSet<PluginCommandProbe>,
}

impl PluginRuntimeNeeds {
    fn is_empty(&self) -> bool {
        !self.node
            && !self.python
            && self.python_imports.is_empty()
            && !self.java
            && !self.shell
            && self.command_probes.is_empty()
    }

    fn requires_runtime(&self) -> bool {
        !self.is_empty()
    }

    fn merge_script_requirements(&mut self, root: &Path) {
        scan_runtime_script_dirs(root, self);
        self.command_probes
            .extend(documented_runtime_command_probes(root));
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

    fn merge_python_requirements_file(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for import_name in python_requirement_imports(&text) {
            self.python = true;
            self.python_imports.insert(import_name);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PluginCommandProbe {
    program: String,
    args: Vec<String>,
}

impl PluginCommandProbe {
    fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
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

fn host_command_component_target_is_registered(component: &str) -> bool {
    let component = component.trim().to_ascii_lowercase();
    if component.is_empty() {
        return false;
    }
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
    let registry = host_plugin_registry();
    registry
        .builtin_components
        .iter()
        .chain(registry.tauri_components.iter())
        .any(|registered| registered == &component)
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
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("requirements.txt"))
            {
                needs.merge_python_requirements_file(&path);
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
    if matches!(
        program.to_ascii_lowercase().as_str(),
        "python" | "python.exe"
    ) && !candidates
        .iter()
        .any(|candidate| candidate == &PathBuf::from("python3"))
    {
        candidates.push(PathBuf::from("python3"));
    }
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

fn python_runtime_candidate() -> Option<PathBuf> {
    runtime_probe_candidates("python")
        .into_iter()
        .find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
}

fn missing_python_imports(needs: &PluginRuntimeNeeds) -> Vec<String> {
    if needs.python_imports.is_empty() {
        return Vec::new();
    }
    let Some(python) = python_runtime_candidate() else {
        return needs.python_imports.iter().cloned().collect();
    };
    needs
        .python_imports
        .iter()
        .filter(|import_name| {
            let script = format!("import {import_name}");
            !Command::new(&python)
                .arg("-c")
                .arg(script)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

fn missing_command_probes(needs: &PluginRuntimeNeeds) -> Vec<String> {
    needs
        .command_probes
        .iter()
        .filter(|probe| !probe_external_command(probe))
        .map(PluginCommandProbe::display)
        .collect()
}

fn probe_external_command(probe: &PluginCommandProbe) -> bool {
    external_command_probe_candidates(&probe.program)
        .into_iter()
        .any(|candidate| {
            Command::new(candidate)
                .args(&probe.args)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandProbeFailureKind {
    Unavailable,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandProbeFailure {
    probe: PluginCommandProbe,
    kind: CommandProbeFailureKind,
}

fn documented_auth_command_failures(root: &Path) -> Vec<CommandProbeFailure> {
    documented_auth_command_probes(root)
        .into_iter()
        .filter_map(|probe| probe_external_command_failure(&probe))
        .collect()
}

fn probe_external_command_failure(probe: &PluginCommandProbe) -> Option<CommandProbeFailure> {
    let mut saw_spawned_process = false;
    for candidate in external_command_probe_candidates(&probe.program) {
        match Command::new(candidate).args(&probe.args).output() {
            Ok(output) if output.status.success() => return None,
            Ok(_) => saw_spawned_process = true,
            Err(_) => {}
        }
    }
    Some(CommandProbeFailure {
        probe: probe.clone(),
        kind: if saw_spawned_process {
            CommandProbeFailureKind::Rejected
        } else {
            CommandProbeFailureKind::Unavailable
        },
    })
}

fn external_command_probe_candidates(program: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(PathBuf::from(program));
    for env_key in external_command_env_keys(program) {
        if let Some(path) = std::env::var_os(env_key).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            if !candidates.iter().any(|candidate| candidate == &path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

fn external_command_env_keys(program: &str) -> Vec<String> {
    let normalized = program
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if normalized.is_empty() {
        Vec::new()
    } else {
        vec![
            format!("DEEPAGENT_{normalized}"),
            format!("DEEPAGENT_PLUGIN_{normalized}"),
            format!("DEEPAGENT_BIN_{normalized}"),
        ]
    }
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

fn python_requirement_imports(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(python_requirement_import_name)
        .collect()
}

fn python_requirement_import_name(line: &str) -> Option<String> {
    let line = line.split('#').next().unwrap_or_default().trim();
    if line.is_empty()
        || line.starts_with('-')
        || line.starts_with('.')
        || line.starts_with("git+")
        || line.starts_with("http://")
        || line.starts_with("https://")
    {
        return None;
    }
    let package = line
        .split(';')
        .next()
        .unwrap_or_default()
        .split(['<', '>', '=', '!', '~'])
        .next()
        .unwrap_or_default()
        .split('[')
        .next()
        .unwrap_or_default()
        .trim();
    if package.is_empty()
        || !package
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return None;
    }
    Some(package.replace('-', "_"))
}

fn documented_runtime_command_probes(root: &Path) -> BTreeSet<PluginCommandProbe> {
    documented_command_probes(root, documented_runtime_command_probe)
}

fn documented_auth_command_probes(root: &Path) -> BTreeSet<PluginCommandProbe> {
    documented_command_probes(root, documented_auth_command_probe)
}

fn documented_command_probes(
    root: &Path,
    parse_probe: fn(&str) -> Option<PluginCommandProbe>,
) -> BTreeSet<PluginCommandProbe> {
    let mut probes = BTreeSet::new();
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
            if !file_type.is_file() || !should_scan_command_probe_file(&path) {
                continue;
            }
            visited_files += 1;
            if visited_files > 512 {
                return probes;
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
            collect_documented_command_probes(&text, &mut probes, parse_probe);
        }
    }
    probes
}

fn should_scan_command_probe_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx" | "txt"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
fn collect_documented_runtime_command_probes(
    text: &str,
    probes: &mut BTreeSet<PluginCommandProbe>,
) {
    collect_documented_command_probes(text, probes, documented_runtime_command_probe);
}

#[cfg(test)]
fn collect_documented_auth_command_probes(text: &str, probes: &mut BTreeSet<PluginCommandProbe>) {
    collect_documented_command_probes(text, probes, documented_auth_command_probe);
}

fn collect_documented_command_probes(
    text: &str,
    probes: &mut BTreeSet<PluginCommandProbe>,
    parse_probe: fn(&str) -> Option<PluginCommandProbe>,
) {
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            continue;
        }
        if let Some(probe) = parse_probe(trimmed) {
            probes.insert(probe);
        }
    }
}

fn documented_runtime_command_probe(line: &str) -> Option<PluginCommandProbe> {
    let line = strip_shell_prompt(line.split('#').next().unwrap_or_default().trim());
    if line.is_empty() || line_contains_shell_control(line) {
        return None;
    }
    let tokens = shell_like_tokens(line);
    let [program, version_arg] = tokens.as_slice() else {
        return None;
    };
    let program = program.trim();
    if !is_bare_executable_name(program) || is_common_shell_command(program) {
        return None;
    }
    let version_arg = version_arg.trim();
    if !matches!(version_arg, "--version" | "-V" | "version") {
        return None;
    }
    Some(PluginCommandProbe {
        program: program.to_string(),
        args: vec![version_arg.to_string()],
    })
}

fn documented_auth_command_probe(line: &str) -> Option<PluginCommandProbe> {
    let line = strip_shell_prompt(line.split('#').next().unwrap_or_default().trim());
    if line.is_empty() || line_contains_shell_control(line) {
        return None;
    }
    let tokens = shell_like_tokens(line);
    let program = tokens.first()?.trim();
    if !is_bare_executable_name(program) || is_common_shell_command(program) {
        return None;
    }
    let args = &tokens[1..];
    let allowed = matches!(args, [verb, noun] if verb == "auth" && noun == "status")
        || matches!(args, [verb, noun] if verb == "login" && noun == "status")
        || matches!(args, [verb] if verb == "whoami");
    if !allowed {
        return None;
    }
    Some(PluginCommandProbe {
        program: program.to_string(),
        args: args.to_vec(),
    })
}

fn credentials_satisfy_documented_auth(root: &Path) -> bool {
    let hints = credential_env_hints(root);
    !hints.is_empty()
        && hints
            .iter()
            .all(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn strip_shell_prompt(line: &str) -> &str {
    line.strip_prefix("$ ")
        .or_else(|| line.strip_prefix("> "))
        .or_else(|| line.strip_prefix("PS> "))
        .unwrap_or(line)
        .trim()
}

fn line_contains_shell_control(line: &str) -> bool {
    ["|", "&&", "||", ";", "$(", "`"]
        .iter()
        .any(|marker| line.contains(marker))
}

fn is_bare_executable_name(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && !program.contains('\\')
        && program
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn is_common_shell_command(program: &str) -> bool {
    matches!(
        program.to_ascii_lowercase().as_str(),
        "cd" | "cp"
            | "curl"
            | "echo"
            | "env"
            | "export"
            | "irm"
            | "mkdir"
            | "mv"
            | "rm"
            | "set"
            | "sh"
            | "bash"
            | "pwsh"
            | "powershell"
            | "test"
    )
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
    let missing_imports = missing_python_imports(needs);
    let missing_commands = missing_command_probes(needs);
    if parts.is_empty() && missing_commands.is_empty() {
        if missing_imports.is_empty() {
            "runtime is unavailable".to_string()
        } else {
            format!(
                "python dependency imports unavailable: {}",
                missing_imports.join(", ")
            )
        }
    } else if missing_imports.is_empty() && missing_commands.is_empty() {
        format!("runtime unavailable: {}", parts.join(", "))
    } else {
        let mut messages = Vec::new();
        if !parts.is_empty() {
            messages.push(format!("runtime unavailable: {}", parts.join(", ")));
        }
        if !missing_imports.is_empty() {
            messages.push(format!(
                "python dependency imports unavailable: {}",
                missing_imports.join(", ")
            ));
        }
        if !missing_commands.is_empty() {
            messages.push(format!(
                "external command probes unavailable: {}",
                missing_commands.join(", ")
            ));
        }
        messages.join("; ")
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

fn mark_plugin_health_stale_for_install(
    state: &mut PluginState,
    id: &str,
    previous: Option<&InstalledPluginState>,
    version: Option<&str>,
    content_hash: Option<&str>,
) -> bool {
    if state.health_checks.contains_key(id)
        && !plugin_health_matches_install(previous, version, content_hash)
    {
        state.health_checks.remove(id);
    }
    !state.health_checks.contains_key(id)
}

fn plugin_health_matches_install(
    previous: Option<&InstalledPluginState>,
    version: Option<&str>,
    content_hash: Option<&str>,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    match (previous.content_hash.as_deref(), content_hash) {
        (Some(previous_hash), Some(current_hash)) => previous_hash == current_hash,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => previous.version.as_deref() == version,
    }
}

fn lightweight_runtime_available(
    needs: &PluginRuntimeNeeds,
    persisted_health: Option<&PluginHealthCheckState>,
) -> bool {
    if !needs.requires_runtime() {
        return true;
    }
    matches!(
        persisted_health.map(|health| health.status),
        Some(
            PluginHealthStatus::Ready
                | PluginHealthStatus::NeedsConfiguration
                | PluginHealthStatus::NeedsAuthorization
                | PluginHealthStatus::ConnectionUnavailable
                | PluginHealthStatus::Incomplete
        )
    )
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

fn plugin_capabilities(
    manifest: Option<&PluginManifest>,
    counts: PluginComponentCounts,
) -> Vec<String> {
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
    capabilities
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
    create_temp_staging_dir(
        &install_root.join(".staging"),
        &sanitize_file_name(label),
        "plugin staging",
    )
}

fn create_temp_staging_dir(base: &Path, label: &str, purpose: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(base).map_err(|e| {
        CoreError::Persistence(format!("create {purpose} root {}: {e}", base.display()))
    })?;
    let prefix = format!("{}-{}-", sanitize_file_name(label), now_string());
    let temp_dir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(base)
        .map_err(|e| {
            CoreError::Persistence(format!(
                "create {purpose} dir under {}: {e}",
                base.display()
            ))
        })?;
    Ok(temp_dir.keep())
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

    let backup_root = create_plugin_staging_dir(install_root, "rollback")?;
    let backup = backup_root.join("previous");
    std::fs::rename(destination, &backup).map_err(|e| {
        let _ = remove_dir_all_with_retry(&backup_root);
        CoreError::Persistence(format!(
            "backup existing plugin directory {} -> {}: {e}",
            destination.display(),
            backup.display()
        ))
    })?;

    match std::fs::rename(stage, destination) {
        Ok(()) => {
            if let Err(error) = remove_dir_all_with_retry(&backup_root) {
                tracing::warn!(
                    path = %backup_root.display(),
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
                Ok(()) => {
                    let _ = remove_dir_all_with_retry(&backup_root);
                    Err(CoreError::Persistence(format!(
                        "activate plugin directory {} -> {} failed and previous version was restored: {activate_err}",
                        stage.display(),
                        destination.display()
                    )))
                }
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

fn plugin_runtime_priority(origin: PluginOrigin) -> u8 {
    match origin {
        PluginOrigin::BuiltIn => 10,
        PluginOrigin::Personal => 30,
        PluginOrigin::Workspace => 40,
        PluginOrigin::Session => 50,
    }
}

/// Normalize a display name into a filesystem-safe slug. Shared by plugin creation and
/// marketplace-free source keys; returns empty when nothing survives.
fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
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
    _state: &PluginState,
) -> Vec<PathBuf> {
    let mut paths = runtime_watch_paths(roots, state_path, loaded);
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

    fn write_plugin_with_runtime_payload(
        root: &Path,
        name: &str,
        package_json: serde_json::Value,
        extra_files: &[(&str, &[u8])],
    ) {
        write_plugin_with_version_and_dependencies(root, name, "0.1.0", &[]);
        let manifest = serde_json::json!({
            "name": name,
            "version": "0.1.0",
            "runtime": {"node": ">=20"},
            "skills": "skills",
        });
        std::fs::write(
            root.join(".codex-plugin").join("plugin.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        let file = File::create(root.join("runtime.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        zip.start_file("package.json", opts).unwrap();
        zip.write_all(serde_json::to_string(&package_json).unwrap().as_bytes())
            .unwrap();
        for (relative, contents) in extra_files {
            zip.start_file(*relative, opts).unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap();
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

    fn write_plugin_with_stdio_mcp(
        root: &Path,
        name: &str,
        command: &str,
        args: Vec<&str>,
        cwd: Option<&str>,
    ) {
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
        let mut server = serde_json::json!({
            "type": "stdio",
            "command": command,
            "args": args,
        });
        if let Some(cwd) = cwd {
            server.as_object_mut().unwrap().insert(
                "cwd".to_string(),
                serde_json::Value::String(cwd.to_string()),
            );
        }
        std::fs::write(
            root.join(".mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "local": server
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn minimal_mcp_node_server() -> &'static str {
        r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });

function send(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}

rl.on('line', (line) => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') {
    send(req.id, {
      protocolVersion: req.params.protocolVersion,
      serverInfo: { name: 'fixture-sidecar', version: '0.1.0' },
      capabilities: { tools: {} }
    });
    return;
  }
  if (req.method === 'tools/list') {
    send(req.id, {
      tools: [
        {
          name: 'fixture_echo',
          description: 'fixture echo tool',
          inputSchema: { type: 'object', properties: {} }
        }
      ]
    });
    return;
  }
  process.stdout.write(JSON.stringify({
    jsonrpc: '2.0',
    id: req.id,
    error: { code: -32601, message: 'method not found' }
  }) + '\n');
});
"#
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

        assert_eq!(plugin.plugin_id, plugin.id);
        let serialized = serde_json::to_value(&plugin).unwrap();
        assert_eq!(
            serialized.get("plugin_id").and_then(|value| value.as_str()),
            Some(plugin.id.as_str())
        );
        assert!(plugin.installed);
        assert_eq!(plugin.state, PluginLifecycleState::Parsed);
        assert_eq!(plugin.health_status, PluginHealthStatus::Ready);
        assert!(plugin.entrypoints.is_empty());
        assert!(!plugin.runtime_required);
    }

    #[test]
    fn list_summaries_excludes_runtime_and_diagnostic_details() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let summaries = svc.list_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "demo@builtin");
        assert!(summaries[0].enabled);
        let serialized = serde_json::to_value(&summaries[0]).unwrap();
        for field in [
            "health_status",
            "runtime_available",
            "entrypoints",
            "permissions",
            "errors",
            "data_dir",
        ] {
            assert!(
                serialized.get(field).is_none(),
                "{field} leaked into summary"
            );
        }
        assert!(!tmp
            .path()
            .join("app-data")
            .join("plugins")
            .join("data")
            .exists());
    }

    #[test]
    fn list_summaries_refreshes_after_toggle() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        assert!(svc.list_summaries().unwrap()[0].enabled);
        svc.set_enabled("demo@builtin", false).unwrap();
        assert!(!svc.list_summaries().unwrap()[0].enabled);
    }

    #[test]
    fn listing_output_styles_does_not_prepare_plugin_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let plugin_root = roots.builtin.join("demo");
        write_plugin_with_app_component_and_output_style(&plugin_root, "demo", "builtin:browser");
        std::fs::write(plugin_root.join("runtime.zip"), b"unused runtime archive").unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let styles = svc.list_output_styles().unwrap();

        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].plugin_id, "demo@builtin");
        assert!(!tmp
            .path()
            .join("app-data")
            .join("plugins")
            .join("data")
            .exists());
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
            &roots.builtin.join("unregistered-host"),
            "unregistered-host",
            "unregistered-control",
            "builtin:unregistered-host",
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("unregistered-host@builtin").unwrap().unwrap();

        assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        let health_error = plugin.health_error.as_deref().unwrap_or_default();
        assert!(health_error.contains("host app component"));
        assert!(health_error.contains(
            "host command 'unregistered-control' is not registered in the desktop host command registry"
        ));
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

        assert_eq!(plugin.execution_kind, PluginExecutionKind::McpSidecar);
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
        assert_eq!(
            after.health_status,
            PluginHealthStatus::ConnectionUnavailable
        );
        assert!(after.last_health_check.as_deref().is_some());
        assert!(after
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("hosted MCP endpoint unavailable"));
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
    fn check_plugin_health_verifies_stdio_mcp_sidecar_entrypoint_and_plugin_data() {
        if !probe_runtime("node", &["--version"]) {
            eprintln!("skipping: node runtime is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("sidecar-plugin");
        write_plugin_with_stdio_mcp(
            &root,
            "sidecar-plugin",
            "node",
            vec!["server.js"],
            Some("${PLUGIN_ROOT}"),
        );
        std::fs::write(root.join("server.js"), minimal_mcp_node_server()).unwrap();
        let app_data = tmp.path().join("app-data");
        let svc = PluginService::new(roots, &app_data);

        let before = svc.read("sidecar-plugin@builtin").unwrap().unwrap();
        assert_eq!(before.execution_kind, PluginExecutionKind::McpSidecar);
        assert!(before.runtime_required);
        assert!(!before.runtime_available);
        assert_eq!(before.health_status, PluginHealthStatus::Unknown);

        let checked = svc
            .check_plugin_health("sidecar-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(checked.health_status, PluginHealthStatus::Ready);
        assert!(checked.runtime_available);
        assert_eq!(checked.state, PluginLifecycleState::Executable);
        assert!(checked.health_error.is_none());
        assert!(
            svc.data_root
                .join(sanitize_file_name("sidecar-plugin@builtin"))
                .is_dir(),
            "PLUGIN_DATA should be created before a sidecar subprocess is considered healthy"
        );
    }

    #[test]
    fn check_plugin_health_marks_stdio_mcp_protocol_failure_failed() {
        if !probe_runtime("node", &["--version"]) {
            eprintln!("skipping: node runtime is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("sidecar-plugin");
        write_plugin_with_stdio_mcp(
            &root,
            "sidecar-plugin",
            "node",
            vec!["server.js"],
            Some("${PLUGIN_ROOT}"),
        );
        std::fs::write(root.join("server.js"), "process.exit(0);\n").unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("sidecar-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(checked.health_status, PluginHealthStatus::Failed);
        assert_eq!(checked.state, PluginLifecycleState::Failed);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("initialize/tools handshake"));
    }

    #[test]
    fn check_plugin_health_marks_stdio_mcp_missing_script_incomplete() {
        if !probe_runtime("node", &["--version"]) {
            eprintln!("skipping: node runtime is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("sidecar-plugin");
        write_plugin_with_stdio_mcp(
            &root,
            "sidecar-plugin",
            "node",
            vec!["missing.js"],
            Some("${PLUGIN_ROOT}"),
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("sidecar-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(checked.health_status, PluginHealthStatus::Incomplete);
        assert_eq!(checked.state, PluginLifecycleState::Incomplete);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("missing.js"));
    }

    #[test]
    fn check_plugin_health_marks_stdio_mcp_missing_command_runtime_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("sidecar-plugin");
        write_plugin_with_stdio_mcp(
            &root,
            "sidecar-plugin",
            "deepagent-definitely-missing-sidecar",
            Vec::new(),
            Some("${PLUGIN_ROOT}"),
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("sidecar-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(
            checked.health_status,
            PluginHealthStatus::RuntimeUnavailable
        );
        assert_eq!(checked.state, PluginLifecycleState::Incomplete);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("deepagent-definitely-missing-sidecar"));
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
    fn python_requirements_mark_missing_imports_unavailable_without_name_hardcoding() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::create_dir_all(root.join("skills").join("demo").join("scripts")).unwrap();
        std::fs::write(
            root.join("skills")
                .join("demo")
                .join("scripts")
                .join("requirements.txt"),
            "deepagent-definitely-missing-python-dependency-xyz==0.0.1\n",
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("analysis-plugin@builtin").unwrap().unwrap();

        assert!(plugin.runtime_required);
        assert!(!plugin.runtime_available);
        assert_eq!(plugin.health_status, PluginHealthStatus::Unknown);
        assert!(plugin.health_error.is_none());

        let checked = svc
            .check_plugin_health("analysis-plugin@builtin")
            .unwrap()
            .unwrap();

        assert!(!checked.runtime_available);
        assert_eq!(
            checked.health_status,
            PluginHealthStatus::RuntimeUnavailable
        );
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("deepagent_definitely_missing_python_dependency_xyz"));
    }

    #[test]
    fn python_requirements_parser_handles_versions_markers_and_comments() {
        let imports = python_requirement_imports(
            r#"
            # Probe first with python -c "import gemmi, numpy"
            gemmi==0.7.5
            numpy==1.26.4; python_version < "3.11"
            numpy==2.4.6; python_version >= "3.11"
            opencv-python[headless]>=4.0 # distribution name differs from import in some packages
            -r nested.txt
            https://example.invalid/pkg.tar.gz
            "#,
        );

        assert!(imports.contains("gemmi"));
        assert!(imports.contains("numpy"));
        assert!(imports.contains("opencv_python"));
        assert_eq!(imports.len(), 3);
    }

    #[test]
    fn documented_cli_version_probe_marks_missing_command_runtime_unavailable_without_name_hardcoding(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::write(
            root.join("README.md"),
            r#"
            Verify installation:

            ```sh
            deepagent-definitely-missing-cli --version
            ```
            "#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("analysis-plugin@builtin").unwrap().unwrap();

        assert!(plugin.runtime_required);
        assert!(!plugin.runtime_available);
        assert_eq!(plugin.health_status, PluginHealthStatus::Unknown);
        assert!(plugin.health_error.is_none());

        let checked = svc
            .check_plugin_health("analysis-plugin@builtin")
            .unwrap()
            .unwrap();

        assert!(!checked.runtime_available);
        assert_eq!(
            checked.health_status,
            PluginHealthStatus::RuntimeUnavailable
        );
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("deepagent-definitely-missing-cli --version"));
    }

    #[test]
    fn documented_runtime_command_probes_only_accept_safe_version_checks() {
        let mut probes = BTreeSet::new();
        collect_documented_runtime_command_probes(
            r#"
            Outside fences this must not execute:
            boltz-api --version

            ```sh
            boltz-api --version
            boltz-api auth status
            boltz-api protein:design start --input @yaml:///tmp/payload.yaml
            curl -fsSL https://install.example/plugin.sh | sh
            $ gh version
            ```
            "#,
            &mut probes,
        );

        assert!(probes.contains(&PluginCommandProbe {
            program: "boltz-api".to_string(),
            args: vec!["--version".to_string()],
        }));
        assert!(probes.contains(&PluginCommandProbe {
            program: "gh".to_string(),
            args: vec!["version".to_string()],
        }));
        assert_eq!(probes.len(), 2);
    }

    #[test]
    fn documented_auth_status_probe_marks_rejected_check_as_needs_configuration() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPAGENT_DEEPAGENT_TEST_AUTH_CLI", "cargo");

        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::write(
            root.join("README.md"),
            r#"
            ```sh
            deepagent-test-auth-cli auth status
            ```
            "#,
        )
        .unwrap();
        let app_data = tmp.path().join("app-data");
        let svc = PluginService::new(roots.clone(), &app_data);

        let before = svc.read("analysis-plugin@builtin").unwrap().unwrap();
        assert_eq!(before.health_status, PluginHealthStatus::Ready);

        let checked = svc
            .check_plugin_health("analysis-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(
            checked.health_status,
            PluginHealthStatus::NeedsConfiguration
        );
        assert_eq!(checked.state, PluginLifecycleState::RuntimeReady);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("deepagent-test-auth-cli auth status"));

        std::env::remove_var("DEEPAGENT_DEEPAGENT_TEST_AUTH_CLI");
    }

    #[test]
    fn documented_auth_status_probe_marks_missing_command_as_runtime_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::write(
            root.join("README.md"),
            r#"
            ```sh
            deepagent-definitely-missing-auth-cli auth status
            ```
            "#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("analysis-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(
            checked.health_status,
            PluginHealthStatus::RuntimeUnavailable
        );
        assert_eq!(checked.state, PluginLifecycleState::Incomplete);
        assert!(checked
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("deepagent-definitely-missing-auth-cli auth status"));
    }

    #[test]
    fn configured_credential_hint_satisfies_documented_auth_probe() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPAGENT_DEEPAGENT_TEST_AUTH_CLI", "cargo");
        std::env::set_var("DEEPAGENT_TEST_PLUGIN_API_KEY", "configured");

        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("analysis-plugin");
        write_plugin(&root, "analysis-plugin");
        std::fs::write(
            root.join("README.md"),
            r#"
            Requires DEEPAGENT_TEST_PLUGIN_API_KEY.

            ```sh
            deepagent-test-auth-cli auth status
            ```
            "#,
        )
        .unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("analysis-plugin@builtin")
            .unwrap()
            .unwrap();

        assert_eq!(checked.health_status, PluginHealthStatus::Ready);
        assert!(checked.health_error.is_none());

        std::env::remove_var("DEEPAGENT_DEEPAGENT_TEST_AUTH_CLI");
        std::env::remove_var("DEEPAGENT_TEST_PLUGIN_API_KEY");
    }

    #[test]
    fn documented_auth_command_probes_only_accept_safe_status_checks() {
        let mut probes = BTreeSet::new();
        collect_documented_auth_command_probes(
            r#"
            ```sh
            boltz-api auth status
            gh auth status
            gh auth login
            boltz-api auth login --device-code
            npm whoami
            curl https://example.invalid/whoami
            ```
            "#,
            &mut probes,
        );

        assert!(probes.contains(&PluginCommandProbe {
            program: "boltz-api".to_string(),
            args: vec!["auth".to_string(), "status".to_string()],
        }));
        assert!(probes.contains(&PluginCommandProbe {
            program: "gh".to_string(),
            args: vec!["auth".to_string(), "status".to_string()],
        }));
        assert!(probes.contains(&PluginCommandProbe {
            program: "npm".to_string(),
            args: vec!["whoami".to_string()],
        }));
        assert_eq!(probes.len(), 3);
    }

    #[test]
    fn external_command_probe_candidates_prefer_path_before_managed_overrides() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var_os("DEEPAGENT_DEEPAGENT_TEST_CLI");
        let managed = "C:\\managed\\deepagent-test-cli.exe";
        std::env::set_var("DEEPAGENT_DEEPAGENT_TEST_CLI", managed);

        let candidates = external_command_probe_candidates("deepagent-test-cli");

        assert_eq!(candidates[0], PathBuf::from("deepagent-test-cli"));
        assert_eq!(candidates[1], PathBuf::from(managed));

        match original {
            Some(value) => std::env::set_var("DEEPAGENT_DEEPAGENT_TEST_CLI", value),
            None => std::env::remove_var("DEEPAGENT_DEEPAGENT_TEST_CLI"),
        }
    }

    fn write_runtime_probe_sentinel(tmp: &Path, marker: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let script = tmp.join("deepagent-sentinel-probe.cmd");
            std::fs::write(
                &script,
                format!(
                    "@echo off\r\necho ran>\"{}\"\r\nexit /b 0\r\n",
                    marker.display()
                ),
            )
            .unwrap();
            script
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let script = tmp.join("deepagent-sentinel-probe");
            std::fs::write(
                &script,
                format!("#!/bin/sh\necho ran > '{}'\nexit 0\n", marker.display()),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).unwrap();
            script
        }
    }

    #[test]
    fn list_keeps_runtime_probe_metadata_lightweight_until_health_check() {
        let _guard = ENV_LOCK.lock().unwrap();
        let env_key = "DEEPAGENT_DEEPAGENT_SENTINEL_PROBE";
        let original = std::env::var_os(env_key);
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("probe-plugin");
        write_plugin(&root, "probe-plugin");
        std::fs::write(
            root.join("README.md"),
            r#"
            Verify installation:

            ```sh
            deepagent-sentinel-probe --version
            ```
            "#,
        )
        .unwrap();
        let marker = tmp.path().join("probe-ran.txt");
        let probe = write_runtime_probe_sentinel(tmp.path(), &marker);
        std::env::set_var(env_key, &probe);
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("probe-plugin@builtin").unwrap().unwrap();

        assert!(plugin.runtime_required);
        assert!(!plugin.runtime_available);
        assert_eq!(plugin.health_status, PluginHealthStatus::Unknown);
        assert_eq!(plugin.state, PluginLifecycleState::Installed);
        assert!(
            !marker.exists(),
            "plugin list must not execute runtime command probes"
        );
        assert!(svc.load_state().unwrap().health_checks.is_empty());

        let checked = svc
            .check_plugin_health("probe-plugin@builtin")
            .unwrap()
            .unwrap();

        assert!(marker.exists(), "health check should run command probes");
        assert!(checked.runtime_required);
        assert!(checked.runtime_available);
        assert_eq!(checked.health_status, PluginHealthStatus::Ready);
        let state = svc.load_state().unwrap();
        assert_eq!(
            state.health_checks["probe-plugin@builtin"].status,
            PluginHealthStatus::Ready
        );

        match original {
            Some(value) => std::env::set_var(env_key, value),
            None => std::env::remove_var(env_key),
        }
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

        assert_eq!(plugin.execution_kind, PluginExecutionKind::McpSidecar);
        assert_eq!(plugin.health_status, PluginHealthStatus::Unknown);
        assert!(plugin.runtime_required);
        assert!(plugin
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint.ends_with("scripts\\post.sh")
                || entrypoint.ends_with("scripts/post.sh")));

        let checked = svc
            .check_plugin_health("hook-plugin@builtin")
            .unwrap()
            .unwrap();
        assert_eq!(checked.health_status, PluginHealthStatus::Ready);
        assert!(checked.runtime_available);
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
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var_os("DEEPAGENT_NODE");
        let managed = "C:\\managed\\node.exe";
        std::env::set_var("DEEPAGENT_NODE", managed);
        let candidates = runtime_probe_candidates("node");

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], PathBuf::from("node"));
        assert_eq!(candidates[1], PathBuf::from(managed));

        match original {
            Some(value) => std::env::set_var("DEEPAGENT_NODE", value),
            None => std::env::remove_var("DEEPAGENT_NODE"),
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
    fn replace_plugin_dir_activates_new_version_and_cleans_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("install-root");
        let destination = install_root.join("demo");
        let stage = create_plugin_staging_dir(&install_root, "demo").unwrap();
        write_plugin_with_version(&destination, "demo", "0.1.0");
        write_plugin_with_version(&stage, "demo", "0.2.0");

        replace_plugin_dir(&stage, &destination, &install_root).unwrap();

        assert!(destination
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file());
        let manifest =
            std::fs::read_to_string(destination.join(".codex-plugin").join("plugin.json")).unwrap();
        assert!(manifest.contains("\"0.2.0\""));
        assert!(!manifest.contains("\"0.1.0\""));
        assert!(!stage.exists());
        let staging_entries = std::fs::read_dir(install_root.join(".staging"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(staging_entries, 0);
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
        let checked = svc.check_plugin_health(&installed.id).unwrap().unwrap();
        let checked_at = checked
            .last_health_check
            .clone()
            .expect("health check must persist a timestamp");
        svc.set_enabled(&installed.id, false).unwrap();
        let state_before_update = svc.load_state().unwrap();
        let installed_at = state_before_update.installed[&installed.id]
            .installed_at
            .clone();

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_plugin_with_version(&source, "demo", "0.2.0");
        let updated = svc.install_from_dir(&source).unwrap();
        assert_eq!(updated.version.as_deref(), Some("0.2.0"));
        assert_eq!(updated.id, installed.id, "the id must be stable");
        assert!(!updated.enabled, "updates must preserve disabled state");
        assert_ne!(
            updated.last_health_check.as_deref(),
            Some(checked_at.as_str()),
            "content-changing updates must not inherit a previous package's health check"
        );

        svc.runtime_projection().unwrap();
        assert_eq!(
            std::fs::read_to_string(data_dir.join("state.json")).unwrap(),
            r#"{"runs":7}"#,
            "§9.1 requires PLUGIN_DATA contents to survive a plugin update"
        );
        let state_after_update = svc.load_state().unwrap();
        let installed_state = &state_after_update.installed[&installed.id];
        assert_eq!(installed_state.installed_at, installed_at);
        assert!(installed_state.last_updated.is_some());
        assert_eq!(
            state_after_update
                .health_checks
                .get(&installed.id)
                .map(|health| health.checked_at.as_str()),
            updated.last_health_check.as_deref(),
            "content-changing updates must persist a fresh health check for the new package"
        );
    }

    #[test]
    fn runtime_payload_bin_entrypoint_is_reported_from_full_package() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("runtime-demo");
        write_plugin_with_runtime_payload(
            &root,
            "runtime-demo",
            serde_json::json!({
                "name": "runtime-demo",
                "bin": {"runtime-demo": "./dist/cli.js"}
            }),
            &[("dist/cli.js", b"console.log('runtime ok');\n")],
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("runtime-demo@builtin").unwrap().unwrap();

        assert!(plugin.has_runtime_payload);
        assert_eq!(plugin.execution_kind, PluginExecutionKind::ManagedRuntime);
        assert!(plugin.entrypoints.iter().any(|entry| {
            entry
                .replace('\\', "/")
                .ends_with("runtime.zip!dist/cli.js")
        }));
        assert_ne!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert!(!plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("entrypoint"));
    }

    #[test]
    fn prepare_runtime_payloads_extracts_builtin_payloads_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_runtime_payload(
            &roots.builtin.join("runtime-demo"),
            "runtime-demo",
            serde_json::json!({
                "name": "runtime-demo",
                "bin": {"runtime-demo": "./dist/cli.js"}
            }),
            &[("dist/cli.js", b"console.log('runtime ok');\n")],
        );
        let data_root = tmp.path().join("app-data").join("plugins").join("data");

        prepare_runtime_payloads(&roots, &data_root);

        // The id `runtime-demo@builtin` sanitizes to `runtime-demo-builtin`;
        // the payload and the marker must land under exactly that directory so
        // the later runtime-projection call reuses them instead of re-extracting.
        let runtime_dir = data_root.join("runtime-demo-builtin").join("runtime");
        assert!(runtime_dir.join("package.json").is_file());
        assert!(runtime_dir.join("dist").join("cli.js").is_file());
        let marker = runtime_dir.join(".payload-size");
        let first = std::fs::read_to_string(&marker).unwrap();

        // A second call is a no-op: the marker already matches the archive size,
        // so the extraction is skipped and the marker is left untouched.
        prepare_runtime_payloads(&roots, &data_root);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), first);
    }

    #[test]
    fn prepare_runtime_payloads_skips_plugins_without_a_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_version_and_dependencies(
            &roots.builtin.join("plain"),
            "plain",
            "0.1.0",
            &[],
        );
        let data_root = tmp.path().join("app-data").join("plugins").join("data");

        prepare_runtime_payloads(&roots, &data_root);

        assert!(
            !data_root.join("plain-builtin").join("runtime").exists(),
            "a plugin without runtime.zip must not get an extracted runtime"
        );
    }

    #[test]
    fn runtime_payload_missing_declared_entrypoint_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_runtime_payload(
            &roots.builtin.join("runtime-demo"),
            "runtime-demo",
            serde_json::json!({
                "name": "runtime-demo",
                "bin": {"runtime-demo": "./dist/missing.js"}
            }),
            &[],
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let plugin = svc.read("runtime-demo@builtin").unwrap().unwrap();

        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert_eq!(plugin.state, PluginLifecycleState::Incomplete);
        assert!(plugin
            .health_error
            .as_deref()
            .unwrap_or_default()
            .contains("missing entrypoint 'dist/missing.js'"));
    }

    #[test]
    fn check_plugin_health_prepares_runtime_payload_and_checks_node_entrypoint() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let root = roots.builtin.join("runtime-demo");
        write_plugin_with_runtime_payload(
            &root,
            "runtime-demo",
            serde_json::json!({
                "name": "runtime-demo",
                "bin": {"runtime-demo": "./dist/cli.js"}
            }),
            &[(
                "dist/cli.js",
                b"#!/usr/bin/env node\nconsole.log('runtime ok');\n",
            )],
        );
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let checked = svc
            .check_plugin_health("runtime-demo@builtin")
            .unwrap()
            .unwrap();

        let data_dir = svc
            .data_root
            .join(sanitize_file_name("runtime-demo@builtin"));
        assert!(data_dir.join("runtime").join("package.json").is_file());
        assert!(data_dir
            .join("runtime")
            .join("dist")
            .join("cli.js")
            .is_file());
        assert!(data_dir.join("workspace").is_dir());
        assert!(!data_dir.join(".deepagent-healthcheck.tmp").exists());
        if probe_runtime("node", &["--version"]) {
            assert_eq!(checked.health_status, PluginHealthStatus::Ready);
            assert_eq!(checked.state, PluginLifecycleState::Executable);
            assert!(checked.health_error.is_none());
        } else {
            assert_eq!(
                checked.health_status,
                PluginHealthStatus::RuntimeUnavailable
            );
        }
    }

    #[test]
    fn runtime_payload_marker_hit_still_restores_workspace_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("plugin");
        write_plugin_with_runtime_payload(
            &plugin_root,
            "runtime-demo",
            serde_json::json!({
                "name": "runtime-demo",
                "bin": {"runtime-demo": "./dist/cli.js"}
            }),
            &[("dist/cli.js", b"console.log('runtime ok');\n")],
        );
        let data_dir = tmp.path().join("data");

        prepare_runtime_payload(&plugin_root, &data_dir).unwrap();
        std::fs::remove_dir_all(data_dir.join("workspace")).unwrap();
        prepare_runtime_payload(&plugin_root, &data_dir).unwrap();

        assert!(data_dir.join("runtime").join(".payload-size").is_file());
        assert!(data_dir.join("workspace").is_dir());
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
    fn set_enabled_invalidates_cached_runtime_projection() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let stale_projection = svc.runtime_projection().unwrap();
        assert_eq!(
            stale_projection.skill_roots,
            vec![roots.builtin.join("demo").join("skills")]
        );
        svc.store_runtime_projection_cache(stale_projection, Vec::new());

        let disabled = svc.set_enabled("demo@builtin", false).unwrap();

        assert!(!disabled.enabled);
        assert!(
            svc.runtime_projection().unwrap().skill_roots.is_empty(),
            "set_enabled must invalidate runtime projection cache even when file snapshots still match"
        );
    }

    #[test]
    fn runtime_projection_mcp_overlay_refreshes_on_enable_disable_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        write_plugin_with_stdio_mcp(
            &roots.builtin.join("mcp-demo"),
            "mcp-demo",
            "node",
            vec!["server.js"],
            None,
        );
        let plugins = PluginService::new(roots, tmp.path().join("app-data"));
        let mcp = crate::mcp_service::McpService::new(std::sync::Arc::new(
            deepagent_persistence::Database::open_in_memory().unwrap(),
        ));

        let initial = plugins.runtime_projection().unwrap();
        let effective = mcp
            .enabled_config_with_plugin_overlay(
                initial.mcp_config.clone(),
                &initial.mcp_server_sources,
            )
            .unwrap();
        assert_eq!(effective.servers.len(), 1);
        let runtime_name = effective.servers.keys().next().unwrap().clone();
        assert!(initial.mcp_server_sources.contains_key(&runtime_name));

        let disabled = plugins.set_enabled("mcp-demo@builtin", false).unwrap();
        assert!(!disabled.enabled);
        let disabled_projection = plugins.runtime_projection().unwrap();
        assert!(disabled_projection.mcp_config.servers.is_empty());
        assert!(disabled_projection.mcp_server_sources.is_empty());
        let disabled_effective = mcp
            .enabled_config_with_plugin_overlay(
                disabled_projection.mcp_config,
                &disabled_projection.mcp_server_sources,
            )
            .unwrap();
        assert!(disabled_effective.servers.is_empty());

        let enabled = plugins.set_enabled("mcp-demo@builtin", true).unwrap();
        assert!(enabled.enabled);
        let refreshed = plugins.runtime_projection().unwrap();
        let refreshed_effective = mcp
            .enabled_config_with_plugin_overlay(
                refreshed.mcp_config.clone(),
                &refreshed.mcp_server_sources,
            )
            .unwrap();
        assert!(refreshed_effective.servers.contains_key(&runtime_name));
        assert!(refreshed.mcp_server_sources.contains_key(&runtime_name));
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
    fn listing_app_cards_does_not_prepare_plugin_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path());
        let plugin_root = roots.builtin.join("demo");
        write_plugin_with_app_component_and_output_style(&plugin_root, "demo", "builtin:browser");
        std::fs::write(plugin_root.join("runtime.zip"), b"unused runtime archive").unwrap();
        let svc = PluginService::new(roots, tmp.path().join("app-data"));

        let apps = svc.list_apps().unwrap();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].plugin_id, "demo@builtin");
        assert!(!tmp
            .path()
            .join("app-data")
            .join("plugins")
            .join("data")
            .exists());
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
            "builtin:unregistered-host",
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
            .contains("builtin:unregistered-host"));
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
}
