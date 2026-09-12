//! Provider-neutral DeepSeek Responses API item model.
//!
//! Aligned with Codex `codex_protocol::models::ResponseItem` and the DeepSeek
//! Responses compatibility matrix. The runtime may still keep `Message` as a
//! UI/context projection, but provider requests and newly persisted protocol
//! metadata use these item semantics.

use deepagent_core::message::{Message, Role};
pub use deepagent_core::response_item::{ResponseInputItem, ResponseItem, ResponseOutputItem};
use std::collections::HashSet;

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
                if message.should_persist_reasoning() {
                    let reasoning = message.reasoning_content.as_deref().unwrap();
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

/// Repair a Responses input history before it is sent to the provider.
///
/// DeepSeek (like Codex/OpenAI Responses) requires every `function_call` or
/// `custom_tool_call` to have a matching output item.  A crash, cancelled tool
/// execution, partial event replay, or an older persisted session can leave a
/// call without its output.  Sending that dangling call makes the provider
/// reject the *next* turn with HTTP 400 and effectively poisons the session.
///
/// The repair is deliberately conservative:
/// - preserve the original item order and all valid calls/outputs;
/// - repair orphaned outputs with a neutral placeholder call and drop duplicates;
/// - synthesize one explicit failure output for each call that is still open.
///
/// This is a request-boundary guard, not a replacement for durable tool
/// completion events.  The runtime still records the real tool result whenever
/// execution reaches the tool pipeline.
pub fn repair_tool_call_pairs(items: &[ResponseInputItem]) -> Vec<ResponseInputItem> {
    let mut call_ids = HashSet::new();
    let mut call_order = Vec::new();
    let mut custom_call_ids = HashSet::new();
    let mut output_ids = HashSet::new();
    for item in items {
        match item {
            ResponseInputItem::FunctionCall { call_id, .. } => {
                if call_ids.insert(call_id.clone()) {
                    call_order.push(call_id.clone());
                }
            }
            ResponseInputItem::CustomToolCall { call_id, .. } => {
                if call_ids.insert(call_id.clone()) {
                    call_order.push(call_id.clone());
                }
                custom_call_ids.insert(call_id.clone());
            }
            _ => {}
        }
    }

    let mut repaired = Vec::with_capacity(items.len());
    for item in items {
        match item {
            ResponseInputItem::FunctionCallOutput { call_id, .. }
            | ResponseInputItem::CustomToolCallOutput { call_id, .. } => {
                // A second result for the same call is invalid.  Keep the
                // first valid result and discard the rest.  If an older or
                // partially-written history contains an output without its
                // call, synthesize a neutral call so the useful result remains
                // provider-valid instead of poisoning the next request.
                if !output_ids.insert(call_id.clone()) {
                    tracing::warn!(call_id = %call_id, "dropping orphan or duplicate Responses tool output");
                    continue;
                }
                if !call_ids.contains(call_id) {
                    tracing::warn!(call_id = %call_id, "missing Responses tool call; synthesizing placeholder call");
                    if matches!(item, ResponseInputItem::CustomToolCallOutput { .. }) {
                        repaired.push(ResponseInputItem::CustomToolCall {
                            call_id: call_id.clone(),
                            name: "apply_patch".to_string(),
                            input: String::new(),
                        });
                    } else {
                        repaired.push(ResponseInputItem::FunctionCall {
                            call_id: call_id.clone(),
                            name: "unknown_tool".to_string(),
                            arguments: "{}".to_string(),
                        });
                    }
                    call_ids.insert(call_id.clone());
                }
                repaired.push(item.clone());
            }
            _ => repaired.push(item.clone()),
        }
    }

    for call_id in call_order {
        if output_ids.contains(&call_id) {
            continue;
        }
        tracing::warn!(call_id = %call_id, "missing Responses tool output; synthesizing failure result");
        let output = r#"{"status":"error","error":"tool result missing; the previous tool execution was interrupted or its result was not persisted"}"#.to_string();
        if custom_call_ids.contains(&call_id) {
            repaired.push(ResponseInputItem::CustomToolCallOutput { call_id, output });
        } else {
            repaired.push(ResponseInputItem::FunctionCallOutput { call_id, output });
        }
    }
    repaired
}

/// Project Responses items back to the legacy [`Message`] view used by UI DTOs
/// and compatibility-only runtime paths.
///
/// The provider-native `ResponseItem` sequence remains the source of truth for
/// model input. This projection is intentionally lossy for provider-only items
/// such as `web_search_call`, mirroring the pre-existing frontend text
/// transcript boundary while keeping tool call/output pairing intact.
pub fn messages_from_response_items(
    instructions: Option<&str>,
    items: &[ResponseItem],
) -> Vec<Message> {
    let mut out = Vec::new();
    if let Some(instructions) = instructions.filter(|text| !text.is_empty()) {
        out.push(Message::system(instructions.to_string()));
    }

    let mut pending_reasoning: Option<String> = None;
    let mut pending_tool_calls: Vec<deepagent_core::message::ToolCall> = Vec::new();

    let flush_tool_calls = |calls: &mut Vec<deepagent_core::message::ToolCall>,
                            out: &mut Vec<Message>| {
        if !calls.is_empty() {
            out.push(Message::assistant("").with_tool_calls(std::mem::take(calls)));
        }
    };

    for item in items {
        match item {
            ResponseItem::Reasoning { content, .. } => {
                pending_reasoning = (!content.is_empty()).then(|| content.clone());
            }
            ResponseItem::Message { role, content } => {
                flush_tool_calls(&mut pending_tool_calls, &mut out);
                let role = match role.as_str() {
                    "system" => Role::System,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    _ => Role::User,
                };
                let mut message = Message::text(role, content.clone());
                if role == Role::Assistant {
                    message.reasoning_content = pending_reasoning.take();
                }
                out.push(message);
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                let arguments = serde_json::from_str(arguments).unwrap_or_else(|error| {
                    serde_json::json!({
                        "__invalid_tool_arguments__": true,
                        "raw": arguments,
                        "parse_error": error.to_string()
                    })
                });
                pending_tool_calls.push(deepagent_core::message::ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments,
                });
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
            } => {
                pending_tool_calls.push(deepagent_core::message::ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: serde_json::json!({ "patch": input }),
                });
            }
            ResponseItem::FunctionCallOutput { call_id, output }
            | ResponseItem::CustomToolCallOutput { call_id, output } => {
                flush_tool_calls(&mut pending_tool_calls, &mut out);
                out.push(Message::tool_result(call_id.clone(), output.clone()));
            }
            ResponseItem::WebSearchCall { .. } => {
                flush_tool_calls(&mut pending_tool_calls, &mut out);
            }
        }
    }
    flush_tool_calls(&mut pending_tool_calls, &mut out);
    out
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
    fn repairs_missing_tool_outputs_without_poisoning_next_request() {
        let items = vec![
            ResponseItem::FunctionCall {
                call_id: "call-missing".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-orphan".into(),
                output: "old".into(),
            },
        ];
        let repaired = repair_tool_call_pairs(&items);
        assert!(repaired.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCallOutput { call_id, output }
                if call_id == "call-missing" && output.contains("tool result missing")
        )));
        assert!(repaired.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCall { call_id, name, .. }
                if call_id == "call-orphan" && name == "unknown_tool"
        )));
    }

    #[test]
    fn repairs_custom_tool_with_custom_output_type() {
        let items = vec![ResponseItem::CustomToolCall {
            call_id: "call-patch".into(),
            name: "apply_patch".into(),
            input: "diff".into(),
        }];
        let repaired = repair_tool_call_pairs(&items);
        assert!(repaired.iter().any(|item| matches!(
            item,
            ResponseItem::CustomToolCallOutput { call_id, .. } if call_id == "call-patch"
        )));
    }

    #[test]
    fn drops_plain_reasoning_from_provider_input() {
        let messages = vec![Message::assistant("hello").with_reasoning("internal")];
        let (_, items) = response_items_from_messages(&messages);
        assert!(!items
            .iter()
            .any(|item| matches!(item, ResponseItem::Reasoning { .. })));
        assert!(matches!(
            &items[0],
            ResponseItem::Message { role, content } if role == "assistant" && content == "hello"
        ));
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

    #[test]
    fn projects_response_items_back_to_messages_for_compatibility() {
        let items = vec![
            ResponseItem::Reasoning {
                id: Some("r1".into()),
                content: "thinking".into(),
            },
            ResponseItem::FunctionCall {
                call_id: "call-1".into(),
                name: "weather".into(),
                arguments: r#"{"city":"Beijing"}"#.into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".into(),
                output: r#"{"ok":true}"#.into(),
            },
            ResponseItem::Message {
                role: "assistant".into(),
                content: "done".into(),
            },
        ];

        let messages = messages_from_response_items(Some("rules"), &items);

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].tool_calls[0].id, "call-1");
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(messages[3].content, "done");
        assert_eq!(messages[3].reasoning_content.as_deref(), Some("thinking"));
    }
}
