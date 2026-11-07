//! Original H126 probes, now using their explicit 1 MiB configured boundary.
//! The original pre-fix failures are retained in the audit/repair documents;
//! this audit threshold is not the default or an MCP protocol limit.

use std::time::Duration;

use deepagent_mcp::{protocol::JsonRpcRequest, McpServerConfig, McpTransport, StdioTransport};

const AUDIT_RESPONSE_LIMIT: usize = 1024 * 1024;
#[cfg(feature = "http")]
const OVERSIZED_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

fn payload_len(response: &deepagent_mcp::protocol::JsonRpcResponse) -> usize {
    response
        .result
        .as_ref()
        .and_then(|result| result.get("payload"))
        .and_then(serde_json::Value::as_str)
        .expect("fixture response must contain its payload")
        .len()
}

#[tokio::test]
async fn audit_stdio_transport_accepts_oversized_response_line() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("large-response-server.cjs");
    std::fs::write(
        &script,
        r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });
rl.on('line', line => {
  const request = JSON.parse(line);
  const size = request.method === 'large' ? 2 * 1024 * 1024 : 16;
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id,
    result: { payload: 'x'.repeat(size) } }) + '\n');
});
"#,
    )
    .unwrap();
    let config = McpServerConfig {
        response_limit_bytes: Some(AUDIT_RESPONSE_LIMIT),
        transport: None,
        command: Some("node".into()),
        args: vec![script.to_string_lossy().into_owned()],
        env: Default::default(),
        cwd: None,
        url: None,
        headers: Default::default(),
    };
    let transport = StdioTransport::spawn(&config).unwrap();
    let small = tokio::time::timeout(
        Duration::from_secs(5),
        transport.send(&JsonRpcRequest::new(1, "small", None)),
    )
    .await
    .expect("small stdio response should complete")
    .unwrap();
    let large = tokio::time::timeout(
        Duration::from_secs(10),
        transport.send(&JsonRpcRequest::new(2, "large", None)),
    )
    .await
    .expect("oversized stdio response must terminate without draining the body");
    transport.close().await.unwrap();

    let small_bytes = payload_len(&small);
    assert_eq!(small_bytes, 16);
    let error = large.unwrap_err();
    eprintln!(
        "audit MCP stdio response: small_payload_bytes={small_bytes}, refusal={error}, configured_limit_bytes={AUDIT_RESPONSE_LIMIT}"
    );
    assert!(
        matches!(
            error,
            deepagent_core::CoreError::McpResponseTooLarge {
                limit_bytes: AUDIT_RESPONSE_LIMIT,
                ..
            }
        ),
        "stdio transport must reject before parsing the oversized payload"
    );
}

#[cfg(feature = "http")]
#[tokio::test]
async fn audit_http_transport_accepts_oversized_response_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_request(socket: &mut tokio::net::TcpStream) -> (usize, serde_json::Value) {
        let mut bytes = Vec::new();
        let header_end = loop {
            assert!(socket.read_buf(&mut bytes).await.unwrap() > 0);
            assert!(
                bytes.len() < 64 * 1024,
                "request headers must remain bounded"
            );
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            assert!(socket.read_buf(&mut bytes).await.unwrap() > 0);
            assert!(bytes.len() < 64 * 1024, "request body must remain bounded");
        }
        let request: serde_json::Value =
            serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
        (content_length, request)
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut observations = Vec::new();
        for response_size in [16, OVERSIZED_RESPONSE_BYTES] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (request_bytes, request) = read_request(&mut socket).await;
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "payload": "x".repeat(response_size) }
            })
            .to_string();
            let sent = socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .is_ok();
            let _ = socket.shutdown().await;
            observations.push((request_bytes, body.len(), sent));
        }
        observations
    });

    let config = serde_json::from_value(serde_json::json!({
        "type":"http", "url":format!("http://localhost:{}/mcp",address.port()),
        "response_limit_bytes":AUDIT_RESPONSE_LIMIT
    }))
    .unwrap();
    let transport = deepagent_mcp::HttpTransport::connect(&config).unwrap();
    let small = tokio::time::timeout(
        Duration::from_secs(5),
        transport.send(&JsonRpcRequest::new(1, "small", None)),
    )
    .await
    .expect("small HTTP response should complete")
    .unwrap();
    let large = tokio::time::timeout(
        Duration::from_secs(10),
        transport.send(&JsonRpcRequest::new(2, "large", None)),
    )
    .await
    .expect("oversized HTTP response must terminate without draining the body");
    let observations = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();

    let small_bytes = payload_len(&small);
    assert_eq!(small_bytes, 16);
    let error = large.unwrap_err();
    eprintln!(
        "audit MCP HTTP response: fixture_planned_bodies={observations:?}, small_payload_bytes={small_bytes}, refusal={error}, configured_limit_bytes={AUDIT_RESPONSE_LIMIT}"
    );
    assert!(
        matches!(
            error,
            deepagent_core::CoreError::McpResponseTooLarge {
                limit_bytes: AUDIT_RESPONSE_LIMIT,
                ..
            }
        ),
        "HTTP transport must reject before parsing the oversized payload"
    );
}
