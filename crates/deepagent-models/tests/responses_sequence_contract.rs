//! H-210: sequence admission precedes semantic effects; gaps remain observable.
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use deepagent_core::{
    error::{CoreError, Result},
    message::Message,
};
use deepagent_models::{
    transport::{EventSink, HttpTransport, TransportRequest},
    DeltaObserver, ModelClient, ModelConfig, ModelStreamEvent, ResponseAccumulator,
    ResponseRequest,
};
use serde_json::{json, Value};

#[path = "support/sequence.rs"]
pub mod sequence_fixture;

#[test]
fn h210_fixture_numbering_never_repairs_explicit_invalid_or_duplicate_values() {
    let events = sequence_fixture::numbered(vec![
        json!({"type":"response.created"}).to_string(),
        json!({"type":"response.output_text.delta","sequence_number":null}).to_string(),
        json!({"type":"response.output_text.delta","sequence_number":0}).to_string(),
        json!({"type":"response.completed","sequence_number":0}).to_string(),
    ]);
    let values: Vec<Value> = events
        .iter()
        .map(|event| serde_json::from_str(event).unwrap())
        .collect();
    assert_eq!(values[0]["sequence_number"], 0);
    assert!(values[1]["sequence_number"].is_null());
    assert_eq!(values[2]["sequence_number"], 0);
    assert_eq!(values[3]["sequence_number"], 0);
    let mut accumulator = ResponseAccumulator::new();
    let mut seen = Seen::default();
    accumulator
        .push_sse_data_observed(&events[0], &mut seen)
        .unwrap();
    reject(
        accumulator
            .push_sse_data_observed(&events[1], &mut seen)
            .unwrap_err(),
    );
}

#[derive(Default)]
struct Seen(Vec<ModelStreamEvent>);
impl DeltaObserver for Seen {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

fn terminal(sequence: Value) -> Value {
    json!({"type":"response.completed","sequence_number":sequence,"response":{
        "id":"synthetic","object":"response","created_at":1,"status":"completed",
        "model":"deepseek-flash","output":[],"usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}}})
}

fn delta(sequence: u64) -> Value {
    json!({"type":"response.output_text.delta","sequence_number":sequence,
        "delta":"synthetic text"})
}

fn reject(error: CoreError) {
    assert!(!error.to_string().contains("PRIVATE"));
    let failure = deepagent_models::classify_model_error(&error);
    assert!(!failure.retryable() && !failure.should_fallback());
    assert!(matches!(error, CoreError::Provider {code:Some(code),..}
        if code=="invalid_response_sequence"));
}

#[test]
fn h210_invalid_sequence_has_no_semantic_projection_and_poisons_the_stream() {
    for prior in [None, Some(7_u64), Some(u64::MAX)] {
        let mut cases = vec![
            Value::Null,
            json!(-1),
            json!(1.5),
            json!("PRIVATE_SEQUENCE"),
            json!(true),
            json!({}),
            json!([]),
            json!(18446744073709551616_f64),
        ];
        if let Some(prior) = prior {
            cases.extend([json!(prior), json!(prior - 1)]);
        }
        for sequence in cases.into_iter().map(Some).chain([None]) {
            let mut accumulator = ResponseAccumulator::new();
            let mut seen = Seen::default();
            if let Some(prior) = prior {
                accumulator
                    .push_sse_data_observed(&delta(prior).to_string(), &mut seen)
                    .unwrap();
            }
            let count = seen.0.len();
            let mut event = terminal(sequence.clone().unwrap_or(Value::Null));
            if sequence.is_none() {
                event.as_object_mut().unwrap().remove("sequence_number");
            }
            reject(
                accumulator
                    .push_sse_data_observed(&event.to_string(), &mut seen)
                    .unwrap_err(),
            );
            assert_eq!(
                seen.0.len(),
                count,
                "rejected event cannot emit usage, text, or Finished"
            );
            // An ignored error must not allow a later terminal to accept a
            // prefix whose integrity was already disproven.
            reject(
                accumulator
                    .push_sse_data_observed(&terminal(json!(u64::MAX)).to_string(), &mut seen)
                    .unwrap_err(),
            );
            assert!(accumulator.finish().is_err());
        }
    }
}

#[test]
fn h210_monotonic_gaps_and_unknown_events_keep_sequence_diagnostics() {
    let mut accumulator = ResponseAccumulator::new();
    let mut seen = Seen::default();
    for event in [
        delta(40),
        json!({"type":"response.future_event","sequence_number":43}),
        terminal(json!(44)),
    ] {
        accumulator
            .push_sse_data_observed(&event.to_string(), &mut seen)
            .unwrap();
    }
    let metadata: Vec<_> = seen
        .0
        .iter()
        .filter_map(|event| {
            let value = serde_json::to_value(event).unwrap();
            (value["type"] == "response_stream_event").then_some(value)
        })
        .collect();
    assert_eq!(
        metadata
            .iter()
            .map(|v| v["sequence_number"].clone())
            .collect::<Vec<_>>(),
        vec![json!(40), json!(43), json!(44)]
    );
    assert_eq!(metadata[1]["sequence_gap"], 2);
    assert!(metadata[0]["sequence_gap"].is_null());
    assert!(metadata[2]["sequence_gap"].is_null());
    assert_eq!(
        accumulator.finish().unwrap().output_text_projection(),
        "synthetic text"
    );
}

struct Replay {
    calls: AtomicUsize,
    invalid: bool,
}
#[async_trait]
impl HttpTransport for Replay {
    async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(call < 2);
        for event in [delta(0), terminal(json!(if self.invalid { 0 } else { 1 }))] {
            if sink.on_event(&event.to_string())? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn h210_each_physical_request_resets_sequence_and_both_entries_reject_duplicates() {
    for invalid in [false, true] {
        for cancelled in [false, true] {
            let transport = Arc::new(Replay {
                calls: AtomicUsize::new(0),
                invalid,
            });
            let client = ModelClient::new(transport.clone(), ModelConfig::deepseek("synthetic"));
            for _ in 0..2 {
                let request =
                    ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]);
                let result = if cancelled {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                if invalid {
                    reject(result.unwrap_err());
                } else {
                    assert_eq!(result.unwrap().usage.unwrap().total_tokens, 11);
                }
            }
            assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn h210_chat_completions_does_not_require_a_responses_sequence() {
    let transport = Arc::new(deepagent_models::MockTransport::new([
        json!({"choices":[{"delta":{"content":"chat control"},"finish_reason":"stop"}]})
            .to_string(),
        "[DONE]".into(),
    ]));
    let client = ModelClient::new(transport, ModelConfig::deepseek_chat("synthetic"));
    let response = client
        .stream_response(ResponseRequest::new(
            "deepseek-flash",
            vec![Message::user("synthetic")],
        ))
        .await
        .unwrap();
    assert_eq!(response.output_text_projection(), "chat control");
}

#[cfg(feature = "http")]
#[tokio::test]
async fn h210_real_http_sequence_check_preserves_gap_and_rejects_replay() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{timeout, Duration},
    };
    for duplicate in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let (offset, length) = loop {
                let mut bytes = [0; 2048];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&bytes[..count]);
                assert!(received.len() < 16384);
                if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&received[..end]).unwrap();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while received.len() < offset + length {
                let mut bytes = [0; 2048];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&bytes[..count]);
            }
            let events = if duplicate {
                vec![delta(0), terminal(json!(0))]
            } else {
                vec![
                    delta(0),
                    json!({"type":"response.future_event","sequence_number":3}),
                    terminal(json!(4)),
                ]
            };
            let body = events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let mut config = ModelConfig::deepseek("synthetic-placeholder");
        config.base_url = format!("http://{address}");
        let transport = deepagent_models::ReqwestTransport::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
        );
        let client = ModelClient::new(Arc::new(transport), config);
        let request = ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]);
        let mut seen = Seen::default();
        let response = timeout(
            Duration::from_secs(5),
            client.stream_response_observed_cancelled(
                request,
                &mut seen,
                Arc::new(AtomicBool::new(false)),
            ),
        )
        .await
        .unwrap();
        if duplicate {
            reject(response.unwrap_err());
            assert!(!seen
                .0
                .iter()
                .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
        } else {
            assert_eq!(response.unwrap().usage.unwrap().total_tokens, 11);
            assert!(seen
                .0
                .iter()
                .any(|event| serde_json::to_value(event).unwrap()["sequence_gap"] == 2));
            // H-200: real HTTP deframing must preserve terminal identity too.
            assert!(seen.0.iter().any(|event| {
                let event = serde_json::to_value(event).unwrap();
                event["event_type"] == "response.completed"
                    && event["response_id"] == "synthetic"
                    && event["provider_model"] == "deepseek-flash"
                    && event["sequence_number"] == 4
            }));
        }
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
