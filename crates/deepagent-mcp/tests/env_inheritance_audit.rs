//! Explicit subprocess-environment audit. Only synthetic credentials enter the
//! isolated test host; the fixture returns booleans, never environment values.

use std::collections::BTreeMap;
use std::time::Duration;

use deepagent_mcp::{connect_transport, McpClient, McpServerConfig};
use serde_json::{json, Value};

const SECRET: &str = "DEEPAGENT_MCP_AUDIT_SECRET";
const LOWERCASE_TOKEN: &str = "deepagent_mcp_audit_token";
const PLAIN: &str = "DEEPAGENT_MCP_AUDIT_PLAIN";
const CHILD: &str = "DEEPAGENT_MCP_ENV_AUDIT_CHILD";
const REPORT: &str = "DEEPAGENT_MCP_ENV_AUDIT_REPORT";

async fn observe_isolated_environment() -> Value {
    let dir = tempfile::tempdir().unwrap();
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
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let nonce = dir.path().file_name().unwrap().to_str().unwrap();
    command
        .env(CHILD, nonce)
        .env(REPORT, &report)
        .env(SECRET, format!("ambient-{nonce}"))
        .env(LOWERCASE_TOKEN, format!("lowercase-{nonce}"))
        .env(PLAIN, "ambient-plain");
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
#[ignore = "audit reproducer: local MCP must not implicitly receive host credentials"]
async fn audit_stdio_does_not_inherit_ambient_credentials() {
    let observed = observe_isolated_environment().await;
    eprintln!("audit MCP environment: {observed}");
    assert_eq!(observed["ambient"]["plain_matches"], true);
    assert_eq!(observed["explicit"]["explicit_secret_matches"], true);
    assert_eq!(observed["explicit"]["plain_matches"], true);
    assert_eq!(
        observed["ambient"]["ambient_secret_present"], false,
        "an MCP with empty config.env received an undeclared host secret"
    );
    assert_eq!(observed["ambient"]["lowercase_token_present"], false);
}

#[tokio::test]
#[ignore = "audit control: explicitly configured MCP environment overrides must survive"]
async fn audit_stdio_preserves_explicit_environment_overrides() {
    let observed = observe_isolated_environment().await;
    assert_eq!(observed["explicit"]["explicit_secret_matches"], true);
    assert_eq!(observed["explicit"]["ambient_secret_matches"], false);
    assert_eq!(observed["explicit"]["plain_matches"], true);
}

#[tokio::test]
#[ignore = "helper: launched only by the isolated environment audit host"]
async fn audit_stdio_environment_fixture_child() {
    let nonce = std::env::var(CHILD).expect("run the outer environment audit, not this helper");
    let report = std::env::var_os(REPORT).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("environment-server.cjs");
    std::fs::write(
        &script,
        r#"
const [nonce, mode] = process.argv.slice(2);
const has = name => Object.prototype.hasOwnProperty.call(process.env, name);
require('node:readline').createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (!Object.prototype.hasOwnProperty.call(request, 'id')) return;
  const result = request.method === 'initialize' ? {
    protocolVersion: request.params.protocolVersion,
    capabilities: { tools: {} },
    serverInfo: { name: 'environment-audit', version: '1' },
    audit: {
      ambient_secret_present: has('DEEPAGENT_MCP_AUDIT_SECRET'),
      ambient_secret_matches: process.env.DEEPAGENT_MCP_AUDIT_SECRET === `ambient-${nonce}`,
      explicit_secret_matches: process.env.DEEPAGENT_MCP_AUDIT_SECRET === `explicit-${nonce}`,
      lowercase_token_present: has('deepagent_mcp_audit_token'),
      lowercase_token_matches: process.env.deepagent_mcp_audit_token === `lowercase-${nonce}`,
      plain_matches: process.env.DEEPAGENT_MCP_AUDIT_PLAIN === `${mode}-plain`,
    },
  } : { tools: [] };
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
}).on('close', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
"#,
    )
    .unwrap();
    let mut observations = serde_json::Map::new();
    for mode in ["ambient", "explicit"] {
        let env = if mode == "explicit" {
            BTreeMap::from([
                (SECRET.to_owned(), format!("explicit-{nonce}")),
                (PLAIN.to_owned(), "explicit-plain".to_owned()),
            ])
        } else {
            BTreeMap::new()
        };
        let config = McpServerConfig {
            transport: None,
            command: Some("node".to_owned()),
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
        let client = McpClient::new(connect_transport(&config).unwrap());
        let initialized = tokio::time::timeout(
            Duration::from_secs(5),
            client.initialize("environment-audit"),
        )
        .await
        .expect("local MCP initialize must finish")
        .unwrap();
        let tools = tokio::time::timeout(Duration::from_secs(5), client.list_tools())
            .await
            .expect("local MCP tools/list must finish")
            .unwrap();
        client.close().await.unwrap();
        assert!(tools.is_empty());
        observations.insert(mode.to_owned(), initialized["audit"].clone());
    }
    std::fs::write(report, serde_json::to_vec(&json!(observations)).unwrap()).unwrap();
}
