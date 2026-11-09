//! H132: the public Chat route preserves policy termination, not tool authority.
use super::*;
use crate::{DeltaObserver, FinishReason, ModelStreamEvent};
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
    let mut chunk = json!({"id":"h132","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{},"finish_reason":reason}]});
    if let Some(sample) = sample {
        chunk["usage"] = sample;
    }
    chunk.to_string()
}

fn client(events: Vec<String>) -> ModelClient {
    ModelClient::new(
        Arc::new(crate::test_sequence::mock(events)),
        ModelConfig::deepseek_chat("synthetic"),
    )
}

#[tokio::test]
async fn h132_filtered_empty_or_partial_content_keeps_reason_usage_and_one_finished() {
    for partial in [false, true] {
        for sample in [Some(usage()), None, Some(Value::Null)] {
            for compatibility_usage_chunk in [false, true] {
                let measured = sample.as_ref().is_some_and(|value| !value.is_null());
                let mut events = Vec::new();
                if partial {
                    events.push(json!({"choices":[{"delta":{"content":"safe partial","reasoning_content":"synthetic reasoning"},"finish_reason":null}]}).to_string());
                }
                events.push(terminal(
                    "content_filter",
                    (!compatibility_usage_chunk)
                        .then(|| sample.clone())
                        .flatten(),
                ));
                if compatibility_usage_chunk {
                    events.push(json!({"choices":[],"usage":sample}).to_string());
                }
                events.push("[DONE]".into());
                let mut observer = Observer::default();
                let response = client(events)
                    .stream_response_observed(
                        ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
                        &mut observer,
                    )
                    .await
                    .unwrap();
                assert_eq!(response.finish_reason, Some(FinishReason::ContentFilter));
                assert_eq!(
                    response.output_text_projection(),
                    if partial { "safe partial" } else { "" }
                );
                assert_eq!(response.usage.is_some(), measured);
                if measured {
                    let u = response.usage.unwrap();
                    assert_eq!(
                        (
                            u.prompt_tokens,
                            u.completion_tokens,
                            u.reasoning_tokens,
                            u.prompt_cache_hit_tokens,
                            u.prompt_cache_miss_tokens
                        ),
                        (9, 2, 1, 4, 5)
                    );
                    assert_eq!(response.raw_usage, Some(usage()));
                }
                assert_eq!(
                    observer
                        .0
                        .iter()
                        .filter(|event| matches!(event, ModelStreamEvent::Usage { .. }))
                        .count(),
                    usize::from(measured)
                );
                assert_eq!(
                    observer
                        .0
                        .iter()
                        .filter(|event| matches!(
                            event,
                            ModelStreamEvent::Finished {
                                reason: Some(FinishReason::ContentFilter)
                            }
                        ))
                        .count(),
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn h132_filter_never_promotes_complete_or_truncated_tool_builders() {
    for arguments in [r#"{"value":1}"#, r#"{"value":"#] {
        let start=json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"h132-call","type":"function","function":{"name":"synthetic_tool","arguments":arguments}}]},"finish_reason":null}]}).to_string();
        let mut observer = Observer::default();
        let response = client(vec![
            start,
            terminal("content_filter", Some(usage())),
            "[DONE]".into(),
        ])
        .stream_response_observed(
            ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
            &mut observer,
        )
        .await
        .unwrap();
        assert_eq!(response.finish_reason, Some(FinishReason::ContentFilter));
        assert!(response.tool_invocations_from_items().is_empty());
        assert!(!observer
            .0
            .iter()
            .any(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. })));
    }
}

#[tokio::test]
async fn h132_unknown_reason_empty_stop_and_missing_done_still_fail_without_fake_finished() {
    for (reason, done) in [
        ("h132_future_reason", true),
        ("stop", true),
        ("content_filter", false),
    ] {
        let mut events = vec![terminal(reason, Some(usage()))];
        if done {
            events.push("[DONE]".into());
        }
        let mut observer = Observer::default();
        let result = client(events)
            .stream_response_observed(
                ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
                &mut observer,
            )
            .await;
        assert!(result.is_err(), "{reason}, done={done}");
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
            1,
            "paid usage precedes terminal validation"
        );
    }
}

#[tokio::test]
async fn h132_cancel_after_filtered_usage_before_done_is_not_a_completed_response() {
    use crate::transport::EventSink;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Waiting(tokio::sync::Semaphore);
    #[async_trait::async_trait]
    impl HttpTransport for Waiting {
        async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            sink.on_event(&terminal("content_filter", Some(usage())))?;
            self.0.add_permits(1);
            std::future::pending().await
        }
    }
    let transport = Arc::new(Waiting(tokio::sync::Semaphore::new(0)));
    let client = ModelClient::new(transport.clone(), ModelConfig::deepseek_chat("synthetic"));
    let cancel = Arc::new(AtomicBool::new(false));
    let mut observer = Observer::default();
    let request = client.stream_response_observed_cancelled(
        ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
        &mut observer,
        cancel.clone(),
    );
    let signal = async {
        transport.0.acquire().await.unwrap().forget();
        cancel.store(true, Ordering::Release);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(request, signal)
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(
        observer
            .0
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::Usage { .. }))
            .count(),
        1
    );
    assert!(!observer
        .0
        .iter()
        .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
}
