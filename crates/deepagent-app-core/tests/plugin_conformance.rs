//! Agent Plugins v1 规范一致性回归。
//!
//! 这里覆盖跨模块/主 loader 接线；细粒度错误变体在各模块单测中固定。

use std::path::Path;

use deepagent_app_core::plugin::component::{discover_skills, parse_mcp, PluginMcpTransport};
use deepagent_app_core::plugin::spec::schema::{
    AGENT_PLUGIN_MCP_SCHEMA_URI, AGENT_PLUGIN_SCHEMA_URI,
};
use deepagent_app_core::plugin::spec::{
    expand_v1, parse_portable, resolve_plugin_relative, PluginName,
};
use deepagent_app_core::plugin_loader::{load_plugins, PluginRoots};

#[test]
fn manifest_examples_parse_with_closed_schema_semantics() {
    let minimal = format!(r#"{{"$schema":"{AGENT_PLUGIN_SCHEMA_URI}","name":"my-plugin"}}"#);
    let (manifest, diagnostics) = parse_portable(&minimal).expect("minimal manifest");
    assert_eq!(manifest.name.as_ref(), "my-plugin");
    assert!(diagnostics.is_empty());

    let full = format!(
        r#"{{
          "$schema":"{AGENT_PLUGIN_SCHEMA_URI}",
          "name":"acme.tools",
          "version":"not-semver",
          "description":"portable plugin",
          "author":{{"name":"Acme","email":"ops@example.com","url":"https://example.com"}},
          "homepage":"not a url",
          "repository":"also not a url",
          "license":"custom-license",
          "keywords":["agent","tools"],
          "extensions":{{"com.example":{{"x":1}}}},
          "unknown":true
        }}"#
    );
    let (manifest, diagnostics) = parse_portable(&full).expect("full manifest");
    assert_eq!(manifest.version.as_deref(), Some("not-semver"));
    assert_eq!(manifest.keywords, vec!["agent", "tools"]);
    assert_eq!(diagnostics.len(), 1, "unknown top-level field is reported");
}

#[test]
fn name_and_path_examples_match_the_spec() {
    for name in ["my-plugin", "acme.tools", "lint3r", "a", "a.b-c"] {
        assert!(PluginName::parse(name).is_ok(), "{name}");
    }
    for name in ["My-Plugin", "-start", "has--double", "too.many..dots", ""] {
        assert!(PluginName::parse(name).is_err(), "{name}");
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    assert_eq!(
        resolve_plugin_relative(root, "./bin/server").expect("relative"),
        root.join("bin").join("server")
    );
    for raw in ["../bin/server", "data", "/abs", "C:\\x", "./"] {
        assert!(
            resolve_plugin_relative(root, raw).is_err(),
            "{raw} should be rejected"
        );
    }
}

#[test]
fn fixed_skills_location_is_non_recursive() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(&root.join("skills").join("a").join("SKILL.md"), "skill");
    write(
        &root
            .join("skills")
            .join("a")
            .join("nested")
            .join("SKILL.md"),
        "nested",
    );

    let (skills, diagnostics) = discover_skills(root);

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "a");
    assert!(diagnostics.is_empty());
}

#[test]
fn mcp_examples_cover_all_three_transports_and_v1_expansion() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("plugin");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(root.join("bin")).expect("dirs");
    let contents = format!(
        r#"{{
          "$schema":"{AGENT_PLUGIN_MCP_SCHEMA_URI}",
          "mcpServers":{{
            "local":{{"type":"stdio","command":"./bin/server","args":["${{PLUGIN_DATA}}/cache"],"env":{{"ROOT":"${{PLUGIN_ROOT}}"}},"cwd":"${{PLUGIN_ROOT}}"}},
            "http":{{"type":"streamable-http","url":"https://example.com/mcp","headers":{{"X-Literal":"${{PLUGIN_ROOT}}"}}}},
            "events":{{"type":"sse","url":"http://localhost:7777/sse"}}
          }}
        }}"#
    );

    let (servers, diagnostics) =
        parse_mcp(&contents, &root, &data, AGENT_PLUGIN_SCHEMA_URI).expect("mcp");

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 3);
    let local = servers
        .iter()
        .find(|server| server.name == "local")
        .expect("local server");
    match &local.transport {
        PluginMcpTransport::Stdio { args, env, cwd, .. } => {
            assert!(args[0].starts_with(&data.display().to_string()));
            assert_eq!(env["ROOT"], root.display().to_string());
            assert_eq!(cwd, &root);
        }
        other => panic!("expected stdio, got {other:?}"),
    }
    let http = servers
        .iter()
        .find(|server| server.name == "http")
        .expect("http server");
    match &http.transport {
        PluginMcpTransport::StreamableHttp { headers, .. } => {
            assert_eq!(headers["X-Literal"], "${PLUGIN_ROOT}");
        }
        other => panic!("expected streamable-http, got {other:?}"),
    }
}

#[test]
fn placeholder_expansion_is_single_pass_and_literal_for_unknowns() {
    let expanded = expand_v1(
        "${PLUGIN_ROOT}/${UNKNOWN}/${PLUGIN_DATA}",
        "/plugin",
        "data",
    );
    assert_eq!(expanded, "/plugin/${UNKNOWN}/data");
}

#[test]
fn main_loader_uses_portable_root_manifest_before_dialects() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let builtin = tmp.path().join("builtin");
    let plugin = builtin.join("demo");
    write(
        &plugin.join("plugin.json"),
        &format!(r#"{{"$schema":"{AGENT_PLUGIN_SCHEMA_URI}","name":"portable-demo"}}"#),
    );
    write(
        &plugin.join(".codex-plugin").join("plugin.json"),
        r#"{"name":"codex-demo"}"#,
    );

    let loaded = load_plugins(&PluginRoots {
        session: Vec::new(),
        builtin,
        workspace: None,
        personal: tmp.path().join("personal"),
        marketplace_cache: tmp.path().join("cache"),
        marketplaces: tmp.path().join("marketplaces"),
    });

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "portable-demo");
    assert_eq!(
        loaded[0]
            .resolved()
            .as_ref()
            .expect("manifest")
            .dialect
            .as_str(),
        "agent-plugin-v1"
    );
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
    std::fs::write(path, contents).expect("write");
}
