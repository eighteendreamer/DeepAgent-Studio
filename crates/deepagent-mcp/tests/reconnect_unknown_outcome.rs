//! Real-process reproducer for the MCP reconnect unknown-outcome boundary.
//!
//! The first generation applies a `tools/call` mutation and exits before the
//! JSON-RPC response reaches the client.  The production reconnect wrapper
//! respawns the server and retries the same request, so the fixture's durable
//! effect counter records two applications for one logical call.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::{
    ConfigReconnectFactory, McpClient, McpServerConfig, ReconnectingTransport, StdioTransport,
};

const FIXTURE_SERVER_JS: &str = r#"
const fs = require('fs');
const readline = require('readline');
const generation = process.env.GENERATION_FILE;
const effects = process.env.EFFECT_FILE;
const firstGeneration = !fs.existsSync(generation);
if (firstGeneration) fs.writeFileSync(generation, 'first');
const rl = readline.createInterface({ input: process.stdin });
function reply(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}
rl.on('line', (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === 'initialize') {
    reply(msg.id, {
      protocolVersion: '2024-11-05',
      capabilities: { tools: {} },
      serverInfo: { name: 'unknown-outcome-fixture', version: '0.0.1' },
    });
  } else if (msg.method === 'tools/call') {
    fs.appendFileSync(effects, 'effect\n');
    if (firstGeneration) process.exit(1); // effect committed, reply lost
    reply(msg.id, { content: [{ type: 'text', text: 'applied' }], isError: false });
  }
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

#[tokio::test]
#[ignore = "audit reproducer: reconnect must not replay an unknown-outcome mutation"]
async fn audit_reconnect_does_not_replay_unknown_outcome_tool_call() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.js");
    std::fs::write(&script, FIXTURE_SERVER_JS).expect("write fixture");
    let generation = dir.path().join("generation.marker");
    let effects = dir.path().join("effects.log");

    let mut env = BTreeMap::new();
    env.insert(
        "GENERATION_FILE".to_string(),
        generation.to_string_lossy().into_owned(),
    );
    env.insert(
        "EFFECT_FILE".to_string(),
        effects.to_string_lossy().into_owned(),
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

    // This is the same production wiring used by McpService::connect_one_resilient:
    // stdio transport + ConfigReconnectFactory + ReconnectingTransport + McpClient.
    let inner = Arc::new(StdioTransport::spawn(&config).expect("spawn fixture"));
    let factory = Arc::new(ConfigReconnectFactory::new(config, "unknown-outcome-audit"));
    let resilient =
        Arc::new(ReconnectingTransport::new(inner, factory).with_backoff(vec![Duration::ZERO]));
    let client = McpClient::new(resilient);
    client
        .initialize("unknown-outcome-audit")
        .await
        .expect("initialize");

    let result = client
        .call_tool("append_record", serde_json::json!({"value":"audit"}))
        .await;
    client.close().await.expect("close");

    let effect_count = std::fs::read_to_string(&effects)
        .expect("effect log")
        .lines()
        .count();
    eprintln!(
        "audit MCP unknown outcome: result_ok={}, effect_count={effect_count}",
        result.is_ok()
    );
    assert_eq!(
        effect_count, 1,
        "one logical tools/call must not be applied twice after its reply is lost"
    );
}
