//! 真实内置插件回归：固定当前插件包的组件计数与加载边界。
//!
//! 这些 fixture 来自桌面应用实际随包资源，不使用人工构造的理想目录。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use deepagent_app_core::plugin_loader::{load_plugins, PluginLoadError, PluginRoots};
use deepagent_app_core::{PluginExecutionKind, PluginHealthStatus, PluginService};
use deepagent_skills::{loader, SkillOrigin, SkillRegistry};

const EXPECTED: &[(&str, u32, u32, u32, u32, u32, u32)] = &[
    // name, skills, mcp, hooks, commands, apps, output styles
    ("browser", 0, 0, 0, 1, 1, 0),
    ("files", 0, 0, 0, 1, 1, 0),
    ("meeting-recorder", 0, 0, 0, 1, 1, 0),
    ("office-agent", 0, 0, 0, 1, 1, 1),
    ("project-map", 0, 0, 0, 1, 1, 0),
    ("side-chat", 0, 0, 0, 1, 1, 0),
    ("superpowers", 14, 0, 0, 0, 0, 0),
    ("terminal", 0, 0, 0, 1, 1, 0),
    ("wedecode", 0, 0, 0, 1, 0, 0),
];

const EXISTING_HOST_ADAPTERS: &[&str] = &[
    "browser",
    "files",
    "meeting-recorder",
    "office-agent",
    "project-map",
    "side-chat",
    "terminal",
    "wedecode",
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

#[test]
fn existing_host_adapters_preserve_state_and_data_from_install_dir() {
    let root = bundled_plugins_root();
    if !root.is_dir() {
        eprintln!("skipping: bundled plugin resources are not present");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("install");
    let plugin_state_dir = install_dir.join("plugins");
    let plugin_data_dir = plugin_state_dir.join("data");
    std::fs::create_dir_all(&plugin_data_dir).unwrap();

    let mut enabled = BTreeMap::new();
    let mut health_checks = BTreeMap::new();
    for (index, name) in EXISTING_HOST_ADAPTERS.iter().enumerate() {
        let id = format!("{name}@builtin");
        enabled.insert(id.clone(), index % 2 == 0);
        health_checks.insert(
            id.clone(),
            serde_json::json!({
                "status": "incomplete",
                "checked_at": format!("2026-08-15T00:00:{index:02}Z"),
                "error": format!("seeded health check for {id}")
            }),
        );
        let data_dir = plugin_data_dir.join(sanitize_plugin_data_dir(&id));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("state-marker.json"),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
    }
    std::fs::write(
        plugin_state_dir.join("state.json"),
        serde_json::json!({
            "version": 1,
            "enabled": enabled,
            "health_checks": health_checks
        })
        .to_string(),
    )
    .unwrap();

    let svc = PluginService::new(
        PluginRoots {
            session: Vec::new(),
            builtin: root.clone(),
            workspace: None,
            personal: plugin_state_dir.join("personal"),
        },
        &install_dir,
    );
    let plugins = svc.list().unwrap();

    for (index, name) in EXISTING_HOST_ADAPTERS.iter().enumerate() {
        let id = format!("{name}@builtin");
        let matches = plugins
            .iter()
            .filter(|plugin| plugin.id == id)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{id} must not be deleted or duplicated");
        let plugin = matches[0];
        assert_eq!(plugin.name, *name);
        assert_eq!(plugin.origin, "builtin");
        assert_eq!(plugin.source.kind, "builtin");
        assert!(plugin.installed, "{id} should still be installed");
        assert!(plugin.available, "{id} should still be available");
        assert_eq!(plugin.enabled, index % 2 == 0, "{id} enabled state");
        assert_eq!(plugin.health_status, PluginHealthStatus::Incomplete);
        assert_eq!(
            plugin.last_health_check.as_deref(),
            Some(format!("2026-08-15T00:00:{index:02}Z").as_str()),
            "{id} last health check"
        );
        assert_eq!(
            plugin.health_error.as_deref(),
            Some(format!("seeded health check for {id}").as_str()),
            "{id} health error"
        );

        let expected_data_dir = plugin_data_dir.join(sanitize_plugin_data_dir(&id));
        assert_eq!(PathBuf::from(&plugin.data_dir), expected_data_dir);
        assert!(
            expected_data_dir.join("state-marker.json").is_file(),
            "{id} user data must remain in the install-dir plugin data tree"
        );
        if *name != "wedecode" {
            assert_eq!(plugin.execution_kind, PluginExecutionKind::HostBacked);
        }
    }
}

#[test]
fn existing_host_adapters_only_expose_registered_renderable_apps() {
    let root = bundled_plugins_root();
    if !root.is_dir() {
        eprintln!("skipping: bundled plugin resources are not present");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let svc = PluginService::new(
        PluginRoots {
            session: Vec::new(),
            builtin: root,
            workspace: None,
            personal: tmp.path().join("personal"),
        },
        tmp.path().join("app-data"),
    );

    for name in EXISTING_HOST_ADAPTERS
        .iter()
        .copied()
        .filter(|name| *name != "wedecode")
    {
        let plugin = svc
            .read(&format!("{name}@builtin"))
            .unwrap()
            .unwrap_or_else(|| panic!("missing bundled host adapter {name}"));
        assert_eq!(
            plugin.execution_kind,
            PluginExecutionKind::HostBacked,
            "{name} should be classified through host bindings, not plugin-id special cases"
        );
        assert_eq!(plugin.command_count, 1, "{name} command count changed");
        assert_eq!(plugin.app_count, 1, "{name} app count changed");
    }

    let app_ids = svc
        .list_apps()
        .unwrap()
        .into_iter()
        .map(|app| (app.plugin_id, app.component))
        .collect::<BTreeMap<_, _>>();
    for name in [
        "browser",
        "files",
        "meeting-recorder",
        "office-agent",
        "project-map",
        "side-chat",
        "terminal",
    ] {
        assert!(
            app_ids.contains_key(&format!("{name}@builtin")),
            "{name} should expose a registered renderable host app"
        );
    }
    assert_eq!(
        app_ids.len(),
        7,
        "only the remaining host apps should be rendered"
    );
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
    let superpowers_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/plugins/superpowers");
    assert!(superpowers_root.join(".codex-plugin/plugin.json").is_file());
    assert!(superpowers_root.join("README.md").is_file());
    assert!(superpowers_root.join("LICENSE").is_file());
    assert!(superpowers_root.join("assets").is_dir());
    assert!(superpowers_root.join("skills").is_dir());
    assert!(superpowers_root
        .join("skills")
        .join("writing-plans")
        .join("SKILL.md")
        .is_file());
    assert!(superpowers_root
        .join("skills")
        .join("using-superpowers")
        .join("references")
        .is_dir());
    assert!(superpowers_root
        .join("skills")
        .join("systematic-debugging")
        .join("SKILL.md")
        .is_file());
}

#[test]
fn superpowers_core_skills_match_and_activate_from_real_bundle() {
    let Some(registry) = superpowers_registry() else {
        eprintln!("skipping: bundled superpowers plugin resource is not present");
        return;
    };

    assert_eq!(registry.len(), 14, "superpowers skill set changed");
    for id in [
        "writing-plans",
        "test-driven-development",
        "systematic-debugging",
        "requesting-code-review",
    ] {
        assert!(
            registry.contains(id),
            "missing superpowers core skill: {id}"
        );
    }

    let cases = [
        (
            "I have a spec and requirements for a multi-step task before touching code",
            "writing-plans",
        ),
        (
            "I am implementing a feature or bugfix before writing implementation code",
            "test-driven-development",
        ),
        (
            "We are encountering a bug, test failure, or unexpected behavior before proposing fixes",
            "systematic-debugging",
        ),
        (
            "I am completing tasks, implementing major features, and before merging need to verify work meets requirements",
            "requesting-code-review",
        ),
    ];

    for (query, expected) in cases {
        let best = registry
            .best_match(query)
            .unwrap_or_else(|| panic!("no superpowers skill matched query: {query:?}"));
        assert_eq!(best.id, expected, "query {query:?} routed to {}", best.id);

        let activated = registry
            .body_for_invoke(&best.id, None)
            .unwrap_or_else(|error| panic!("failed to activate {expected}: {error}"));
        assert_eq!(activated.id, expected);
        assert!(
            activated.body.contains("# "),
            "activated skill body should include real SKILL.md content for {expected}"
        );
        assert!(
            activated
                .base_dir
                .as_deref()
                .is_some_and(|base| base.replace('\\', "/").contains("/superpowers/skills/")),
            "activated skill should retain its on-disk bundle path: {:?}",
            activated.base_dir
        );
    }
}

#[test]
fn superpowers_skill_resources_are_discoverable_on_activation() {
    let Some(registry) = superpowers_registry() else {
        eprintln!("skipping: bundled superpowers plugin resource is not present");
        return;
    };

    let activated = registry
        .body_for_invoke("writing-skills", None)
        .expect("writing-skills activates from real superpowers bundle");

    assert!(
        activated
            .resources
            .contains(&"examples/CLAUDE_MD_TESTING.md".to_string()),
        "writing-skills should expose bundled example resources, got {:?}",
        activated.resources
    );
}

fn superpowers_registry() -> Option<SkillRegistry> {
    let root = bundled_plugins_root().join("superpowers/skills");
    if !root.is_dir() {
        return None;
    }

    let mut registry = SkillRegistry::new();
    for skill in loader::discover(&root, SkillOrigin::Plugin).expect("discover superpowers skills")
    {
        registry.register(skill);
    }
    Some(registry)
}

fn bundled_plugins_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src-tauri/resources/plugins")
}

fn sanitize_plugin_data_dir(id: &str) -> String {
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
