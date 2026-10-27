//! A cancelled partial stdio write must never be replayed during recovery.

use std::sync::Arc;
use std::time::Duration;

use deepagent_core::error::CoreError;
use deepagent_mcp::{
    protocol::JsonRpcRequest, ConfigReconnectFactory, McpServerConfig, McpTransport,
    ReconnectingTransport, StdioTransport,
};
use deepagent_tools::ToolExecutionContext;

const DEADLINE: Duration = Duration::from_secs(5);
const SERVER: &str = r#"
const fs = require('node:fs');
const path = require('node:path');
const generations = path.join(__dirname, 'generations');
const first = !fs.existsSync(generations);
fs.appendFileSync(generations, 'start\n');
const reply = (id, result) => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result})+'\n');
let buffer = '';
process.stdin.on('data', chunk => {
  buffer += chunk.toString('utf8');
  if (first && buffer.length > 1024) {
    process.stdin.pause();
    fs.writeFileSync(path.join(__dirname, 'blocked'), String(buffer.length));
    return;
  }
  let end;
  while ((end = buffer.indexOf('\n')) >= 0) {
    const req = JSON.parse(buffer.slice(0, end));
    buffer = buffer.slice(end + 1);
    if (req.method === 'initialize') reply(req.id, {
      protocolVersion:'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'h136',version:'1'}
    });
    else if (req.method === 'tools/call') {
      fs.appendFileSync(path.join(__dirname, 'effects'), req.params.arguments.label+'\n');
      reply(req.id, {content:[{type:'text',text:'applied'}],isError:false});
    }
    else if (req.id !== undefined) reply(req.id, {});
  }
});
// Keep the paused first generation alive until the transport owner closes it.
setTimeout(() => process.exit(2), 15000);
"#;

#[tokio::test]
async fn cancelled_partial_write_recovers_for_new_calls_without_replaying_the_cancelled_one() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.cjs");
    std::fs::write(&script, SERVER).unwrap();
    let config = McpServerConfig {
        transport: None,
        command: Some("node".into()),
        args: vec![script.to_string_lossy().into_owned()],
        env: Default::default(),
        cwd: None,
        url: None,
        headers: Default::default(),
    };
    let original = Arc::new(StdioTransport::spawn(&config).unwrap());
    deepagent_mcp::McpClient::new(original.clone())
        .initialize("h136")
        .await
        .unwrap();
    let factory = Arc::new(ConfigReconnectFactory::new(config, "h136"));
    let transport = Arc::new(
        ReconnectingTransport::new(original.clone(), factory).with_backoff(vec![Duration::ZERO]),
    );
    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let call = tokio::spawn({
        let transport = transport.clone();
        async move {
            transport
                .send_with_context(
                    &JsonRpcRequest::new(1, "tools/call", Some(serde_json::json!({
                        "name":"append", "arguments":{"label":"cancelled", "payload":"x".repeat(8 * 1024 * 1024)}
                    }))),
                    context,
                )
                .await
        }
    });
    let marker = dir.path().join("blocked");
    tokio::time::timeout(DEADLINE, async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("peer must really pause during an incomplete request");
    let bytes: usize = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    assert!(bytes > 0 && bytes < 8 * 1024 * 1024);
    cancel.cancel();
    let error = tokio::time::timeout(DEADLINE, call)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, CoreError::RemoteOutcomeUnknown { .. }));
    assert!(!dir.path().join("effects").exists());

    // The existing control request detects the retired generation and recovers;
    // the old business call must not be carried by the reconnect factory.
    tokio::time::timeout(
        DEADLINE,
        transport.send(&JsonRpcRequest::new(2, "ping", None)),
    )
    .await
    .unwrap()
    .unwrap();
    let next = tokio::time::timeout(
        DEADLINE,
        transport.send(&JsonRpcRequest::new(
            3,
            "tools/call",
            Some(serde_json::json!({"name":"append", "arguments":{"label":"independent"}})),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    transport.close().await.unwrap();
    original.close().await.unwrap();
    assert!(next.error.is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effects")).unwrap(),
        "independent\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("generations"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}
