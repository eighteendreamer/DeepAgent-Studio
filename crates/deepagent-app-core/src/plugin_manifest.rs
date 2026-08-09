//! Plugin manifest parsing and path validation.
//!
//! The manifest shape intentionally follows the Codex plugin contract while
//! keeping DeepAgent-specific fields small and serde-friendly. Runtime loaders
//! should call this module instead of reading `plugin.json` ad hoc so every
//! component path goes through the same escape checks.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

const CODEX_MANIFEST: &str = ".codex-plugin/plugin.json";
const CLAUDE_MANIFEST: &str = ".claude-plugin/plugin.json";
const ROOT_MANIFEST: &str = "plugin.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginInterface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_policy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_service_url: Option<String>,
    #[serde(default)]
    pub default_prompt: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_icon: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_dark: Option<PathBuf>,
    #[serde(default)]
    pub screenshots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginRuntimeRequirements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginManifestPaths {
    #[serde(default)]
    pub skills: Vec<PathBuf>,
    #[serde(default)]
    pub commands: Vec<PathBuf>,
    #[serde(default)]
    pub agents: Vec<PathBuf>,
    #[serde(default)]
    pub output_styles: Vec<PathBuf>,
    #[serde(default)]
    pub mcp_server_paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers_inline: Option<serde_json::Value>,
    #[serde(default)]
    pub hook_paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_inline: Option<serde_json::Value>,
    #[serde(default)]
    pub app_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub paths: PluginManifestPaths,
    #[serde(default)]
    pub interface: PluginInterface,
    #[serde(default)]
    pub runtime: PluginRuntimeRequirements,
    pub manifest_path: PathBuf,
}

impl PluginManifest {
    pub fn display_name(&self) -> String {
        non_empty(self.interface.display_name.as_deref())
            .unwrap_or(&self.name)
            .to_string()
    }

    pub fn short_description(&self) -> String {
        non_empty(self.interface.short_description.as_deref())
            .or_else(|| non_empty(self.description.as_deref()))
            .unwrap_or("DeepAgent plugin")
            .to_string()
    }

    pub fn long_description(&self) -> Option<String> {
        non_empty(self.interface.long_description.as_deref())
            .or_else(|| non_empty(self.description.as_deref()))
            .map(str::to_string)
    }

    pub fn developer_name(&self) -> Option<String> {
        non_empty(self.interface.developer_name.as_deref())
            .or_else(|| {
                self.author
                    .as_ref()
                    .and_then(|a| non_empty(Some(a.name.as_str())))
            })
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawPluginManifest {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<RawPluginAuthor>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    dependencies: Vec<RawDependencyRef>,
    #[serde(default)]
    paths: Option<RawPluginManifestPaths>,
    #[serde(default)]
    skills: Option<RawPathList>,
    #[serde(default)]
    commands: Option<RawPathList>,
    #[serde(default)]
    agents: Option<RawPathList>,
    #[serde(default, rename = "outputStyles", alias = "output_styles")]
    output_styles: Option<RawPathList>,
    #[serde(default, rename = "mcpServers", alias = "mcp_servers")]
    mcp_servers: Option<RawPathOrObject>,
    #[serde(default)]
    hooks: Option<RawPathListOrObject>,
    #[serde(default)]
    apps: Option<RawPathList>,
    #[serde(default)]
    interface: Option<RawPluginInterface>,
    #[serde(default)]
    runtime: Option<RawPluginRuntimeRequirements>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawPluginRuntimeRequirements {
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    python: Option<String>,
    #[serde(default)]
    java: Option<String>,
    #[serde(default)]
    preference: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawPluginManifestPaths {
    #[serde(default)]
    skills: Option<RawPathList>,
    #[serde(default)]
    commands: Option<RawPathList>,
    #[serde(default)]
    agents: Option<RawPathList>,
    #[serde(default, rename = "outputStyles", alias = "output_styles")]
    output_styles: Option<RawPathList>,
    #[serde(
        default,
        rename = "mcpServers",
        alias = "mcp_servers",
        alias = "mcp_servers_path"
    )]
    mcp_servers: Option<RawPathOrObject>,
    #[serde(default)]
    hooks: Option<RawPathListOrObject>,
    #[serde(default)]
    apps: Option<RawPathList>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPluginAuthor {
    name: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPluginInterface {
    #[serde(default, rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
    #[serde(default, rename = "shortDescription", alias = "short_description")]
    short_description: Option<String>,
    #[serde(default, rename = "longDescription", alias = "long_description")]
    long_description: Option<String>,
    #[serde(default, rename = "developerName", alias = "developer_name")]
    developer_name: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(
        default,
        rename = "websiteURL",
        alias = "websiteUrl",
        alias = "website_url"
    )]
    website_url: Option<String>,
    #[serde(
        default,
        rename = "privacyPolicyURL",
        alias = "privacyPolicyUrl",
        alias = "privacy_policy_url"
    )]
    privacy_policy_url: Option<String>,
    #[serde(
        default,
        rename = "termsOfServiceURL",
        alias = "termsOfServiceUrl",
        alias = "terms_of_service_url"
    )]
    terms_of_service_url: Option<String>,
    #[serde(default, rename = "defaultPrompt", alias = "default_prompt")]
    default_prompt: Vec<String>,
    #[serde(default, rename = "brandColor", alias = "brand_color")]
    brand_color: Option<String>,
    #[serde(default, rename = "composerIcon", alias = "composer_icon")]
    composer_icon: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default, rename = "logoDark", alias = "logo_dark")]
    logo_dark: Option<String>,
    #[serde(default)]
    screenshots: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawDependencyRef {
    String(String),
    Object {
        name: String,
        #[serde(default)]
        marketplace: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawPathList {
    One(String),
    Many(Vec<String>),
}

impl RawPathList {
    fn strings(self) -> Vec<String> {
        match self {
            Self::One(path) => vec![path],
            Self::Many(paths) => paths,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawPathOrObject {
    Path(String),
    Paths(Vec<String>),
    Items(Vec<RawPathOrObjectItem>),
    Object(BTreeMap<String, serde_json::Value>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawPathOrObjectItem {
    Path(String),
    Object(BTreeMap<String, serde_json::Value>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawPathListOrObject {
    Path(String),
    Paths(Vec<String>),
    Items(Vec<RawPathListOrObjectItem>),
    Object(BTreeMap<String, serde_json::Value>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawPathListOrObjectItem {
    Path(String),
    Object(BTreeMap<String, serde_json::Value>),
}

pub fn find_plugin_manifest_path(root: &Path) -> Option<PathBuf> {
    let codex = root.join(CODEX_MANIFEST);
    if codex.is_file() {
        return Some(codex);
    }
    let claude = root.join(CLAUDE_MANIFEST);
    if claude.is_file() {
        return Some(claude);
    }
    let root_manifest = root.join(ROOT_MANIFEST);
    if root_manifest.is_file() {
        return Some(root_manifest);
    }
    None
}

pub fn load_plugin_manifest(root: &Path) -> Result<Option<PluginManifest>> {
    let Some(path) = find_plugin_manifest_path(root) else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Persistence(format!("read plugin manifest {}: {e}", path.display()))
    })?;
    let raw: RawPluginManifest = serde_json::from_str(&text)
        .map_err(|e| CoreError::invalid(format!("parse {}: {e}", path.display())))?;
    raw_manifest_to_manifest(root, path, raw).map(Some)
}

fn raw_manifest_to_manifest(
    root: &Path,
    manifest_path: PathBuf,
    raw: RawPluginManifest,
) -> Result<PluginManifest> {
    let name = raw.name.trim().to_string();
    if name.is_empty() {
        return Err(CoreError::invalid("plugin name cannot be empty"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(CoreError::invalid(format!(
            "plugin name contains invalid path characters: {name}"
        )));
    }

    let RawPluginManifest {
        name: _,
        version,
        description,
        author,
        homepage,
        repository,
        license,
        keywords,
        dependencies,
        paths: raw_paths,
        skills,
        commands,
        agents,
        output_styles,
        mcp_servers,
        hooks,
        apps,
        interface,
        runtime,
    } = raw;
    let raw_paths = raw_paths.unwrap_or_default();

    let mut paths = PluginManifestPaths {
        skills: component_paths(root, prefer_path_list(raw_paths.skills, skills), "skills")?,
        commands: component_paths(
            root,
            prefer_path_list(raw_paths.commands, commands),
            "commands",
        )?,
        agents: component_paths(root, prefer_path_list(raw_paths.agents, agents), "agents")?,
        output_styles: component_paths(
            root,
            prefer_path_list(raw_paths.output_styles, output_styles),
            "output-styles",
        )?,
        app_paths: component_paths(root, prefer_path_list(raw_paths.apps, apps), ".app.json")?,
        ..Default::default()
    };

    match raw_paths.mcp_servers.or(mcp_servers) {
        Some(spec) => apply_mcp_spec(root, spec, &mut paths)?,
        None => {
            let default_path = root.join(".mcp.json");
            if default_path.is_file() {
                paths.mcp_server_paths.push(default_path);
            }
        }
    }

    match raw_paths.hooks.or(hooks) {
        Some(spec) => apply_hooks_spec(root, spec, &mut paths)?,
        None => {
            let default_path = root.join("hooks").join("hooks.json");
            if default_path.is_file() {
                paths.hook_paths.push(default_path);
            }
        }
    }

    let interface = interface
        .map(|i| raw_interface_to_interface(root, i))
        .transpose()?
        .unwrap_or_default();

    Ok(PluginManifest {
        name,
        version: version.and_then(trimmed_string),
        description: description.and_then(trimmed_string),
        author: author.map(|a| PluginAuthor {
            name: a.name,
            email: a.email.and_then(trimmed_string),
            url: a.url.and_then(trimmed_string),
        }),
        homepage: homepage.and_then(trimmed_string),
        repository: repository.and_then(trimmed_string),
        license: license.and_then(trimmed_string),
        keywords: cleaned_vec(keywords),
        dependencies: dependencies
            .into_iter()
            .filter_map(|dep| match dep {
                RawDependencyRef::String(value) => trimmed_string(value),
                RawDependencyRef::Object { name, marketplace } => {
                    trimmed_string(name).map(|n| match marketplace.and_then(trimmed_string) {
                        Some(m) => format!("{n}@{m}"),
                        None => n,
                    })
                }
            })
            .collect(),
        paths,
        interface,
        runtime: runtime
            .map(|runtime| PluginRuntimeRequirements {
                node: runtime.node.and_then(trimmed_string),
                python: runtime.python.and_then(trimmed_string),
                java: runtime.java.and_then(trimmed_string),
                preference: runtime.preference.and_then(trimmed_string),
            })
            .unwrap_or_default(),
        manifest_path,
    })
}

fn prefer_path_list(
    preferred: Option<RawPathList>,
    fallback: Option<RawPathList>,
) -> Option<RawPathList> {
    preferred.or(fallback)
}

fn raw_interface_to_interface(root: &Path, raw: RawPluginInterface) -> Result<PluginInterface> {
    Ok(PluginInterface {
        display_name: raw.display_name.and_then(trimmed_string),
        short_description: raw.short_description.and_then(trimmed_string),
        long_description: raw.long_description.and_then(trimmed_string),
        developer_name: raw.developer_name.and_then(trimmed_string),
        category: raw.category.and_then(trimmed_string),
        capabilities: cleaned_vec(raw.capabilities),
        permissions: cleaned_vec(raw.permissions),
        website_url: raw.website_url.and_then(trimmed_string),
        privacy_policy_url: raw.privacy_policy_url.and_then(trimmed_string),
        terms_of_service_url: raw.terms_of_service_url.and_then(trimmed_string),
        default_prompt: cleaned_vec(raw.default_prompt)
            .into_iter()
            .take(3)
            .collect(),
        brand_color: raw.brand_color.and_then(trimmed_string),
        composer_icon: raw
            .composer_icon
            .map(|path| resolve_relative_path(root, &path))
            .transpose()?,
        logo: raw
            .logo
            .map(|path| resolve_relative_path(root, &path))
            .transpose()?,
        logo_dark: raw
            .logo_dark
            .map(|path| resolve_relative_path(root, &path))
            .transpose()?,
        screenshots: resolve_paths(root, raw.screenshots)?,
    })
}

fn component_paths(
    root: &Path,
    raw: Option<RawPathList>,
    default_name: &str,
) -> Result<Vec<PathBuf>> {
    if let Some(raw) = raw {
        return resolve_paths(root, raw.strings());
    }
    let default_path = root.join(default_name);
    if default_path.exists() {
        Ok(vec![default_path])
    } else {
        Ok(Vec::new())
    }
}

fn apply_mcp_spec(
    root: &Path,
    spec: RawPathOrObject,
    paths: &mut PluginManifestPaths,
) -> Result<()> {
    match spec {
        RawPathOrObject::Path(path) => {
            push_unique_paths(
                &mut paths.mcp_server_paths,
                resolve_paths(root, vec![path])?,
            );
        }
        RawPathOrObject::Paths(raw_paths) => {
            push_unique_paths(&mut paths.mcp_server_paths, resolve_paths(root, raw_paths)?);
        }
        RawPathOrObject::Items(items) => {
            for item in items {
                match item {
                    RawPathOrObjectItem::Path(path) => {
                        push_unique_paths(
                            &mut paths.mcp_server_paths,
                            resolve_paths(root, vec![path])?,
                        );
                    }
                    RawPathOrObjectItem::Object(object) => {
                        merge_mcp_inline(&mut paths.mcp_servers_inline, object)?;
                    }
                }
            }
        }
        RawPathOrObject::Object(object) => {
            merge_mcp_inline(&mut paths.mcp_servers_inline, object)?;
        }
    }
    Ok(())
}

fn apply_hooks_spec(
    root: &Path,
    spec: RawPathListOrObject,
    paths: &mut PluginManifestPaths,
) -> Result<()> {
    match spec {
        RawPathListOrObject::Path(path) => {
            push_unique_paths(&mut paths.hook_paths, resolve_paths(root, vec![path])?);
        }
        RawPathListOrObject::Paths(raw_paths) => {
            push_unique_paths(&mut paths.hook_paths, resolve_paths(root, raw_paths)?);
        }
        RawPathListOrObject::Items(items) => {
            for item in items {
                match item {
                    RawPathListOrObjectItem::Path(path) => {
                        push_unique_paths(&mut paths.hook_paths, resolve_paths(root, vec![path])?);
                    }
                    RawPathListOrObjectItem::Object(object) => {
                        merge_hooks_inline(&mut paths.hooks_inline, object)?;
                    }
                }
            }
        }
        RawPathListOrObject::Object(object) => {
            merge_hooks_inline(&mut paths.hooks_inline, object)?;
        }
    }
    Ok(())
}

fn push_unique_paths(target: &mut Vec<PathBuf>, paths: Vec<PathBuf>) {
    for path in paths {
        if !target.iter().any(|existing| existing == &path) {
            target.push(path);
        }
    }
}

fn merge_mcp_inline(
    target: &mut Option<serde_json::Value>,
    object: BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    let mut merged = target
        .take()
        .map(extract_mcp_servers)
        .transpose()?
        .unwrap_or_default();
    for (name, config) in extract_mcp_servers(map_to_value(object))? {
        merged.insert(name, config);
    }
    *target = Some(serde_json::json!({ "mcpServers": merged }));
    Ok(())
}

fn extract_mcp_servers(
    value: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| CoreError::invalid("inline mcpServers must be an object"))?;
    if let Some(servers) = object.remove("mcpServers") {
        return servers
            .as_object()
            .cloned()
            .ok_or_else(|| CoreError::invalid("inline mcpServers.mcpServers must be an object"));
    }
    Ok(object)
}

fn merge_hooks_inline(
    target: &mut Option<serde_json::Value>,
    object: BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    let mut merged = target
        .take()
        .map(extract_hook_groups)
        .transpose()?
        .unwrap_or_default();
    for (event, groups) in extract_hook_groups(map_to_value(object))? {
        if let Some(existing) = merged.get_mut(&event) {
            if let (Some(existing_groups), Some(mut incoming_groups)) =
                (existing.as_array_mut(), groups.as_array().cloned())
            {
                existing_groups.append(&mut incoming_groups);
                continue;
            }
        }
        merged.insert(event, groups);
    }
    *target = Some(serde_json::json!({ "hooks": merged }));
    Ok(())
}

fn extract_hook_groups(
    value: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| CoreError::invalid("inline hooks must be an object"))?;
    if let Some(hooks) = object.remove("hooks") {
        return hooks
            .as_object()
            .cloned()
            .ok_or_else(|| CoreError::invalid("inline hooks.hooks must be an object"));
    }
    Ok(object)
}

fn map_to_value(map: BTreeMap<String, serde_json::Value>) -> serde_json::Value {
    serde_json::Value::Object(map.into_iter().collect())
}

fn resolve_paths(root: &Path, raw_paths: Vec<String>) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for raw in raw_paths {
        let path = resolve_relative_path(root, &raw)?;
        if !paths.iter().any(|existing| existing == &path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub fn resolve_relative_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::invalid("plugin component path cannot be empty"));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(CoreError::invalid(format!(
            "plugin path must be relative: {trimmed}"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::invalid(format!(
                    "plugin path escapes plugin root: {trimmed}"
                )));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(CoreError::invalid(
            "plugin component path cannot resolve to root",
        ));
    }

    Ok(root.join(normalized))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn trimmed_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn cleaned_vec(values: Vec<String>) -> Vec<String> {
    values.into_iter().filter_map(trimmed_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn codex_manifest_takes_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("plugin.json"),
            r#"{"name":"root","description":"root"}"#,
        );
        write(
            &tmp.path().join(".codex-plugin").join("plugin.json"),
            r#"{"name":"codex","description":"codex"}"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        assert_eq!(manifest.name, "codex");
        assert!(manifest
            .manifest_path
            .ends_with(Path::new(".codex-plugin").join("plugin.json")));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("plugin.json"),
            r#"{"name":"escape","skills":"../skills"}"#,
        );

        let err = load_plugin_manifest(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("escapes plugin root"));
    }

    #[test]
    fn loads_claude_manifest_when_codex_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("commands")).unwrap();
        std::fs::create_dir_all(tmp.path().join("skills")).unwrap();
        write(
            &tmp.path().join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "claude-style",
                "description": "Claude layout",
                "commands": "commands",
                "skills": ["skills"],
                "mcpServers": {
                    "docs": { "command": "node", "args": ["server.js"] }
                },
                "hooks": {
                    "PreToolUse": [
                        { "hooks": [ { "type": "command", "command": "echo ok" } ] }
                    ]
                }
            }"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        assert_eq!(manifest.name, "claude-style");
        assert!(manifest
            .manifest_path
            .ends_with(Path::new(".claude-plugin").join("plugin.json")));
        assert_eq!(manifest.paths.commands, vec![tmp.path().join("commands")]);
        assert_eq!(manifest.paths.skills, vec![tmp.path().join("skills")]);
        assert!(manifest.paths.mcp_servers_inline.is_some());
        assert!(manifest.paths.hooks_inline.is_some());
    }

    #[test]
    fn paths_object_wins_over_top_level_component_fields() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("preferred-skills")).unwrap();
        std::fs::create_dir_all(tmp.path().join("fallback-skills")).unwrap();
        write(
            &tmp.path().join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "paths-first",
                "skills": "fallback-skills",
                "paths": {
                    "skills": "preferred-skills",
                    "mcpServers": ".mcp.json"
                }
            }"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        assert_eq!(
            manifest.paths.skills,
            vec![tmp.path().join("preferred-skills")]
        );
        assert_eq!(
            manifest.paths.mcp_server_paths,
            vec![tmp.path().join(".mcp.json")]
        );
    }

    #[test]
    fn output_styles_accept_top_level_and_paths_object_fields() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("preferred-styles")).unwrap();
        std::fs::create_dir_all(tmp.path().join("fallback-styles")).unwrap();
        write(
            &tmp.path().join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "style-first",
                "outputStyles": "fallback-styles",
                "paths": {
                    "outputStyles": "preferred-styles"
                }
            }"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        assert_eq!(
            manifest.paths.output_styles,
            vec![tmp.path().join("preferred-styles")]
        );
    }

    #[test]
    fn mcp_servers_accept_mixed_path_and_inline_specs() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("mcp-extra.json"), r#"{"mcpServers":{}}"#);
        write(
            &tmp.path().join("plugin.json"),
            r#"{
                "name": "mixed-mcp",
                "mcpServers": [
                    "mcp-extra.json",
                    { "inline": { "command": "node", "args": ["server.js"] } },
                    { "mcpServers": { "wrapped": { "command": "python", "args": ["server.py"] } } }
                ]
            }"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        let inline = manifest.paths.mcp_servers_inline.as_ref().unwrap();

        assert_eq!(
            manifest.paths.mcp_server_paths,
            vec![tmp.path().join("mcp-extra.json")]
        );
        assert_eq!(
            inline["mcpServers"]["inline"]["command"].as_str(),
            Some("node")
        );
        assert_eq!(
            inline["mcpServers"]["wrapped"]["command"].as_str(),
            Some("python")
        );
    }

    #[test]
    fn hooks_accept_mixed_path_and_inline_specs() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("hooks").join("first.json"),
            r#"{"hooks":{}}"#,
        );
        write(
            &tmp.path().join("plugin.json"),
            r#"{
                "name": "mixed-hooks",
                "hooks": [
                    "hooks/first.json",
                    {
                        "PreToolUse": [
                            { "hooks": [ { "type": "command", "command": "echo pre" } ] }
                        ]
                    },
                    {
                        "hooks": {
                            "PreToolUse": [
                                { "hooks": [ { "type": "command", "command": "echo wrapped" } ] }
                            ],
                            "PostToolUse": [
                                { "hooks": [ { "type": "command", "command": "echo post" } ] }
                            ]
                        }
                    }
                ]
            }"#,
        );

        let manifest = load_plugin_manifest(tmp.path()).unwrap().unwrap();
        let inline = manifest.paths.hooks_inline.as_ref().unwrap();

        assert_eq!(
            manifest.paths.hook_paths,
            vec![tmp.path().join("hooks").join("first.json")]
        );
        assert_eq!(
            inline["hooks"]["PreToolUse"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            inline["hooks"]["PostToolUse"].as_array().map(Vec::len),
            Some(1)
        );
    }
}
