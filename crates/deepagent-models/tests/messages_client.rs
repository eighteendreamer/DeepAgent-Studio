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

#[tokio::test]
async fn messages_cache_marks_stable_sections_and_completed_history_only() {
    let transport = Arc::new(RecordingTransport {
        events: events(),
        ..Default::default()
    });
    let mut config = ModelConfig::deepseek("fixture-key").with_wire_mode(WireMode::Messages);
    config.base_url = "https://api.anthropic.com".into();
    let client = ModelClient::new(transport.clone(), config);
    let tools = vec![
        deepagent_models::ToolSchema::function("z_tool", "last", json!({"type":"object"})),
        deepagent_models::ToolSchema::function("a_tool", "first", json!({"type":"object"})),
    ];
    let request = ResponseRequest::from_response_items(
        "claude-fixture",
        Some("stable rules".into()),
        vec![
            ResponseItem::Message {
                role: "user".into(),
                content: "earlier question".into(),
            },
            ResponseItem::Message {
                role: "assistant".into(),
                content: "earlier answer".into(),
            },
            ResponseItem::Message {
                role: "user".into(),
                content: "current question".into(),
            },
        ],
    )
    .with_max_output_tokens(128)
    .with_tools(tools.clone());
    client.stream_response(request).await.unwrap();
    let recorded = transport.request.lock().unwrap().clone().unwrap();
    let body: Value = serde_json::from_str(&recorded.body).unwrap();
    assert_eq!(body["system"][0]["text"], "stable rules");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["tools"][0]["name"], "a_tool");
    assert!(body["tools"][0].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        body["messages"][1]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    assert!(body["messages"][2]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(recorded.body.matches("\"cache_control\"").count(), 3);
    assert_eq!(
        json!(client.project_tool_schemas("claude-fixture", &tools).tools),
        body["tools"]
    );
}

#[tokio::test]
async fn messages_cache_skips_thinking_only_history_and_deepseek_auto_cache() {
    for base in [
        "https://api.anthropic.com",
        "https://api.deepseek.com/anthropic",
    ] {
        let transport = Arc::new(RecordingTransport {
            events: events(),
            ..Default::default()
        });
        let mut config = ModelConfig::deepseek("fixture-key").with_wire_mode(WireMode::Messages);
        config.base_url = base.into();
        let client = ModelClient::new(transport.clone(), config);
        let request = ResponseRequest::from_response_items(
            "claude-fixture",
            Some("rules".into()),
            vec![
                ResponseItem::Message {
                    role: "user".into(),
                    content: "earlier".into(),
                },
                ResponseItem::Reasoning {
                    id: None,
                    content: "reasoning".into(),
                    signature: Some("sig".into()),
                },
                ResponseItem::Message {
                    role: "user".into(),
                    content: "current".into(),
                },
            ],
        )
        .with_max_output_tokens(128);
        client.stream_response(request).await.unwrap();
        let recorded = transport.request.lock().unwrap().clone().unwrap();
        let body: Value = serde_json::from_str(&recorded.body).unwrap();
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][2]["content"][0]
            .get("cache_control")
            .is_none());
        if base.contains("deepseek.com") {
            assert!(!recorded.body.contains("\"cache_control\""));
            assert_eq!(body["system"], "rules");
        } else {
            assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
            assert_eq!(recorded.body.matches("\"cache_control\"").count(), 1);
        }
    }
}

#[tokio::test]
async fn messages_cache_keeps_stable_instructions_separate_from_runtime_system_items() {
    let mut stable_block = None;
    for dynamic in ["environment one", "environment two"] {
        let transport = Arc::new(RecordingTransport {
            events: events(),
            ..Default::default()
        });
        let mut config = ModelConfig::deepseek("fixture-key").with_wire_mode(WireMode::Messages);
        config.base_url = "https://api.anthropic.com".into();
        let client = ModelClient::new(transport.clone(), config);
        let request = ResponseRequest::from_response_items(
            "claude-fixture",
            Some("stable rules".into()),
            vec![
                ResponseItem::Message {
                    role: "user".into(),
                    content: "hello".into(),
                },
                ResponseItem::Message {
                    role: "system".into(),
                    content: dynamic.into(),
                },
            ],
        )
        .with_max_output_tokens(128);
        client.stream_response(request).await.unwrap();
        let recorded = transport.request.lock().unwrap().clone().unwrap();
        let body: Value = serde_json::from_str(&recorded.body).unwrap();
        assert_eq!(body["system"][0]["text"], "stable rules");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["system"][1]["text"], dynamic);
        assert!(body["system"][1].get("cache_control").is_none());
        if let Some(first) = &stable_block {
            assert_eq!(first, &body["system"][0]);
        } else {
            stable_block = Some(body["system"][0].clone());
        }
        assert_eq!(recorded.body.matches("\"cache_control\"").count(), 1);
    }
}

#[tokio::test]
async fn messages_cache_preserves_thinking_signature_and_tool_result_pair() {
    let transport = Arc::new(RecordingTransport {
        events: events(),
        ..Default::default()
    });
    let mut config = ModelConfig::deepseek("fixture-key").with_wire_mode(WireMode::Messages);
    config.base_url = "https://api.anthropic.com".into();
    let client = ModelClient::new(transport.clone(), config);
    let request = ResponseRequest::from_response_items(
        "claude-fixture",
        Some("rules".into()),
        vec![
            ResponseItem::Message {
                role: "user".into(),
                content: "read".into(),
            },
            ResponseItem::Reasoning {
                id: None,
                content: "thinking".into(),
                signature: Some("signature".into()),
            },
            ResponseItem::FunctionCall {
                call_id: "call-1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".into(),
                output: deepagent_core::tool_content::ToolResultBody::Text("result".into()),
            },
            ResponseItem::Message {
                role: "user".into(),
                content: "continue".into(),
            },
        ],
    )
    .with_max_output_tokens(128);
    client.stream_response(request).await.unwrap();
    let recorded = transport.request.lock().unwrap().clone().unwrap();
    let body: Value = serde_json::from_str(&recorded.body).unwrap();
    let assistant = &body["messages"][1]["content"];
    assert_eq!(assistant[0]["type"], "thinking");
    assert_eq!(assistant[0]["signature"], "signature");
    assert!(assistant[0].get("cache_control").is_none());
    assert_eq!(assistant[1]["type"], "tool_use");
    assert_eq!(assistant[1]["id"], "call-1");
    assert_eq!(assistant[1]["cache_control"]["type"], "ephemeral");
    let user = &body["messages"][2]["content"];
    assert_eq!(user[0]["type"], "tool_result");
    assert_eq!(user[0]["tool_use_id"], "call-1");
    assert_eq!(user[1]["text"], "continue");
    assert!(user[0].get("cache_control").is_none());
    assert!(user[1].get("cache_control").is_none());
    assert_eq!(recorded.body.matches("\"cache_control\"").count(), 2);
}
