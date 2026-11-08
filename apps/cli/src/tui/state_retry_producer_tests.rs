//! Real ModelAgent retry/fallback/length/cancel producers -> the TUI consumer.
use super::*;
use deepagent_core::error::{CoreError, Result};
use deepagent_models::{
    transport::EventSink, HttpTransport, ModelClient, ModelConfig, TransportRequest, WireMode,
};
use deepagent_runtime::{events::RuntimeEventSink, Agent, AgentDecision, ModelAgent};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum End {
    Stop,
    Disconnect,
    Overload,
    Length,
    Cancel,
}

struct Script {
    mode: WireMode,
    ends: Vec<End>,
    requests: Mutex<Vec<Value>>,
    cancel: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl HttpTransport for Script {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len() - 1
        };
        let end = *self.ends.get(index).expect("unexpected provider retry");
        let text = if matches!(end, End::Stop) {
            "valid"
        } else {
            "discarded"
        };
        let thought = if matches!(end, End::Stop) {
            "valid thought"
        } else {
            "discarded thought"
        };
        let mut events = match self.mode {
            WireMode::Responses => vec![
                json!({"type":"response.reasoning_text.delta","sequence_number":0,"delta":thought}),
                json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"reply",
                    "output_index":0,"content_index":0,"delta":text}),
            ],
            WireMode::ChatCompletions => vec![json!({"choices":[{"index":0,
                "delta":{"content":text,"reasoning_content":thought},"finish_reason":null}]})],
        };
        if matches!(end, End::Stop | End::Length) {
            let length = matches!(end, End::Length);
            events.push(match self.mode {
                WireMode::Responses => {
                    let mut event = json!({"type":if length { "response.incomplete" } else { "response.completed" },
                        "sequence_number":2,"response":{"id":format!("h258-{index}"),"object":"response",
                            "created_at":1,"model":"primary","status":if length {"incomplete"} else {"completed"},
                            "output":[{"id":"reply","type":"message","role":"assistant",
                                "content":[{"type":"output_text","text":text,"annotations":[]}]}]}});
                    if length { event["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"}); }
                    event
                }
                WireMode::ChatCompletions => json!({"choices":[{"index":0,"delta":{},
                    "finish_reason":if length { "length" } else { "stop" }}]}),
            });
        }
        for event in events {
            if sink.on_event(&event.to_string())? {
                return Ok(());
            }
        }
        match end {
            End::Stop | End::Length => {
                assert_eq!(self.mode, WireMode::ChatCompletions);
                sink.on_event("[DONE]")?;
                Ok(())
            }
            End::Disconnect => Err(CoreError::other("synthetic connection interrupted")),
            End::Overload => Err(CoreError::provider(
                Some(529),
                Some("overloaded_error".into()),
                "synthetic overload",
            )),
            End::Cancel => {
                self.cancel.store(true, Ordering::Release);
                std::future::pending().await
            }
        }
    }
}

struct ViewSink {
    state: Mutex<ChatState>,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RuntimeEventSink for ViewSink {
    fn emit(&self, event: RuntimeEvent) {
        self.state.lock().unwrap().on_runtime(event.clone());
        self.events.lock().unwrap().push(event);
    }
}

async fn exercise(mode: WireMode, ends: Vec<End>, success: bool, resets: usize) {
    let calls = ends.len();
    let fallback = ends.iter().any(|end| matches!(end, End::Overload));
    let length = ends.iter().any(|end| matches!(end, End::Length));
    let cancel = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(Script {
        mode,
        ends,
        requests: Mutex::new(vec![]),
        cancel: cancel.clone(),
    });
    let view = Arc::new(ViewSink {
        state: Mutex::new(ChatState::new(None)),
        events: Mutex::new(vec![]),
    });
    let client = Arc::new(ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("synthetic").with_wire_mode(mode),
    ));
    let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
        .with_events(view.clone())
        .with_max_model_attempts(3)
        .with_fallback_model(Some("fallback".into()));
    let result = tokio::time::timeout(
        Duration::from_secs(6),
        agent.think_cancelled(0, &[], Some(cancel.clone())),
    )
    .await
    .expect("bounded synthetic agent did not return");
    assert_eq!(result.is_ok(), success, "{mode:?}: {result:?}");
    let events = view.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelAttemptReset { .. }))
            .count(),
        resets
    );
    let completed = events
        .iter()
        .position(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. }));
    assert_eq!(completed.is_some(), success);
    if let Some(completed) = completed {
        assert!(!events[completed + 1..]
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelAttemptReset { .. })));
    }
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), calls);
    if fallback {
        assert_eq!(requests.last().unwrap()["model"], "fallback");
    }
    if length {
        let key = if mode == WireMode::Responses {
            "max_output_tokens"
        } else {
            "max_tokens"
        };
        assert!(requests[1][key].as_u64().unwrap() > requests[0][key].as_u64().unwrap_or(0));
    }
    let mut state = view.state.lock().unwrap();
    let terminal = match result {
        Ok(AgentDecision::CompleteItems { message, .. }) => RuntimeEvent::RunCompleted {
            message: message.content,
            edited_files: vec![],
        },
        Ok(other) => panic!("unexpected decision: {other:?}"),
        Err(_) if cancel.load(Ordering::Acquire) => RuntimeEvent::RunCancelled {
            edited_files: vec![],
        },
        Err(error) => RuntimeEvent::RunFailed {
            reason: error.to_string(),
        },
    };
    // The ModelAgent is real; terminal projection here is a consumer unit check.
    // Full Kernel/RunStore/cold-reopen coverage belongs to retry_audit.rs.
    state.on_runtime(terminal);
    let cells = state.drain_flushable();
    assert!(
        !format!("{cells:?}").contains("discarded"),
        "{mode:?}: {cells:?}"
    );
    if success {
        assert!(cells.contains(&HistoryCell::Assistant {
            text: "valid".into()
        }));
    }
}

#[tokio::test]
async fn h258_actual_producer_retries_falls_back_and_escalates_before_committing_either_protocol() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for (ends, resets) in [
            (vec![End::Stop], 0),
            (vec![End::Disconnect, End::Disconnect, End::Stop], 2),
            (vec![End::Overload, End::Stop], 1),
            (vec![End::Length, End::Stop], 1),
        ] {
            exercise(mode, ends, true, resets).await;
        }
    }
}

#[tokio::test]
async fn h258_actual_producer_exhaustion_and_true_cancellation_never_commit_speculative_text() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for (ends, resets) in [
            (vec![End::Disconnect, End::Disconnect, End::Disconnect], 2),
            (vec![End::Cancel], 0),
            (vec![End::Disconnect, End::Cancel], 1),
        ] {
            exercise(mode, ends, false, resets).await;
        }
    }
}
