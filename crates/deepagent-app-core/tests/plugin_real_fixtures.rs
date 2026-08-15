//! 真实内置插件回归：固定当前插件包的组件计数与加载边界。
//!
//! 这些 fixture 来自桌面应用实际随包资源，不使用人工构造的理想目录。

use std::path::{Path, PathBuf};

use deepagent_app_core::plugin_loader::{load_plugins, PluginLoadError, PluginRoots};

const EXPECTED: &[(&str, u32, u32, u32, u32, u32, u32)] = &[
    // name, skills, mcp, hooks, commands, apps, output styles
    ("boltz-api-cli", 8, 0, 0, 0, 0, 0),
    ("browser", 0, 0, 0, 1, 1, 0),
    ("computer-use", 0, 0, 0, 1, 1, 0),
    ("figma", 12, 1, 0, 4, 1, 0),
    ("files", 0, 0, 0, 1, 1, 0),
    ("meeting-recorder", 0, 0, 0, 1, 1, 0),
    ("office-agent", 0, 0, 0, 1, 1, 1),
    ("project-map", 0, 0, 0, 1, 1, 0),
    ("side-chat", 0, 0, 0, 1, 1, 0),
    ("superpowers", 14, 0, 0, 0, 0, 0),
    ("terminal", 0, 0, 0, 1, 1, 0),
    ("wedecode", 0, 0, 0, 1, 0, 0),
];

#[test]
fn bundled_plugins_keep_their_component_counts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/plugins");
    if !root.is_dir() {
        eprintln!("skipping: bundled plugin resources are not present");
        return;
    }

    let loaded = load_plugins(&PluginRoots {
        session: Vec::new(),
        builtin: root,
        workspace: None,
        personal: PathBuf::from("__missing_personal_plugins__"),
        marketplace_cache: PathBuf::from("__missing_marketplace_cache__"),
        marketplaces: PathBuf::from("__missing_marketplaces__"),
    });

    assert_eq!(loaded.len(), EXPECTED.len(), "built-in plugin set changed");
    for (name, skills, mcp, hooks, commands, apps, output_styles) in EXPECTED {
        let plugin = loaded
            .iter()
            .find(|plugin| plugin.name == *name)
            .unwrap_or_else(|| panic!("missing bundled plugin {name}"));
        assert!(plugin.resolved().is_some(), "{name} must resolve");
        assert!(
            !plugin.errors.iter().any(is_fatal_loader_error),
            "{name} has fatal loader errors: {:?}",
            plugin.errors
        );

        let manifest = &plugin.resolved().expect("checked above").manifest;
        assert_eq!(count_skills(manifest), *skills, "{name} skills");
        assert_eq!(count_mcp(manifest), *mcp, "{name} MCP");
        assert_eq!(
            count_files(&manifest.paths.hook_paths),
            *hooks,
            "{name} hooks"
        );
        assert_eq!(
            count_markdown_like(&manifest.paths.commands),
            *commands,
            "{name} commands"
        );
        assert_eq!(
            count_existing(&manifest.paths.app_paths),
            *apps,
            "{name} apps"
        );
        assert_eq!(
            count_markdown_like(&manifest.paths.output_styles),
            *output_styles,
            "{name} output styles"
        );
    }
}

fn is_fatal_loader_error(error: &PluginLoadError) -> bool {
    error.severity.as_str() == "error"
}

fn count_skills(manifest: &deepagent_app_core::plugin_manifest::PluginManifest) -> u32 {
    manifest
        .paths
        .skills
        .iter()
        .map(|path| {
            if path.join("SKILL.md").is_file() {
                return 1;
            }
            std::fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.path().join("SKILL.md").is_file())
                .count() as u32
        })
        .sum()
}

fn count_mcp(manifest: &deepagent_app_core::plugin_manifest::PluginManifest) -> u32 {
    manifest
        .paths
        .mcp_server_paths
        .iter()
        .filter(|path| path.is_file())
        .count() as u32
        + manifest
            .paths
            .mcp_servers_inline
            .as_ref()
            .and_then(|value| value.get("mcpServers").or(Some(value)))
            .and_then(|value| value.as_object())
            .map(|value| value.len() as u32)
            .unwrap_or_default()
}

fn count_files(paths: &[PathBuf]) -> u32 {
    paths.iter().filter(|path| path.is_file()).count() as u32
}

fn count_existing(paths: &[PathBuf]) -> u32 {
    paths.iter().filter(|path| path.exists()).count() as u32
}

fn count_markdown_like(paths: &[PathBuf]) -> u32 {
    paths
        .iter()
        .map(|path| count_markdown_path(path.as_path()))
        .sum()
}

fn count_markdown_path(path: &Path) -> u32 {
    if path.is_file() {
        return u32::from(
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(extension.to_ascii_lowercase().as_str(), "md" | "mdx")
                }),
        );
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| count_markdown_path(&entry.path()))
        .sum()
}

#[test]
fn complete_bundled_plugins_are_real_resources() {
    for name in ["superpowers", "figma", "boltz-api-cli"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src-tauri/resources/plugins")
            .join(name);
        assert!(root.is_dir(), "missing bundled plugin root: {name}");
        let manifest = root.join(".codex-plugin").join("plugin.json");
        assert!(manifest.is_file(), "missing manifest: {name}");
        let text = std::fs::read_to_string(&manifest).expect("manifest text");
        assert!(text.contains("\"name\""), "manifest missing name: {name}");
        assert!(
            root.join("skills").is_dir() || root.join("commands").is_dir(),
            "expected real plugin content for {name}"
        );
    }

    let figma_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/plugins/figma");
    assert!(
        figma_root.join("commands").is_dir(),
        "figma commands missing"
    );
    assert!(
        figma_root.join("hooks.json").is_file(),
        "figma hooks missing"
    );
    assert!(
        figma_root.join(".app.json").is_file(),
        "figma app config missing"
    );
}
