//! Explicit audit probes. These assert required behavior and intentionally fail
//! on the audited production implementation; ordinary test runs ignore them.

use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::{McpClient, McpServerConfig, StdioTransport};

#[cfg(feature = "http")]
#[tokio::test]
#[ignore = "audit reproducer: Streamable HTTP must retain the server-assigned session header"]
async fn audit_http_initialization_retains_session_header() {
    use deepagent_mcp::{protocol::JsonRpcRequest, McpTransport};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut received = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                assert!(socket.read_buf(&mut bytes).await.unwrap() > 0);
                assert!(bytes.len() < 65_536);
                if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() < header_end + length {
                assert!(socket.read_buf(&mut bytes).await.unwrap() > 0);
                assert!(bytes.len() < 65_536);
            }
            let request: serde_json::Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let method = request["method"].as_str().unwrap();
            let has_session = headers
                .lines()
                .any(|line| line == "mcp-session-id: audit-session");
            received.push((method.to_owned(), has_session));
            let (status, session_header, body) = if method == "initialize" {
                (
                    "200 OK",
                    "Mcp-Session-Id: audit-session\r\n",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": request["id"], "result": {
                            "protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "http-session-audit", "version": "1"}
                        }
                    })
                    .to_string(),
                )
            } else if has_session {
                (
                    "200 OK",
                    "",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": request["id"], "result": {"tools": []}
                    })
                    .to_string(),
                )
            } else {
                ("400 Bad Request", "", "missing Mcp-Session-Id".into())
            };
            socket.write_all(format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        received
    });
    let transport =
        deepagent_mcp::HttpTransport::with_url(format!("http://{address}/mcp"), vec![]).unwrap();
    let discovered = tokio::time::timeout(Duration::from_secs(5), async {
        // Drive the transport directly with a matching version to isolate
        // session headers from McpClient's separate version-negotiation policy.
        transport
            .send(&JsonRpcRequest::new(
                1,
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": "http-session-audit", "version": "1"}
                })),
            ))
            .await?;
        transport
            .send(&JsonRpcRequest::new(2, "tools/list", None))
            .await
    })
    .await
    .expect("local HTTP fixture must respond");
    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
    eprintln!("audit MCP HTTP session: received={received:?}, discovery={discovered:?}");
    assert!(
        discovered.is_ok(),
        "operational HTTP requests must carry the session assigned during initialize"
    );
}

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
