//! H-69: budget the actual final wire, not raw images or token estimates.
use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::json;
use std::sync::Mutex;

use crate::test_wire as wire_fixture;

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
        let mode = wire_fixture::MODES
            .into_iter()
            .find(|mode| request.url.ends_with(wire_fixture::endpoint(*mode)))
            .expect("known model endpoint");
        for event in wire_fixture::start(mode)
            .into_iter()
            .chain([wire_fixture::delta(mode, "ok", 0)])
            .chain(wire_fixture::finish(mode, "ok", 1))
        {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

// route 0/1 exercise native Responses / the Responses-to-Chat bridge;
// route 2 exercises direct Chat, independent of the config's default wire.
// route 3 exercises Messages through the same public Responses entry points.
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
    let mut config = ModelConfig::deepseek("PRIVATE_SYNTHETIC_KEY").with_wire_mode(match route {
        1 => WireMode::ChatCompletions,
        3 => WireMode::Messages,
        _ => WireMode::Responses,
    });
    if route == 3 {
        config.defaults.max_output_tokens = Some(1024);
    }
    config
}

fn request(text: &str) -> ResponseRequest {
    ResponseRequest::new("deepseek-flash", vec![Message::user(text)])
}

fn invalid(error: CoreError) {
    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    let kind = crate::classify_model_error(&error);
    assert_eq!(kind, crate::ModelFailureKind::InvalidRequest);
    assert!(!kind.retryable());
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
    for route in 0..4 {
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
            assert_eq!(
                send(&client, request("正常对照 🦀"), route, entry, false)
                    .await
                    .unwrap()
                    .output_text_projection(),
                "ok"
            );
        }
        assert_eq!(transport.0.lock().unwrap().len(), 4);
    }
}

#[tokio::test]
async fn h69_exact_production_boundary_is_inclusive_on_every_wire() {
    for route in 0..4 {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(transport.clone(), config(route));
        send(&client, request("x"), route, 3, false).await.unwrap();
        let overhead = transport.0.lock().unwrap()[0] - 1;
        transport.0.lock().unwrap().clear();
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
    for route in 0..4 {
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

        // Measure this route's real overhead, then prove a valid default is
        // included in the final budget. A multi-MiB function name is not a
        // valid Responses choice and would now fail before body encoding.
        send(&client, request("x"), route, 2, false).await.unwrap();
        let overhead = transport.0.lock().unwrap()[0] - 1;
        transport.0.lock().unwrap().clear();
        let at_limit = "x".repeat(LIMIT - overhead);
        send(&client, request(&at_limit), route, 2, false)
            .await
            .unwrap();
        assert_eq!(*transport.0.lock().unwrap(), vec![LIMIT]);
        transport.0.lock().unwrap().clear();

        let mut with_defaults = config(route);
        with_defaults.defaults.tool_choice = Some(json!("auto"));
        let client = ModelClient::new(transport.clone(), with_defaults);
        invalid(
            send(&client, request(&at_limit), route, 2, false)
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

    for route in 0..4 {
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
            let content = if route == 3 {
                &body[key][0]["content"][0]["text"]
            } else {
                &body[key][0]["content"]
            };
            assert_eq!(content, "HTTP normal control");
            let mode = match route {
                0 => WireMode::Responses,
                3 => WireMode::Messages,
                _ => WireMode::ChatCompletions,
            };
            let data = String::from_utf8(wire_fixture::encode(
                wire_fixture::start(mode)
                    .into_iter()
                    .chain([wire_fixture::delta(mode, "ok", 0)])
                    .chain(wire_fixture::finish(mode, "ok", 1)),
            ))
            .unwrap();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let response = timeout(
            Duration::from_secs(5),
            send(&client, request("HTTP normal control"), route, 3, false),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.output_text_projection(), "ok");
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
