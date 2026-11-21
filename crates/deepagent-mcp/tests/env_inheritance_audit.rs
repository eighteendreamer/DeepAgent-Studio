//! Explicit subprocess-environment audit. Only synthetic credentials enter the
//! isolated test host; the fixture returns booleans, never environment values.

use std::collections::BTreeMap;
use std::time::Duration;

use deepagent_mcp::{
    connect_transport, ConfigReconnectFactory, McpClient, McpServerConfig, ReconnectFactory,
};
use serde_json::{json, Value};

const SECRET: &str = "DEEPAGENT_MCP_AUDIT_SECRET";
const LOWERCASE_TOKEN: &str = "deepagent_mcp_audit_token";
const PLAIN: &str = "DEEPAGENT_MCP_AUDIT_PLAIN";
const CHILD: &str = "DEEPAGENT_MCP_ENV_AUDIT_CHILD";
const REPORT: &str = "DEEPAGENT_MCP_ENV_AUDIT_REPORT";
const SERVER: &str = include_str!("fixtures/environment-server.cjs");

#[path = "support/node_runtime.rs"]
mod node_runtime;

async fn observe_isolated_environment() -> Value {
    let node_executable = node_runtime::resolve_node_runtime().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    let report = dir.path().join("observations.json");
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "audit_stdio_environment_fixture_child",
            "--ignored",
            "--test-threads=1",
        ])
        .env_clear()
        .kill_on_drop(true);
    // Retain runtime essentials, not the calling shell's real credentials.
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "PATHEXT",
        "COMSPEC",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let nonce = dir.path().file_name().unwrap().to_str().unwrap();
    command
        .env(CHILD, nonce)
        .env(REPORT, &report)
        .env(node_runtime::NODE_RUNTIME_ENV, node_executable)
        .env(SECRET, format!("ambient-{nonce}"))
        .env(LOWERCASE_TOKEN, format!("lowercase-{nonce}"))
        .env(PLAIN, "ambient-plain")
        .env("MCP_CONTEXT_BLOB", format!("opaque-{nonce}"))
        .env(
            "HTTP_PROXY",
            format!("http://user:synthetic-{nonce}@127.0.0.1:9"),
        )
        .env("SSL_CERT_FILE", format!("{nonce}-ca.pem"))
        .env("NODE_OPTIONS", "--no-warnings")
        .env("LANG", "C.UTF-8")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("TEMP", &scratch)
        .env("TMP", &scratch)
        .env("TMPDIR", &scratch);
    #[cfg(windows)]
    command.creation_flags(0x08000000);

    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("isolated audit host must finish within its deadline")
        .expect("isolated audit host must start");
    // Do not print child stdout/stderr even on failure: a future regression in
    // the probe must not turn environment contents into test diagnostics.
    assert!(output.status.success(), "isolated audit host failed");
    serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap()
}

#[tokio::test]
async fn audit_stdio_does_not_inherit_ambient_credentials() {
    let observed = observe_isolated_environment().await;
    eprintln!("audit MCP environment: {observed}");
    // The new authorization boundary excludes ALL non-base ambient entries,
    // not just names recognized by the diagnostics redactor.
    assert_eq!(observed["ambient"]["plain_present"], false);
    assert_eq!(observed["explicit"]["explicit_secret_matches"], true);
    assert_eq!(observed["explicit"]["plain_matches"], true);
    assert_eq!(
        observed["ambient"]["ambient_secret_present"], false,
        "an MCP with empty config.env received an undeclared host secret"
    );
    assert_eq!(observed["ambient"]["lowercase_token_present"], false);
    for field in [
        "opaque_present",
        "proxy_present",
        "ca_present",
        "bootstrap_present",
    ] {
        assert_eq!(observed["ambient"][field], false, "{field}");
    }
    assert_eq!(observed["ambient"]["runtime_base_present"], true);
}

#[tokio::test]
async fn audit_stdio_preserves_explicit_environment_overrides() {
    let observed = observe_isolated_environment().await;
    assert_eq!(observed["explicit"]["explicit_secret_matches"], true);
    assert_eq!(observed["explicit"]["ambient_secret_matches"], false);
    assert_eq!(observed["explicit"]["plain_matches"], true);
}

#[tokio::test]
async fn declared_forwarding_keeps_runtime_proxy_ca_empty_and_unicode_values() {
    let observed = observe_isolated_environment().await;
    for field in [
        "ambient_secret_matches",
        "lowercase_token_matches",
        "plain_matches",
        "proxy_matches",
        "ca_matches",
        "runtime_base_present",
        "locale_matches",
        "empty_matches",
        "unicode_matches",
    ] {
        assert_eq!(observed["forwarded"][field], true, "{field}");
    }
    assert_eq!(observed["forwarded"]["opaque_present"], false);
    assert_eq!(observed["forwarded"]["bootstrap_present"], false);
}

#[tokio::test]
async fn explicit_case_override_uses_platform_environment_name_semantics() {
    let observed = observe_isolated_environment().await;
    assert_eq!(observed["case_override"]["mixed_path_matches"], true);
    assert_eq!(observed["case_override"]["runtime_base_present"], true);
    assert_eq!(observed["case_override"]["lowercase_secret_matches"], true);
    assert_eq!(
        observed["case_override"]["explicit_secret_matches"],
        cfg!(windows)
    );
    assert_eq!(observed["case_override"]["ambient_secret_matches"], false);
}

#[tokio::test]
async fn real_config_reconnect_applies_the_same_environment_boundary() {
    let observed = observe_isolated_environment().await;
    for mode in ["ambient", "explicit"] {
        assert_eq!(observed[format!("reconnected_{mode}")], observed[mode]);
    }
}

#[tokio::test]
#[ignore = "helper: launched only by the isolated environment audit host"]
async fn audit_stdio_environment_fixture_child() {
    let nonce = std::env::var(CHILD).expect("run the outer environment audit, not this helper");
    let node_executable = node_runtime::isolated_node_runtime();
    let report = std::env::var_os(REPORT).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("environment-server.cjs");
    std::fs::write(&script, SERVER).unwrap();
    let mut observations = serde_json::Map::new();
    for mode in ["ambient", "explicit", "forwarded", "case_override"] {
        let mut env = if mode == "explicit" {
            BTreeMap::from([
                (SECRET.to_owned(), format!("explicit-{nonce}")),
                (PLAIN.to_owned(), "explicit-plain".to_owned()),
            ])
        } else {
            BTreeMap::new()
        };
        if mode == "forwarded" {
            for name in [
                SECRET,
                LOWERCASE_TOKEN,
                PLAIN,
                "HTTP_PROXY",
                "SSL_CERT_FILE",
            ] {
                env.insert(name.into(), format!("${{{name}}}"));
            }
            env.insert("MCP_EMPTY".into(), "".into());
            env.insert("MCP_LABEL".into(), "本地 环境 🚀".into());
        }
        if mode == "case_override" {
            env.insert(SECRET.to_ascii_lowercase(), format!("explicit-{nonce}"));
            let separator = if cfg!(windows) { ';' } else { ':' };
            env.insert(
                "pAtH".into(),
                format!(
                    "{}{separator}{nonce}-explicit-path",
                    std::env::var("PATH").unwrap()
                ),
            );
        }
        let mut config = McpServerConfig {
            response_limit_bytes: None,
            transport: None,
            command: Some(node_executable.to_string_lossy().into_owned()),
            args: vec![
                script.to_string_lossy().into_owned(),
                nonce.clone(),
                mode.to_owned(),
            ],
            env,
            cwd: Some(dir.path().to_path_buf()),
            url: None,
            headers: BTreeMap::new(),
        };
        config.expand_with(&|name| std::env::var(name).ok());
        let client = McpClient::new(connect_transport(&config).unwrap());
        let initialized = tokio::time::timeout(
            Duration::from_secs(5),
            client.initialize("environment-audit"),
        )
        .await
        .expect("local MCP initialize must finish")
        .unwrap();
        assert_eq!(initialized["audit"]["fixture_runtime_present"], false);
        let tools = tokio::time::timeout(Duration::from_secs(5), client.list_tools())
            .await
            .expect("local MCP tools/list must finish")
            .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.call_tool("env_probe", json!({})),
        )
        .await
        .unwrap()
        .unwrap();
        client.close().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "env_probe");
        assert!(!result.is_error);
        assert_eq!(result.extra["structuredContent"], initialized["audit"]);
        observations.insert(mode.to_owned(), initialized["audit"].clone());
        if matches!(mode, "ambient" | "explicit") {
            let factory = ConfigReconnectFactory::new(config, "environment-audit");
            let transport = tokio::time::timeout(Duration::from_secs(5), factory.reconnect())
                .await
                .unwrap()
                .unwrap();
            let recovered = McpClient::new(transport);
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                recovered.call_tool("env_probe", json!({})),
            )
            .await
            .unwrap()
            .unwrap();
            recovered.close().await.unwrap();
            assert!(!result.is_error);
            observations.insert(
                format!("reconnected_{mode}"),
                result.extra["structuredContent"].clone(),
            );
        }
    }
    std::fs::write(report, serde_json::to_vec(&json!(observations)).unwrap()).unwrap();
}
