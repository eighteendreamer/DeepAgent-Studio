//! UI-facing plugin service.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    pub developer: Option<String>,
    pub source: PluginSourceDto,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
    pub app_count: u32,
    pub output_style_count: u32,
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
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            version: PLUGIN_STATE_SCHEMA_VERSION,
            enabled: BTreeMap::new(),
            installed: BTreeMap::new(),
            marketplaces: BTreeMap::new(),
        }
    }
}

pub struct PluginService {
    roots: PluginRoots,
    state_path: PathBuf,
    data_root: PathBuf,
}

impl PluginService {
    pub fn new(roots: PluginRoots, app_data_dir: impl AsRef<Path>) -> Self {
        let plugin_data = app_data_dir.as_ref().join("plugins");
        Self {
            roots,
            state_path: plugin_data.join("state.json"),
            data_root: plugin_data.join("data"),
        }
    }

    pub fn roots(&self) -> &PluginRoots {
        &self.roots
    }

    pub fn list(&self) -> Result<Vec<PluginDto>> {
        if let Err(error) = self.cleanup_orphaned_cache() {
            tracing::warn!(error = %error, "failed to clean orphaned plugin cache");
        }
        let state = self.load_state()?;
        let loaded = load_plugins(&self.roots);
        let dependencies = self.dependency_outcome(&loaded, &state);
        Ok(loaded
            .iter()
            .map(|plugin| self.dto_from_loaded(plugin, &loaded, &state, &dependencies))
            .collect())
    }

    pub fn reload(&self) -> Result<Vec<PluginDto>> {
        self.list()
    }

    pub fn read(&self, id: &str) -> Result<Option<PluginDto>> {
        Ok(self.list()?.into_iter().find(|plugin| plugin.id == id))
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

        if destination.exists() {
            safe_remove_dir(&self.roots.personal, &destination)?;
        }
        copy_dir(source, &destination)?;

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
                    .map(|plugin| plugin.manifest.is_some())
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
                            .and_then(|plugin| plugin.manifest.as_ref()?.version.as_deref())
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
        let (_, entry) = self.marketplace_entry(&marketplace_key, plugin)?;
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

        let materialized = self.materialize_marketplace_plugin_source(&marketplace_key, &entry)?;
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
            .join(sanitize_file_name(&marketplace_key));
        let plugin_dir = marketplace_dir.join(sanitize_file_name(&manifest.name));
        if plugin_dir.exists() {
            mark_stale_marketplace_plugin_versions(
                &self.roots.marketplace_cache,
                &plugin_dir,
                &plugin_dir.join(sanitize_file_name(&version)),
            )?;
        }
        let destination = plugin_dir.join(sanitize_file_name(&version));
        copy_dir(source, &destination)?;

        let id = plugin_id(&manifest.name, &marketplace_key);
        let mut state = self.load_state()?;
        state.enabled.insert(id.clone(), true);
        state.installed.insert(
            id.clone(),
            InstalledPluginState {
                version: manifest.version.clone().or(Some(version)),
                install_path: destination.display().to_string(),
                installed_at: now_string(),
                last_updated: None,
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
        let inputs = enabled_plugins.into_iter().filter_map(|plugin| {
            let manifest = plugin.manifest.as_ref()?;
            Some(EnabledPluginRuntimeInput {
                id: &plugin.id,
                name: &plugin.name,
                source_priority: plugin_runtime_priority(plugin.origin),
                root: &plugin.root,
                data_dir: self.data_root.join(sanitize_file_name(&plugin.id)),
                manifest,
            })
        });
        Ok(PluginRuntimeProjection::from_enabled_plugins(inputs))
    }

    pub fn list_apps(&self) -> Result<Vec<crate::plugin_runtime::PluginAppEntry>> {
        Ok(self.runtime_projection()?.app_entries)
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
            && plugin.manifest.is_some()
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
        let Some(manifest) = plugin.manifest.as_ref() else {
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
        versions_differ(entry.version.as_deref(), manifest.version.as_deref())
    }

    fn dto_from_loaded(
        &self,
        plugin: &LoadedPlugin,
        loaded: &[LoadedPlugin],
        state: &PluginState,
        dependencies: &PluginDependencyOutcome,
    ) -> PluginDto {
        let available = plugin.available && plugin.manifest.is_some();
        let enabled = self.is_effectively_enabled(plugin, state)
            && !dependencies.demoted.contains(&plugin.id);
        let manifest = plugin.manifest.as_ref();
        let skill_count = manifest.map(count_skills).unwrap_or_default();
        let mcp_server_count = manifest.map(count_mcp_servers).unwrap_or_default();
        let hook_count = manifest.map(count_hooks).unwrap_or_default();
        let command_count = manifest.map(count_commands).unwrap_or_default();
        let app_count = manifest.map(count_apps).unwrap_or_default();
        let output_style_count = manifest.map(count_output_styles).unwrap_or_default();
        let mut capabilities = manifest
            .map(|m| m.interface.capabilities.clone())
            .unwrap_or_default();
        if capabilities.is_empty() {
            if skill_count > 0 {
                capabilities.push("Skill".to_string());
            }
            if mcp_server_count > 0 {
                capabilities.push("MCP".to_string());
            }
            if hook_count > 0 {
                capabilities.push("Hooks".to_string());
            }
            if app_count > 0 {
                capabilities.push("App".to_string());
            }
            if output_style_count > 0 {
                capabilities.push("Output Style".to_string());
            }
        }

        let mut errors = plugin.errors.clone();
        errors.extend_from_slice(dependencies.errors_for(&plugin.id));
        let required_by = self.reverse_dependents(plugin, loaded, state, dependencies);

        PluginDto {
            id: plugin.id.clone(),
            name: plugin.name.clone(),
            display_name: manifest
                .map(PluginManifest::display_name)
                .unwrap_or_else(|| plugin.name.clone()),
            description: manifest
                .map(PluginManifest::short_description)
                .unwrap_or_else(|| "Plugin failed to load".to_string()),
            long_description: manifest.and_then(PluginManifest::long_description),
            version: manifest.and_then(|m| m.version.clone()),
            local_version: manifest.and_then(|m| m.version.clone()),
            developer: manifest.and_then(PluginManifest::developer_name),
            source: PluginSourceDto {
                kind: plugin.origin.as_str().to_string(),
                name: plugin.source_key.clone(),
                marketplace: plugin.marketplace.clone(),
                path: Some(plugin.root.display().to_string()),
            },
            origin: plugin.origin.as_str().to_string(),
            path: Some(plugin.root.display().to_string()),
            manifest_path: manifest.map(|m| m.manifest_path.display().to_string()),
            installed: manifest.is_some(),
            enabled,
            available,
            update_available: self.plugin_update_available(plugin, state),
            overridden_by: plugin.overridden_by.clone(),
            category: manifest.and_then(|m| m.interface.category.clone()),
            keywords: manifest.map(|m| m.keywords.clone()).unwrap_or_default(),
            capabilities,
            permissions: manifest
                .map(|m| m.interface.permissions.clone())
                .unwrap_or_default(),
            skill_count,
            mcp_server_count,
            hook_count,
            command_count,
            app_count,
            output_style_count,
            icon_path: manifest
                .and_then(|m| m.interface.composer_icon.as_ref())
                .map(path_string),
            logo_path: manifest
                .and_then(|m| m.interface.logo.as_ref())
                .map(path_string),
            brand_color: manifest.and_then(|m| m.interface.brand_color.clone()),
            required_by,
            errors,
        }
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
                        .manifest
                        .as_ref()
                        .map(PluginManifest::display_name)
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
        write_file_atomically(
            &self.state_path,
            serde_json::to_string_pretty(&state)
                .map_err(CoreError::from)?
                .as_bytes(),
        )
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
            } => {
                let staging = self.marketplace_staging_dir(marketplace, &entry.name)?;
                let plugin_root = materialize_git_plugin(
                    url,
                    path.as_deref(),
                    ref_name.as_deref(),
                    sha.as_deref(),
                    &staging,
                )?;
                Ok(MaterializedPluginSource::staged(plugin_root, staging))
            }
            PluginMarketplaceSource::GitHub {
                repo,
                path,
                ref_name,
                sha,
                ..
            } => {
                let staging = self.marketplace_staging_dir(marketplace, &entry.name)?;
                let url = format!("https://github.com/{repo}.git");
                let plugin_root = materialize_git_plugin(
                    &url,
                    path.as_deref(),
                    ref_name.as_deref(),
                    sha.as_deref(),
                    &staging,
                )?;
                Ok(MaterializedPluginSource::staged(plugin_root, staging))
            }
            PluginMarketplaceSource::GitSubdir {
                url,
                path,
                ref_name,
                sha,
                ..
            } => {
                let staging = self.marketplace_staging_dir(marketplace, &entry.name)?;
                let plugin_root = materialize_git_plugin(
                    url,
                    Some(path),
                    ref_name.as_deref(),
                    sha.as_deref(),
                    &staging,
                )?;
                Ok(MaterializedPluginSource::staged(plugin_root, staging))
            }
            PluginMarketplaceSource::ZipUrl { url, sha256, .. } => {
                let staging = self.marketplace_staging_dir(marketplace, &entry.name)?;
                let archive = staging.join("plugin.zip");
                download_file(url, &archive)?;
                if let Some(expected) = sha256.as_deref() {
                    verify_sha256_file(&archive, expected)?;
                }
                let unpacked = staging.join("unpacked");
                extract_zip_into(&archive, &unpacked)?;
                let plugin_root = resolve_plugin_root(&unpacked)?;
                Ok(MaterializedPluginSource::staged(plugin_root, staging))
            }
            PluginMarketplaceSource::Npm {
                package,
                version,
                registry,
                ..
            } => {
                let staging = self.marketplace_staging_dir(marketplace, &entry.name)?;
                let plugin_root = materialize_npm_plugin(
                    package,
                    version.as_deref(),
                    registry.as_deref(),
                    &staging,
                )?;
                Ok(MaterializedPluginSource::staged(plugin_root, staging))
            }
            PluginMarketplaceSource::Unsupported { kind, .. } => Err(CoreError::invalid(format!(
                "marketplace source '{kind}' is not supported"
            ))),
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

#[derive(Default)]
struct MaterializedMarketplace {
    manifest_path: Option<PathBuf>,
    source_root: Option<PathBuf>,
}

struct MaterializedPluginSource {
    plugin_root: PathBuf,
    cleanup_root: Option<PathBuf>,
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
    }

    let root = match subdir {
        Some(subdir) => safe_join_materialized_subdir(&repo_dir, subdir)?,
        None => repo_dir,
    };
    resolve_plugin_root(&root)
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
            let orphan = unique_orphan_cache_dir(plugin_dir, active_name)?;
            move_managed_child_dir(base, &path, &orphan)?;
            mark_cache_dir_orphaned(base, &orphan)?;
        } else if !is_cache_dir_orphaned(&path) {
            mark_cache_dir_orphaned(base, &path)?;
        }
    }
    Ok(())
}

fn unique_orphan_cache_dir(plugin_dir: &Path, version: &std::ffi::OsStr) -> Result<PathBuf> {
    let version = version.to_string_lossy();
    let version = sanitize_file_name(&version);
    let version = version.trim_matches('.');
    let version = if version.is_empty() {
        "version"
    } else {
        version
    };
    let now = now_string();
    for attempt in 0..100u32 {
        let candidate = plugin_dir.join(format!(
            ".orphaned-{version}-{}-{now}-{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CoreError::other(format!(
        "failed to allocate orphaned plugin cache dir under {}",
        plugin_dir.display()
    )))
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

fn move_managed_child_dir(base: &Path, source: &Path, target: &Path) -> Result<()> {
    let base = std::fs::canonicalize(base).map_err(|e| {
        CoreError::Persistence(format!("canonicalize cache base {}: {e}", base.display()))
    })?;
    let source = std::fs::canonicalize(source).map_err(|e| {
        CoreError::Persistence(format!(
            "canonicalize plugin cache source {}: {e}",
            source.display()
        ))
    })?;
    if source == base || !source.starts_with(&base) {
        return Err(CoreError::invalid(format!(
            "refusing to move path outside plugin cache: {}",
            source.display()
        )));
    }
    let parent = target.parent().ok_or_else(|| {
        CoreError::invalid(format!(
            "plugin cache target has no parent: {}",
            target.display()
        ))
    })?;
    let parent = std::fs::canonicalize(parent).map_err(|e| {
        CoreError::Persistence(format!(
            "canonicalize plugin cache target parent {}: {e}",
            parent.display()
        ))
    })?;
    if !parent.starts_with(&base) {
        return Err(CoreError::invalid(format!(
            "refusing to move plugin cache outside cache root: {}",
            target.display()
        )));
    }
    if target.exists() {
        return Err(CoreError::invalid(format!(
            "plugin cache target already exists: {}",
            target.display()
        )));
    }
    std::fs::rename(&source, target).map_err(|e| {
        CoreError::Persistence(format!(
            "move plugin cache {} -> {}: {e}",
            source.display(),
            target.display()
        ))
    })
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

fn run_external(program: &str, args: &[String], cwd: Option<&Path>) -> Result<()> {
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
        return Ok(());
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

fn path_string(path: &PathBuf) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
                        "component": format!("builtin:{name}"),
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
        assert_eq!(
            installed.last_updated.as_deref(),
            Some("2026-07-22T11:00:00Z")
        );
        let marketplace = state.marketplaces.get("team").unwrap();
        assert_eq!(marketplace.git_ref.as_deref(), Some("main"));
        assert_eq!(marketplace.sparse_path.as_deref(), Some("plugins"));

        svc.save_state(&state).unwrap();
        let rewritten = std::fs::read_to_string(&state_path).unwrap();
        assert!(rewritten.contains("\"version\": 1"));
        assert!(rewritten.contains("install_path"));
        assert!(rewritten.contains("installed_at"));
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
        write_plugin_with_app_and_output_style(&roots.builtin.join("demo"), "demo");
        let svc = PluginService::new(roots.clone(), tmp.path().join("app-data"));

        let plugin = svc.read("demo@builtin").unwrap().unwrap();
        assert_eq!(plugin.app_count, 1);
        assert_eq!(plugin.output_style_count, 1);

        let apps = svc.list_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].plugin_id, "demo@builtin");
        assert_eq!(apps[0].component, "builtin:demo");

        let styles = svc.list_output_styles().unwrap();
        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].plugin_id, "demo@builtin");
        assert_eq!(styles[0].name, "demo:concise");
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
            marketplace_root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "demo",
                  "version": "0.1.0",
                  "description": "Demo marketplace plugin",
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
