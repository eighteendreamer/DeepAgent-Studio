//! Real-process reproducer for the MCP reconnect unknown-outcome boundary.
//!
//! The first generation applies a `tools/call` mutation and exits before the
//! JSON-RPC response reaches the client.  The production reconnect wrapper
//! must report the unknown outcome without replaying that request. A later,
//! independent call recovers the connection before dispatch.

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
async fn audit_reconnect_does_not_replay_unknown_outcome_tool_call() {
    assert_unknown_outcome(false).await;
}

#[tokio::test]
async fn context_call_recovers_for_a_new_request_without_replaying_the_old_one() {
    assert_unknown_outcome(true).await;
}

async fn assert_unknown_outcome(with_context: bool) {
    assert!(
        node_available(),
        "Node is required for the real-process regression"
    );

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
        response_limit_bytes: None,
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

    let result = if with_context {
        client
            .call_tool_with_context(
                "append_record",
                serde_json::json!({"value":"audit"}),
                deepagent_tools::ToolExecutionContext::default(),
            )
            .await
    } else {
        client
            .call_tool("append_record", serde_json::json!({"value":"audit"}))
            .await
    };

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
    assert!(matches!(
        result,
        Err(deepagent_core::error::CoreError::RemoteOutcomeUnknown { .. })
    ));
    let next = client
        .call_tool("append_record", serde_json::json!({"value":"independent"}))
        .await;
    client.close().await.expect("close");
    assert!(!next.expect("new request on recovered generation").is_error);
    let after = std::fs::read_to_string(&effects).unwrap().lines().count();
    assert_eq!(
        after, 2,
        "one old effect and one independently requested effect"
    );
    eprintln!("MCP recovered independent request: context={with_context}, total_effects={after}");
}
