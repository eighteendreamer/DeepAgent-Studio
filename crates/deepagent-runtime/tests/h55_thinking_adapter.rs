//! H-55: exercise public adapters; never contact a provider or use credentials.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use async_trait::async_trait;
use deepagent_core::{
    error::{CoreError, Result},
    message::Message,
};
use deepagent_models::{
    transport::{EventSink, HttpTransport, TransportRequest},
    ChatCompletionRequest, ModelClient, ModelConfig, ReasoningEffort, ResponseDefaults,
    ResponseRequest, WireMode,
};
use deepagent_runtime::{Agent, AgentDecision, ModelAgent};
use serde_json::{json, Value};
use tokio::sync::Notify;

struct ScriptedTransport {
    mode: WireMode,
    first_error: Option<u16>,
    hold: bool,
    started: Notify,
    requests: Mutex<Vec<Value>>,
}

impl ScriptedTransport {
    fn new(mode: WireMode, first_error: Option<u16>, hold: bool) -> Self {
        Self {
            mode,
            first_error,
            hold,
            started: Notify::new(),
            requests: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl HttpTransport for ScriptedTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let suffix = match self.mode {
            WireMode::Responses => "/responses",
            WireMode::ChatCompletions => "/chat/completions",
            WireMode::Messages => "/v1/messages",
        };
        assert!(request.url.ends_with(suffix));
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len()
        };
        if attempt == 1 {
            if let Some(status) = self.first_error {
                return Err(CoreError::provider(
                    Some(status),
                    Some(
                        if status == 429 {
                            "rate_limit_exceeded"
                        } else {
                            "overloaded_error"
                        }
                        .into(),
                    ),
                    "H55 synthetic retry",
                ));
            }
        }
        if self.hold {
            self.started.notify_one();
            return std::future::pending().await;
        }
        let events = match self.mode {
            WireMode::Responses => vec![json!({
                "type":"response.completed", "sequence_number":0,
                "response":{"id":"h55-fixture","object":"response","created_at":1,
                    "model":"primary","status":"completed","output":[{
                        "type":"message","id":"h55-message","role":"assistant",
                        "content":[{"type":"output_text","text":"H55 complete","annotations":[]}]
                    }]}
            })
            .to_string()],
            WireMode::ChatCompletions => vec![
                json!({"choices":[{"index":0,"delta":{"content":"H55 complete"},
                    "finish_reason":"stop"}]})
                .to_string(),
                "[DONE]".into(),
            ],
            WireMode::Messages => [
                json!({"type":"message_start","message":{"id":"h55-fixture","type":"message",
                    "role":"assistant","model":"primary","content":[],"stop_reason":null,
                    "stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,
                    "content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,
                    "delta":{"type":"text_delta","text":"H55 complete"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn",
                    "stop_sequence":null},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"}),
            ]
            .into_iter()
            .map(|event| event.to_string())
            .collect(),
        };
        for event in events {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

fn client(transport: Arc<ScriptedTransport>) -> Arc<ModelClient> {
    let config = ModelConfig::deepseek("h55-synthetic")
        .with_wire_mode(transport.mode)
        .with_defaults(ResponseDefaults {
            reasoning_effort: Some(ReasoningEffort::High),
            max_output_tokens: Some(1024),
            ..Default::default()
        });
    Arc::new(ModelClient::new(transport, config))
}

fn assert_wire(body: &Value, mode: WireMode, effort: ReasoningEffort) {
    match mode {
        WireMode::Responses => {
            assert_eq!(body["reasoning"]["effort"], effort.label());
            assert!(body.get("thinking").is_none());
            assert!(body.get("reasoning_effort").is_none());
        }
        WireMode::ChatCompletions => {
            assert!(body.get("reasoning").is_none());
            if effort == ReasoningEffort::None {
                assert_eq!(body["thinking"], json!({"type":"disabled"}));
                assert!(body.get("reasoning_effort").is_none());
            } else {
                assert_eq!(body["thinking"], json!({"type":"enabled"}));
                assert_eq!(body["reasoning_effort"], effort.label());
            }
        }
        WireMode::Messages => {
            assert!(body.get("reasoning").is_none());
            assert!(body.get("reasoning_effort").is_none());
            assert_eq!(body["max_tokens"], 1024);
            if effort == ReasoningEffort::None {
                assert_eq!(body["thinking"], json!({"type":"disabled"}));
                assert!(body.get("output_config").is_none());
            } else {
                assert_eq!(body["thinking"], json!({"type":"adaptive"}));
                assert_eq!(body["output_config"]["effort"], effort.label());
            }
        }
    }
}

#[tokio::test]
async fn h55_agent_preserves_efforts_on_retries_and_surfaces_overload_without_retry() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::parse("medium").expect("medium must remain a distinct canonical tier"),
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ] {
            for first_error in [None, Some(429), Some(503), Some(529)] {
                for cancel_aware in [false, true] {
                    let transport = Arc::new(ScriptedTransport::new(mode, first_error, false));
                    let mut agent = ModelAgent::new(
                        client(transport.clone()),
                        "primary",
                        "synthetic",
                        "synthetic",
                        vec![],
                    )
                    .with_reasoning_effort(effort)
                    .with_max_model_attempts(3);
                    let result = if cancel_aware {
                        agent
                            .think_cancelled(0, &[], Some(Arc::new(AtomicBool::new(false))))
                            .await
                    } else {
                        agent.think(0, &[]).await
                    };
                    if matches!(first_error, Some(503 | 529)) {
                        assert_eq!(
                            deepagent_models::classify_model_error(&result.unwrap_err()),
                            deepagent_models::ModelFailureKind::Overloaded
                        );
                    } else {
                        let AgentDecision::CompleteItems { message, .. } = result.unwrap() else {
                            panic!("a completed synthetic reply must finish the agent");
                        };
                        assert_eq!(message.content, "H55 complete");
                    }
                    let requests = transport.requests.lock().unwrap();
                    assert_eq!(requests.len(), if first_error == Some(429) { 2 } else { 1 });
                    for body in requests.iter() {
                        assert_wire(body, mode, effort);
                    }
                    assert_eq!(requests[0]["model"], "primary");
                    if first_error == Some(429) {
                        assert_eq!(requests[1]["model"], "primary");
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h55_omission_and_explicit_none_are_distinct_on_all_public_response_entries() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for explicit in [None, Some(ReasoningEffort::None)] {
            for cancel_aware in [false, true] {
                let transport = Arc::new(ScriptedTransport::new(mode, None, false));
                let client = client(transport.clone());
                let mut request = ResponseRequest::new("primary", vec![Message::user("synthetic")]);
                request.reasoning_effort = explicit;
                let response = if cancel_aware {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                }
                .unwrap();
                assert_eq!(response.output_text_projection(), "H55 complete");
                let requests = transport.requests.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_wire(
                    &requests[0],
                    mode,
                    explicit.unwrap_or(ReasoningEffort::High),
                );
            }
        }
    }
}

#[tokio::test]
async fn h55_direct_chat_toggle_and_none_survive_high_defaults() {
    for form in 0..3 {
        for cancel_aware in [false, true] {
            let transport = Arc::new(ScriptedTransport::new(
                WireMode::ChatCompletions,
                None,
                false,
            ));
            let client = client(transport.clone());
            let request = ChatCompletionRequest::new("primary", vec![Message::user("synthetic")]);
            let request = match form {
                0 => request,
                1 => request.with_reasoning_effort(ReasoningEffort::None),
                _ => request.with_thinking(false),
            };
            let response = if cancel_aware {
                client
                    .stream_chat_completion_cancelled(request, Arc::new(AtomicBool::new(false)))
                    .await
            } else {
                client.stream_chat_completion(request).await
            }
            .unwrap();
            assert_eq!(response.output_text_projection(), "H55 complete");
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_wire(
                &requests[0],
                WireMode::ChatCompletions,
                if form == 0 {
                    ReasoningEffort::High
                } else {
                    ReasoningEffort::None
                },
            );
        }
    }
}

#[tokio::test]
async fn h55_none_effort_cancels_before_and_during_each_adapter_without_retry() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        let transport = Arc::new(ScriptedTransport::new(mode, None, true));
        let mut agent = ModelAgent::new(
            client(transport.clone()),
            "primary",
            "synthetic",
            "synthetic",
            vec![],
        )
        .with_reasoning_effort(ReasoningEffort::None);
        let error = agent
            .think_cancelled(0, &[], Some(Arc::new(AtomicBool::new(true))))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
        assert!(transport.requests.lock().unwrap().is_empty());

        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let mut agent = ModelAgent::new(
            client(transport.clone()),
            "primary",
            "synthetic",
            "synthetic",
            vec![],
        )
        .with_reasoning_effort(ReasoningEffort::None);
        let mut worker =
            tokio::spawn(async move { agent.think_cancelled(0, &[], Some(worker_cancel)).await });
        let started =
            tokio::time::timeout(Duration::from_secs(3), transport.started.notified()).await;
        cancel.store(true, Ordering::Release);
        let result = match tokio::time::timeout(Duration::from_secs(3), &mut worker).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                worker.abort();
                let _ = worker.await;
                panic!("the cancelled adapter did not return within the test deadline");
            }
        };
        started.expect("the actual model transport must start before cancellation");
        let error = result.unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_wire(&requests[0], mode, ReasoningEffort::None);
    }
}
