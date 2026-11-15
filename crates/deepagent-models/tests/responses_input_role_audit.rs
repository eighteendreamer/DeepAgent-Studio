//! Audit DeepSeek Responses role validation for public native input items.

#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, ToolCall};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    classify_model_error, ChatCompletionRequest, DeltaObserver, ModelClient, ModelConfig,
    ModelFailureKind, ModelStreamEvent, Response, ResponseInputItem, ResponseRequest, WireMode,
};

#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn EventSink = &mut sequence_fixture::sink(sink);
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let events = if request.url.ends_with("/responses") {
            vec![
                r#"{"type":"response.output_text.delta","delta":"offline"}"#,
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
            ]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"offline"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        for event in events {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn request(role: &str, attachments: bool) -> ResponseRequest {
    let item = if attachments {
        ResponseInputItem::InputMessage {
            role: role.to_owned(),
            content: "invalid role audit".to_owned(),
            attachments: Vec::new(),
        }
    } else {
        ResponseInputItem::Message {
            role: role.to_owned(),
            content: "invalid role audit".to_owned(),
        }
    };
    ResponseRequest::from_response_items("deepseek-flash", None, vec![item])
}

#[tokio::test]
async fn audit_invalid_responses_message_roles_are_rejected_before_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    let message_result = client.stream_response(request("moderator", false)).await;
    let input_message_result = client.stream_response(request("moderator", true)).await;
    let requests = transport.requests.lock().unwrap();
    eprintln!(
        "Responses role audit: message_error={} input_message_error={} transport_calls={} wire_roles={:?}",
        message_result.is_err(),
        input_message_result.is_err(),
        requests.len(),
        requests
            .iter()
            .map(|body| body["input"][0]["role"].clone())
            .collect::<Vec<_>>(),
    );

    assert!(
        message_result.is_err() && input_message_result.is_err(),
        "unrecognized Responses message roles must be rejected at the provider boundary"
    );
    assert_eq!(
        requests.len(),
        0,
        "invalid roles must not be sent to the provider transport"
    );
}

#[tokio::test]
async fn audit_documented_responses_message_roles_reach_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    for role in ["user", "assistant", "system", "developer"] {
        client.stream_response(request(role, false)).await.unwrap();
    }
    let requests = transport.requests.lock().unwrap();
    let roles: Vec<&str> = requests
        .iter()
        .map(|body| body["input"][0]["role"].as_str().unwrap())
        .collect();
    eprintln!("Responses documented role control: {roles:?}");
    assert_eq!(roles, ["user", "assistant", "system", "developer"]);
}

const ROLE_ERROR: &str =
    "invalid input: Responses message role must be user, assistant, system, or developer";

#[derive(Default)]
struct Observer(usize);

impl DeltaObserver for Observer {
    fn on_event(&mut self, _: ModelStreamEvent) {
        self.0 += 1;
    }
}

async fn send(
    client: &ModelClient,
    request: ResponseRequest,
    entry: usize,
    observer: &mut Observer,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(false));
    match entry {
        0 => client.stream_response(request).await,
        1 => client.stream_response_cancelled(request, cancel).await,
        2 => client.stream_response_observed(request, observer).await,
        3 => {
            client
                .stream_response_observed_cancelled(request, observer, cancel)
                .await
        }
        _ => unreachable!(),
    }
}

fn client(transport: Arc<RecordingTransport>, mode: WireMode) -> ModelClient {
    let mut config = ModelConfig::deepseek("PRIVATE_CREDENTIAL");
    config.wire_mode = mode;
    ModelClient::new(transport, config)
}

#[tokio::test]
async fn invalid_roles_fail_on_every_response_entry_before_chat_projection_or_transport() {
    let mut failures = Vec::new();
    let mut transport_calls = 0;
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(RecordingTransport::default());
        let client = client(transport.clone(), mode);
        for (index, role) in [
            "",
            "moderator",
            "tool",
            "function",
            "USER",
            "Assistant",
            " user",
            "user ",
            "user\n",
            "user\0",
            "用户",
            "PRIVATE_ROLE",
        ]
        .into_iter()
        .enumerate()
        {
            for input_message in [false, true] {
                for entry in 0..4 {
                    for mutate in [false, true] {
                        let mut request = request(role, input_message);
                        if mutate {
                            let item = request.input.pop().unwrap();
                            request = self::request("user", false);
                            request.input.push(item);
                        }
                        let mut observer = Observer::default();
                        let result = send(&client, request, entry, &mut observer).await;
                        let rejected = match result {
                            Err(error) => {
                                matches!(error, CoreError::Invalid(_))
                                    && error.to_string() == ROLE_ERROR
                                    && classify_model_error(&error)
                                        == ModelFailureKind::InvalidRequest
                                    && !classify_model_error(&error).retryable()
                                    && !classify_model_error(&error).should_fallback()
                            }
                            Ok(_) => false,
                        };
                        if !rejected || observer.0 != 0 {
                            failures.push((mode, index, input_message, entry, mutate));
                        }
                    }
                }
            }
        }
        transport_calls += transport.requests.lock().unwrap().len();
    }
    assert_eq!(
        transport_calls,
        0,
        "invalid roles reached transport; failed cases={}",
        failures.len()
    );
    assert!(failures.is_empty(), "role admission failed: {failures:?}");
}

#[tokio::test]
async fn documented_roles_keep_order_and_existing_chat_projection_on_every_entry() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(RecordingTransport::default());
        let client = client(transport.clone(), mode);
        for input_message in [false, true] {
            for entry in 0..4 {
                let items = ["user", "assistant", "system", "developer"]
                    .into_iter()
                    .map(|role| request(role, input_message).input.remove(0))
                    .collect();
                let request = ResponseRequest::from_response_items("deepseek-flash", None, items);
                send(&client, request, entry, &mut Observer::default())
                    .await
                    .unwrap();
            }
        }
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 8);
        for body in requests.iter() {
            let key = if mode == WireMode::Responses {
                "input"
            } else {
                "messages"
            };
            let roles: Vec<_> = body[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["role"].as_str().unwrap())
                .collect();
            let expected = if mode == WireMode::Responses {
                ["user", "assistant", "system", "developer"]
            } else {
                ["user", "assistant", "system", "user"]
            };
            assert_eq!(roles, expected);
        }
    }
}

#[test]
fn historical_string_roles_round_trip_but_provider_validation_and_serialization_reject_them() {
    for kind in ["message", "input_message"] {
        let history =
            serde_json::json!({"type":kind,"role":"PRIVATE_ROLE","content":"PRIVATE_CONTENT"});
        let item: ResponseInputItem = serde_json::from_value(history.clone()).unwrap();
        assert_eq!(serde_json::to_value(&item).unwrap(), history);
        let request = ResponseRequest::from_response_items("deepseek-flash", None, vec![item]);
        let original = request.input.clone();
        let error = request.validate().unwrap_err();
        assert_eq!(error.to_string(), ROLE_ERROR);
        let error = serde_json::to_string(&request).unwrap_err();
        assert_eq!(error.to_string(), ROLE_ERROR);
        assert_eq!(request.input, original);
    }
}

#[test]
fn final_public_input_cannot_bypass_serialization_or_read_only_wire_pricing() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(RecordingTransport::default());
        let client = client(transport.clone(), mode);
        for input_message in [false, true] {
            let mut request = request("user", false);
            request
                .input
                .push(self::request("PRIVATE_ROLE", input_message).input.remove(0));
            assert_eq!(request.validate().unwrap_err().to_string(), ROLE_ERROR);
            assert_eq!(
                serde_json::to_value(&request).unwrap_err().to_string(),
                ROLE_ERROR
            );
            assert!(client
                .response_prompt_byte_estimate(&request)
                .unwrap_err()
                .to_string()
                .contains(ROLE_ERROR));
            assert_eq!(transport.requests.lock().unwrap().len(), 0);
        }
    }
}

#[tokio::test]
async fn native_chat_tools_and_responses_tool_output_items_are_not_message_roles() {
    let messages = vec![
        Message::user("fixture"),
        Message::assistant("").with_tool_calls(vec![ToolCall {
            id: "call_fixture".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        }]),
        Message::tool_result("call_fixture", "fixture"),
    ];
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(RecordingTransport::default());
        let client = client(transport.clone(), mode);
        for entry in 0..4 {
            let request = ResponseRequest::new("deepseek-flash", messages.clone());
            send(&client, request, entry, &mut Observer::default())
                .await
                .unwrap();
        }
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for body in requests.iter() {
            if mode == WireMode::Responses {
                assert_eq!(body["input"][2]["type"], "function_call_output");
                assert!(body["input"][2].get("role").is_none());
            } else {
                assert_eq!(body["messages"][2]["role"], "tool");
                assert_eq!(body["messages"][2]["tool_call_id"], "call_fixture");
            }
        }
    }
    let transport = Arc::new(RecordingTransport::default());
    let client = client(transport.clone(), WireMode::ChatCompletions);
    client
        .stream_chat_completion(ChatCompletionRequest::new("deepseek-flash", messages))
        .await
        .unwrap();
    assert_eq!(
        transport.requests.lock().unwrap()[0]["messages"][2]["role"],
        "tool"
    );
}
