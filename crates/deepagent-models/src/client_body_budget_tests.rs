//! H-69: budget the actual final wire, not raw images or token estimates.
use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::json;
use std::sync::Mutex;

// Deliberately use the documented production limit, not a lowered test limit.
const LIMIT: usize = 48 * 1024 * 1024;

#[derive(Default)]
struct Recorder(Mutex<Vec<usize>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        request: TransportRequest,
        sink: &mut dyn crate::transport::EventSink,
    ) -> Result<()> {
        self.0.lock().unwrap().push(request.body.len());
        let events = if request.url.ends_with("/responses") {
            vec![
                r#"{"type":"response.completed","response":{"id":"h69","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
            ]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        let mut sequence = crate::test_sequence::FixtureSequence::default();
        for event in events {
            if sink.on_event(&sequence.event(event))? {
                break;
            }
        }
        Ok(())
    }
}

// route 0/1 exercise native Responses / the Responses-to-Chat bridge;
// route 2 exercises direct Chat, independent of the config's default wire.
async fn send(
    client: &ModelClient,
    request: ResponseRequest,
    route: usize,
    entry: usize,
    cancelled: bool,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(cancelled));
    let mut observer = crate::stream::NoopObserver;
    if route == 2 {
        let chat = response_to_chat_completion(request)?;
        match entry {
            0 => client.stream_chat_completion(chat).await,
            1 => {
                client
                    .stream_chat_completion_observed(chat, &mut observer)
                    .await
            }
            2 => client.stream_chat_completion_cancelled(chat, cancel).await,
            3 => {
                client
                    .stream_chat_completion_observed_cancelled(chat, &mut observer, cancel)
                    .await
            }
            _ => unreachable!(),
        }
    } else {
        match entry {
            0 => client.stream_response(request).await,
            1 => {
                client
                    .stream_response_observed(request, &mut observer)
                    .await
            }
            2 => client.stream_response_cancelled(request, cancel).await,
            3 => {
                client
                    .stream_response_observed_cancelled(request, &mut observer, cancel)
                    .await
            }
            _ => unreachable!(),
        }
    }
}

fn config(route: usize) -> ModelConfig {
    ModelConfig::deepseek("PRIVATE_SYNTHETIC_KEY").with_wire_mode(if route == 1 {
        WireMode::ChatCompletions
    } else {
        WireMode::Responses
    })
}

fn request(text: &str) -> ResponseRequest {
    ResponseRequest::new("deepseek-flash", vec![Message::user(text)])
}

fn invalid(error: CoreError) {
    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    let kind = crate::classify_model_error(&error);
    assert_eq!(kind, crate::ModelFailureKind::InvalidRequest);
    assert!(!kind.retryable());
    assert!(!kind.should_fallback());
    let text = error.to_string();
    assert!(text.contains("request_body_too_large"), "{text}");
    assert!(text.contains("48 MiB"));
    assert!(!text.contains("PRIVATE"));
    assert!(text.len() < 250, "diagnostics must not contain the payload");
}

#[tokio::test]
async fn h69_all_dispatch_entries_reject_final_oversize_and_keep_small_controls() {
    // Escaping doubles this text: raw character count alone fits the limit.
    let oversized = format!("PRIVATE_PAYLOAD{}", "\\".repeat(LIMIT / 2));
    assert!(oversized.len() < LIMIT);
    for route in 0..3 {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(transport.clone(), config(route));
        for entry in 0..4 {
            invalid(
                send(&client, request(&oversized), route, entry, false)
                    .await
                    .unwrap_err(),
            );
            assert!(transport.0.lock().unwrap().is_empty());
        }
        for entry in [2, 3] {
            invalid(
                send(&client, request(&oversized), route, entry, true)
                    .await
                    .unwrap_err(),
            );
        }
        assert!(transport.0.lock().unwrap().is_empty());
        for entry in 0..4 {
            send(&client, request("正常对照 🦀"), route, entry, false)
                .await
                .unwrap();
        }
        assert_eq!(transport.0.lock().unwrap().len(), 4);
    }
}

#[tokio::test]
async fn h69_exact_production_boundary_is_inclusive_on_every_wire() {
    for route in 0..3 {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(transport.clone(), config(route));
        let mut base = request("");
        base.stream = true;
        // Nonempty presence validation is only relevant when actually sent.
        let overhead = if route == 0 {
            serde_json::to_vec(&base).unwrap().len()
        } else {
            let mut chat = ChatCompletionRequest::new("deepseek-flash", vec![Message::user("")]);
            chat.stream = true;
            serde_json::to_vec(&chat).unwrap().len()
        };
        for size in [LIMIT - 1, LIMIT, LIMIT + 1] {
            let padded = request(&"x".repeat(size - overhead));
            if size > LIMIT {
                invalid(send(&client, padded, route, 3, false).await.unwrap_err());
            } else {
                send(&client, padded, route, 3, false).await.unwrap();
            }
        }
        assert_eq!(*transport.0.lock().unwrap(), vec![LIMIT - 1, LIMIT]);
    }
}

#[tokio::test]
async fn h69_tools_defaults_and_history_refresh_share_the_full_body_budget() {
    let half = "x".repeat(LIMIT / 2);
    for route in 0..3 {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(transport.clone(), config(route));
        let mut tools = request(&half);
        tools.tools.push(crate::ToolSchema::function(
            "synthetic_tool",
            &half,
            json!({"type":"object","properties":{}}),
        ));
        invalid(send(&client, tools, route, 3, false).await.unwrap_err());

        let mut refreshed = request("small original history");
        refreshed.replace_history_from(&request(&"PRIVATE".repeat(LIMIT / 7 + 1)));
        invalid(send(&client, refreshed, route, 0, false).await.unwrap_err());
        assert!(transport.0.lock().unwrap().is_empty());

        let defaults = ResponseDefaults {
            tool_choice: Some(json!({"type":"function","function":{"name":half}})),
            ..Default::default()
        };
        let client = ModelClient::new(transport.clone(), config(route).with_defaults(defaults));
        invalid(
            send(&client, request(&half), route, 2, false)
                .await
                .unwrap_err(),
        );
        assert!(transport.0.lock().unwrap().is_empty());
    }
}

#[cfg(feature = "http")]
#[tokio::test]
async fn h69_real_http_transport_sees_only_the_small_control_on_each_route() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    for route in 0..3 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = config(route);
        cfg.base_url = format!("http://{}", listener.local_addr().unwrap());
        let transport = crate::reqwest_transport::ReqwestTransport::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
        );
        let client = ModelClient::new(Arc::new(transport), cfg);
        invalid(
            send(&client, request(&"x".repeat(LIMIT)), route, 3, false)
                .await
                .unwrap_err(),
        );
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let header_end;
            let length;
            loop {
                let mut buffer = [0; 1024];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                received.extend_from_slice(&buffer[..n]);
                assert!(received.len() < 16 * 1024);
                if let Some(end) = received.windows(4).position(|s| s == b"\r\n\r\n") {
                    header_end = end + 4;
                    let headers = std::str::from_utf8(&received[..end]).unwrap();
                    length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break;
                }
            }
            while received.len() < header_end + length {
                let mut buffer = [0; 1024];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                received.extend_from_slice(&buffer[..n]);
                assert!(received.len() < 16 * 1024);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&received[header_end..header_end + length]).unwrap();
            let key = if route == 0 { "input" } else { "messages" };
            assert_eq!(body[key][0]["content"], "HTTP normal control");
            let data = if route == 0 {
                "data: {\"type\":\"response.completed\",\"sequence_number\":0,\"response\":{\"id\":\"h69\",\"object\":\"response\",\"created_at\":1,\"model\":\"deepseek-flash\",\"output\":[],\"status\":\"completed\"}}\n\n"
            } else {
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        timeout(
            Duration::from_secs(5),
            send(&client, request("HTTP normal control"), route, 3, false),
        )
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
