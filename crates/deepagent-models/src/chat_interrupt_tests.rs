//! H144: documented interrupted Chat outcomes retain meaning, never tool authority.
use super::*;
use crate::{DeltaObserver, ModelStreamEvent};
use deepagent_core::message::Message;
use serde_json::{json, Value};

#[derive(Default)]
struct Observer(Vec<ModelStreamEvent>);
impl DeltaObserver for Observer {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

fn usage() -> Value {
    json!({"prompt_tokens":9,"completion_tokens":2,"total_tokens":11,
        "prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":5,
        "completion_tokens_details":{"reasoning_tokens":1}})
}

fn terminal(reason: &str, sample: Option<Value>) -> String {
    let mut event =
        json!({"choices":[{"index":0,"delta":{"content":null},"finish_reason":reason}]});
    if let Some(sample) = sample {
        event["usage"] = sample;
    }
    event.to_string()
}

fn client(events: Vec<String>) -> ModelClient {
    ModelClient::new(
        Arc::new(crate::test_sequence::mock(events)),
        ModelConfig::deepseek_chat("synthetic"),
    )
}

#[tokio::test]
async fn h144_empty_and_partial_interruptions_preserve_typed_reason_and_optional_usage() {
    for reason in ["aborted", "insufficient_system_resource"] {
        for partial in [false, true] {
            for sample in [Some(usage()), None, Some(Value::Null)] {
                let measured = sample.as_ref().is_some_and(|value| !value.is_null());
                let mut events = Vec::new();
                if partial {
                    events.push(json!({"choices":[{"delta":{"content":"interrupted partial","reasoning_content":"synthetic reasoning"},"finish_reason":null}]}).to_string());
                }
                events.extend([terminal(reason, sample), "[DONE]".into()]);
                let mut observer = Observer::default();
                let response = client(events)
                    .stream_response_observed(
                        ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
                        &mut observer,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(response.finish_reason).unwrap(),
                    reason
                );
                let decoded: crate::FinishReason = serde_json::from_value(json!(reason)).unwrap();
                assert_eq!(response.finish_reason, Some(decoded));
                assert_eq!(
                    response.output_text_projection(),
                    if partial { "interrupted partial" } else { "" }
                );
                assert_eq!(response.usage.is_some(), measured);
                assert_eq!(response.raw_usage, measured.then(usage));
                if let Some(usage) = response.usage {
                    assert_eq!(
                        (
                            usage.prompt_tokens,
                            usage.completion_tokens,
                            usage.prompt_cache_hit_tokens,
                            usage.prompt_cache_miss_tokens,
                            usage.reasoning_tokens
                        ),
                        (9, 2, 4, 5, 1)
                    );
                }
                assert_eq!(
                    observer
                        .0
                        .iter()
                        .filter(|event| matches!(event, ModelStreamEvent::Usage { .. }))
                        .count(),
                    usize::from(measured)
                );
                let finished: Vec<_> = observer
                    .0
                    .iter()
                    .filter_map(|event| match event {
                        ModelStreamEvent::Finished { reason } => {
                            Some(serde_json::to_value(reason).unwrap())
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(finished, [json!(reason)]);
            }
        }
    }
}

#[tokio::test]
async fn h144_interrupted_builders_are_not_adopted_but_normal_tools_still_complete() {
    for reason in ["aborted", "insufficient_system_resource", "tool_calls"] {
        for arguments in [r#"{"value":1}"#, r#"{"value":"#] {
            if reason == "tool_calls" && arguments.ends_with(':') {
                continue;
            }
            let mut observer = Observer::default();
            let response = client(vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"h144-call","type":"function","function":{"name":"synthetic_tool","arguments":arguments}}]},"finish_reason":null}]}).to_string(),
                terminal(reason, Some(usage())),
                "[DONE]".into(),
            ])
            .stream_response_observed(
                ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
                &mut observer,
            )
            .await
            .unwrap();
            let completed = observer
                .0
                .iter()
                .filter(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. }))
                .count();
            assert_eq!(completed, usize::from(reason == "tool_calls"));
            assert_eq!(response.tool_invocations_from_items().len(), completed);
        }
    }
}

#[tokio::test]
async fn h144_known_interruptions_still_require_done_and_unknown_values_stay_errors() {
    for (reason, done) in [
        ("aborted", false),
        ("insufficient_system_resource", false),
        ("future_h144_reason", true),
        ("stop", true),
    ] {
        let mut events = vec![terminal(reason, Some(usage()))];
        if done {
            events.push("[DONE]".into());
        }
        let mut observer = Observer::default();
        assert!(client(events)
            .stream_response_observed(
                ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
                &mut observer,
            )
            .await
            .is_err());
        assert!(!observer
            .0
            .iter()
            .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
        assert_eq!(
            observer
                .0
                .iter()
                .filter(|event| matches!(event, ModelStreamEvent::Usage { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn h144_interrupt_failure_classification_is_code_driven_not_local_cancellation() {
    for (code, expected) in [
        ("aborted", "Aborted"),
        ("insufficient_system_resource", "Overloaded"),
    ] {
        for message in [
            "synthetic localized provider reason",
            "cancelled by user; context length exceeded",
        ] {
            let error =
                deepagent_core::error::CoreError::provider(None, Some(code.into()), message);
            let kind = crate::classify_model_error(&error);
            assert_eq!(format!("{kind:?}"), expected);
            assert!(!kind.retryable());
        }
        assert_eq!(
            crate::classify_model_error(&deepagent_core::error::CoreError::provider(
                Some(401),
                Some(code.into()),
                "authentication control"
            )),
            crate::ModelFailureKind::Authentication
        );
    }
    assert_eq!(
        crate::classify_model_error(&deepagent_core::error::CoreError::other(
            "request cancelled by user"
        )),
        crate::ModelFailureKind::Cancelled
    );
}
