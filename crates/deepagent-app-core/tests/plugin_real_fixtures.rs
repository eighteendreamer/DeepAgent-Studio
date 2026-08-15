//! 真实内置插件回归：固定当前插件包的组件计数与加载边界。
//!
//! 这些 fixture 来自桌面应用实际随包资源，不使用人工构造的理想目录。

use std::path::{Path, PathBuf};
use std::process::Command;

use deepagent_app_core::plugin_loader::{load_plugins, PluginLoadError, PluginRoots};
use deepagent_app_core::{
    PluginExecutionKind, PluginHealthStatus, PluginLifecycleState, PluginService,
};
use deepagent_skills::{loader, SkillOrigin, SkillRegistry};

const EXPECTED: &[(&str, u32, u32, u32, u32, u32, u32)] = &[
    // name, skills, mcp, hooks, commands, apps, output styles
    ("boltz-api-cli", 8, 0, 0, 0, 0, 0),
    ("browser", 0, 0, 0, 1, 1, 0),
    ("computer-use", 0, 0, 0, 1, 1, 0),
    ("figma", 12, 1, 1, 4, 1, 0),
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

    let boltz_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/plugins/boltz-api-cli");
    assert!(
        boltz_root
            .join("tests")
            .join("test_scan_sites.py")
            .is_file(),
        "boltz dry-run fixture missing"
    );
    assert!(
        boltz_root
            .join("skills")
            .join("boltz-protein-design")
            .join("scripts")
            .join("scan_sites.py")
            .is_file(),
        "boltz scan_sites script missing"
    );
}

#[test]
fn bundled_figma_requires_authorization_and_projects_real_runtime_entries() {
    let root = bundled_plugins_root();
    if !root.is_dir() {
        eprintln!("skipping: bundled plugin resources are not present");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let svc = PluginService::new(
        PluginRoots {
            session: Vec::new(),
            builtin: root.clone(),
            workspace: None,
            personal: tmp.path().join("personal"),
            marketplace_cache: tmp.path().join("cache"),
            marketplaces: tmp.path().join("marketplaces"),
        },
        tmp.path().join("app-data"),
    );

    let figma = svc.read("figma@builtin").unwrap().expect("bundled figma");

    assert_eq!(figma.execution_kind, PluginExecutionKind::DshSidecar);
    assert_eq!(figma.health_status, PluginHealthStatus::NeedsAuthorization);
    assert_eq!(figma.state, PluginLifecycleState::RuntimeReady);
    assert_eq!(figma.skill_count, 12);
    assert_eq!(figma.command_count, 4);
    assert_eq!(figma.agent_count, 4);
    assert_eq!(figma.hook_count, 1);
    assert_eq!(figma.mcp_server_count, 1);
    assert_eq!(figma.app_count, 1);
    assert!(figma
        .health_error
        .as_deref()
        .unwrap_or_default()
        .contains("authorization"));

    let projection = svc.runtime_projection().unwrap();
    assert!(projection
        .mcp_server_sources
        .values()
        .any(|source| source.plugin_id == "figma@builtin" && source.declared_name == "figma"));
    assert!(projection
        .hook_definitions
        .hooks
        .get("PostToolUse")
        .into_iter()
        .flatten()
        .any(|group| group.matcher.as_deref() == Some("Write|Edit")
            && group.hooks.iter().any(|hook| hook
                .command
                .replace('\\', "/")
                .ends_with("figma/scripts/post_write_figma_parity_check.sh"))));
    assert!(projection
        .connector_entries
        .iter()
        .any(|connector| connector.plugin_id == "figma@builtin"
            && connector.provider == "figma"
            && connector.id == "connector_68df038e0ba48191908c8434991bbac2"));
    assert!(
        !projection
            .app_entries
            .iter()
            .any(|app| app.plugin_id == "figma@builtin"),
        "figma connector must not be exposed as a renderable builtin app"
    );
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

#[test]
fn boltz_api_cli_python_dry_run_executes_bundled_script_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/plugins/boltz-api-cli");
    if !root.is_dir() {
        eprintln!("skipping: bundled boltz plugin resource is not present");
        return;
    }
    let Some(python) = python_command() else {
        eprintln!("skipping: python runtime is not available");
        return;
    };

    let test_file = root.join("tests").join("test_scan_sites.py");
    let output = Command::new(&python)
        .arg(&test_file)
        .current_dir(&root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", python.display()));

    assert!(
        output.status.success(),
        "boltz python dry-run failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Ran 6 tests") && stderr.contains("OK"),
        "unexpected boltz dry-run output:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
}

fn python_command() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("python"), PathBuf::from("python3")];
    if let Some(path) = std::env::var_os("DEEPAGENT_PYTHON").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    candidates.into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}
