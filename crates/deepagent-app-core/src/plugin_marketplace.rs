//! Marketplace state and local catalog parsing for plugin management.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

const CODEX_MARKETPLACE: &str = ".codex-plugin/marketplace.json";
const CODEX_AGENTS_MARKETPLACE: &str = ".agents/plugins/marketplace.json";
const CODEX_AGENTS_API_MARKETPLACE: &str = ".agents/plugins/api_marketplace.json";
const CLAUDE_MARKETPLACE: &str = ".claude-plugin/marketplace.json";
const ROOT_MARKETPLACE: &str = "marketplace.json";
const ROOT_API_MARKETPLACE: &str = "api_marketplace.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplaceDto {
    pub name: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPluginMarketplaceDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplaceEntryDto {
    pub marketplace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_full_name: Option<String>,
    #[serde(default)]
    pub stargazers_count: u64,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub components: PluginMarketplaceComponentSummary,
    pub skill_count: u32,
    pub command_count: u32,
    pub agent_count: u32,
    pub hook_count: u32,
    pub mcp_count: u32,
    pub app_count: u32,
    pub output_style_count: u32,
    pub runtime: PluginMarketplaceRuntimeSummary,
    pub runtime_required: bool,
    #[serde(default)]
    pub runtime_requirements: Vec<String>,
    pub has_runtime_payload: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    pub source_kind: String,
    pub source: String,
    pub installable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_block_reason: Option<String>,
    pub installed: bool,
    pub enabled: bool,
    pub update_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_installation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_authentication: Option<String>,
    #[serde(default)]
    pub authentication_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginMarketplaceEntriesQueryDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default)]
    pub query: String,
    #[serde(default = "default_marketplace_page")]
    pub page: u32,
    #[serde(default = "default_marketplace_page_size")]
    pub per_page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplacePageDto {
    pub entries: Vec<PluginMarketplaceEntryDto>,
    pub total_count: u32,
    pub page: u32,
    pub per_page: u32,
    pub has_next: bool,
    pub query: String,
}

fn default_marketplace_page() -> u32 {
    1
}

fn default_marketplace_page_size() -> u32 {
    100
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceCatalog {
    pub name: String,
    pub display_name: Option<String>,
    pub manifest_path: PathBuf,
    pub root: PathBuf,
    pub entries: Vec<PluginMarketplaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceEntry {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub repository_full_name: Option<String>,
    pub stargazers_count: u64,
    pub topics: Vec<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub license: Option<String>,
    pub content_hash: Option<String>,
    /// The curator's attribution for this entry.
    ///
    /// A Claude plugin's own manifest often has no `author` while the catalog
    /// entry does, which makes this the only attribution available for it. It
    /// feeds level 2 of [`crate::plugin::dialect::presentation`].
    pub author_name: Option<String>,
    pub component_summary: PluginMarketplaceComponentSummary,
    pub runtime_summary: PluginMarketplaceRuntimeSummary,
    pub source: PluginMarketplaceSource,
    pub policy_installation: Option<String>,
    pub policy_authentication: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplaceComponentSummary {
    pub skills: u32,
    pub commands: u32,
    pub agents: u32,
    pub hooks: u32,
    pub mcp: u32,
    pub apps: u32,
    #[serde(rename = "outputStyles", alias = "output_style_count")]
    pub output_styles: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplaceRuntimeSummary {
    pub required: bool,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(rename = "hasRuntimePayload", alias = "has_runtime_payload")]
    pub has_runtime_payload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMarketplaceSource {
    Local {
        path: PathBuf,
        display: String,
    },
    Git {
        url: String,
        path: Option<String>,
        ref_name: Option<String>,
        sha: Option<String>,
        display: String,
    },
    GitHub {
        repo: String,
        path: Option<String>,
        ref_name: Option<String>,
        sha: Option<String>,
        display: String,
    },
    GitSubdir {
        url: String,
        path: String,
        ref_name: Option<String>,
        sha: Option<String>,
        display: String,
    },
    ZipUrl {
        url: String,
        sha256: Option<String>,
        display: String,
    },
    Npm {
        package: String,
        version: String,
        registry: Option<String>,
        integrity: Option<String>,
        sha256: Option<String>,
        display: String,
    },
    Unsupported {
        kind: String,
        display: String,
    },
}

impl PluginMarketplaceSource {
    pub fn kind(&self) -> &str {
        match self {
            Self::Local { .. } => "local",
            Self::Git { .. } => "git",
            Self::GitHub { .. } => "github",
            Self::GitSubdir { .. } => "git-subdir",
            Self::ZipUrl { .. } => "zip-url",
            Self::Npm { .. } => "npm",
            Self::Unsupported { kind, .. } => kind,
        }
    }

    pub fn display(&self) -> &str {
        match self {
            Self::Local { display, .. }
            | Self::Git { display, .. }
            | Self::GitHub { display, .. }
            | Self::GitSubdir { display, .. }
            | Self::ZipUrl { display, .. }
            | Self::Npm { display, .. }
            | Self::Unsupported { display, .. } => display,
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Local { path, .. } => Some(path.as_path()),
            Self::Git { .. }
            | Self::GitHub { .. }
            | Self::GitSubdir { .. }
            | Self::ZipUrl { .. }
            | Self::Npm { .. }
            | Self::Unsupported { .. } => None,
        }
    }

    pub fn commit(&self) -> Option<&str> {
        match self {
            Self::Git { sha, .. } | Self::GitHub { sha, .. } | Self::GitSubdir { sha, .. } => {
                sha.as_deref()
            }
            Self::Local { .. }
            | Self::ZipUrl { .. }
            | Self::Npm { .. }
            | Self::Unsupported { .. } => None,
        }
    }

    pub fn content_hash(&self) -> Option<String> {
        match self {
            Self::ZipUrl {
                sha256: Some(sha256),
                ..
            } => Some(format!("sha256:{sha256}")),
            Self::Npm {
                sha256: Some(sha256),
                ..
            } => Some(format!("sha256:{sha256}")),
            Self::Npm {
                integrity: Some(integrity),
                ..
            } => Some(integrity.clone()),
            Self::Local { .. }
            | Self::Git { .. }
            | Self::GitHub { .. }
            | Self::GitSubdir { .. }
            | Self::ZipUrl { sha256: None, .. }
            | Self::Npm { .. }
            | Self::Unsupported { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawMarketplace {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    interface: Option<RawMarketplaceInterface>,
    #[serde(default)]
    plugins: Vec<RawMarketplaceEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMarketplaceInterface {
    #[serde(default, rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMarketplaceEntry {
    name: String,
    #[serde(default, rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "full_name", alias = "repositoryFullName")]
    repository_full_name: Option<String>,
    #[serde(default)]
    stargazers_count: u64,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default, rename = "contentHash", alias = "content_hash")]
    content_hash: Option<String>,
    #[serde(default)]
    author: Option<RawMarketplaceAuthor>,
    #[serde(default)]
    components: Option<RawMarketplaceComponentSummary>,
    #[serde(default)]
    runtime: Option<RawMarketplaceRuntimeSummary>,
    source: RawMarketplaceSource,
    #[serde(default)]
    policy: Option<RawMarketplacePolicy>,
}

/// A catalog entry's author, accepted either as a bare name or as an object.
///
/// Claude's `marketplace.json` uses the object form (`{"name": ..., "email":
/// ...}`); the bare-string form appears in hand-written catalogs. This is
/// presentation metadata, so a malformed shape must not reject the entry — hence
/// every field is optional and unknown members are ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawMarketplaceAuthor {
    Name(String),
    Object {
        #[serde(default)]
        name: Option<String>,
    },
}

impl RawMarketplaceAuthor {
    fn into_name(self) -> Option<String> {
        match self {
            Self::Name(name) => trimmed_string(name),
            Self::Object { name } => name.and_then(trimmed_string),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawMarketplacePolicy {
    #[serde(default)]
    installation: Option<String>,
    #[serde(default)]
    authentication: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawMarketplaceComponentSummary {
    #[serde(default, alias = "skillCount", alias = "skill_count")]
    skills: u32,
    #[serde(default, alias = "commandCount", alias = "command_count")]
    commands: u32,
    #[serde(default, alias = "agentCount", alias = "agent_count")]
    agents: u32,
    #[serde(default, alias = "hookCount", alias = "hook_count")]
    hooks: u32,
    #[serde(default, alias = "mcpCount", alias = "mcp_count")]
    mcp: u32,
    #[serde(default, alias = "appCount", alias = "app_count")]
    apps: u32,
    #[serde(default, rename = "outputStyles", alias = "output_style_count")]
    output_styles: u32,
}

impl From<RawMarketplaceComponentSummary> for PluginMarketplaceComponentSummary {
    fn from(raw: RawMarketplaceComponentSummary) -> Self {
        Self {
            skills: raw.skills,
            commands: raw.commands,
            agents: raw.agents,
            hooks: raw.hooks,
            mcp: raw.mcp,
            apps: raw.apps,
            output_styles: raw.output_styles,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawMarketplaceRuntimeSummary {
    #[serde(default)]
    required: bool,
    #[serde(default)]
    requirements: Vec<String>,
    #[serde(default, rename = "hasRuntimePayload", alias = "has_runtime_payload")]
    has_runtime_payload: bool,
}

impl From<RawMarketplaceRuntimeSummary> for PluginMarketplaceRuntimeSummary {
    fn from(raw: RawMarketplaceRuntimeSummary) -> Self {
        let mut requirements = raw
            .requirements
            .into_iter()
            .filter_map(trimmed_string)
            .collect::<Vec<_>>();
        requirements.sort();
        requirements.dedup();
        Self {
            required: raw.required || !requirements.is_empty() || raw.has_runtime_payload,
            requirements,
            has_runtime_payload: raw.has_runtime_payload,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawMarketplaceSource {
    String(String),
    Object(BTreeMap<String, serde_json::Value>),
}

pub fn find_marketplace_manifest_path(root_or_file: &Path) -> Option<PathBuf> {
    if root_or_file.is_file() {
        return root_or_file
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| {
                name.eq_ignore_ascii_case(ROOT_MARKETPLACE)
                    || name.eq_ignore_ascii_case(ROOT_API_MARKETPLACE)
            })
            .map(|_| root_or_file.to_path_buf());
    }

    [
        root_or_file.join(CODEX_MARKETPLACE),
        root_or_file.join(CODEX_AGENTS_MARKETPLACE),
        root_or_file.join(CODEX_AGENTS_API_MARKETPLACE),
        root_or_file.join(CLAUDE_MARKETPLACE),
        root_or_file.join(ROOT_MARKETPLACE),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

pub fn marketplace_root_from_manifest(manifest_path: &Path) -> PathBuf {
    let Some(parent) = manifest_path.parent() else {
        return PathBuf::new();
    };
    match parent.file_name().and_then(|name| name.to_str()) {
        Some(".codex-plugin" | ".claude-plugin") => parent
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| parent.to_path_buf()),
        Some("plugins")
            if parent
                .parent()
                .and_then(|agents| agents.file_name())
                .and_then(|name| name.to_str())
                == Some(".agents") =>
        {
            parent
                .parent()
                .and_then(|agents| agents.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| parent.to_path_buf())
        }
        _ => parent.to_path_buf(),
    }
}

pub fn load_marketplace_catalog(manifest_path: &Path) -> Result<PluginMarketplaceCatalog> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| {
        CoreError::Persistence(format!(
            "read marketplace manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let raw: RawMarketplace = serde_json::from_str(&text)
        .map_err(|e| CoreError::invalid(format!("parse marketplace manifest: {e}")))?;
    let root = marketplace_root_from_manifest(manifest_path);
    let name = raw.name.and_then(trimmed_string).unwrap_or_else(|| {
        root.file_name()
            .and_then(|name| name.to_str())
            .map(slugify)
            .unwrap_or_else(|| "marketplace".to_string())
    });
    let entries = raw
        .plugins
        .into_iter()
        .map(|entry| normalize_entry(&root, entry))
        .collect::<Result<Vec<_>>>()?;
    Ok(PluginMarketplaceCatalog {
        name,
        display_name: raw
            .interface
            .and_then(|i| i.display_name)
            .and_then(trimmed_string),
        manifest_path: manifest_path.to_path_buf(),
        root,
        entries,
    })
}

fn normalize_entry(root: &Path, raw: RawMarketplaceEntry) -> Result<PluginMarketplaceEntry> {
    let name =
        trimmed_string(raw.name).ok_or_else(|| CoreError::invalid("plugin name is empty"))?;
    let source = normalize_source(root, raw.source)?;
    Ok(PluginMarketplaceEntry {
        name,
        display_name: raw.display_name.and_then(trimmed_string),
        description: raw.description.and_then(trimmed_string),
        repository_full_name: raw.repository_full_name.and_then(trimmed_string),
        stargazers_count: raw.stargazers_count,
        topics: raw.topics,
        version: raw.version.and_then(trimmed_string),
        category: raw.category.and_then(trimmed_string),
        license: raw.license.and_then(trimmed_string),
        content_hash: raw.content_hash.and_then(trimmed_string),
        author_name: raw.author.and_then(RawMarketplaceAuthor::into_name),
        component_summary: raw.components.map(Into::into).unwrap_or_default(),
        runtime_summary: raw.runtime.map(Into::into).unwrap_or_default(),
        source,
        policy_installation: raw
            .policy
            .as_ref()
            .and_then(|policy| policy.installation.clone())
            .and_then(trimmed_string),
        policy_authentication: raw
            .policy
            .and_then(|policy| policy.authentication)
            .and_then(trimmed_string),
    })
}

fn normalize_source(root: &Path, raw: RawMarketplaceSource) -> Result<PluginMarketplaceSource> {
    match raw {
        RawMarketplaceSource::String(path) => local_source(root, path),
        RawMarketplaceSource::Object(mut object) => {
            let kind = string_field(&mut object, "source")
                .or_else(|| string_field(&mut object, "type"))
                .unwrap_or_else(|| "local".to_string());
            match kind.as_str() {
                "local" => {
                    let path = string_field(&mut object, "path").ok_or_else(|| {
                        CoreError::invalid("local marketplace source missing path")
                    })?;
                    local_source(root, path)
                }
                "git" => git_source(object),
                "github" => github_source(object),
                "git-subdir" => git_subdir_source(object),
                "zip-url" | "url" => zip_url_source(object),
                "npm" => npm_source(object),
                other => Ok(PluginMarketplaceSource::Unsupported {
                    kind: other.to_string(),
                    display: other.to_string(),
                }),
            }
        }
    }
}

fn local_source(root: &Path, raw_path: String) -> Result<PluginMarketplaceSource> {
    let display = raw_path.trim().replace('\\', "/");
    if display.is_empty() {
        return Err(CoreError::invalid("local marketplace source path is empty"));
    }
    let path = safe_join_marketplace_path(root, &display)?;
    Ok(PluginMarketplaceSource::Local { path, display })
}

fn git_source(mut object: BTreeMap<String, serde_json::Value>) -> Result<PluginMarketplaceSource> {
    let url = string_field(&mut object, "url")
        .or_else(|| string_field(&mut object, "repo"))
        .ok_or_else(|| CoreError::invalid("git marketplace source missing url"))?;
    let path = optional_remote_subdir(&mut object, "path")?;
    let ref_name =
        string_field(&mut object, "ref").or_else(|| string_field(&mut object, "refName"));
    let sha = string_field(&mut object, "sha");
    let display = remote_display(&url, path.as_deref(), ref_name.as_deref());
    if let Err(error) = validate_git_remote(&url, ref_name.as_deref(), sha.as_deref()) {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "git".to_string(),
            display: format!("{display} ({error})"),
        });
    }
    Ok(PluginMarketplaceSource::Git {
        url,
        path,
        ref_name,
        sha,
        display,
    })
}

fn github_source(
    mut object: BTreeMap<String, serde_json::Value>,
) -> Result<PluginMarketplaceSource> {
    let repo = string_field(&mut object, "repo")
        .or_else(|| string_field(&mut object, "url"))
        .ok_or_else(|| CoreError::invalid("github marketplace source missing repo"))?;
    validate_github_repo(&repo)?;
    let path = optional_remote_subdir(&mut object, "path")?;
    let ref_name =
        string_field(&mut object, "ref").or_else(|| string_field(&mut object, "refName"));
    let sha = string_field(&mut object, "sha");
    if let Err(error) = validate_git_ref_and_sha(ref_name.as_deref(), sha.as_deref()) {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "github".to_string(),
            display: format!("{repo} ({error})"),
        });
    }
    let display = remote_display(&repo, path.as_deref(), ref_name.as_deref());
    Ok(PluginMarketplaceSource::GitHub {
        repo,
        path,
        ref_name,
        sha,
        display,
    })
}

fn git_subdir_source(
    mut object: BTreeMap<String, serde_json::Value>,
) -> Result<PluginMarketplaceSource> {
    let url = string_field(&mut object, "url")
        .or_else(|| string_field(&mut object, "repo"))
        .ok_or_else(|| CoreError::invalid("git-subdir marketplace source missing url"))?;
    let path = string_field(&mut object, "path")
        .ok_or_else(|| CoreError::invalid("git-subdir marketplace source missing path"))
        .and_then(normalize_remote_subdir)?;
    let ref_name =
        string_field(&mut object, "ref").or_else(|| string_field(&mut object, "refName"));
    let sha = string_field(&mut object, "sha");
    let display = remote_display(&url, Some(&path), ref_name.as_deref());
    if let Err(error) = validate_git_remote(&url, ref_name.as_deref(), sha.as_deref()) {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "git-subdir".to_string(),
            display: format!("{display} ({error})"),
        });
    }
    Ok(PluginMarketplaceSource::GitSubdir {
        url,
        path,
        ref_name,
        sha,
        display,
    })
}

fn zip_url_source(
    mut object: BTreeMap<String, serde_json::Value>,
) -> Result<PluginMarketplaceSource> {
    let url = string_field(&mut object, "url")
        .ok_or_else(|| CoreError::invalid("zip-url marketplace source missing url"))?;
    if !url.starts_with("https://") {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "zip-url".to_string(),
            display: format!("{url} (requires https)"),
        });
    }
    let sha256 = string_field(&mut object, "sha256");
    let Some(sha256) = sha256 else {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "zip-url".to_string(),
            display: format!("{url} (missing sha256)"),
        });
    };
    if !is_sha256_hex(&sha256) {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "zip-url".to_string(),
            display: format!("{url} (invalid sha256)"),
        });
    }
    let display = url.clone();
    Ok(PluginMarketplaceSource::ZipUrl {
        url,
        sha256: Some(sha256),
        display,
    })
}

fn npm_source(mut object: BTreeMap<String, serde_json::Value>) -> Result<PluginMarketplaceSource> {
    let package = string_field(&mut object, "package")
        .ok_or_else(|| CoreError::invalid("npm marketplace source missing package"))?;
    let version = string_field(&mut object, "version");
    let registry = string_field(&mut object, "registry");
    let integrity = string_field(&mut object, "integrity");
    let sha256 = string_field(&mut object, "sha256");
    let display = version
        .as_ref()
        .map(|version| format!("{package}@{version}"))
        .unwrap_or_else(|| package.clone());
    if let Err(error) = validate_npm_source(
        &package,
        version.as_deref(),
        registry.as_deref(),
        integrity.as_deref(),
        sha256.as_deref(),
    ) {
        return Ok(PluginMarketplaceSource::Unsupported {
            kind: "npm".to_string(),
            display: format!("{display} ({error})"),
        });
    }
    Ok(PluginMarketplaceSource::Npm {
        package,
        version: version.unwrap_or_default(),
        registry,
        integrity,
        sha256,
        display,
    })
}

fn safe_join_marketplace_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(CoreError::invalid(
            "marketplace plugin source must be relative to the marketplace root",
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::invalid(
                    "marketplace plugin source cannot escape the marketplace root",
                ));
            }
        }
    }
    Ok(root.join(path))
}

fn string_field(object: &mut BTreeMap<String, serde_json::Value>, key: &str) -> Option<String> {
    object
        .remove(key)
        .and_then(|value| value.as_str().map(str::to_string))
        .and_then(trimmed_string)
}

fn optional_remote_subdir(
    object: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>> {
    string_field(object, key)
        .map(normalize_remote_subdir)
        .transpose()
}

fn normalize_remote_subdir(raw: String) -> Result<String> {
    let display = raw.trim().trim_start_matches("./").replace('\\', "/");
    if display.is_empty() {
        return Err(CoreError::invalid(
            "remote plugin source path cannot be empty",
        ));
    }
    let path = Path::new(&display);
    if path.is_absolute() {
        return Err(CoreError::invalid(
            "remote plugin source path must be relative",
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::invalid(
                    "remote plugin source path cannot escape the repository root",
                ));
            }
        }
    }
    Ok(display)
}

fn validate_github_repo(repo: &str) -> Result<()> {
    let repo = repo.trim().trim_end_matches(".git");
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || parts.next().is_some()
        || !is_safe_remote_segment(owner)
        || !is_safe_remote_segment(name)
    {
        return Err(CoreError::invalid(format!(
            "invalid github marketplace repo: {repo}"
        )));
    }
    Ok(())
}

fn validate_git_remote(url: &str, ref_name: Option<&str>, sha: Option<&str>) -> Result<()> {
    validate_git_url(url)?;
    validate_git_ref_and_sha(ref_name, sha)
}

fn validate_git_ref_and_sha(ref_name: Option<&str>, sha: Option<&str>) -> Result<()> {
    if let Some(ref_name) = ref_name {
        validate_git_ref(ref_name)?;
    }
    if let Some(sha) = sha {
        validate_git_sha(sha)?;
    }
    Ok(())
}

fn validate_git_url(url: &str) -> Result<()> {
    let trimmed = url.trim();
    if has_whitespace_or_control(trimmed) || trimmed.contains('\\') {
        return Err(CoreError::invalid(
            "git marketplace source url contains unsafe characters",
        ));
    }
    if trimmed.starts_with("https://") {
        return validate_url_authority(trimmed, "https://", "git marketplace source url");
    }
    if trimmed.starts_with("ssh://") {
        return validate_url_authority(trimmed, "ssh://", "git marketplace source url");
    }
    if let Some((host, repo)) = trimmed
        .strip_prefix("git@")
        .and_then(|rest| rest.split_once(':'))
    {
        if !host.is_empty()
            && !repo.is_empty()
            && host
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
            && !repo.starts_with('-')
            && !repo.contains("..")
        {
            return Ok(());
        }
    }
    Err(CoreError::invalid(
        "git marketplace source url must use https://, ssh://, or git@host:path",
    ))
}

fn validate_url_authority(url: &str, scheme: &str, label: &str) -> Result<()> {
    let rest = url.trim_start_matches(scheme);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() || authority.starts_with('.') || authority.contains("..") {
        return Err(CoreError::invalid(format!("{label} missing valid host")));
    }
    Ok(())
}

fn validate_git_ref(ref_name: &str) -> Result<()> {
    let ref_name = ref_name.trim();
    if ref_name.is_empty()
        || ref_name.starts_with('-')
        || ref_name.contains("..")
        || ref_name.contains('\\')
        || has_whitespace_or_control(ref_name)
    {
        return Err(CoreError::invalid(format!(
            "invalid git ref in marketplace source: {ref_name}"
        )));
    }
    Ok(())
}

fn validate_git_sha(sha: &str) -> Result<()> {
    let sha = sha.trim();
    if !(6..=64).contains(&sha.len()) || !sha.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(CoreError::invalid(format!(
            "invalid git sha in marketplace source: {sha}"
        )));
    }
    Ok(())
}

fn validate_npm_source(
    package: &str,
    version: Option<&str>,
    registry: Option<&str>,
    integrity: Option<&str>,
    sha256: Option<&str>,
) -> Result<()> {
    validate_npm_package(package)?;
    let version =
        version.ok_or_else(|| CoreError::invalid("npm marketplace source missing version"))?;
    validate_npm_version(version)?;
    if let Some(registry) = registry {
        validate_npm_registry(registry)?;
    }
    match (integrity, sha256) {
        (None, None) => {
            return Err(CoreError::invalid(
                "npm marketplace source missing integrity or sha256",
            ))
        }
        (Some(integrity), _) => validate_npm_integrity(integrity)?,
        (None, Some(sha256)) => validate_sha256_hex(sha256)?,
    }
    if let Some(sha256) = sha256 {
        validate_sha256_hex(sha256)?;
    }
    Ok(())
}

fn validate_npm_package(package: &str) -> Result<()> {
    let trimmed = package.trim();
    let segments = if let Some(scoped) = trimmed.strip_prefix('@') {
        scoped.split('/').collect::<Vec<_>>()
    } else {
        trimmed.split('/').collect::<Vec<_>>()
    };
    let expected = if trimmed.starts_with('@') { 2 } else { 1 };
    if trimmed.is_empty()
        || segments.len() != expected
        || segments
            .iter()
            .any(|segment| !is_safe_remote_segment(segment))
    {
        return Err(CoreError::invalid(format!(
            "invalid npm marketplace package: {package}"
        )));
    }
    Ok(())
}

fn validate_npm_version(version: &str) -> Result<()> {
    let version = version.trim();
    if version.is_empty()
        || version == "."
        || version == ".."
        || version.contains(['/', '\\', '@', ':'])
        || has_whitespace_or_control(version)
        || version
            .chars()
            .any(|ch| matches!(ch, '^' | '~' | '*' | '<' | '>' | '='))
    {
        return Err(CoreError::invalid(format!(
            "invalid npm marketplace package version: {version}"
        )));
    }
    if !version.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '.' | '-' | '_' | '+' | '~' | '^' | '*' | '<' | '>' | '='
            )
    }) {
        return Err(CoreError::invalid(format!(
            "invalid npm marketplace package version: {version}"
        )));
    }
    Ok(())
}

fn validate_npm_integrity(integrity: &str) -> Result<()> {
    let integrity = integrity.trim();
    let Some((algorithm, digest)) = integrity.split_once('-') else {
        return Err(CoreError::invalid("invalid npm marketplace integrity"));
    };
    if !matches!(algorithm, "sha256" | "sha384" | "sha512") || digest.is_empty() {
        return Err(CoreError::invalid("invalid npm marketplace integrity"));
    }
    if digest
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=')))
    {
        return Err(CoreError::invalid("invalid npm marketplace integrity"));
    }
    Ok(())
}

fn validate_sha256_hex(value: &str) -> Result<()> {
    if !is_sha256_hex(value) {
        return Err(CoreError::invalid(format!("invalid sha256: {value}")));
    }
    Ok(())
}

fn validate_npm_registry(registry: &str) -> Result<()> {
    let registry = registry.trim();
    if has_whitespace_or_control(registry) || registry.contains('\\') {
        return Err(CoreError::invalid(
            "npm marketplace registry contains unsafe characters",
        ));
    }
    if !registry.starts_with("https://") {
        return Err(CoreError::invalid(
            "npm marketplace registry must use https://",
        ));
    }
    validate_url_authority(registry, "https://", "npm marketplace registry")
}

fn is_safe_remote_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn has_whitespace_or_control(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn remote_display(base: &str, path: Option<&str>, ref_name: Option<&str>) -> String {
    let mut display = base.to_string();
    if let Some(path) = path {
        display.push_str("#path=");
        display.push_str(path);
    }
    if let Some(ref_name) = ref_name {
        display.push('@');
        display.push_str(ref_name);
    }
    display
}

pub fn normalize_marketplace_name(input: &str) -> String {
    let name = input
        .trim()
        .trim_end_matches('/')
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or(input)
        .trim_end_matches(".git");
    slugify(name)
}

pub fn slugify(input: &str) -> String {
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
    if out.is_empty() {
        "marketplace".to_string()
    } else {
        out
    }
}

fn trimmed_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_local_marketplace_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        std::fs::write(
            root.join(".codex-plugin").join("marketplace.json"),
            r#"{
              "name": "team",
              "interface": { "displayName": "Team Plugins" },
              "plugins": [
                {
                  "name": "demo",
                  "version": "1.2.3",
                  "description": "Demo plugin",
                  "source": { "source": "local", "path": "./plugins/demo" },
                  "category": "Developer Tools",
                  "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }
                }
              ]
            }"#,
        )
        .unwrap();

        let path = find_marketplace_manifest_path(root).unwrap();
        let catalog = load_marketplace_catalog(&path).unwrap();
        assert_eq!(catalog.name, "team");
        assert_eq!(catalog.display_name.as_deref(), Some("Team Plugins"));
        assert_eq!(catalog.entries[0].name, "demo");
        assert_eq!(catalog.entries[0].source.kind(), "local");
        assert_eq!(
            catalog.entries[0].source.local_path().unwrap(),
            root.join("plugins").join("demo")
        );
    }

    /// A catalog entry's `author` is the only attribution many Claude plugins
    /// have: their own manifest omits it while the catalog supplies it. Both the
    /// object form Claude uses and the bare-string form hand-written catalogs use
    /// must resolve to a name.
    #[test]
    fn reads_entry_author_in_both_shapes_and_tolerates_a_broken_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "name": "team",
              "plugins": [
                {
                  "name": "object-author",
                  "author": { "name": "Daisy Hollman", "email": "daisy@anthropic.com" },
                  "source": { "source": "local", "path": "./plugins/a" }
                },
                {
                  "name": "string-author",
                  "author": "Anthropic",
                  "source": { "source": "local", "path": "./plugins/b" }
                },
                {
                  "name": "blank-author",
                  "author": { "name": "   " },
                  "source": { "source": "local", "path": "./plugins/c" }
                },
                {
                  "name": "no-author",
                  "source": { "source": "local", "path": "./plugins/d" }
                }
              ]
            }"#,
        )
        .unwrap();

        let path = find_marketplace_manifest_path(root).unwrap();
        let catalog = load_marketplace_catalog(&path).unwrap();
        let authors = catalog
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.author_name.as_deref()))
            .collect::<Vec<_>>();

        assert_eq!(
            authors,
            vec![
                ("object-author", Some("Daisy Hollman")),
                ("string-author", Some("Anthropic")),
                // Presentation metadata must never reject an entry, and a blank
                // name is an absent name rather than an empty one.
                ("blank-author", None),
                ("no-author", None),
            ]
        );
    }

    /// The real `anthropics/claude-code` catalog, read from the reference
    /// checkout. That repository is not open source, so it cannot be vendored
    /// here; the test skips when the checkout is absent.
    #[test]
    fn parses_the_real_claude_code_catalog() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../借鉴/claude-code/.claude-plugin/marketplace.json");
        if !path.is_file() {
            eprintln!("skipping: 借鉴/claude-code/.claude-plugin/marketplace.json is not present");
            return;
        }

        let catalog = load_marketplace_catalog(&path).unwrap();

        assert_eq!(catalog.name, "claude-code-plugins");
        let hookify = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "hookify")
            .expect("hookify is in the catalog");
        assert_eq!(hookify.author_name.as_deref(), Some("Daisy Hollman"));
        assert_eq!(hookify.category.as_deref(), Some("productivity"));
        assert_eq!(hookify.version.as_deref(), Some("0.1.0"));

        // The catalog copy is a superset of the plugin manifest's own
        // description, which is why presentation prefers it.
        assert!(hookify
            .description
            .as_deref()
            .expect("a description")
            .contains("Define rules via simple markdown files"));

        // `agent-sdk-dev` has no `author` in the catalog, so the field must be
        // absent rather than defaulted to the catalog owner.
        let sdk = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "agent-sdk-dev")
            .expect("agent-sdk-dev is in the catalog");
        assert_eq!(sdk.author_name, None);
    }

    #[test]
    fn loads_codex_agents_marketplace_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agents").join("plugins")).unwrap();
        std::fs::write(
            root.join(".agents")
                .join("plugins")
                .join("marketplace.json"),
            r#"{
              "name": "codex",
              "plugins": [
                { "name": "demo", "source": { "source": "local", "path": "./plugins/demo" } }
              ]
            }"#,
        )
        .unwrap();

        let path = find_marketplace_manifest_path(root).unwrap();
        let catalog = load_marketplace_catalog(&path).unwrap();

        assert_eq!(
            path,
            root.join(".agents")
                .join("plugins")
                .join("marketplace.json")
        );
        assert_eq!(catalog.root, root);
        assert_eq!(
            catalog.entries[0].source.local_path().unwrap(),
            root.join("plugins").join("demo")
        );
    }

    #[test]
    fn loads_codex_agents_api_marketplace_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agents").join("plugins")).unwrap();
        std::fs::write(
            root.join(".agents")
                .join("plugins")
                .join("api_marketplace.json"),
            r#"{
              "name": "codex-api",
              "plugins": [
                { "name": "demo", "source": "./plugins/demo" }
              ]
            }"#,
        )
        .unwrap();

        let path = find_marketplace_manifest_path(root).unwrap();
        let catalog = load_marketplace_catalog(&path).unwrap();

        assert_eq!(
            path,
            root.join(".agents")
                .join("plugins")
                .join("api_marketplace.json")
        );
        assert_eq!(catalog.root, root);
        assert_eq!(
            catalog.entries[0].source.local_path().unwrap(),
            root.join("plugins").join("demo")
        );
    }

    #[test]
    fn rejects_marketplace_source_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "plugins": [
                { "name": "bad", "source": { "source": "local", "path": "../bad" } }
              ]
            }"#,
        )
        .unwrap();

        let err = load_marketplace_catalog(&root.join("marketplace.json")).unwrap_err();
        assert!(err.to_string().contains("cannot escape"));
    }

    #[test]
    fn parses_remote_marketplace_sources_without_materializing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "git-plugin",
                  "source": {
                    "source": "git",
                    "url": "https://example.com/team/plugins.git",
                    "path": "./plugins/git-plugin",
                    "ref": "main"
                  }
                },
                {
                  "name": "github-plugin",
                  "license": "Apache-2.0",
                  "contentHash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                  "components": {
                    "skills": 2,
                    "commands": 1,
                    "agents": 1,
                    "hooks": 1,
                    "mcp": 1,
                    "apps": 1,
                    "outputStyles": 1
                  },
                  "runtime": {
                    "required": true,
                    "requirements": ["node >=20"],
                    "hasRuntimePayload": true
                  },
                  "source": {
                    "source": "github",
                    "repo": "owner/repo",
                    "path": "packages/github-plugin",
                    "sha": "abc123"
                  }
                },
                {
                  "name": "npm-plugin",
                  "source": {
                    "source": "npm",
                    "package": "@scope/plugin",
                    "version": "1.2.3",
                    "registry": "https://registry.npmjs.org",
                    "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
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

        let catalog = load_marketplace_catalog(&root.join("marketplace.json")).unwrap();
        let sources = catalog
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.name.as_str(),
                    entry.source.kind(),
                    entry.source.display(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            sources[0],
            (
                "git-plugin",
                "git",
                "https://example.com/team/plugins.git#path=plugins/git-plugin@main"
            )
        );
        assert_eq!(
            sources[1],
            (
                "github-plugin",
                "github",
                "owner/repo#path=packages/github-plugin"
            )
        );
        assert_eq!(catalog.entries[1].license.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            catalog.entries[1].content_hash.as_deref(),
            Some("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(catalog.entries[1].component_summary.skills, 2);
        assert_eq!(catalog.entries[1].component_summary.commands, 1);
        assert_eq!(catalog.entries[1].component_summary.output_styles, 1);
        assert!(catalog.entries[1].runtime_summary.required);
        assert_eq!(
            catalog.entries[1].runtime_summary.requirements,
            vec!["node >=20".to_string()]
        );
        assert!(catalog.entries[1].runtime_summary.has_runtime_payload);
        assert_eq!(sources[2], ("npm-plugin", "npm", "@scope/plugin@1.2.3"));
        assert_eq!(
            sources[3],
            ("zip-plugin", "zip-url", "https://example.com/plugin.zip")
        );
        assert_eq!(
            catalog.entries[3].source.content_hash().as_deref(),
            Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(catalog
            .entries
            .iter()
            .all(|entry| entry.source.local_path().is_none()));
    }

    #[test]
    fn invalid_remote_sources_are_listed_as_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "insecure-git",
                  "source": {
                    "source": "git",
                    "url": "http://example.com/team/plugins.git"
                  }
                },
                {
                  "name": "file-git-subdir",
                  "source": {
                    "source": "git-subdir",
                    "url": "file:///tmp/plugins.git",
                    "path": "plugins/demo"
                  }
                },
                {
                  "name": "missing-npm-integrity",
                  "source": {
                    "source": "npm",
                    "package": "@scope/plugin",
                    "version": "1.2.3"
                  }
                },
                {
                  "name": "bad-npm-registry",
                  "source": {
                    "source": "npm",
                    "package": "@scope/plugin",
                    "version": "1.2.3",
                    "registry": "http://registry.example.com"
                  }
                },
                {
                  "name": "bad-npm-version",
                  "source": {
                    "source": "npm",
                    "package": "scope/plugin",
                    "version": "../1.2.3"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let catalog = load_marketplace_catalog(&root.join("marketplace.json")).unwrap();
        let unsupported = catalog
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.name.as_str(),
                    entry.source.kind(),
                    entry.source.display(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(unsupported.len(), 5);
        assert_eq!(unsupported[0].0, "insecure-git");
        assert_eq!(unsupported[0].1, "git");
        assert!(unsupported[0].2.contains("must use https"));
        assert_eq!(unsupported[1].1, "git-subdir");
        assert!(unsupported[1].2.contains("must use https"));
        assert_eq!(unsupported[2].1, "npm");
        assert!(unsupported[2].2.contains("missing integrity or sha256"));
        assert_eq!(unsupported[3].1, "npm");
        assert!(unsupported[3].2.contains("registry must use https"));
        assert_eq!(unsupported[4].1, "npm");
        assert!(unsupported[4].2.contains("invalid npm marketplace package"));
        assert!(catalog
            .entries
            .iter()
            .all(|entry| matches!(entry.source, PluginMarketplaceSource::Unsupported { .. })));
    }

    #[test]
    fn zip_url_requires_https_and_valid_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "name": "remote",
              "plugins": [
                {
                  "name": "missing",
                  "source": {
                    "source": "zip-url",
                    "url": "https://example.com/missing.zip"
                  }
                },
                {
                  "name": "http",
                  "source": {
                    "source": "zip-url",
                    "url": "http://example.com/plugin.zip",
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                  }
                },
                {
                  "name": "bad-sha",
                  "source": {
                    "source": "zip-url",
                    "url": "https://example.com/plugin.zip",
                    "sha256": "abc"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let catalog = load_marketplace_catalog(&root.join("marketplace.json")).unwrap();

        assert!(catalog.entries.iter().all(|entry| {
            matches!(
                entry.source,
                PluginMarketplaceSource::Unsupported { ref kind, .. } if kind == "zip-url"
            )
        }));
        assert!(catalog.entries[0]
            .source
            .display()
            .contains("missing sha256"));
        assert!(catalog.entries[1]
            .source
            .display()
            .contains("requires https"));
        assert!(catalog.entries[2]
            .source
            .display()
            .contains("invalid sha256"));
    }

    #[test]
    fn rejects_remote_marketplace_source_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("marketplace.json"),
            r#"{
              "plugins": [
                {
                  "name": "bad",
                  "source": {
                    "source": "git-subdir",
                    "url": "https://example.com/team/plugins.git",
                    "path": "../plugins/bad"
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let err = load_marketplace_catalog(&root.join("marketplace.json")).unwrap_err();
        assert!(err.to_string().contains("cannot escape"));
    }
}
