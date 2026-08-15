//! Real-process end-to-end test for the §5.2 MCP resilience stack:
//! a genuine stdio MCP server (Node fixture) that crashes on its first `ping`,
//! recovered by [`ReconnectingTransport`] + [`ConfigReconnectFactory`] with the
//! liveness `ping` acting as the recovery trigger — the exact production
//! wiring (`connect_one_resilient` + `LivenessProbe`), not mocks.
//!
//! Closes the "liveness/reconnect never verified against a real crashing
//! server" gap logged in 执行报告/17. Skips (with a message) when `node` is
//! not on PATH so offline/CI runs stay green.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::{
    ConfigReconnectFactory, McpClient, McpServerConfig, ReconnectFactory, ReconnectingTransport,
    StdioTransport,
};

/// A minimal but spec-shaped MCP stdio server. Generation 1 (no marker file
/// yet) creates the marker and exits mid-`ping` — a real crash with a broken
/// pipe. Generation 2 (marker present) answers `ping` normally forever.
const FIXTURE_SERVER_JS: &str = r#"
const fs = require('fs');
const readline = require('readline');
const marker = process.env.CRASH_MARKER;
const firstGeneration = !fs.existsSync(marker);
if (firstGeneration) fs.writeFileSync(marker, 'gen1');
const rl = readline.createInterface({ input: process.stdin });
function reply(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}
rl.on('line', (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === 'initialize') {
    reply(msg.id, {
      protocolVersion: '2025-06-18',
      capabilities: { tools: {} },
      serverInfo: { name: 'crash-fixture', version: '0.0.1' },
    });
  } else if (msg.method === 'ping') {
    if (firstGeneration) process.exit(1); // crash instead of replying
    reply(msg.id, {});
  } else if (msg.method === 'tools/list') {
    reply(msg.id, { tools: [] });
  }
});
"#;

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn ping_probe_recovers_real_crashed_stdio_server() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.js");
    std::fs::write(&script, FIXTURE_SERVER_JS).expect("write fixture");
    let marker = dir.path().join("crashed.marker");

    let mut env = BTreeMap::new();
    env.insert(
        "CRASH_MARKER".to_string(),
        marker.to_string_lossy().into_owned(),
    );
    let config = McpServerConfig {
        transport: None,
        command: Some("node".to_string()),
        args: vec![script.to_string_lossy().into_owned()],
        env,
        cwd: None,
        url: None,
        headers: Default::default(),
    };

    // Production wiring (mcp_service::connect_one_resilient): real spawn +
    // self-healing wrapper + initialize handshake.
    let inner = Arc::new(StdioTransport::spawn(&config).expect("spawn fixture"));
    let factory = Arc::new(ConfigReconnectFactory::new(config.clone(), "liveness-e2e"));
    let resilient = Arc::new(
        ReconnectingTransport::new(inner, factory).with_backoff(vec![Duration::from_millis(50); 3]),
    );
    let client = Arc::new(McpClient::new(resilient));
    client.initialize("liveness-e2e").await.expect("initialize");

    // The probe's ping hits generation 1, which really dies mid-request
    // (broken pipe / EOF). The reconnect wrapper must respawn the server
    // (generation 2, marker present) and the retried ping must succeed.
    tokio::time::timeout(Duration::from_secs(30), client.ping())
        .await
        .expect("ping must not hang")
        .expect("ping must recover through a real process restart");

    // Generation 1 really ran and crashed (marker written by gen 1 only).
    assert!(marker.exists(), "fixture generation 1 never ran");

    // The recovered connection is fully usable, not just ping-alive.
    let tools = client
        .list_tools()
        .await
        .expect("tools/list after recovery");
    assert!(tools.is_empty());

    client.close().await.expect("close");
}

/// The reconnect factory itself must produce a handshake-completed transport
/// from a cold config (what every recovery attempt relies on).
#[tokio::test]
async fn reconnect_factory_spawns_real_ready_transport() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.js");
    std::fs::write(&script, FIXTURE_SERVER_JS).expect("write fixture");
    let marker = dir.path().join("crashed.marker");
    // Pre-create the marker so the fixture starts healthy (generation 2).
    std::fs::write(&marker, "gen1").expect("marker");

    let mut env = BTreeMap::new();
    env.insert(
        "CRASH_MARKER".to_string(),
        marker.to_string_lossy().into_owned(),
    );
    let config = McpServerConfig {
        transport: None,
        command: Some("node".to_string()),
        args: vec![script.to_string_lossy().into_owned()],
        env,
        cwd: None,
        url: None,
        headers: Default::default(),
    };

    let factory = ConfigReconnectFactory::new(config, "liveness-e2e");
    let transport = tokio::time::timeout(Duration::from_secs(30), factory.reconnect())
        .await
        .expect("reconnect must not hang")
        .expect("factory must return a ready transport");
    // Ready means initialize already ran; ping must work immediately.
    let client = McpClient::new(transport);
    client.ping().await.expect("ping on factory transport");
    client.close().await.expect("close");
}
