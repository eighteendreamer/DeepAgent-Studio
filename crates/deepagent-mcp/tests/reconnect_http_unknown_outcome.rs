//! H-06: exercise reqwest's real POST path, including a lost response.
#![cfg(feature = "http")]

use std::sync::Arc;
use std::time::Duration;

use deepagent_core::error::CoreError;
use deepagent_mcp::{
    ConfigReconnectFactory, HttpTransport, McpClient, McpServerConfig, ReconnectingTransport,
    TransportType,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn http_lost_reply_is_unknown_and_only_a_new_call_is_dispatched_after_recovery() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://localhost:{}/mcp",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        let mut methods = Vec::new();
        let mut effects = Vec::new();
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request: Value = {
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                let mut length = None;
                loop {
                    line.clear();
                    assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = Some(value.trim().parse::<usize>().unwrap());
                        }
                    }
                }
                let mut body = vec![0; length.expect("reqwest content length")];
                reader.read_exact(&mut body).await.unwrap();
                serde_json::from_slice(&body).unwrap()
            };
            let method = request["method"].as_str().unwrap().to_string();
            methods.push(method.clone());
            let result = match method.as_str() {
                "initialize" => json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},
                    "serverInfo":{"name":"h06-http","version":"1"}}),
                "notifications/initialized" => {
                    stream
                        .write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    continue;
                }
                "tools/call" => {
                    effects.push(
                        request["params"]["arguments"]["label"]
                            .as_str()
                            .unwrap()
                            .to_string(),
                    );
                    if effects.len() == 1 {
                        // The effect is observable; close before HTTP headers.
                        drop(stream);
                        continue;
                    }
                    json!({"content":[],"isError":false})
                }
                _ => panic!("unexpected request: {method}"),
            };
            let body = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        (methods, effects)
    });
    let config = McpServerConfig {
        response_limit_bytes: None,
        transport: Some(TransportType::Http),
        url: Some(url.clone()),
        command: None,
        args: Vec::new(),
        env: Default::default(),
        cwd: None,
        headers: Default::default(),
    };
    let transport = ReconnectingTransport::new(
        Arc::new(HttpTransport::with_url(url, Vec::new()).unwrap()),
        Arc::new(ConfigReconnectFactory::new(config, "h06-http")),
    )
    .with_backoff(vec![Duration::ZERO]);
    let client = McpClient::new(Arc::new(transport));
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        client.initialize("h06-http").await.unwrap();
        let error = client
            .call_tool("append_record", json!({"label":"primary"}))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::RemoteOutcomeUnknown { .. }));
        assert!(
            !client
                .call_tool("append_record", json!({"label":"independent"}))
                .await
                .unwrap()
                .is_error
        );
        server.await.unwrap()
    })
    .await;
    client.close().await.unwrap();
    let (methods, effects) = result.expect("local HTTP regression deadline");
    assert_eq!(
        methods,
        vec![
            "initialize",
            "notifications/initialized",
            "tools/call",
            "initialize",
            "notifications/initialized",
            "tools/call"
        ]
    );
    assert_eq!(effects, vec!["primary", "independent"]);
}
