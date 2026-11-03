//! H-208: provider terminal envelopes are validated before any success projection.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_core::{
    error::{CoreError, Result},
    message::Message,
};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    classify_model_error, DeltaObserver, FinishReason, ModelClient, ModelConfig, ModelStreamEvent,
    Response, ResponseAccumulator, ResponseRequest, Usage,
};
use serde_json::{json, Value};

const KINDS: [&str; 3] = [
    "response.completed",
    "response.incomplete",
    "response.failed",
];

fn terminal(kind: &str) -> Value {
    json!({"type":kind,"response":{
        "id":"h208-response","object":"response","created_at":1,
        "status":kind.strip_prefix("response.").unwrap(),"model":"deepseek-flash","output":[],
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11},
        "error":if kind=="response.failed" {json!({"code":"fixture_failed","message":"controlled failure"})}else{Value::Null},
        "incomplete_details":if kind=="response.incomplete" {json!({"reason":"max_output_tokens"})}else{Value::Null}
    }})
}

struct Replay {
    events: Vec<String>,
    calls: AtomicUsize,
    consumed: AtomicUsize,
}

#[async_trait]
impl HttpTransport for Replay {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert!(request.url.ends_with("/responses"));
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        for event in &self.events {
            self.consumed.fetch_add(1, Ordering::SeqCst);
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    events: Vec<ModelStreamEvent>,
    usage: Vec<(Usage, Value)>,
}

impl DeltaObserver for Recorder {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.events.push(event);
    }
    fn on_usage(&mut self, usage: Usage, raw: &Value) {
        self.usage.push((usage, raw.clone()));
        self.on_event(ModelStreamEvent::Usage { usage });
    }
}

async fn observe(event: Value, cancel_aware: bool) -> (Result<Response>, Recorder, Arc<Replay>) {
    let transport = Arc::new(Replay {
        events: vec![
            json!({"type":"response.output_text.delta","delta":"partial-display"}).to_string(),
            event.to_string(),
            terminal("response.completed").to_string(),
        ],
        calls: AtomicUsize::new(0),
        consumed: AtomicUsize::new(0),
    });
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-placeholder"),
    );
    let request = ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic terminal")]);
    let mut recorder = Recorder::default();
    let result = if cancel_aware {
        client
            .stream_response_observed_cancelled(
                request,
                &mut recorder,
                Arc::new(AtomicBool::new(false)),
            )
            .await
    } else {
        client
            .stream_response_observed(request, &mut recorder)
            .await
    };
    (result, recorder, transport)
}

fn invalid_cases(kind: &str) -> Vec<(Value, &'static str)> {
    let mut cases = vec![];
    for response in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!("PRIVATE_TERMINAL")),
        Some(json!(false)),
    ] {
        let mut event = terminal(kind);
        if let Some(response) = response {
            event["response"] = response;
        } else {
            event.as_object_mut().unwrap().remove("response");
        }
        cases.push((event, "response"));
    }
    for field in ["id", "object", "created_at", "status", "model", "output"] {
        let mut event = terminal(kind);
        event["response"].as_object_mut().unwrap().remove(field);
        cases.push((event, field));
        for value in [Value::Null, json!(true), json!({"PRIVATE_TERMINAL":1})] {
            let mut event = terminal(kind);
            event["response"][field] = value;
            cases.push((event, field));
        }
    }
    for (field, values) in [
        ("id", vec![json!(""), json!(" \t\n"), json!(1)]),
        (
            "object",
            vec![json!("PRIVATE_TERMINAL"), json!(""), json!(1)],
        ),
        ("created_at", vec![json!("1"), json!(1.5)]),
        (
            "status",
            vec![json!("in_progress"), json!(""), json!("PRIVATE_TERMINAL")],
        ),
        ("model", vec![json!(""), json!(" \n"), json!(1)]),
        (
            "output",
            vec![
                json!("PRIVATE_TERMINAL"),
                json!(1),
                json!([null]),
                json!([false]),
                json!(["PRIVATE_TERMINAL"]),
            ],
        ),
    ] {
        for value in values {
            let mut event = terminal(kind);
            event["response"][field] = value;
            cases.push((event, field));
        }
    }
    for status in ["completed", "incomplete", "failed"] {
        if kind.strip_prefix("response.") != Some(status) {
            let mut event = terminal(kind);
            event["response"]["status"] = json!(status);
            cases.push((event, "status"));
        }
    }
    for field in ["usage", "error", "incomplete_details"] {
        for value in [json!([]), json!(false), json!("PRIVATE_TERMINAL")] {
            let mut event = terminal(kind);
            event["response"][field] = value;
            cases.push((event, field));
        }
    }
    for reason in [json!(false), json!(1), json!("PRIVATE_TERMINAL")] {
        let mut event = terminal(kind);
        event["response"]["incomplete_details"] = json!({"reason":reason});
        cases.push((event, "incomplete_details.reason"));
    }
    cases
}

#[tokio::test]
async fn h208_all_terminal_envelopes_reject_missing_wrong_typed_and_mismatched_fields() {
    let mut violations = vec![];
    let mut checked = 0;
    for kind in KINDS {
        for (index, (event, field)) in invalid_cases(kind).into_iter().enumerate() {
            for cancel_aware in [false, true] {
                let (result, recorder, transport) = observe(event.clone(), cancel_aware).await;
                let valid_error = matches!(&result,Err(CoreError::Provider{code:Some(code),message,..})
                    if code=="malformed_response_terminal" && message.contains(kind) && message.contains(field)
                        && !message.contains("PRIVATE_TERMINAL"));
                let published = recorder
                    .events
                    .iter()
                    .any(|e| matches!(e, ModelStreamEvent::Finished { .. }));
                if !valid_error
                    || published
                    || !recorder.usage.is_empty()
                    || transport.consumed.load(Ordering::SeqCst) != 2
                {
                    violations.push(format!("{kind}/{index}/{field}/{cancel_aware}"));
                }
                if let Err(error) = result {
                    assert!(!classify_model_error(&error).retryable());
                }
                assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
                checked += 1;
            }
        }
    }
    eprintln!(
        "H-208 checked {checked} invalid terminal/client combinations; violations={}",
        violations.len()
    );
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn h208_rejected_terminal_cannot_make_the_public_accumulator_finish_successfully() {
    let mut checked = 0;
    for kind in KINDS {
        for (event, _) in invalid_cases(kind) {
            let mut acc = ResponseAccumulator::new();
            let mut recorder = Recorder::default();
            let result = acc.push_sse_data_observed(&event.to_string(), &mut recorder);
            assert!(result.is_err());
            assert!(
                acc.finish().is_err(),
                "ignoring the parser error must not expose an accepted terminal"
            );
            assert!(!recorder
                .events
                .iter()
                .any(|e| matches!(e, ModelStreamEvent::Finished { .. })));
            checked += 1;
        }
    }
    eprintln!("H-208 checked {checked} rejected accumulator terminals and finish results");
}

#[tokio::test]
async fn h208_optional_objects_signed_timestamps_and_provider_aliases_stay_compatible() {
    for kind in KINDS {
        for nulls in [false, true] {
            for created_at in [json!(-1), json!(u64::MAX)] {
                let mut event = terminal(kind);
                event["response"]["created_at"] = created_at;
                event["response"]["model"] = json!("provider-future-alias");
                for field in ["usage", "error", "incomplete_details"] {
                    if nulls {
                        event["response"][field] = Value::Null;
                    } else {
                        event["response"].as_object_mut().unwrap().remove(field);
                    }
                }
                let (result, recorder, transport) = observe(event, false).await;
                assert_eq!(transport.consumed.load(Ordering::SeqCst), 2);
                assert!(recorder.usage.is_empty());
                if kind == "response.failed" {
                    assert!(
                        matches!(result, Err(CoreError::Provider {code:Some(code),..}) if code=="response_failed")
                    );
                } else {
                    assert!(result.is_ok(), "{result:?}");
                }
            }
        }
    }
    for reason in [None, Some("max_output_tokens"), Some("content_filter")] {
        let mut event = terminal("response.incomplete");
        event["response"]["incomplete_details"] = json!({});
        event["response"]["error"] = json!({});
        if let Some(reason) = reason {
            event["response"]["incomplete_details"]["reason"] = json!(reason);
        }
        let (result, _, _) = observe(event, true).await;
        // H-82 owns reason-to-runtime recovery policy; H-208 only validates its shape.
        assert_eq!(result.unwrap().finish_reason, Some(FinishReason::Length));
    }
}

#[tokio::test]
async fn h208_valid_terminals_keep_optional_usage_provider_failure_and_unknown_extensions() {
    for kind in KINDS {
        for optional in ["missing", "null", "measured"] {
            for cancel_aware in [false, true] {
                let mut event = terminal(kind);
                if optional == "missing" {
                    event["response"].as_object_mut().unwrap().remove("usage");
                }
                if optional == "null" {
                    event["response"]["usage"] = Value::Null;
                }
                event["response"]["future_extension"] = json!({"accepted":true});
                let (result, recorder, transport) = observe(event, cancel_aware).await;
                assert_eq!(transport.consumed.load(Ordering::SeqCst), 2);
                assert_eq!(recorder.usage.len(), usize::from(optional == "measured"));
                if optional == "measured" {
                    assert_eq!(recorder.usage[0].0.total_tokens, 11);
                }
                let finished = recorder
                    .events
                    .iter()
                    .filter(|e| matches!(e, ModelStreamEvent::Finished { .. }))
                    .count();
                if kind == "response.failed" {
                    assert!(
                        matches!(result,Err(CoreError::Provider{code:Some(code),..}) if code=="fixture_failed")
                    );
                    assert_eq!(finished, 0);
                } else {
                    let response = result.unwrap();
                    assert_eq!(finished, 1);
                    assert_eq!(
                        response.finish_reason,
                        Some(if kind == "response.completed" {
                            FinishReason::Stop
                        } else {
                            FinishReason::Length
                        })
                    );
                    assert_eq!(response.usage.is_some(), optional == "measured");
                }
            }
        }
    }
}

#[tokio::test]
async fn h208_chat_completions_remains_an_independent_wire_protocol() {
    let transport = Arc::new(deepagent_models::MockTransport::new([
        json!({"choices":[{"index":0,"delta":{"content":"chat-control"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}})
        .to_string(),
        "[DONE]".into(),
    ]));
    let client = ModelClient::new(transport, ModelConfig::deepseek_chat("offline-placeholder"));
    let mut recorder = Recorder::default();
    let response = client
        .stream_response_observed(
            ResponseRequest::new("deepseek-flash", vec![Message::user("control")]),
            &mut recorder,
        )
        .await
        .unwrap();
    assert_eq!(response.finish_reason, Some(FinishReason::Stop));
    assert_eq!(response.usage.unwrap().total_tokens, 3);
    assert!(recorder
        .events
        .iter()
        .any(|e| matches!(e,ModelStreamEvent::ContentDelta{text} if text=="chat-control")));
}
