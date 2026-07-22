//! Lightweight plugin preflight scanner.
//!
//! This is deliberately deterministic and offline. It is the foundation for a
//! later AI/security-review step, not a replacement for user approval.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use deepagent_mcp::config::McpConfig;
use deepagent_skills::SkillOrigin;
use serde::{Deserialize, Serialize};

use crate::plugin_manifest::{load_plugin_manifest, PluginManifest};

const BLOCKLIST_ENV: &str = "DEEPAGENT_PLUGIN_BLOCKLIST";
const ALLOWED_SOURCE_ROOTS_ENV: &str = "DEEPAGENT_PLUGIN_ALLOWED_SOURCE_ROOTS";
const MARKETPLACE_ALLOWED_SOURCE_KINDS_ENV: &str =
    "DEEPAGENT_PLUGIN_MARKETPLACE_ALLOWED_SOURCE_KINDS";
const MARKETPLACE_BLOCKED_SOURCE_KINDS_ENV: &str =
    "DEEPAGENT_PLUGIN_MARKETPLACE_BLOCKED_SOURCE_KINDS";
const RESERVED_PLUGIN_NAMES: &[&str] = &[
    "builtin",
    "cache",
    "data",
    "inline",
    "marketplace",
    "marketplaces",
    "plugin",
    "session",
    "state",
];

pub fn is_reserved_plugin_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    RESERVED_PLUGIN_NAMES.contains(&normalized.as_str())
}

pub fn is_blocked_plugin_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    blocked_plugin_names().contains(normalized.as_str())
}

pub fn marketplace_source_kind_policy_error(kind: &str) -> Option<String> {
    let blocked = marketplace_source_kind_set(MARKETPLACE_BLOCKED_SOURCE_KINDS_ENV);
    let allowed = marketplace_source_kind_set(MARKETPLACE_ALLOWED_SOURCE_KINDS_ENV);
    marketplace_source_kind_policy_error_with_rules(kind, &allowed, &blocked)
}

pub(crate) fn marketplace_source_kind_policy_error_with_rules(
    kind: &str,
    allowed: &BTreeSet<String>,
    blocked: &BTreeSet<String>,
) -> Option<String> {
    let normalized = normalize_source_kind(kind);
    if normalized.is_empty() {
        return Some("marketplace source kind is empty".to_string());
    }

    if source_kind_set_contains(&blocked, &normalized) {
        return Some(format!(
            "marketplace source kind '{kind}' is blocked by {MARKETPLACE_BLOCKED_SOURCE_KINDS_ENV}"
        ));
    }

    if !allowed.is_empty() && !source_kind_set_contains(&allowed, &normalized) {
        return Some(format!(
            "marketplace source kind '{kind}' is not allowed by {MARKETPLACE_ALLOWED_SOURCE_KINDS_ENV}"
        ));
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRiskItemDto {
    pub severity: String,
    pub category: String,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginComponentSummaryDto {
    pub kind: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginScanReportDto {
    pub source_dir: String,
    pub manifest_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    pub file_count: u32,
    pub total_bytes: u64,
    #[serde(default)]
    pub component_summaries: Vec<PluginComponentSummaryDto>,
    pub risks: Vec<PluginRiskItemDto>,
    pub errors: Vec<String>,
}

pub fn scan_plugin_dir(source_dir: &Path) -> Result<PluginScanReportDto> {
    if !source_dir.is_dir() {
        return Err(CoreError::invalid(format!(
            "plugin source is not a directory: {}",
            source_dir.display()
        )));
    }

    let mut report = PluginScanReportDto {
        source_dir: source_dir.display().to_string(),
        manifest_ok: false,
        plugin_name: None,
        file_count: 0,
        total_bytes: 0,
        component_summaries: Vec::new(),
        risks: Vec::new(),
        errors: Vec::new(),
    };

    apply_source_policy(source_dir, &mut report);

    match load_plugin_manifest(source_dir) {
        Ok(Some(manifest)) => {
            report.manifest_ok = true;
            report.plugin_name = Some(manifest.name.clone());
            scan_manifest_security(&manifest, &mut report);
            summarize_manifest_components(source_dir, &manifest, &mut report);
        }
        Ok(None) => report.errors.push("plugin manifest not found".to_string()),
        Err(err) => report.errors.push(err.to_string()),
    }

    scan_tree(source_dir, source_dir, &mut report)?;
    Ok(report)
}

fn scan_manifest_security(manifest: &PluginManifest, report: &mut PluginScanReportDto) {
    if is_reserved_plugin_name(&manifest.name) {
        report.errors.push(format!(
            "plugin name '{}' is reserved by DeepAgent",
            manifest.name
        ));
        report.risks.push(PluginRiskItemDto {
            severity: "critical".to_string(),
            category: "blocklist".to_string(),
            title: "Reserved plugin name".to_string(),
            detail: "Reserved names cannot be installed as third-party plugins.".to_string(),
            path: Some(manifest.manifest_path.display().to_string()),
        });
    }

    if is_blocked_plugin_name(&manifest.name) {
        report.errors.push(format!(
            "plugin name '{}' is blocked by {}",
            manifest.name, BLOCKLIST_ENV
        ));
        report.risks.push(PluginRiskItemDto {
            severity: "critical".to_string(),
            category: "blocklist".to_string(),
            title: "Blocked plugin name".to_string(),
            detail: format!("This plugin name is blocked by {BLOCKLIST_ENV}."),
            path: Some(manifest.manifest_path.display().to_string()),
        });
    }

    if !manifest.paths.mcp_server_paths.is_empty() || manifest.paths.mcp_servers_inline.is_some() {
        report.risks.push(PluginRiskItemDto {
            severity: "medium".to_string(),
            category: "mcp".to_string(),
            title: "Declares MCP servers".to_string(),
            detail: "MCP servers can connect to external services or spawn local processes."
                .to_string(),
            path: None,
        });
        scan_duplicate_mcp_servers(manifest, report);
    }
    if !manifest.paths.hook_paths.is_empty() || manifest.paths.hooks_inline.is_some() {
        report.risks.push(PluginRiskItemDto {
            severity: "high".to_string(),
            category: "hooks".to_string(),
            title: "Declares hooks".to_string(),
            detail: "Hooks can run during agent lifecycle events and should be reviewed before enabling.".to_string(),
            path: None,
        });
    }
    for permission in &manifest.interface.permissions {
        let severity = permission_severity(permission);
        report.risks.push(PluginRiskItemDto {
            severity: severity.to_string(),
            category: "permission".to_string(),
            title: format!("Requests {permission}"),
            detail: "Review the declared plugin permission before installing.".to_string(),
            path: Some(manifest.manifest_path.display().to_string()),
        });
    }
}

fn summarize_manifest_components(
    source_dir: &Path,
    manifest: &PluginManifest,
    report: &mut PluginScanReportDto,
) {
    summarize_skills(source_dir, manifest, report);
    summarize_commands(source_dir, manifest, report);
    summarize_agents(source_dir, manifest, report);
    summarize_output_styles(source_dir, manifest, report);
    report
        .component_summaries
        .sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
}

fn summarize_skills(
    source_dir: &Path,
    manifest: &PluginManifest,
    report: &mut PluginScanReportDto,
) {
    for root in manifest.paths.skills.iter().filter(|path| path.is_dir()) {
        match deepagent_skills::loader::discover_recursive(root, SkillOrigin::Plugin, 3) {
            Ok(skills) => {
                for skill in skills {
                    report.component_summaries.push(PluginComponentSummaryDto {
                        kind: "skill".to_string(),
                        name: skill.meta.name,
                        description: single_line(&skill.meta.description),
                        path: skill
                            .base_dir
                            .as_ref()
                            .map(|path| relative_display(source_dir, path)),
                        details: skill
                            .meta
                            .version
                            .map(|version| vec![format!("version: {version}")])
                            .unwrap_or_default(),
                    });
                }
            }
            Err(error) => {
                report
                    .risks
                    .push(component_summary_error("skills", root, error.to_string()))
            }
        }
    }
}

fn summarize_commands(
    source_dir: &Path,
    manifest: &PluginManifest,
    report: &mut PluginScanReportDto,
) {
    for root in manifest.paths.commands.iter().filter(|path| path.is_dir()) {
        let files = match markdown_files(root, false) {
            Ok(files) => files,
            Err(error) => {
                report
                    .risks
                    .push(component_summary_error("commands", root, error.to_string()));
                continue;
            }
        };
        for file in files {
            match deepagent_prompts::load_command_file(&file) {
                Ok(command) => {
                    let mut details = Vec::new();
                    if !command.allowed_tools.is_empty() {
                        details.push(format!(
                            "allowed tools: {}",
                            command.allowed_tools.join(", ")
                        ));
                    }
                    if command.disable_model_invocation {
                        details.push("user-only".to_string());
                    }
                    report.component_summaries.push(PluginComponentSummaryDto {
                        kind: "command".to_string(),
                        name: format!("/{}:{}", manifest.name, command.name),
                        description: single_line_or(&command.description, "Plugin prompt command"),
                        path: Some(relative_display(source_dir, &file)),
                        details,
                    });
                }
                Err(error) => report.risks.push(component_summary_error(
                    "commands",
                    &file,
                    error.to_string(),
                )),
            }
        }
    }
}

fn summarize_agents(
    source_dir: &Path,
    manifest: &PluginManifest,
    report: &mut PluginScanReportDto,
) {
    for root in manifest.paths.agents.iter().filter(|path| path.is_dir()) {
        let files = match markdown_files(root, false) {
            Ok(files) => files,
            Err(error) => {
                report
                    .risks
                    .push(component_summary_error("agents", root, error.to_string()));
                continue;
            }
        };
        for file in files {
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) => {
                    report
                        .risks
                        .push(component_summary_error("agents", &file, error.to_string()));
                    continue;
                }
            };
            match deepagent_prompts::AgentDef::parse(&text) {
                Some(agent) => {
                    let mut details = Vec::new();
                    if !agent.tools.is_empty() {
                        details.push(format!("tools: {}", agent.tools.join(", ")));
                    }
                    if let Some(model) = agent.model.name() {
                        details.push(format!("model: {model}"));
                    }
                    report.component_summaries.push(PluginComponentSummaryDto {
                        kind: "agent".to_string(),
                        name: agent.name,
                        description: single_line(&agent.description),
                        path: Some(relative_display(source_dir, &file)),
                        details,
                    });
                }
                None => report.risks.push(component_summary_error(
                    "agents",
                    &file,
                    "agent file is missing name or description".to_string(),
                )),
            }
        }
    }
}

fn summarize_output_styles(
    source_dir: &Path,
    manifest: &PluginManifest,
    report: &mut PluginScanReportDto,
) {
    for root in manifest
        .paths
        .output_styles
        .iter()
        .filter(|path| path.exists())
    {
        let files = if root.is_file() {
            if is_markdown_file(root) {
                vec![root.clone()]
            } else {
                Vec::new()
            }
        } else {
            match markdown_files(root, true) {
                Ok(files) => files,
                Err(error) => {
                    report.risks.push(component_summary_error(
                        "output-styles",
                        root,
                        error.to_string(),
                    ));
                    continue;
                }
            }
        };
        for file in files {
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) => {
                    report.risks.push(component_summary_error(
                        "output-styles",
                        &file,
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let fm = deepagent_prompts::frontmatter::parse(&text);
            let name = fm
                .get("name")
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    file.file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "style".to_string());
            let description = fm
                .get("description")
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_string)
                .or_else(|| first_markdown_paragraph(&fm.body))
                .unwrap_or_else(|| "Plugin output style".to_string());
            let mut details = Vec::new();
            if fm
                .get_bool("force-for-plugin")
                .or_else(|| fm.get_bool("force_for_plugin"))
                .unwrap_or(false)
            {
                details.push("forced for plugin".to_string());
            }
            report.component_summaries.push(PluginComponentSummaryDto {
                kind: "output-style".to_string(),
                name,
                description: single_line(&description),
                path: Some(relative_display(source_dir, &file)),
                details,
            });
        }
    }
}

fn component_summary_error(component: &str, path: &Path, detail: String) -> PluginRiskItemDto {
    PluginRiskItemDto {
        severity: "medium".to_string(),
        category: format!("{component}-summary"),
        title: format!("Failed to summarize {component}"),
        detail,
        path: Some(path.display().to_string()),
    }
}

fn scan_duplicate_mcp_servers(manifest: &PluginManifest, report: &mut PluginScanReportDto) {
    let mut seen = BTreeMap::<String, String>::new();
    for path in &manifest.paths.mcp_server_paths {
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(path)
            .map_err(|e| CoreError::Persistence(format!("read {}: {e}", path.display())))
            .and_then(|text| McpConfig::parse(&text))
        {
            Ok(config) => record_mcp_names(config, path.display().to_string(), &mut seen, report),
            Err(error) => report.risks.push(PluginRiskItemDto {
                severity: "high".to_string(),
                category: "mcp-invalid".to_string(),
                title: "Invalid MCP config".to_string(),
                detail: error.to_string(),
                path: Some(path.display().to_string()),
            }),
        }
    }
    if let Some(value) = manifest.paths.mcp_servers_inline.as_ref() {
        match serde_json::to_string(value)
            .map_err(CoreError::from)
            .and_then(|text| McpConfig::parse(&text))
        {
            Ok(config) => {
                record_mcp_names(config, "manifest inline".to_string(), &mut seen, report)
            }
            Err(error) => report.risks.push(PluginRiskItemDto {
                severity: "high".to_string(),
                category: "mcp-invalid".to_string(),
                title: "Invalid inline MCP config".to_string(),
                detail: error.to_string(),
                path: Some(manifest.manifest_path.display().to_string()),
            }),
        }
    }
}

fn record_mcp_names(
    config: McpConfig,
    source: String,
    seen: &mut BTreeMap<String, String>,
    report: &mut PluginScanReportDto,
) {
    for name in config.servers.keys() {
        if let Some(previous) = seen.insert(name.clone(), source.clone()) {
            report.risks.push(PluginRiskItemDto {
                severity: "high".to_string(),
                category: "duplicate-mcp".to_string(),
                title: format!("Duplicate MCP server: {name}"),
                detail: format!(
                    "MCP server '{name}' is declared more than once ({previous}, {source})."
                ),
                path: Some(source.clone()),
            });
        }
    }
}

fn permission_severity(permission: &str) -> &'static str {
    let normalized = permission.to_ascii_lowercase();
    if normalized.contains("write")
        || normalized.contains("exec")
        || normalized.contains("runtime")
        || normalized.contains("hook")
    {
        "high"
    } else if normalized.contains("network")
        || normalized.contains("mcp")
        || normalized.contains("read")
        || normalized.contains("browser")
    {
        "medium"
    } else {
        "low"
    }
}

fn blocked_plugin_names() -> BTreeSet<String> {
    std::env::var(BLOCKLIST_ENV)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split([',', ';', '\n', '\r', '\t', ' '])
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn marketplace_source_kind_set(env_name: &str) -> BTreeSet<String> {
    std::env::var(env_name)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split([',', ';', '\n', '\r', '\t', ' '])
                .map(normalize_source_kind)
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn normalize_source_kind(kind: &str) -> String {
    kind.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn source_kind_set_contains(set: &BTreeSet<String>, normalized_kind: &str) -> bool {
    set.contains("*") || set.contains("all") || set.contains(normalized_kind)
}

fn apply_source_policy(source_dir: &Path, report: &mut PluginScanReportDto) {
    let Some(allowed_roots) = std::env::var_os(ALLOWED_SOURCE_ROOTS_ENV) else {
        return;
    };
    let roots = std::env::split_paths(&allowed_roots)
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return;
    }
    let source = std::fs::canonicalize(source_dir).unwrap_or_else(|_| source_dir.to_path_buf());
    if roots.iter().any(|root| source.starts_with(root)) {
        return;
    }
    report.errors.push(format!(
        "plugin source {} is not allowed by {}",
        source.display(),
        ALLOWED_SOURCE_ROOTS_ENV
    ));
    report.risks.push(PluginRiskItemDto {
        severity: "critical".to_string(),
        category: "source-policy".to_string(),
        title: "Source rejected by policy".to_string(),
        detail: format!("The plugin source is outside every root in {ALLOWED_SOURCE_ROOTS_ENV}."),
        path: Some(source.display().to_string()),
    });
}

fn scan_tree(root: &Path, dir: &Path, report: &mut PluginScanReportDto) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        CoreError::Persistence(format!("read plugin directory {}: {e}", dir.display()))
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy() == ".git" {
                continue;
            }
            scan_tree(root, &path, report)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        report.file_count += 1;
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        report.total_bytes = report.total_bytes.saturating_add(size);
        let rel = relative_display(root, &path);
        if size > 10 * 1024 * 1024 {
            report.risks.push(PluginRiskItemDto {
                severity: "medium".to_string(),
                category: "large-file".to_string(),
                title: "Large file".to_string(),
                detail: format!("File is larger than 10 MiB ({size} bytes)."),
                path: Some(rel.clone()),
            });
        }
        if is_suspicious_install_file(&rel) {
            report.risks.push(PluginRiskItemDto {
                severity: "critical".to_string(),
                category: "install-script".to_string(),
                title: "Suspicious install script".to_string(),
                detail: "Install/setup scripts should only be installed from trusted sources."
                    .to_string(),
                path: Some(rel.clone()),
            });
        }
        if let Some(ext) = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
        {
            match ext.as_str() {
                "exe" | "dll" | "dylib" | "so" | "bin" => report.risks.push(PluginRiskItemDto {
                    severity: "critical".to_string(),
                    category: "binary".to_string(),
                    title: "Binary executable or library".to_string(),
                    detail: "Binary files should only be installed from trusted sources.".to_string(),
                    path: Some(rel.clone()),
                }),
                "ps1" | "bat" | "cmd" | "sh" | "bash" | "zsh" => report.risks.push(PluginRiskItemDto {
                    severity: "high".to_string(),
                    category: "script".to_string(),
                    title: "Executable script".to_string(),
                    detail: "Scripts can run local commands when wired through hooks, MCP, or install flows.".to_string(),
                    path: Some(rel.clone()),
                }),
                _ => {}
            }
        }
    }
    Ok(())
}

fn markdown_files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(root).map_err(|e| {
        CoreError::Persistence(format!("read markdown dir {}: {e}", root.display()))
    })?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if recursive {
                out.extend(markdown_files(&path, recursive)?);
            }
        } else if file_type.is_file() && is_markdown_file(&path) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "mdx"))
        .unwrap_or(false)
}

fn relative_display(root: &Path, path: &PathBuf) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn single_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn single_line_or(input: &str, fallback: &str) -> String {
    let line = single_line(input);
    if line.is_empty() {
        fallback.to_string()
    } else {
        line
    }
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
    let joined = single_line(&paragraph.join(" "));
    if joined.is_empty() {
        None
    } else {
        Some(truncate_chars(&joined, 240))
    }
}

fn truncate_chars(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.to_string();
    }
    let keep = cap.saturating_sub(3);
    let mut out = input.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

fn is_suspicious_install_file(relative_path: &str) -> bool {
    let file_name = relative_path
        .rsplit('/')
        .next()
        .unwrap_or(relative_path)
        .to_ascii_lowercase();
    file_name == "install.sh"
        || file_name == "setup.sh"
        || file_name == "postinstall.sh"
        || file_name == "install.ps1"
        || file_name == "setup.ps1"
        || file_name == "postinstall.ps1"
        || file_name.starts_with("postinstall.")
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
    fn scan_reports_duplicate_mcp_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{
              "name": "mcp-plugin",
              "mcpServers": ["mcp-one.json", "mcp-two.json"]
            }"#,
        );
        write(
            &root.join("mcp-one.json"),
            r#"{"mcpServers":{"docs":{"command":"node"}}}"#,
        );
        write(
            &root.join("mcp-two.json"),
            r#"{"mcpServers":{"docs":{"command":"python"}}}"#,
        );

        let report = scan_plugin_dir(root).unwrap();

        assert!(report
            .risks
            .iter()
            .any(|risk| { risk.category == "duplicate-mcp" && risk.title.contains("docs") }));
    }

    #[test]
    fn scan_blocks_reserved_plugin_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"builtin"}"#,
        );

        let report = scan_plugin_dir(root).unwrap();

        assert!(report.errors.iter().any(|error| error.contains("reserved")));
        assert!(report.risks.iter().any(|risk| risk.category == "blocklist"));
    }

    #[test]
    fn scan_flags_suspicious_install_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"installer"}"#,
        );
        write(
            &root.join("scripts").join("postinstall.ps1"),
            "Write-Host ok",
        );

        let report = scan_plugin_dir(root).unwrap();

        assert!(report
            .risks
            .iter()
            .any(|risk| risk.category == "install-script"));
    }

    #[test]
    fn scan_summarizes_declared_plugin_components() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join(".codex-plugin").join("plugin.json"),
            r#"{
              "name": "review-kit",
              "skills": "./skills",
              "commands": "./commands",
              "agents": "./agents",
              "outputStyles": "./output-styles"
            }"#,
        );
        write(
            &root.join("skills").join("review").join("SKILL.md"),
            "---\nname: Review Skill\ndescription: Use for code review.\nversion: 1.0.0\n---\nBody",
        );
        write(
            &root.join("commands").join("review.md"),
            "---\ndescription: Review a diff\nallowed-tools: Read, Grep\n---\nReview $ARGUMENTS",
        );
        write(
            &root.join("agents").join("critic.md"),
            "---\nname: critic\ndescription: Finds design risks\ntools: Read, Grep\nmodel: inherit\n---\nBody",
        );
        write(
            &root.join("output-styles").join("concise.md"),
            "---\nname: concise\ndescription: Keep answers short\nforce-for-plugin: true\n---\nRespond briefly.",
        );

        let report = scan_plugin_dir(root).unwrap();

        assert!(report.component_summaries.iter().any(|summary| {
            summary.kind == "skill"
                && summary.name == "Review Skill"
                && summary.description == "Use for code review."
        }));
        assert!(report.component_summaries.iter().any(|summary| {
            summary.kind == "command"
                && summary.name == "/review-kit:review"
                && summary.details.iter().any(|detail| detail.contains("Read"))
        }));
        assert!(report.component_summaries.iter().any(|summary| {
            summary.kind == "agent"
                && summary.name == "critic"
                && summary.details.iter().any(|detail| detail.contains("Read"))
        }));
        assert!(report.component_summaries.iter().any(|summary| {
            summary.kind == "output-style"
                && summary.name == "concise"
                && summary
                    .details
                    .iter()
                    .any(|detail| detail == "forced for plugin")
        }));
    }

    #[test]
    fn marketplace_source_kind_policy_respects_allow_and_block_lists() {
        let allowed = ["local", "npm"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let blocked = ["git"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();

        assert!(
            marketplace_source_kind_policy_error_with_rules("local", &allowed, &blocked).is_none()
        );
        assert!(
            marketplace_source_kind_policy_error_with_rules("git", &allowed, &blocked)
                .unwrap()
                .contains("blocked")
        );
        assert!(
            marketplace_source_kind_policy_error_with_rules("zip-url", &allowed, &blocked)
                .unwrap()
                .contains("not allowed")
        );
    }
}
