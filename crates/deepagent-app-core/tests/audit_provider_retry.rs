//! Opt-in audit probes against the real HTTP transport and model retry loop.
#![cfg(feature = "web")]

use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
use deepagent_runtime::{Agent, ModelAgent};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        assert!(count > 0, "fixture request ended before its body");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(
            bytes.len() < 1024 * 1024,
            "fixture request is unexpectedly large"
        );
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
            let length: usize = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .expect("model request must specify its length")
                .1
                .trim()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return;
            }
        }
    }
}

#[tokio::test]
async fn audit_rate_limit_retry_waits_for_provider_recovery() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider_wait = Duration::from_secs(3);
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let mut received = Vec::new();
            let mut responded_at = None;
            let mut recovered = false;
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_request(&mut stream).await;
                received.push(Instant::now());
                recovered = attempt > 0
                    && responded_at.is_some_and(|time: Instant| time.elapsed() >= provider_wait);
                let (status, content_type, extra, body) = if recovered {
                    (
                        "200 OK",
                        "text/event-stream",
                        "",
                        concat!(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"recovered\"},",
                            "\"finish_reason\":\"stop\"}]}\n\n",
                            "data: [DONE]\n\n"
                        ),
                    )
                } else {
                    (
                        "429 Too Many Requests",
                        "application/json",
                        "Retry-After: 3\r\nx-request-id: audit-provider-request\r\n",
                        r#"{"error":{"code":"rate_limit_exceeded","message":"rate limited"}}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                if attempt == 0 {
                    responded_at = Some(Instant::now());
                }
                stream.shutdown().await.unwrap();
            }
            (received[1].duration_since(received[0]), recovered)
        })
        .await
        .expect("fixture did not receive two complete requests within 10 seconds")
    });

    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let transport = Arc::new(ReqwestTransport::with_client(http));
    let mut config = ModelConfig::deepseek_chat("audit-placeholder");
    config.base_url = format!("http://{address}/v1");
    let client = Arc::new(ModelClient::new(transport, config));
    let mut agent = ModelAgent::new(client, "deepseek-flash", "audit", "hello", vec![])
        .with_max_model_attempts(2);
    let result = agent.think(0, &[]).await;
    let (retry_gap, recovered) = server.await.unwrap();
    eprintln!(
        "provider_wait_ms={}, actual_retry_gap_ms={}, provider_recovered={}, agent_ok={}, error={:?}",
        provider_wait.as_millis(),
        retry_gap.as_millis(),
        recovered,
        result.is_ok(),
        result.as_ref().err()
    );
    assert!(
        recovered && result.is_ok(),
        "retry should respect the bounded provider delay instead of exhausting attempts before recovery"
    );
}
