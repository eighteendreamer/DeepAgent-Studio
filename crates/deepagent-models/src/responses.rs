//! Provider-neutral DeepSeek Responses API item model.
//!
//! Aligned with Codex `codex_protocol::models::ResponseItem` and the DeepSeek
//! Responses compatibility matrix. The runtime may still keep `Message` as a
//! UI/context projection, but provider requests and newly persisted protocol
//! metadata use these item semantics.

use deepagent_core::message::{Message, Role};
pub use deepagent_core::response_item::{ResponseInputItem, ResponseItem, ResponseOutputItem};

/// Convert the context projection into ordered Responses input items.
pub fn response_items_from_messages(messages: &[Message]) -> (Option<String>, Vec<ResponseItem>) {
    let mut instructions = Vec::new();
    let mut items = Vec::new();
    let custom_call_ids: std::collections::HashSet<&str> = messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .filter(|call| call.name == "apply_patch")
        .map(|call| call.id.as_str())
        .collect();
    for message in messages {
        match message.role {
            Role::System => instructions.push(message.content.clone()),
            Role::Tool => {
                let call_id = message.tool_call_id.clone().unwrap_or_default();
                if custom_call_ids.contains(call_id.as_str()) {
                    items.push(ResponseItem::CustomToolCallOutput {
                        call_id,
                        output: message.content.clone(),
                    });
                } else {
                    items.push(ResponseItem::FunctionCallOutput {
                        call_id,
                        output: message.content.clone(),
                    });
                }
            }
            _ => {
                if let Some(reasoning) = message
                    .reasoning_content
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
                    items.push(ResponseItem::Reasoning {
                        id: None,
                        content: reasoning.to_string(),
                    });
                }
                for call in &message.tool_calls {
                    if call.name == "apply_patch" {
                        items.push(ResponseItem::CustomToolCall {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            input: call
                                .arguments
                                .get("patch")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                    } else {
                        let arguments = serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string());
                        items.push(ResponseItem::FunctionCall {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            arguments,
                        });
                    }
                }
                if !message.content.is_empty() || message.tool_calls.is_empty() {
                    items.push(ResponseItem::Message {
                        role: message.role.as_str().to_string(),
                        content: message.content.clone(),
                    });
                }
            }
        }
    }
    let instructions = (!instructions.is_empty()).then(|| instructions.join("\n\n"));
    (instructions, items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::message::ToolCall;

    #[test]
    fn maps_tool_roundtrip_to_responses_items() {
        let messages = vec![
            Message::system("rules"),
            Message::assistant("")
                .with_reasoning("thinking")
                .with_tool_calls(vec![ToolCall {
                    id: "call-1".into(),
                    name: "weather".into(),
                    arguments: serde_json::json!({"city":"Beijing"}),
                }]),
            Message::tool_result("call-1", r#"{"ok":true}"#),
        ];
        let (instructions, items) = response_items_from_messages(&messages);
        assert_eq!(instructions.as_deref(), Some("rules"));
        assert!(
            matches!(&items[0], ResponseItem::Reasoning { content, .. } if content == "thinking")
        );
        assert!(
            matches!(&items[1], ResponseItem::FunctionCall { call_id, arguments, .. } if call_id == "call-1" && arguments.contains("Beijing"))
        );
        assert!(
            matches!(&items[2], ResponseItem::FunctionCallOutput { call_id, .. } if call_id == "call-1")
        );
    }

    #[test]
    fn maps_apply_patch_roundtrip_to_custom_tool_items() {
        let patch = "*** Begin Patch\n*** End Patch";
        let messages = vec![
            Message::assistant("").with_tool_calls(vec![ToolCall {
                id: "call-patch".into(),
                name: "apply_patch".into(),
                arguments: serde_json::json!({"patch": patch}),
            }]),
            Message::tool_result("call-patch", "patch applied"),
        ];

        let (_, items) = response_items_from_messages(&messages);

        assert!(matches!(
            &items[0],
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
            } if call_id == "call-patch" && name == "apply_patch" && input == patch
        ));
        assert!(matches!(
            &items[1],
            ResponseItem::CustomToolCallOutput { call_id, output }
                if call_id == "call-patch" && output == "patch applied"
        ));
    }
}
