//! Runtime projection helpers for enabled plugins.
//!
//! The plugin service keeps packages inert until a caller asks for the
//! effective runtime projection. This module turns enabled plugin manifests
//! into concrete roots/configs that existing subsystems can consume.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::HookDefinitions;
use deepagent_mcp::config::{McpConfig, McpServerConfig};
use serde::{Deserialize, Serialize};

use crate::plugin::spec::placeholder::{PLUGIN_DATA_VAR, PLUGIN_ROOT_VAR};
use crate::plugin_manifest::PluginManifest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommandRoot {
    pub plugin_id: String,
    pub plugin_name: String,
    pub path: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAgentRoot {
    pub plugin_id: String,
    pub plugin_name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMcpServerSource {
    pub plugin_id: String,
    pub plugin_name: String,
    pub declared_name: String,
    pub runtime_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAppEntry {
    pub plugin_id: String,
    pub plugin_name: String,
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub placement: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginOutputStyleEntry {
    pub plugin_id: String,
    pub plugin_name: String,
    pub name: String,
    pub description: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_for_plugin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRuntimeProjection {
    pub skill_roots: Vec<PathBuf>,
    pub mcp_config: McpConfig,
    #[serde(default)]
    pub mcp_server_sources: BTreeMap<String, PluginMcpServerSource>,
    pub hook_definitions: HookDefinitions,
    pub hook_config_paths: Vec<PathBuf>,
    pub command_roots: Vec<PluginCommandRoot>,
    pub agent_roots: Vec<PluginAgentRoot>,
    pub app_config_paths: Vec<PathBuf>,
    pub app_entries: Vec<PluginAppEntry>,
    pub output_styles: Vec<PluginOutputStyleEntry>,
    pub errors: Vec<PluginRuntimeError>,
}

impl Default for PluginRuntimeProjection {
    fn default() -> Self {
        Self {
            skill_roots: Vec::new(),
            mcp_config: McpConfig {
                servers: Default::default(),
            },
            mcp_server_sources: BTreeMap::new(),
            hook_definitions: HookDefinitions::default(),
            hook_config_paths: Vec::new(),
            command_roots: Vec::new(),
            agent_roots: Vec::new(),
            app_config_paths: Vec::new(),
            app_entries: Vec::new(),
            output_styles: Vec::new(),
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRuntimeError {
    pub plugin_id: String,
    pub component: String,
    pub path: Option<String>,
    pub message: String,
}

pub(crate) struct EnabledPluginRuntimeInput<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub source_priority: u8,
    pub root: &'a Path,
    pub data_dir: PathBuf,
    pub manifest: &'a PluginManifest,
}

impl PluginRuntimeProjection {
    pub(crate) fn from_enabled_plugins<'a>(
        plugins: impl IntoIterator<Item = EnabledPluginRuntimeInput<'a>>,
    ) -> Self {
        let mut projection = PluginRuntimeProjection::default();
        let mut plugins = plugins.into_iter().collect::<Vec<_>>();
        plugins.sort_by(|a, b| {
            b.source_priority
                .cmp(&a.source_priority)
                .then_with(|| a.id.cmp(b.id))
        });
        let mut seen_mcp_declared_names = BTreeMap::<String, PluginMcpServerSource>::new();
        for plugin in plugins {
            project_plugin(plugin, &mut projection, &mut seen_mcp_declared_names);
        }
        projection
    }
}

fn project_plugin(
    plugin: EnabledPluginRuntimeInput<'_>,
    projection: &mut PluginRuntimeProjection,
    seen_mcp_declared_names: &mut BTreeMap<String, PluginMcpServerSource>,
) {
    for path in existing_dirs(&plugin.manifest.paths.skills) {
        push_unique_path(&mut projection.skill_roots, path);
    }
    for path in existing_dirs(&plugin.manifest.paths.commands) {
        projection.command_roots.push(PluginCommandRoot {
            plugin_id: plugin.id.to_string(),
            plugin_name: plugin.name.to_string(),
            path,
            data_dir: plugin.data_dir.clone(),
        });
    }
    for path in existing_dirs(&plugin.manifest.paths.agents) {
        projection.agent_roots.push(PluginAgentRoot {
            plugin_id: plugin.id.to_string(),
            plugin_name: plugin.name.to_string(),
            path,
        });
    }
    project_output_styles(plugin.id, plugin.name, plugin.manifest, projection);
    for path in plugin
        .manifest
        .paths
        .app_paths
        .iter()
        .filter(|path| path.exists())
    {
        push_unique_path(&mut projection.app_config_paths, path.clone());
        match load_plugin_apps(plugin.id, plugin.name, path) {
            Ok(mut apps) => projection.app_entries.append(&mut apps),
            Err(error) => push_error(projection, plugin.id, "apps", Some(path), error),
        }
    }

    project_mcp(
        plugin.id,
        plugin.name,
        plugin.root,
        &plugin.data_dir,
        plugin.manifest,
        projection,
        seen_mcp_declared_names,
    );
    project_hooks(
        plugin.id,
        plugin.root,
        &plugin.data_dir,
        plugin.manifest,
        projection,
    );
}

fn project_output_styles(
    plugin_id: &str,
    plugin_name: &str,
    manifest: &PluginManifest,
    projection: &mut PluginRuntimeProjection,
) {
    let mut loaded_paths = Vec::new();
    for path in manifest
        .paths
        .output_styles
        .iter()
        .filter(|path| path.exists())
    {
        match load_plugin_output_styles(plugin_id, plugin_name, path, &mut loaded_paths) {
            Ok(mut styles) => projection.output_styles.append(&mut styles),
            Err(error) => push_error(projection, plugin_id, "output-styles", Some(path), error),
        }
    }
}

fn project_mcp(
    plugin_id: &str,
    plugin_name: &str,
    plugin_root: &Path,
    plugin_data: &Path,
    manifest: &PluginManifest,
    projection: &mut PluginRuntimeProjection,
    seen_declared_names: &mut BTreeMap<String, PluginMcpServerSource>,
) {
    for path in &manifest.paths.mcp_server_paths {
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(path)
            .map_err(|e| CoreError::Persistence(format!("read {}: {e}", path.display())))
            .and_then(|text| McpConfig::parse(&text))
        {
            Ok(config) => merge_mcp_config(
                plugin_id,
                plugin_name,
                plugin_root,
                plugin_data,
                &manifest.runtime,
                config,
                projection,
                Some(path),
                seen_declared_names,
            ),
            Err(error) => push_error(projection, plugin_id, "mcp", Some(path), error),
        }
    }

    if let Some(value) = manifest.paths.mcp_servers_inline.as_ref() {
        match serde_json::to_string(value)
            .map_err(CoreError::from)
            .and_then(|text| McpConfig::parse(&text))
        {
            Ok(config) => merge_mcp_config(
                plugin_id,
                plugin_name,
                plugin_root,
                plugin_data,
                &manifest.runtime,
                config,
                projection,
                None,
                seen_declared_names,
            ),
            Err(error) => push_error(projection, plugin_id, "mcp", None, error),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_mcp_config(
    plugin_id: &str,
    plugin_name: &str,
    plugin_root: &Path,
    plugin_data: &Path,
    runtime: &crate::plugin_manifest::PluginRuntimeRequirements,
    mut config: McpConfig,
    projection: &mut PluginRuntimeProjection,
    path: Option<&PathBuf>,
    seen_declared_names: &mut BTreeMap<String, PluginMcpServerSource>,
) {
    config.expand_with(&|var| plugin_env(var, plugin_id, plugin_root, plugin_data));
    let plugin_key = safe_identifier(plugin_name);
    for (server_name, mut server) in config.servers {
        inject_plugin_env(&mut server, plugin_id, plugin_root, plugin_data);
        inject_plugin_runtime_requirement(&mut server, runtime);
        if let Err(error) = server.validate() {
            push_error(projection, plugin_id, "mcp", path, error);
            continue;
        }
        let declared_key = mcp_declared_key(&server_name);
        if let Some(existing) = seen_declared_names.get(&declared_key) {
            projection.errors.push(PluginRuntimeError {
                plugin_id: plugin_id.to_string(),
                component: "mcp".to_string(),
                path: path.map(|p| p.display().to_string()),
                message: format!(
                    "MCP server '{server_name}' skipped because plugin '{}' already declares '{}'",
                    existing.plugin_id, existing.declared_name
                ),
            });
            continue;
        }
        let server_key = safe_identifier(&server_name);
        let namespaced = format!("plugin__{plugin_key}__{server_key}");
        if projection.mcp_config.servers.contains_key(&namespaced) {
            projection.errors.push(PluginRuntimeError {
                plugin_id: plugin_id.to_string(),
                component: "mcp".to_string(),
                path: path.map(|p| p.display().to_string()),
                message: format!("duplicate namespaced MCP server: {namespaced}"),
            });
            continue;
        }
        let source = PluginMcpServerSource {
            plugin_id: plugin_id.to_string(),
            plugin_name: plugin_name.to_string(),
            declared_name: server_name.clone(),
            runtime_name: namespaced.clone(),
            source_path: path.map(|p| p.display().to_string()),
        };
        seen_declared_names.insert(declared_key, source.clone());
        projection
            .mcp_server_sources
            .insert(namespaced.clone(), source);
        projection.mcp_config.servers.insert(namespaced, server);
    }
}

fn project_hooks(
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
    manifest: &PluginManifest,
    projection: &mut PluginRuntimeProjection,
) {
    for path in &manifest.paths.hook_paths {
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(path)
            .map_err(|e| CoreError::Persistence(format!("read {}: {e}", path.display())))
            .and_then(|text| HookDefinitions::parse(&text))
        {
            Ok(mut defs) => {
                expand_hook_commands(&mut defs, plugin_id, plugin_root, plugin_data);
                merge_hook_definitions(&mut projection.hook_definitions, defs);
                push_unique_path(&mut projection.hook_config_paths, path.clone());
            }
            Err(error) => push_error(projection, plugin_id, "hooks", Some(path), error),
        }
    }

    if let Some(value) = manifest.paths.hooks_inline.as_ref() {
        match parse_inline_hooks(value) {
            Ok(mut defs) => {
                expand_hook_commands(&mut defs, plugin_id, plugin_root, plugin_data);
                merge_hook_definitions(&mut projection.hook_definitions, defs);
            }
            Err(error) => push_error(projection, plugin_id, "hooks", None, error),
        }
    }
}

fn parse_inline_hooks(value: &serde_json::Value) -> Result<HookDefinitions> {
    let wrapped = value
        .as_object()
        .map(|object| object.contains_key("hooks"))
        .unwrap_or(false);
    let value = if wrapped {
        value.clone()
    } else {
        serde_json::json!({ "hooks": value })
    };
    serde_json::from_value::<HookDefinitions>(value)
        .map_err(|e| CoreError::invalid(format!("invalid inline hooks: {e}")))
}

pub fn merge_hook_definitions(target: &mut HookDefinitions, incoming: HookDefinitions) {
    for (event, mut groups) in incoming.hooks {
        target.hooks.entry(event).or_default().append(&mut groups);
    }
}

fn load_plugin_output_styles(
    plugin_id: &str,
    plugin_name: &str,
    path: &Path,
    loaded_paths: &mut Vec<PathBuf>,
) -> Result<Vec<PluginOutputStyleEntry>> {
    if path.is_file() {
        return if is_markdown_file(path) {
            load_output_style_file(plugin_id, plugin_name, path, loaded_paths)
                .map(|style| style.into_iter().collect::<Vec<PluginOutputStyleEntry>>())
        } else {
            Ok(Vec::new())
        };
    }
    if !path.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    collect_output_style_files(plugin_id, plugin_name, path, loaded_paths, &mut entries)?;
    Ok(entries)
}

fn collect_output_style_files(
    plugin_id: &str,
    plugin_name: &str,
    dir: &Path,
    loaded_paths: &mut Vec<PathBuf>,
    out: &mut Vec<PluginOutputStyleEntry>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        CoreError::Persistence(format!("read output styles dir {}: {e}", dir.display()))
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_output_style_files(plugin_id, plugin_name, &path, loaded_paths, out)?;
        } else if file_type.is_file() && is_markdown_file(&path) {
            if let Some(style) =
                load_output_style_file(plugin_id, plugin_name, &path, loaded_paths)?
            {
                out.push(style);
            }
        }
    }
    Ok(())
}

fn load_output_style_file(
    plugin_id: &str,
    plugin_name: &str,
    path: &Path,
    loaded_paths: &mut Vec<PathBuf>,
) -> Result<Option<PluginOutputStyleEntry>> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if loaded_paths.iter().any(|existing| existing == &canonical) {
        return Ok(None);
    }
    loaded_paths.push(canonical);

    let text = std::fs::read_to_string(path)
        .map_err(|e| CoreError::Persistence(format!("read {}: {e}", path.display())))?;
    let fm = deepagent_prompts::frontmatter::parse(&text);
    let base_name = fm
        .get("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "style".to_string());
    let prompt = fm.body.trim().to_string();
    if prompt.is_empty() {
        return Err(CoreError::invalid(format!(
            "output style {} has an empty prompt",
            path.display()
        )));
    }
    let name = format!("{plugin_name}:{base_name}");
    let description = fm
        .get("description")
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
        .or_else(|| first_markdown_paragraph(&prompt))
        .unwrap_or_else(|| format!("Output style from {plugin_name} plugin"));
    let force_for_plugin = fm
        .get_bool("force-for-plugin")
        .or_else(|| fm.get_bool("force_for_plugin"));

    Ok(Some(PluginOutputStyleEntry {
        plugin_id: plugin_id.to_string(),
        plugin_name: plugin_name.to_string(),
        name,
        description,
        prompt,
        force_for_plugin,
        source_path: Some(path.display().to_string()),
    }))
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "mdx"))
        .unwrap_or(false)
}

fn first_markdown_paragraph(markdown: &str) -> Option<String> {
    let mut paragraph = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') && paragraph.is_empty() {
            continue;
        }
        paragraph.push(trimmed);
    }
    let joined = paragraph.join(" ");
    let joined = joined.trim();
    if joined.is_empty() {
        None
    } else {
        Some(truncate_chars(joined, 240))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawPluginAppDocument {
    Wrapped { apps: Vec<RawPluginApp> },
    List(Vec<RawPluginApp>),
    One(RawPluginApp),
}

#[derive(Debug, Deserialize)]
struct RawPluginApp {
    id: String,
    #[serde(default, alias = "displayName")]
    title: Option<String>,
    #[serde(default, alias = "desc")]
    description: Option<String>,
    #[serde(default)]
    placement: Option<String>,
    component: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

fn load_plugin_apps(
    plugin_id: &str,
    plugin_name: &str,
    path: &Path,
) -> Result<Vec<PluginAppEntry>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CoreError::Persistence(format!("read {}: {e}", path.display())))?;
    let raw = serde_json::from_str::<RawPluginAppDocument>(&text)
        .map_err(|e| CoreError::invalid(format!("invalid app config: {e}")))?;
    let apps = match raw {
        RawPluginAppDocument::Wrapped { apps } | RawPluginAppDocument::List(apps) => apps,
        RawPluginAppDocument::One(app) => vec![app],
    };
    let mut entries = Vec::new();
    for app in apps {
        if let Some(entry) = normalize_plugin_app(plugin_id, plugin_name, path, app) {
            entries.push(entry?);
        }
    }
    Ok(entries)
}

fn normalize_plugin_app(
    plugin_id: &str,
    plugin_name: &str,
    path: &Path,
    raw: RawPluginApp,
) -> Option<Result<PluginAppEntry>> {
    let id = trimmed(raw.id)?;
    let component = match trimmed(raw.component) {
        Some(component) => component,
        None => {
            return Some(Err(CoreError::invalid(format!(
                "plugin app {id} missing component"
            ))));
        }
    };
    let title = raw.title.and_then(trimmed).unwrap_or_else(|| id.clone());
    Some(Ok(PluginAppEntry {
        plugin_id: plugin_id.to_string(),
        plugin_name: plugin_name.to_string(),
        id,
        title,
        description: raw.description.and_then(trimmed),
        placement: raw
            .placement
            .and_then(trimmed)
            .unwrap_or_else(|| "right-sidebar".to_string()),
        component,
        icon: raw.icon.and_then(trimmed),
        category: raw.category.and_then(trimmed),
        source_path: Some(path.display().to_string()),
    }))
}

fn trimmed(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn expand_hook_commands(
    defs: &mut HookDefinitions,
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
) {
    for groups in defs.hooks.values_mut() {
        for group in groups {
            for action in &mut group.hooks {
                action.command =
                    expand_plugin_vars(&action.command, plugin_id, plugin_root, plugin_data);
                inject_hook_env(action, plugin_id, plugin_root, plugin_data);
            }
        }
    }
}

fn inject_hook_env(
    action: &mut deepagent_hooks::HookAction,
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
) {
    let root = plugin_root.display().to_string();
    let data = plugin_data.display().to_string();
    action
        .env
        .insert("DEEPAGENT_PLUGIN_ID".to_string(), plugin_id.to_string());
    action
        .env
        .insert("DEEPAGENT_PLUGIN_ROOT".to_string(), root.clone());
    action
        .env
        .insert("DEEPAGENT_PLUGIN_DATA".to_string(), data.clone());
    action
        .env
        .insert("CLAUDE_PLUGIN_ROOT".to_string(), root.clone());
    action
        .env
        .insert("CLAUDE_PLUGIN_DATA".to_string(), data.clone());
    // Agent Plugins §9.1 requires the unprefixed names, and requires the client
    // to set them *after* applying configured env so they replace any same-named
    // entry. The prefixed variants above stay for the plugins that already use
    // them.
    action.env.insert(PLUGIN_ROOT_VAR.to_string(), root);
    action.env.insert(PLUGIN_DATA_VAR.to_string(), data);
}

fn inject_plugin_env(
    server: &mut McpServerConfig,
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
) {
    let root = plugin_root.display().to_string();
    let data = plugin_data.display().to_string();
    server
        .env
        .entry("DEEPAGENT_PLUGIN_ID".to_string())
        .or_insert_with(|| plugin_id.to_string());
    server
        .env
        .entry("DEEPAGENT_PLUGIN_ROOT".to_string())
        .or_insert_with(|| root.clone());
    server
        .env
        .entry("DEEPAGENT_PLUGIN_DATA".to_string())
        .or_insert_with(|| data.clone());
    server
        .env
        .entry("CLAUDE_PLUGIN_ROOT".to_string())
        .or_insert_with(|| root.clone());
    server
        .env
        .entry("CLAUDE_PLUGIN_DATA".to_string())
        .or_insert_with(|| data.clone());
    // §9.1: the client supplies PLUGIN_ROOT / PLUGIN_DATA and sets them after
    // the configured env, "replacing any entries with equivalent names". So
    // these two use `insert` rather than the `or_insert_with` the prefixed
    // variants use — a plugin must not be able to shadow them.
    server.env.insert(PLUGIN_ROOT_VAR.to_string(), root);
    server.env.insert(PLUGIN_DATA_VAR.to_string(), data);
}

fn inject_plugin_runtime_requirement(
    server: &mut McpServerConfig,
    runtime: &crate::plugin_manifest::PluginRuntimeRequirements,
) {
    let requirements = [
        ("DEEPAGENT_RUNTIME_NODE_REQUIREMENT", runtime.node.as_ref()),
        (
            "DEEPAGENT_RUNTIME_PYTHON_REQUIREMENT",
            runtime.python.as_ref(),
        ),
        ("DEEPAGENT_RUNTIME_JAVA_REQUIREMENT", runtime.java.as_ref()),
    ];
    for (key, value) in requirements {
        if let Some(value) = value {
            server
                .env
                .entry(key.to_string())
                .or_insert_with(|| value.clone());
        }
    }
    if let Some(preference) = runtime.preference.as_ref() {
        server
            .env
            .entry("DEEPAGENT_RUNTIME_PREFERENCE".to_string())
            .or_insert_with(|| preference.clone());
    }
}

fn expand_plugin_vars(
    input: &str,
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
) -> String {
    deepagent_mcp::config::expand(input, &|var| {
        plugin_env(var, plugin_id, plugin_root, plugin_data)
    })
}

fn plugin_env(
    var: &str,
    plugin_id: &str,
    plugin_root: &Path,
    plugin_data: &Path,
) -> Option<String> {
    match var {
        "DEEPAGENT_PLUGIN_ID" => Some(plugin_id.to_string()),
        // `PLUGIN_ROOT` / `PLUGIN_DATA` are the portable names from Agent
        // Plugins §9.1. Without them here the shared expander would fall through
        // to `std::env::var`, find nothing, and expand a spec-conformant
        // `${PLUGIN_ROOT}` to an empty string.
        PLUGIN_ROOT_VAR | "DEEPAGENT_PLUGIN_ROOT" | "CLAUDE_PLUGIN_ROOT" => {
            Some(plugin_root.display().to_string())
        }
        PLUGIN_DATA_VAR | "DEEPAGENT_PLUGIN_DATA" | "CLAUDE_PLUGIN_DATA" => {
            Some(plugin_data.display().to_string())
        }
        "DEEPAGENT_NODE"
        | "DEEPAGENT_PYTHON"
        | "DEEPAGENT_JAVA"
        | "DEEPAGENT_RUNTIME_ROOT"
        | "DEEPAGENT_RUNTIME_SOURCE" => std::env::var(var).ok(),
        "DEEPAGENT_PROJECT_ROOT" => Some("${DEEPAGENT_PROJECT_ROOT}".to_string()),
        other => std::env::var(other).ok(),
    }
}

fn existing_dirs(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|path| path.is_dir()).cloned().collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn push_error(
    projection: &mut PluginRuntimeProjection,
    plugin_id: &str,
    component: &str,
    path: Option<&PathBuf>,
    error: CoreError,
) {
    projection.errors.push(PluginRuntimeError {
        plugin_id: plugin_id.to_string(),
        component: component.to_string(),
        path: path.map(|p| p.display().to_string()),
        message: error.to_string(),
    });
}

fn safe_identifier(input: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "plugin".to_string()
    } else {
        out
    }
}

fn mcp_declared_key(input: &str) -> String {
    input.trim().to_ascii_lowercase()
}

fn truncate_chars(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.to_string();
    }
    let mut out = input
        .chars()
        .take(cap.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::load_plugin_manifest;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn projects_enabled_plugin_components_and_runtime_env() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo-plugin");
        let data_dir = tmp.path().join("data").join("demo-plugin-builtin");
        for dir in ["skills", "commands", "agents"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::create_dir_all(root.join("output-styles")).unwrap();
        write(
            &root.join("output-styles").join("ship.md"),
            "---\nname: ship-it\ndescription: Concise shipping updates\nforce-for-plugin: true\n---\nRespond with crisp release-note style bullets.",
        );
        write(
            &root.join(".app.json"),
            r#"{
                "apps": [
                    {
                        "id": "browser-panel",
                        "title": "Plugin Browser",
                        "description": "Open the browser from a plugin declaration",
                        "placement": "right-sidebar",
                        "component": "builtin:browser",
                        "icon": "browser",
                        "category": "Developer Tools"
                    }
                ]
            }"#,
        );
        write(
            &root.join(".mcp.json"),
            &serde_json::json!({
                "mcpServers": {
                    "local-fs": {
                        "command": "node",
                        "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"],
                        "env": {
                            "PLUGIN_DATA": "${DEEPAGENT_PLUGIN_DATA}"
                        }
                    }
                }
            })
            .to_string(),
        );
        write(
            &root.join("hooks").join("hooks.json"),
            &serde_json::json!({
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": "${DEEPAGENT_PLUGIN_ROOT}/hooks/check.sh",
                                    "timeout": 5
                                }
                            ]
                        }
                    ]
                }
            })
            .to_string(),
        );
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "demo-plugin",
                "skills": "skills",
                "commands": "commands",
                "agents": "agents",
                "outputStyles": "output-styles",
                "apps": ".app.json",
                "mcpServers": ".mcp.json",
                "hooks": "hooks/hooks.json",
                "runtime": {"node": ">=20.19", "preference": "prefer_local"}
            }"#,
        );
        let manifest = load_plugin_manifest(&root).unwrap().unwrap();

        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: "demo-plugin@builtin",
                name: "demo-plugin",
                source_priority: 10,
                root: &root,
                data_dir: data_dir.clone(),
                manifest: &manifest,
            }]);

        assert_eq!(projection.skill_roots, vec![root.join("skills")]);
        assert_eq!(projection.command_roots[0].path, root.join("commands"));
        assert_eq!(projection.agent_roots[0].path, root.join("agents"));
        assert_eq!(projection.output_styles.len(), 1);
        assert_eq!(projection.output_styles[0].name, "demo-plugin:ship-it");
        assert_eq!(
            projection.output_styles[0].description,
            "Concise shipping updates"
        );
        assert_eq!(projection.output_styles[0].force_for_plugin, Some(true));
        assert_eq!(projection.app_config_paths, vec![root.join(".app.json")]);
        assert_eq!(projection.app_entries.len(), 1);
        assert_eq!(projection.app_entries[0].plugin_id, "demo-plugin@builtin");
        assert_eq!(projection.app_entries[0].id, "browser-panel");
        assert_eq!(projection.app_entries[0].title, "Plugin Browser");
        assert_eq!(
            projection.app_entries[0].component,
            "builtin:browser".to_string()
        );

        let server = projection
            .mcp_config
            .servers
            .get("plugin__demo_plugin__local_fs")
            .expect("namespaced plugin MCP server");
        assert_eq!(server.command.as_deref(), Some("node"));
        assert_eq!(server.args, vec![format!("{}/server.js", root.display())]);
        assert_eq!(server.env["DEEPAGENT_PLUGIN_ID"], "demo-plugin@builtin");
        assert_eq!(
            server.env["DEEPAGENT_PLUGIN_ROOT"],
            root.display().to_string()
        );
        assert_eq!(server.env["CLAUDE_PLUGIN_ROOT"], root.display().to_string());
        assert_eq!(server.env["PLUGIN_DATA"], data_dir.display().to_string());
        assert_eq!(server.env["DEEPAGENT_RUNTIME_NODE_REQUIREMENT"], ">=20.19");

        let groups = projection
            .hook_definitions
            .hooks
            .get("PreToolUse")
            .expect("projected hook event");
        assert_eq!(
            groups[0].hooks[0].command,
            format!("{}/hooks/check.sh", root.display())
        );
        let hook_env = &groups[0].hooks[0].env;
        assert_eq!(hook_env["DEEPAGENT_PLUGIN_ID"], "demo-plugin@builtin");
        assert_eq!(
            hook_env["DEEPAGENT_PLUGIN_ROOT"],
            root.display().to_string()
        );
        assert_eq!(
            hook_env["DEEPAGENT_PLUGIN_DATA"],
            data_dir.display().to_string()
        );
        assert_eq!(hook_env["CLAUDE_PLUGIN_ROOT"], root.display().to_string());
        assert_eq!(
            hook_env["CLAUDE_PLUGIN_DATA"],
            data_dir.display().to_string()
        );
        // §9.1: the portable names must reach the subprocess too, not only our
        // prefixed variants.
        assert_eq!(hook_env[PLUGIN_ROOT_VAR], root.display().to_string());
        assert_eq!(hook_env[PLUGIN_DATA_VAR], data_dir.display().to_string());
        assert!(projection.errors.is_empty());
    }

    /// Agent Plugins §9.1 requires `PLUGIN_ROOT` and `PLUGIN_DATA` in every
    /// plugin subprocess environment, set *after* the configured `env` so they
    /// replace a same-named entry. A plugin must not be able to point them
    /// elsewhere.
    #[test]
    fn portable_plugin_variables_override_plugin_declared_values() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("env-plugin");
        let data_dir = tmp.path().join("data").join("env-plugin");
        write(
            &root.join(".mcp.json"),
            &serde_json::json!({
                "mcpServers": {
                    "svc": {
                        "command": "node",
                        "env": {
                            // Both a hijack attempt and an ordinary variable:
                            // the first must be replaced, the second preserved.
                            "PLUGIN_ROOT": "/tmp/attacker",
                            "PLUGIN_DATA": "/tmp/attacker-data",
                            "KEEP_ME": "untouched"
                        }
                    }
                }
            })
            .to_string(),
        );
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "env-plugin", "mcpServers": ".mcp.json"}"#,
        );
        let manifest = load_plugin_manifest(&root).unwrap().unwrap();

        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: "env-plugin@personal",
                name: "env-plugin",
                source_priority: 30,
                root: &root,
                data_dir: data_dir.clone(),
                manifest: &manifest,
            }]);

        let server = projection
            .mcp_config
            .servers
            .get("plugin__env_plugin__svc")
            .expect("namespaced plugin MCP server");

        assert_eq!(
            server.env[PLUGIN_ROOT_VAR],
            root.display().to_string(),
            "the client must replace a plugin-declared PLUGIN_ROOT"
        );
        assert_eq!(
            server.env[PLUGIN_DATA_VAR],
            data_dir.display().to_string(),
            "the client must replace a plugin-declared PLUGIN_DATA"
        );
        assert_eq!(
            server.env["KEEP_ME"], "untouched",
            "unrelated configured env must survive"
        );
    }

    /// The portable `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` spellings must resolve on
    /// the dialect path too. Before they were added to the lookup they fell
    /// through to `std::env::var`, found nothing, and expanded to an empty
    /// string — silently producing a broken command line.
    #[test]
    fn portable_placeholders_resolve_in_dialect_sourced_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ph-plugin");
        let data_dir = tmp.path().join("data").join("ph-plugin");
        write(
            &root.join(".mcp.json"),
            &serde_json::json!({
                "mcpServers": {
                    "svc": {
                        "command": "node",
                        "args": ["${PLUGIN_ROOT}/server.js", "--data", "${PLUGIN_DATA}/cache"]
                    }
                }
            })
            .to_string(),
        );
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name": "ph-plugin", "mcpServers": ".mcp.json"}"#,
        );
        let manifest = load_plugin_manifest(&root).unwrap().unwrap();

        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: "ph-plugin@personal",
                name: "ph-plugin",
                source_priority: 30,
                root: &root,
                data_dir: data_dir.clone(),
                manifest: &manifest,
            }]);

        let server = projection
            .mcp_config
            .servers
            .get("plugin__ph_plugin__svc")
            .expect("namespaced plugin MCP server");

        assert_eq!(
            server.args,
            vec![
                format!("{}/server.js", root.display()),
                "--data".to_string(),
                format!("{}/cache", data_dir.display()),
            ]
        );
    }

    #[test]
    fn output_styles_use_file_name_and_body_description_fallbacks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("style-plugin");
        let styles = root.join("output-styles").join("nested");
        std::fs::create_dir_all(&styles).unwrap();
        write(
            &styles.join("narrative.md"),
            "# Narrative\n\nTell the story plainly.\n\nUse short paragraphs.",
        );
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "style-plugin",
                "outputStyles": "output-styles"
            }"#,
        );
        let manifest = load_plugin_manifest(&root).unwrap().unwrap();

        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: "style-plugin@personal",
                name: "style-plugin",
                source_priority: 30,
                root: &root,
                data_dir: tmp.path().join("data"),
                manifest: &manifest,
            }]);

        assert_eq!(projection.output_styles.len(), 1);
        assert_eq!(projection.output_styles[0].name, "style-plugin:narrative");
        assert_eq!(
            projection.output_styles[0].description,
            "Tell the story plainly."
        );
        assert_eq!(
            projection.output_styles[0].prompt,
            "# Narrative\n\nTell the story plainly.\n\nUse short paragraphs."
        );
    }

    #[test]
    fn duplicate_declared_mcp_names_keep_higher_priority_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let high = tmp.path().join("workspace-plugin");
        let low = tmp.path().join("builtin-plugin");
        write(
            &high.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"workspace-plugin","mcpServers":".mcp.json"}"#,
        );
        write(
            &high.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"command":"node"}}}"#,
        );
        write(
            &low.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"builtin-plugin","mcpServers":".mcp.json"}"#,
        );
        write(
            &low.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"command":"python"}}}"#,
        );
        let high_manifest = load_plugin_manifest(&high).unwrap().unwrap();
        let low_manifest = load_plugin_manifest(&low).unwrap().unwrap();

        let projection = PluginRuntimeProjection::from_enabled_plugins([
            EnabledPluginRuntimeInput {
                id: "builtin-plugin@builtin",
                name: "builtin-plugin",
                source_priority: 10,
                root: &low,
                data_dir: tmp.path().join("data").join("builtin"),
                manifest: &low_manifest,
            },
            EnabledPluginRuntimeInput {
                id: "workspace-plugin@workspace",
                name: "workspace-plugin",
                source_priority: 40,
                root: &high,
                data_dir: tmp.path().join("data").join("workspace"),
                manifest: &high_manifest,
            },
        ]);

        assert!(projection
            .mcp_config
            .servers
            .contains_key("plugin__workspace_plugin__docs"));
        assert!(!projection
            .mcp_config
            .servers
            .contains_key("plugin__builtin_plugin__docs"));
        assert_eq!(
            projection.mcp_server_sources["plugin__workspace_plugin__docs"].declared_name,
            "docs"
        );
        assert!(projection.errors.iter().any(|error| {
            error.plugin_id == "builtin-plugin@builtin"
                && error.component == "mcp"
                && error.message.contains("already declares 'docs'")
        }));
    }

    #[test]
    fn inline_bare_hook_map_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("inline-hooks");
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "inline-hooks",
                "hooks": {
                    "UserPromptSubmit": [
                        { "hooks": [ { "type": "command", "command": "echo ${CLAUDE_PLUGIN_ROOT}" } ] }
                    ]
                }
            }"#,
        );
        let manifest = load_plugin_manifest(&root).unwrap().unwrap();

        let projection =
            PluginRuntimeProjection::from_enabled_plugins([EnabledPluginRuntimeInput {
                id: "inline-hooks@personal",
                name: "inline-hooks",
                source_priority: 30,
                root: &root,
                data_dir: tmp.path().join("data"),
                manifest: &manifest,
            }]);

        let groups = projection
            .hook_definitions
            .hooks
            .get("UserPromptSubmit")
            .expect("inline bare hook map");
        assert_eq!(
            groups[0].hooks[0].command,
            format!("echo {}", root.display())
        );
        assert!(projection.errors.is_empty());
    }
}
