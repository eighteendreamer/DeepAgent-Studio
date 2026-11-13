use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{
    message::{Message, ToolCall},
    CoreError, Result,
};
use deepagent_models::{
    transport::EventSink, HttpTransport, ModelClient, ModelConfig, ResponseInputItem,
    ResponseRequest, ToolSchema, TransportRequest, WireMode,
};
use serde_json::{json, Value};

fn function() -> ToolSchema {
    ToolSchema::function(
        "apply_patch",
        "synthetic extension, not a native patch tool",
        json!({"type":"object","properties":{"value":{"type":"integer"}},"required":["value"]}),
    )
}

fn request(custom: bool) -> ResponseRequest {
    let (call, output) = if custom {
        (
            ResponseInputItem::CustomToolCall {
                call_id: "same-name".into(),
                name: "apply_patch".into(),
                input: "synthetic patch".into(),
            },
            ResponseInputItem::CustomToolCallOutput {
                call_id: "same-name".into(),
                output: "synthetic result".into(),
            },
        )
    } else {
        (
            ResponseInputItem::FunctionCall {
                call_id: "same-name".into(),
                name: "apply_patch".into(),
                arguments: r#"{"value":7}"#.into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "same-name".into(),
                output: "synthetic result".into(),
            },
        )
    };
    let mut schema = function();
    if custom {
        schema.kind = "custom".into();
    }
    ResponseRequest::from_response_items(
        "deepseek-flash",
        Some("fixture".into()),
        vec![call, output],
    )
    .with_tools(vec![schema])
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("synthetic-dispatched"))
    }
}

#[test]
fn h156_message_function_preserves_all_arguments_instead_of_guessing_custom() {
    for arguments in [json!({"value":7}), json!({"patch":"text","extra":true})] {
        let messages = vec![
            Message::assistant("").with_tool_calls(vec![ToolCall {
                id: "same-name".into(),
                name: "apply_patch".into(),
                arguments: arguments.clone(),
            }]),
            Message::tool_result("same-name", "synthetic result"),
        ];
        let (_, items) = deepagent_models::response_items_from_messages(&messages);
        assert!(
            matches!(&items[0], ResponseInputItem::FunctionCall { arguments: raw, .. }
            if serde_json::from_str::<Value>(raw).unwrap() == arguments)
        );
        assert!(matches!(
            &items[1],
            ResponseInputItem::FunctionCallOutput { .. }
        ));
        let roundtrip = deepagent_models::messages_from_response_items(None, &items);
        assert_eq!(roundtrip, messages);
    }
}

#[tokio::test]
async fn h156_explicit_tool_kind_is_preserved_on_both_client_entries_and_wire_modes() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for custom in [false, true] {
            for cancel_entry in [false, true] {
                let transport = Arc::new(Recorder::default());
                let mut config = ModelConfig::deepseek("synthetic");
                config.wire_mode = mode;
                let client = ModelClient::new(transport.clone(), config);
                let mut request = request(custom);
                if !custom && mode == WireMode::Responses {
                    request.tool_choice = Some(json!({"type":"function","name":"apply_patch"}));
                }
                let original = request.clone();
                let schemas = client.project_tool_schemas(&request.model, &request.tools);
                let result = if cancel_entry {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                if custom && mode == WireMode::ChatCompletions {
                    assert!(matches!(result, Err(CoreError::Invalid(_))));
                    assert!(transport.0.lock().unwrap().is_empty());
                    continue; // Chat has no native custom declaration; do not expand that protocol.
                }
                assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
                let wire = transport.0.lock().unwrap();
                assert_eq!(wire.len(), 1);
                assert_eq!(schemas.tools, wire[0]["tools"].as_array().unwrap().clone());
                if mode == WireMode::Responses {
                    let expected = if custom {
                        json!({"type":"custom","name":"apply_patch"})
                    } else {
                        json!({"type":"function","name":"apply_patch","description":function().function.description,"parameters":function().function.parameters})
                    };
                    assert_eq!(wire[0]["tools"][0], expected);
                    assert_eq!(
                        wire[0]["input"][0]["type"],
                        if custom {
                            "custom_tool_call"
                        } else {
                            "function_call"
                        }
                    );
                    assert_eq!(
                        wire[0]["input"][1]["type"],
                        if custom {
                            "custom_tool_call_output"
                        } else {
                            "function_call_output"
                        }
                    );
                    assert_eq!(
                        serde_json::to_value(original).unwrap()["tools"][0],
                        expected
                    );
                } else {
                    assert_eq!(wire[0]["tools"][0]["type"], "function");
                    let args: Value = serde_json::from_str(
                        wire[0]["messages"][1]["tool_calls"][0]["function"]["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        args,
                        if custom {
                            json!({"patch":"synthetic patch"})
                        } else {
                            json!({"value":7})
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn h156_custom_wire_is_minimal_and_cannot_collide_with_a_function() {
    let valid = request(true);
    assert_eq!(
        serde_json::to_value(&valid).unwrap()["tools"][0],
        json!({"type":"custom","name":"apply_patch"})
    );
    let mut invalid = valid.clone();
    invalid.tools[0].function.name = "unsupported-custom".into();
    assert!(invalid.validate().is_err());
    assert!(serde_json::to_value(invalid).is_err());
    let mut duplicate = valid;
    duplicate.tools.push(function());
    assert!(duplicate.validate().is_err());
    assert!(serde_json::to_value(duplicate).is_err());
}
