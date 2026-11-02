//! Reconnect must enforce the same MCP initialize contract as first connect.

use std::collections::BTreeMap;

use deepagent_mcp::{ConfigReconnectFactory, McpServerConfig, ReconnectFactory};

const SERVER: &str = r#"
const readline = require('node:readline');
const mode = process.env.AUDIT_MODE;
function reply(id, result) {
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id, result}) + '\n');
}
readline.createInterface({input: process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method !== 'initialize') return;
  let result = {protocolVersion:'2024-11-05', capabilities:{tools:{}},
    serverInfo:{name:'reconnect-contract-fixture',version:'1'}};
  if (mode === 'empty') result = {};
  if (mode === 'missing-version') delete result.protocolVersion;
  if (mode === 'missing-capabilities') delete result.capabilities;
  if (mode === 'missing-info') delete result.serverInfo;
  if (mode === 'wrong-version-type') result.protocolVersion = 42;
  if (mode === 'unsupported') result.protocolVersion = '2099-01-01';
  reply(req.id, result);
});
"#;

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn config(script: &std::path::Path, mode: &str) -> McpServerConfig {
    let mut env = BTreeMap::new();
    env.insert("AUDIT_MODE".to_string(), mode.to_string());
    McpServerConfig {
        transport: None,
        command: Some("node".to_string()),
        args: vec![script.to_string_lossy().into_owned()],
        env,
        cwd: None,
        url: None,
        headers: Default::default(),
    }
}

#[tokio::test]
async fn valid_reconnect_initialize_is_accepted() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.js");
    std::fs::write(&script, SERVER).unwrap();
    let factory = ConfigReconnectFactory::new(config(&script, "valid"), "reconnect-audit");
    let transport = factory.reconnect().await.unwrap();
    transport.close().await.unwrap();
}

#[tokio::test]
async fn audit_reconnect_rejects_invalid_initialize_results() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.js");
    std::fs::write(&script, SERVER).unwrap();
    let mut observations = Vec::new();
    for mode in [
        "empty",
        "missing-version",
        "missing-capabilities",
        "missing-info",
        "wrong-version-type",
        "unsupported",
    ] {
        let factory = ConfigReconnectFactory::new(config(&script, mode), "reconnect-audit");
        match factory.reconnect().await {
            Ok(transport) => {
                observations.push((mode, true));
                transport.close().await.unwrap();
            }
            Err(_) => observations.push((mode, false)),
        }
    }
    eprintln!("MCP reconnect initialize contract: {observations:?}");
    assert!(
        observations.iter().all(|(_, accepted)| !accepted),
        "invalid initialize results must not produce a ready reconnect transport"
    );
}
