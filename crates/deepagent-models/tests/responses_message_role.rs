//! H-209: native output message identity must not be defaulted from UI semantics.
#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use sequence_fixture::NumberedAccumulator as ResponseAccumulator;

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use deepagent_models::{
    transport::{EventSink, HttpTransport, TransportRequest},
    DeltaObserver, ModelClient, ModelConfig, ModelStreamEvent, ResponseRequest,
};
use serde_json::{json, Value};

struct Replay {
    events: Vec<Value>,
    calls: AtomicUsize,
    delivered: AtomicUsize,
}

#[async_trait]
impl HttpTransport for Replay {
    async fn stream(
        &self,
        _: TransportRequest,
        sink: &mut dyn EventSink,
    ) -> deepagent_core::Result<()> {
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        for event in sequence_fixture::numbered(self.events.iter().map(Value::to_string)) {
            self.delivered.fetch_add(1, Ordering::SeqCst);
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Seen(Vec<ModelStreamEvent>);
impl DeltaObserver for Seen {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

fn message() -> Value {
    json!({"type":"message","id":"PRIVATE_ITEM","status":"completed",
        "role":"assistant","content":[{"type":"output_text","text":"PRIVATE_CONTENT"}]})
}

fn terminal(kind: &str, output: Vec<Value>) -> Value {
    json!({"type":kind,"response":{"id":"PRIVATE_RESPONSE","object":"response",
        "created_at":1,"model":"deepseek-flash","status":kind.strip_prefix("response.").unwrap(),
        "output":output,"incomplete_details":{"reason":"max_output_tokens"},
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}}})
}

fn invalid_messages() -> Vec<Value> {
    let mut values = vec![];
    for role in [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!({})),
        Some(json!(1)),
        Some(json!([])),
        Some(json!("")),
        Some(json!(" ")),
        Some(json!("Assistant")),
        Some(json!("user")),
        Some(json!("system")),
        Some(json!("developer")),
        Some(json!("PRIVATE_ROLE")),
    ] {
        let mut item = message();
        match role {
            Some(value) => item["role"] = value,
            None => {
                item.as_object_mut().unwrap().remove("role");
            }
        }
        values.push(item);
    }
    values
}

fn rejected(error: CoreError) {
    assert!(
        !error.to_string().contains("PRIVATE"),
        "error must omit received values"
    );
    assert!(error.to_string().contains("message.role"), "{error}");
    let failure = deepagent_models::classify_model_error(&error);
    assert!(!failure.retryable());
    assert!(matches!(error, CoreError::Provider {code:Some(code),..}
        if code=="malformed_response_message"));
}

#[tokio::test]
async fn h209_native_message_role_is_checked_on_all_admission_paths() {
    for item in invalid_messages() {
        for source in ["added", "done", "completed", "incomplete"] {
            for cancelled_entry in [false, true] {
                let broken = match source {
                    "added" => json!({"type":"response.output_item.added","item":item}),
                    "done" => json!({"type":"response.output_item.done","item":item}),
                    "completed" => terminal("response.completed", vec![item.clone()]),
                    _ => terminal("response.incomplete", vec![item.clone()]),
                };
                let transport = Arc::new(Replay {
                    events: vec![broken, terminal("response.completed", vec![])],
                    calls: AtomicUsize::new(0),
                    delivered: AtomicUsize::new(0),
                });
                let client =
                    ModelClient::new(transport.clone(), ModelConfig::deepseek("synthetic"));
                let request =
                    ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]);
                let mut seen = Seen::default();
                let result = if cancelled_entry {
                    client
                        .stream_response_observed_cancelled(
                            request,
                            &mut seen,
                            Arc::new(AtomicBool::new(false)),
                        )
                        .await
                } else {
                    client.stream_response_observed(request, &mut seen).await
                };
                rejected(result.expect_err("native message must supply the assistant role"));
                assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
                assert_eq!(transport.delivered.load(Ordering::SeqCst), 1);
                assert!(!seen
                    .0
                    .iter()
                    .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
            }
        }
    }
}

#[test]
fn h209_invalid_terminal_message_cannot_leave_an_accepted_public_accumulator() {
    for kind in ["response.completed", "response.incomplete"] {
        for item in invalid_messages() {
            let mut accumulator = ResponseAccumulator::new();
            let mut seen = Seen::default();
            let result = accumulator
                .push_sse_data_observed(&terminal(kind, vec![item]).to_string(), &mut seen);
            rejected(result.unwrap_err());
            assert!(
                accumulator.finish().is_err(),
                "failed parsing is not successful assembly"
            );
            assert!(!seen
                .0
                .iter()
                .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
        }
    }
}

#[tokio::test]
async fn h209_valid_role_keeps_done_only_deltas_partial_content_and_other_items() {
    for with_delta in [false, true] {
        for omit_content in [false, true] {
            let mut item = message();
            item["future_extension"] = json!(true);
            if omit_content {
                item.as_object_mut().unwrap().remove("content");
            }
            let function = json!({"type":"function_call","id":"fn","call_id":"fn-call",
                "name":"read_file","arguments":"{}"});
            let custom = json!({"type":"custom_tool_call","id":"custom","call_id":"custom-call",
                "name":"apply_patch","input":"synthetic patch"});
            let mut events = vec![];
            if with_delta {
                events.push(json!({"type":"response.output_text.delta",
                "item_id":"PRIVATE_ITEM","output_index":0,"delta":"PRIVATE_CONTENT"}));
            }
            events.push(json!({"type":"response.output_item.done","output_index":0,"item":item}));
            events.push(terminal(
                "response.completed",
                vec![item.clone(), function, custom],
            ));
            let transport = Arc::new(Replay {
                events,
                calls: AtomicUsize::new(0),
                delivered: AtomicUsize::new(0),
            });
            let client = ModelClient::new(transport, ModelConfig::deepseek("synthetic"));
            let response = client
                .stream_response(ResponseRequest::new(
                    "deepseek-flash",
                    vec![Message::user("synthetic")],
                ))
                .await
                .unwrap();
            assert_eq!(
                response.output_text_projection(),
                if with_delta || !omit_content {
                    "PRIVATE_CONTENT"
                } else {
                    ""
                }
            );
            assert_eq!(response.tool_invocations_from_items().len(), 2);
            assert_eq!(response.usage.unwrap().total_tokens, 11);
        }
    }
}
