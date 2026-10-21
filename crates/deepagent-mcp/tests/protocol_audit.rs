//! Explicit audit probes. These assert required behavior and intentionally fail
//! on the audited production implementation; ordinary test runs ignore them.

use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::{McpClient, McpServerConfig, StdioTransport};

#[tokio::test]
#[ignore = "audit reproducer: initialize must be followed by notifications/initialized"]
async fn audit_strict_stdio_server_completes_initialization() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("strict-server.cjs");
    std::fs::write(
        &script,
        r#"
const readline = require('node:readline');
let ready = false;
const seen = [];
function send(message) { process.stdout.write(JSON.stringify(message) + '\n'); }
readline.createInterface({ input: process.stdin }).on('line', line => {
  const msg = JSON.parse(line);
  seen.push(msg.method);
  if (msg.method === 'initialize') {
    send({ jsonrpc: '2.0', id: msg.id, result: {
      protocolVersion: msg.params.protocolVersion,
      capabilities: { tools: {} }, serverInfo: { name: 'strict-audit', version: '1' }
    }});
  } else if (msg.method === 'notifications/initialized') {
    ready = true;
  } else if (msg.method === 'tools/list') {
    send(ready
      ? { jsonrpc: '2.0', id: msg.id, result: { tools: [] } }
      : { jsonrpc: '2.0', id: msg.id, error: {
          code: -32002, message: 'not initialized; received=' + seen.join(',')
        }});
  }
});
"#,
    )
    .unwrap();
    let config = McpServerConfig {
        transport: None,
        command: Some("node".into()),
        args: vec![script.to_string_lossy().into_owned()],
        env: Default::default(),
        cwd: None,
        url: None,
        headers: Default::default(),
    };
    let client = McpClient::new(Arc::new(StdioTransport::spawn(&config).unwrap()));
    let discovered = tokio::time::timeout(Duration::from_secs(5), async {
        client.initialize("protocol-audit").await?;
        client.list_tools().await
    })
    .await
    .expect("strict fixture must respond within the deadline");
    client.close().await.unwrap();
    eprintln!("audit MCP initialization: discovery={discovered:?}");
    assert!(
        discovered.is_ok(),
        "a conforming server must see initialized before operational requests"
    );
}
