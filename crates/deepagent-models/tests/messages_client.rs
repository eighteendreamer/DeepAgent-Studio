use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseItem, ResponseRequest, WireMode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingTransport {
    request: Mutex<Option<TransportRequest>>,
    events: Vec<String>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        *self.request.lock().unwrap() = Some(request);
        for event in &self.events {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn events() -> Vec<String> {
    [
        json!({"type":"message_start","message":{"id":"msg-1","type":"message",
            "role":"assistant","model":"fixture","content":[],"stop_reason":null,
            "stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,
            "content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,
            "delta":{"type":"text_delta","text":"ok"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn",
            "stop_sequence":null},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| event.to_string())
    .collect()
}

#[tokio::test]
async fn messages_client_uses_messages_endpoint_and_neutral_result() {
    let transport = Arc::new(RecordingTransport {
        events: events(),
        ..Default::default()
    });
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("fixture-key").with_wire_mode(WireMode::Messages),
    );
    let request = ResponseRequest::from_response_items(
        "fixture-model",
        Some("rules".into()),
        vec![ResponseItem::Message {
            role: "user".into(),
            content: "hello".into(),
        }],
    )
    .with_max_output_tokens(128);
    let response = client.stream_response(request).await.unwrap();
    assert_eq!(response.output_text_projection(), "ok");
    let request = transport.request.lock().unwrap().clone().unwrap();
    assert_eq!(request.url, "https://api.deepseek.com/v1/messages");
    assert_eq!(request.api_key, "fixture-key");
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["system"], "rules");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["stream"], true);
}

#[test]
fn messages_endpoint_appends_protocol_path_to_base_url_once() {
    for (base, expected) in [
        (
            "https://api.anthropic.com",
            "https://api.anthropic.com/v1/messages",
        ),
        (
            "https://api.anthropic.com/v1/",
            "https://api.anthropic.com/v1/messages",
        ),
        (
            "https://api.deepseek.com/anthropic",
            "https://api.deepseek.com/anthropic/v1/messages",
        ),
        (
            "https://api.deepseek.com/anthropic/v1///",
            "https://api.deepseek.com/anthropic/v1/messages",
        ),
        (
            "https://provider.test/tenant/v10",
            "https://provider.test/tenant/v10/v1/messages",
        ),
    ] {
        let mut config = ModelConfig::deepseek("synthetic").with_wire_mode(WireMode::Messages);
        config.base_url = base.into();
        assert_eq!(config.messages_endpoint(), expected);
    }
}
