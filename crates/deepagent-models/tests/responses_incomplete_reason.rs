//! H-82: incompleteness is not synonymous with an exhausted output budget.
#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use sequence_fixture::NumberedAccumulator as ResponseAccumulator;

use deepagent_core::error::CoreError;
use deepagent_models::{DeltaObserver, FinishReason, ModelStreamEvent, Usage};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest};

struct Replay {
    reason: Option<&'static str>,
    calls: AtomicUsize,
    cancel_after_delta: Option<Arc<AtomicBool>>,
}

#[async_trait::async_trait]
impl HttpTransport for Replay {
    async fn stream(
        &self,
        _: TransportRequest,
        sink: &mut dyn EventSink,
    ) -> deepagent_core::Result<()> {
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        let delta = json!({"type":"response.output_text.delta","delta":"synthetic partial"});
        let mut sequence = sequence_fixture::FixtureSequence::default();
        if sink.on_event(&sequence.event(&delta.to_string()))? {
            return Ok(());
        }
        if let Some(cancel) = &self.cancel_after_delta {
            cancel.store(true, Ordering::Release);
            // Cancellation is observed at the existing asynchronous wait
            // boundary, not between synchronous in-memory sink callbacks.
            std::future::pending::<()>().await;
        }
        let details = self.reason.map(|reason| json!({"reason":reason})).into();
        sink.on_event(&sequence.event(&terminal(details, vec![]).to_string()))?;
        Ok(())
    }
}

#[derive(Default)]
struct Observed {
    events: Vec<ModelStreamEvent>,
    usage: Vec<Usage>,
}

impl DeltaObserver for Observed {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.events.push(event);
    }

    fn on_usage(&mut self, usage: Usage, _: &Value) {
        self.usage.push(usage);
    }
}

fn terminal(details: Value, output: Vec<Value>) -> Value {
    json!({"type":"response.incomplete", "response":{
        "id":"h82-response", "object":"response", "created_at":1,
        "model":"deepseek-flash", "status":"incomplete", "output":output,
        "incomplete_details":details,
        "usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18,
            "input_tokens_details":{"cached_tokens":4}}
    }})
}

fn call() -> Value {
    json!({"type":"function_call","id":"h82-item","call_id":"h82-call",
        "name":"write_file","arguments":"{\"path\":\"synthetic.txt\"}"})
}

#[test]
fn h82_known_reasons_preserve_typed_finish_usage_and_partial_text() {
    for (reason, expected) in [
        ("max_output_tokens", FinishReason::Length),
        ("content_filter", FinishReason::ContentFilter),
    ] {
        let mut acc = ResponseAccumulator::new();
        let mut observed = Observed::default();
        acc.push_sse_data_observed(
            &json!({"type":"response.output_text.delta","delta":"synthetic partial"}).to_string(),
            &mut observed,
        )
        .unwrap();
        assert!(acc
            .push_sse_data_observed(
                &terminal(json!({"reason":reason}), vec![]).to_string(),
                &mut observed,
            )
            .unwrap());
        let response = acc.finish().unwrap();
        assert_eq!(response.finish_reason, Some(expected));
        assert_eq!(response.output_text_projection(), "synthetic partial");
        assert_eq!(observed.usage.len(), 1);
        assert_eq!(response.usage, Some(observed.usage[0]));
        assert_eq!(observed.usage[0].prompt_tokens, 12);
        assert_eq!(observed.usage[0].completion_tokens, 6);
        assert_eq!(observed.usage[0].prompt_cache_hit_tokens, 4);
        assert_eq!(observed.usage[0].prompt_cache_miss_tokens, 8);
        assert!(matches!(observed.events.last(),
            Some(ModelStreamEvent::Finished {reason:Some(actual)}) if *actual==expected));
    }
}

#[test]
fn h82_missing_reason_is_not_length_and_preserves_valid_paid_usage() {
    for details in [Value::Null, json!({})] {
        for absent in [false, true] {
            let mut event = terminal(details.clone(), vec![]);
            if absent {
                event["response"]
                    .as_object_mut()
                    .unwrap()
                    .remove("incomplete_details");
            }
            let mut acc = ResponseAccumulator::new();
            let mut observed = Observed::default();
            let error = acc
                .push_sse_data_observed(&event.to_string(), &mut observed)
                .expect_err("unknown cause cannot authorize output-budget recovery");
            assert!(matches!(error, CoreError::Provider {code:Some(code),..}
                if code=="incomplete_reason_unknown"));
            assert_eq!(observed.usage.len(), 1);
            assert!(acc.finish().is_err());
            assert!(!observed
                .events
                .iter()
                .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
        }
    }
}

#[test]
fn h82_unknown_or_malformed_reason_remains_an_envelope_error() {
    for reason in [json!("PRIVATE_FUTURE_REASON"), Value::Null, json!(7)] {
        let mut acc = ResponseAccumulator::new();
        let mut observed = Observed::default();
        let error = acc
            .push_sse_data_observed(
                &terminal(json!({"reason":reason}), vec![]).to_string(),
                &mut observed,
            )
            .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE"));
        assert!(matches!(error, CoreError::Provider {code:Some(code),..}
            if code=="malformed_response_terminal"));
        assert!(acc.finish().is_err());
        assert!(
            observed.usage.is_empty(),
            "H-208 rejects malformed envelopes first"
        );
    }
}

#[test]
fn h82_both_incomplete_causes_never_promote_unfinished_tool_builders() {
    for reason in ["max_output_tokens", "content_filter"] {
        for explicitly_done in [false, true] {
            let mut acc = ResponseAccumulator::new();
            let mut observed = Observed::default();
            let item = call();
            acc.push_sse_data_observed(
                &json!({"type":"response.output_item.added","output_index":0,"item":item})
                    .to_string(),
                &mut observed,
            )
            .unwrap();
            if explicitly_done {
                acc.push_sse_data_observed(
                    &json!({"type":"response.output_item.done","output_index":0,"item":item})
                        .to_string(),
                    &mut observed,
                )
                .unwrap();
            }
            acc.push_sse_data_observed(
                &terminal(json!({"reason":reason}), vec![item]).to_string(),
                &mut observed,
            )
            .unwrap();
            let response = acc.finish().unwrap();
            assert_eq!(
                response.tool_invocations_from_items().len(),
                usize::from(explicitly_done)
            );
            assert_eq!(
                observed
                    .events
                    .iter()
                    .filter(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. }))
                    .count(),
                usize::from(explicitly_done)
            );
            assert_eq!(
                response.finish_reason,
                Some(if reason == "content_filter" {
                    FinishReason::ContentFilter
                } else {
                    FinishReason::Length
                })
            );
        }
    }
}

#[tokio::test]
async fn h82_public_response_entries_preserve_reason_and_usage_without_retry() {
    for reason in [Some("max_output_tokens"), Some("content_filter"), None] {
        for entry in 0..4 {
            let replay = Arc::new(Replay {
                reason,
                calls: AtomicUsize::new(0),
                cancel_after_delta: None,
            });
            let client = ModelClient::new(replay.clone(), ModelConfig::deepseek("synthetic"));
            let request = ResponseRequest::new(
                "deepseek-flash",
                vec![deepagent_core::message::Message::user("synthetic input")],
            );
            let mut observed = Observed::default();
            let cancel = Arc::new(AtomicBool::new(false));
            let result = match entry {
                0 => client.stream_response(request).await,
                1 => {
                    client
                        .stream_response_observed(request, &mut observed)
                        .await
                }
                2 => client.stream_response_cancelled(request, cancel).await,
                _ => {
                    client
                        .stream_response_observed_cancelled(request, &mut observed, cancel)
                        .await
                }
            };
            if let Some(reason) = reason {
                let response = result.unwrap();
                assert_eq!(
                    response.finish_reason,
                    Some(if reason == "content_filter" {
                        FinishReason::ContentFilter
                    } else {
                        FinishReason::Length
                    })
                );
                assert_eq!(response.usage.unwrap().total_tokens, 18);
            } else {
                let error = result.unwrap_err();
                let kind = deepagent_models::classify_model_error(&error);
                assert!(!kind.retryable());
                assert!(matches!(error, CoreError::Provider {code:Some(code),..}
                    if code=="incomplete_reason_unknown"));
            }
            if entry == 1 || entry == 3 {
                assert_eq!(observed.usage.len(), 1);
            }
            assert_eq!(replay.calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn h82_cancellation_before_or_during_filtered_stream_remains_cancellation() {
    for pre_cancelled in [false, true] {
        let cancel = Arc::new(AtomicBool::new(pre_cancelled));
        let replay = Arc::new(Replay {
            reason: Some("content_filter"),
            calls: AtomicUsize::new(0),
            cancel_after_delta: Some(cancel.clone()),
        });
        let client = ModelClient::new(replay.clone(), ModelConfig::deepseek("synthetic"));
        let request = ResponseRequest::new(
            "deepseek-flash",
            vec![deepagent_core::message::Message::user("synthetic input")],
        );
        let mut observed = Observed::default();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.stream_response_observed_cancelled(request, &mut observed, cancel),
        )
        .await
        .expect("cancellation interrupts the pending terminal")
        .unwrap_err();
        assert_eq!(
            deepagent_models::classify_model_error(&error),
            deepagent_models::ModelFailureKind::Cancelled
        );
        assert_eq!(
            replay.calls.load(Ordering::SeqCst),
            usize::from(!pre_cancelled)
        );
        assert!(
            observed.usage.is_empty(),
            "the paid terminal was not received"
        );
        assert!(!observed
            .events
            .iter()
            .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
    }
}
