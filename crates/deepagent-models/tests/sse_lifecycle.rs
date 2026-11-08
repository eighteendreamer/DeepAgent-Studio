//! H-131: successful SSE uses progress/idle/cancel, not a lifetime stopwatch.
#![cfg(feature = "http")]

#[path = "support/sse_http_fixture.rs"]
mod fixture;

use deepagent_core::{error::Result, message::Message};
use deepagent_models::{
    DeltaObserver, FinishReason, HttpTransport, ModelClient, ModelStreamEvent, ReqwestTransport,
    Response, ResponseRequest, TransportRequest, TransportTimeouts, WireMode,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::sync::Notify;

const TOTAL: Duration = Duration::from_millis(250);

fn timeouts() -> TransportTimeouts {
    TransportTimeouts {
        connect: Duration::from_secs(2),
        first_byte: Duration::from_secs(2),
        idle: Duration::from_secs(1),
        total: TOTAL,
    }
}

fn delta(mode: WireMode, index: usize) -> Vec<u8> {
    let payload = match mode {
        WireMode::Responses => serde_json::json!({"type":"response.output_text.delta",
            "sequence_number":index,"item_id":"h30-output","output_index":0,"content_index":0,
            "delta":index.to_string()}),
        WireMode::ChatCompletions => serde_json::json!({"choices":[{
            "index":0,"delta":{"content":index.to_string()},"finish_reason":null}]}),
    };
    format!("data: {payload}\n\n").into_bytes()
}

fn active_chunks(mode: WireMode) -> Vec<(Duration, Vec<u8>)> {
    let mut chunks: Vec<_> = (0..6)
        .map(|index| (Duration::from_millis(60), delta(mode, index)))
        .collect();
    match mode {
        WireMode::Responses => {
            let mut terminal: serde_json::Value =
                serde_json::from_str(&fixture::response_terminal()).unwrap();
            terminal["sequence_number"] = 6.into();
            terminal["response"]["output"][0]["content"][0]["text"] = "012345".into();
            terminal["response"]["usage"] =
                serde_json::json!({"input_tokens":9,"output_tokens":2,"total_tokens":11});
            chunks.push((
                Duration::from_millis(60),
                format!("data: {terminal}\n\n").into_bytes(),
            ));
        }
        WireMode::ChatCompletions => {
            chunks.push((
                Duration::from_millis(60),
                b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
                    .to_vec(),
            ));
            chunks.push((Duration::from_millis(60), b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2,\"total_tokens\":11}}\n\n".to_vec()));
            chunks.push((Duration::from_millis(60), b"data: [DONE]\n\n".to_vec()));
        }
    }
    chunks
}

struct Observer {
    text: Arc<Mutex<String>>,
    first: Arc<Notify>,
}
impl DeltaObserver for Observer {
    fn on_event(&mut self, event: ModelStreamEvent) {
        if let ModelStreamEvent::ContentDelta { text } = event {
            self.text.lock().unwrap().push_str(&text);
            self.first.notify_one();
        }
    }
}

async fn response(
    client: &ModelClient,
    cancel_aware: bool,
    observer: &mut Observer,
) -> Result<Response> {
    let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("synthetic")]);
    let operation = async {
        if cancel_aware {
            client
                .stream_response_observed_cancelled(
                    request,
                    observer,
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        } else {
            client.stream_response_observed(request, observer).await
        }
    };
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .expect("bounded fixture response")
}

#[tokio::test]
async fn h131_active_stream_keeps_text_usage_and_terminal_in_both_protocols_and_entries() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            let server =
                fixture::Fixture::start_timed(active_chunks(mode), Duration::ZERO, 200).await;
            let transport = fixture::Fixture::transport().with_timeouts(timeouts());
            let client = ModelClient::new(Arc::new(transport), server.config(mode));
            let text = Arc::new(Mutex::new(String::new()));
            let mut observer = Observer {
                text: text.clone(),
                first: Arc::new(Notify::new()),
            };
            let started = tokio::time::Instant::now();
            let result = response(&client, cancel_aware, &mut observer).await;
            let elapsed = started.elapsed();
            assert_eq!(server.finish().await.len(), 1);
            let result = result.unwrap_or_else(|error| panic!("{mode:?}/{cancel_aware}: {error}"));
            assert!(elapsed > TOTAL);
            assert_eq!(result.output_text_projection(), "012345");
            assert_eq!(text.lock().unwrap().as_str(), "012345");
            assert_eq!(result.finish_reason, Some(FinishReason::Stop));
            let usage = result.usage.unwrap();
            assert_eq!((usage.prompt_tokens, usage.completion_tokens), (9, 2));
        }
    }
}

#[tokio::test]
async fn h131_idle_still_interrupts_after_progress_has_already_outlived_total() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            let mut chunks = active_chunks(mode);
            chunks[6].0 = Duration::from_millis(600);
            let server = fixture::Fixture::start_timed(chunks, Duration::ZERO, 200).await;
            let mut limits = timeouts();
            limits.idle = Duration::from_millis(200);
            let client = ModelClient::new(
                Arc::new(fixture::Fixture::transport().with_timeouts(limits)),
                server.config(mode),
            );
            let text = Arc::new(Mutex::new(String::new()));
            let mut observer = Observer {
                text: text.clone(),
                first: Arc::new(Notify::new()),
            };
            let result = response(&client, cancel_aware, &mut observer).await;
            assert_eq!(server.finish().await.len(), 1);
            assert!(result.unwrap_err().to_string().contains("SSE idle timeout"));
            assert_eq!(text.lock().unwrap().as_str(), "012345");
        }
    }
}

#[tokio::test]
async fn h131_true_cancellation_interrupts_an_active_stream_after_its_old_total_budget() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let chunks = (0..30)
            .map(|index| (Duration::from_millis(60), delta(mode, index)))
            .collect();
        let server = fixture::Fixture::start_timed(chunks, Duration::ZERO, 200).await;
        let client = ModelClient::new(
            Arc::new(fixture::Fixture::transport().with_timeouts(timeouts())),
            server.config(mode),
        );
        let first = Arc::new(Notify::new());
        let ready = first.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let mut worker = tokio::spawn(async move {
            let mut observer = Observer {
                text: Arc::new(Mutex::new(String::new())),
                first,
            };
            client
                .stream_response_observed_cancelled(
                    ResponseRequest::new("deepseek-v4-flash", vec![Message::user("synthetic")]),
                    &mut observer,
                    flag,
                )
                .await
        });
        let ready = tokio::time::timeout(Duration::from_secs(2), ready.notified()).await;
        if ready.is_err() {
            worker.abort();
            let _ = worker.await;
            server.finish().await;
            panic!("actual semantic delta did not arrive");
        }
        tokio::time::sleep(TOTAL + Duration::from_millis(100)).await;
        let active = !worker.is_finished();
        cancel.store(true, Ordering::Release);
        let result = match tokio::time::timeout(Duration::from_secs(1), &mut worker).await {
            Ok(joined) => joined.unwrap(),
            Err(_) => {
                worker.abort();
                let _ = worker.await;
                panic!("cancel-aware stream did not stop");
            }
        };
        assert_eq!(server.finish().await.len(), 1);
        assert!(
            active,
            "active SSE was cut off before actual cancellation: {result:?}"
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("request cancelled"));
    }
}

#[tokio::test]
async fn h131_headers_and_dripping_error_bodies_keep_their_bounded_failure_guards() {
    for cancel_aware in [false, true] {
        for (status, header_delay) in [
            (200, Duration::from_millis(500)),
            (429, Duration::ZERO),
            (503, Duration::ZERO),
        ] {
            let bytes = br#"{"error":{"message":"synthetic failure"}}"#;
            let chunks = bytes
                .iter()
                .map(|byte| (Duration::from_millis(50), vec![*byte]))
                .collect();
            let server = fixture::Fixture::start_timed(chunks, header_delay, status).await;
            let mut limits = timeouts();
            limits.first_byte = Duration::from_millis(100);
            let transport = fixture::Fixture::transport().with_timeouts(limits);
            let request = TransportRequest {
                url: format!("{}/responses", server.base_url),
                api_key: "synthetic".into(),
                body: "{}".into(),
            };
            let mut sink = |_: &str| Ok(false);
            let operation = async {
                if cancel_aware {
                    transport
                        .stream_cancelled(request, &mut sink, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    transport.stream(request, &mut sink).await
                }
            };
            let error = tokio::time::timeout(Duration::from_secs(3), operation)
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(server.finish().await.len(), 1);
            assert!(
                if status == 200 {
                    error.to_string().contains("first-byte timeout")
                } else {
                    error.to_string().contains("deadline exceeded")
                },
                "{error}"
            );
        }
    }
}

#[tokio::test]
async fn h131_get_and_explicit_caller_client_deadlines_are_not_disabled() {
    for cancel_aware in [false, true] {
        for status in [200, 503] {
            let chunks = br#"{"ok":true}"#.iter().map(|byte| (Duration::from_millis(60), vec![*byte])).collect();
            let server = fixture::Fixture::start_timed(chunks, Duration::ZERO, status).await;
            let transport = fixture::Fixture::transport().with_timeouts(timeouts());
            let url = format!("{}/models", server.base_url);
            let error = if cancel_aware {
                transport
                    .get_json_cancelled(&url, "synthetic", Arc::new(AtomicBool::new(false)))
                    .await
            } else {
                transport.get_json(&url, "synthetic").await
            }
            .unwrap_err();
            assert!(
                error.to_string().contains("GET total deadline exceeded"),
                "{error}"
            );
            assert_eq!(server.finish().await.len(), 1);
        }
        let server =
            fixture::Fixture::start_timed(active_chunks(WireMode::Responses), Duration::ZERO, 200)
                .await;
        let supplied_client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(150))
            .build()
            .unwrap();
        let client = ModelClient::new(
            Arc::new(ReqwestTransport::with_client(supplied_client).with_timeouts(timeouts())),
            server.config(WireMode::Responses),
        );
        let mut observer = Observer {
            text: Arc::new(Mutex::new(String::new())),
            first: Arc::new(Notify::new()),
        };
        assert!(response(&client, cancel_aware, &mut observer)
            .await
            .is_err());
        assert_eq!(server.finish().await.len(), 1);
    }
    let chunks = active_chunks(WireMode::Responses)
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();
    let server = fixture::Fixture::start(chunks).await;
    let client = ModelClient::new(
        Arc::new(fixture::Fixture::transport().with_timeouts(timeouts())),
        server.config(WireMode::Responses),
    );
    let mut observer = Observer {
        text: Arc::new(Mutex::new(String::new())),
        first: Arc::new(Notify::new()),
    };
    assert_eq!(
        response(&client, false, &mut observer)
            .await
            .unwrap()
            .output_text_projection(),
        "012345"
    );
    assert_eq!(server.finish().await.len(), 1);
}
