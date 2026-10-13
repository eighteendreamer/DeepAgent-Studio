//! Opt-in acceptance of the shipped Windows ZIP through the existing host seams.
//! REA_PLUGIN_ZIP, REA_RUNTIME_ROOT and REA_MANAGED_FIXTURE must be supplied.
#![cfg(windows)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use deepagent_app_core::{
    McpService, PluginRoots, PluginService, ProjectService, RuntimeBroker, RuntimeService,
    UnavailableDownloader,
};
use deepagent_builtins::{DeferredToolSnapshot, ToolSearchTool};
use deepagent_mcp::adapters_for;
use deepagent_persistence::Database;
use deepagent_tools::{Permission, PermissionSet, Tool, ToolRegistry};
use serde_json::json;

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")))
}

#[tokio::test]
#[ignore = "requires packaged REA, installed managed Node 24 and a benign managed assembly"]
async fn rea_zip_installs_discovers_and_analyzes_without_target_execution() {
    tokio::time::timeout(Duration::from_secs(120), verify_artifact())
        .await
        .expect("artifact acceptance deadline");
}

async fn verify_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("REA 插件 验证");
    let source = root.join("zip source");
    std::fs::create_dir_all(&source).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(required_path("REA_PLUGIN_ZIP")).unwrap())
            .unwrap();
    assert!(archive.len() < 100_000);
    assert!(archive.decompressed_size().unwrap() < 512 * 1024 * 1024);
    archive.extract(&source).unwrap();
    assert!(source.join(".codex-plugin/plugin.json").is_file());
    assert!(source.join("runtime.zip").is_file());
    assert!(!source.join("runtime").exists());
    let plugins = PluginService::new(
        PluginRoots {
            session: vec![],
            builtin: root.join("builtin"),
            workspace: None,
            personal: root.join("app data/plugins/personal"),
        },
        root.join("app data"),
    );
    let scan = plugins.scan_plugin(&source).unwrap();
    assert!(scan.errors.is_empty(), "scan: {:?}", scan.errors);
    println!("scan risks: {:?}", scan.risks);
    let installed = plugins.install_from_dir(&source).unwrap();
    assert!(installed.enabled && installed.available && installed.has_runtime_payload);
    assert_eq!(installed.skill_count, 1);
    assert_eq!(installed.mcp_server_count, 1);
    let projection = plugins.runtime_projection().unwrap();
    assert!(projection.errors.is_empty(), "{:?}", projection.errors);
    assert_eq!(projection.skill_roots.len(), 1);
    assert!(projection.skill_roots[0]
        .join("rea-analysis/SKILL.md")
        .is_file());
    let skills = deepagent_skills::loader::discover(
        &projection.skill_roots[0],
        deepagent_skills::SkillOrigin::Plugin,
    )
    .unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].meta.id, "rea-analysis");
    let db = Arc::new(Database::open_in_memory().unwrap());
    let projects = Arc::new(ProjectService::new(db.clone()));
    projects.add_project(root.to_str().unwrap()).unwrap();
    let runtime = Arc::new(RuntimeBroker::new(Arc::new(RuntimeService::new(
        &[required_path("REA_RUNTIME_ROOT")],
        Arc::new(UnavailableDownloader),
    ))));
    let service = McpService::new(db).with_runtime(runtime.clone(), projects);
    let (registry, failures) = service
        .connect_enabled_with_plugin_overlay(projection.mcp_config, &projection.mcp_server_sources)
        .await
        .unwrap();
    assert!(failures.is_empty(), "MCP startup: {failures:?}");
    let registry = Arc::new(registry);
    let tools = registry.all_tools();
    println!("discovered {} REA tools", tools.len());
    assert_eq!(tools.len(), 170);
    let mut router = ToolRegistry::new();
    for adapter in adapters_for(registry.clone()) {
        router.register(adapter).unwrap();
    }
    let discovered = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let search = ToolSearchTool::new(
        tools
            .iter()
            .map(|tool| DeferredToolSnapshot {
                name: tool.namespaced_name.clone(),
                description: tool.description.clone(),
            })
            .collect(),
        discovered.clone(),
    );
    let matches = search
        .invoke(json!({"query":"+rea analyze javascript application"}))
        .await
        .unwrap();
    assert!(matches.ok);
    let granted = PermissionSet::from_iter_perms([Permission::Network]);
    let js_tool = tools
        .iter()
        .find(|t| {
            t.namespaced_name
                .ends_with("__analyze_javascript_application")
        })
        .unwrap();
    assert!(discovered
        .lock()
        .unwrap()
        .contains(&js_tool.namespaced_name));
    assert!(router
        .get(&js_tool.namespaced_name)
        .unwrap()
        .tool
        .should_defer());
    assert!(router
        .check(&js_tool.namespaced_name, &PermissionSet::default(), false)
        .is_err());
    let app = root.join("示例 application");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"acceptance-app","version":"1.0.0","main":"main.js"}"#,
    )
    .unwrap();
    std::fs::write(app.join("main.js"), r#"const {ipcMain}=require('electron'); const fs=require('node:fs'); fs.writeFileSync('TARGET_MUST_NOT_EXECUTE','ran'); ipcMain.handle('sample-channel',()=>42);"#).unwrap();
    let node = runtime
        .resolve(
            &deepagent_app_core::RuntimeRequirement {
                kind: deepagent_app_core::RuntimeKind::Node,
                version: Some("=24.21.0".into()),
                preference: deepagent_app_core::RuntimePreference::PreferLocal,
            },
            None,
            None,
        )
        .unwrap();
    let asar = root.join("示例 app.asar");
    let asar_module =
        PathBuf::from(&installed.data_dir).join("runtime/node_modules/@electron/asar/lib/asar.js");
    let pack = std::process::Command::new(&node.executable).args([
        "--input-type=module", "-e",
        "import {pathToFileURL} from 'node:url'; const {createPackage}=await import(pathToFileURL(process.argv[1])); await createPackage(process.argv[2],process.argv[3]);",
    ]).arg(&asar_module).arg(&app).arg(&asar).output().unwrap();
    assert!(
        pack.status.success(),
        "ASAR fixture: {}",
        String::from_utf8_lossy(&pack.stderr)
    );
    for path in [&app, &asar] {
        let out = router
            .invoke(
                &js_tool.namespaced_name,
                json!({"input_path":path}),
                &granted,
                false,
            )
            .await
            .unwrap();
        assert!(out.ok, "JavaScript analysis: {}", out.value);
        assert!(
            out.value["structuredContent"].is_object(),
            "missing structured evidence"
        );
        let text_evidence: serde_json::Value =
            serde_json::from_str(out.value["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(out.value["structuredContent"], text_evidence);
        assert!(out.value["structuredContent"]["evidence_id"].is_string());
        let summary = out.value.to_string();
        assert!(summary.contains("sample-channel"), "IPC evidence missing");
        println!(
            "{} analysis passed; complete structured evidence retained",
            path.file_name().unwrap().to_string_lossy()
        );
    }
    let managed_tool = tools
        .iter()
        .find(|t| t.namespaced_name.ends_with("__inspect_managed_artifact"))
        .unwrap();
    let managed = router
        .invoke(
            &managed_tool.namespaced_name,
            json!({"path":required_path("REA_MANAGED_FIXTURE")}),
            &granted,
            false,
        )
        .await
        .unwrap();
    assert!(managed.ok, "managed inspection: {}", managed.value);
    assert!(managed.value["structuredContent"].is_object());
    assert!(managed.value.to_string().contains("System.Console"));
    assert!(!app.join("TARGET_MUST_NOT_EXECUTE").exists());
    assert!(!PathBuf::from(&installed.data_dir)
        .join("runtime/TARGET_MUST_NOT_EXECUTE")
        .exists());
    registry.close_all().await.unwrap();
    plugins.set_enabled(&installed.id, false).unwrap();
    let disabled = plugins.runtime_projection().unwrap();
    assert!(disabled.skill_roots.is_empty() && disabled.mcp_config.servers.is_empty());
    println!("managed inspection, target non-execution and plugin disable passed");
}
