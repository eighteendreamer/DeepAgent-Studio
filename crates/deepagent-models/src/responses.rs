//! Provider-neutral DeepSeek Responses API item model.
//!
//! Aligned with Codex `codex_protocol::models::ResponseItem` and the DeepSeek
//! Responses compatibility matrix. The runtime may still keep `Message` as a
//! UI/context projection, but provider requests and newly persisted protocol
//! metadata use these item semantics.

use deepagent_core::message::{Message, Role};
pub use deepagent_core::response_item::{
    ResponseInputItem, ResponseItem, ResponseOutputItem, ToolSearchExecution,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Guard identities before pairing repair and again at dispatch/public edits.
/// Calls and outputs each need globally unique ids across native tool kinds;
/// one call and its output may share an id. Never erase an ambiguous execution
/// history, invent replacement identities, or disclose its contents in errors.
pub(crate) fn validate_tool_call_ids(
    items: &[ResponseInputItem],
) -> deepagent_core::error::Result<()> {
    let mut call_ids = HashMap::new();
    let mut output_ids = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let (call_id, kind, seen) = match item {
            ResponseInputItem::FunctionCall { call_id, .. } => {
                (call_id.as_str(), "function_call", &mut call_ids)
            }
            ResponseInputItem::FunctionCallOutput { call_id, .. } => {
                (call_id.as_str(), "function_call_output", &mut output_ids)
            }
            ResponseInputItem::CustomToolCall { call_id, .. } => {
                (call_id.as_str(), "custom_tool_call", &mut call_ids)
            }
            ResponseInputItem::CustomToolCallOutput { call_id, .. } => {
                (call_id.as_str(), "custom_tool_call_output", &mut output_ids)
            }
            ResponseInputItem::ToolSearchCall {
                call_id,
                execution: ToolSearchExecution::Client,
                ..
            } => (
                call_id.as_deref().unwrap_or_default(),
                "tool_search_call",
                &mut call_ids,
            ),
            ResponseInputItem::ToolSearchOutput {
                call_id,
                execution: ToolSearchExecution::Client,
                ..
            } => (
                call_id.as_deref().unwrap_or_default(),
                "tool_search_output",
                &mut output_ids,
            ),
            _ => continue,
        };
        if call_id.trim().is_empty() {
            return Err(deepagent_core::error::CoreError::invalid(format!(
                "Responses input item {index} ({kind}) requires a non-empty call_id"
            )));
        }
        if let Some(previous) = seen.insert(call_id, index) {
            return Err(deepagent_core::error::CoreError::invalid(format!(
                "Responses input item {index} ({kind}) repeats call_id from item {previous}"
            )));
        }
    }
    Ok(())
}

/// Correct only output variants for unique, trustworthy call identities.
/// Keep original order and payload bytes for the Chat projection; pairing
/// repair may subsequently move items for the Responses wire. Ambiguous ids
/// remain errors, never a reason to select one of several executions.
pub(crate) fn normalize_tool_output_kinds(
    items: &[ResponseInputItem],
) -> deepagent_core::error::Result<Cow<'_, [ResponseInputItem]>> {
    validate_tool_call_ids(items)?;
    let calls: HashMap<_, _> = items
        .iter()
        .filter_map(|item| item.client_tool_call_id().map(|id| (id, item)))
        .collect();
    let mut normalized = Cow::Borrowed(items);
    for (index, item) in items.iter().enumerate() {
        if let Some(call) = item
            .client_tool_output_call_id()
            .and_then(|id| calls.get(id))
        {
            if matches!(call, ResponseInputItem::ToolSearchCall { .. })
                != matches!(item, ResponseInputItem::ToolSearchOutput { .. })
            {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "Responses input item {index} has a mismatched native discovery output kind"
                )));
            }
        }
        let (call_id, output, custom) = match item {
            ResponseInputItem::FunctionCallOutput { call_id, output } => (call_id, output, false),
            ResponseInputItem::CustomToolCallOutput { call_id, output } => (call_id, output, true),
            _ => continue,
        };
        let Some(call) = calls.get(call_id.as_str()) else {
            continue;
        };
        let call_custom = matches!(call, ResponseInputItem::CustomToolCall { .. });
        if call_custom != custom {
            normalized.to_mut()[index] = if call_custom {
                ResponseInputItem::CustomToolCallOutput {
                    call_id: call_id.clone(),
                    output: output.clone(),
                }
            } else {
                ResponseInputItem::FunctionCallOutput {
                    call_id: call_id.clone(),
                    output: output.clone(),
                }
            };
            tracing::debug!(
                item_index = index,
                "Responses tool output kind differs from its call"
            );
        }
    }
    Ok(normalized)
}

/// A public edit after construction must not bypass native-kind repair.
/// Use the same normalization rule without mutating authoritative input.
pub(crate) fn validate_tool_call_history(
    items: &[ResponseInputItem],
) -> deepagent_core::error::Result<()> {
    let normalized = normalize_tool_output_kinds(items)?;
    if let Some(index) = items
        .iter()
        .zip(normalized.iter())
        .position(|(a, b)| a != b)
    {
        let kind = match &items[index] {
            ResponseInputItem::FunctionCallOutput { .. } => "function_call_output",
            ResponseInputItem::CustomToolCallOutput { .. } => "custom_tool_call_output",
            _ => unreachable!("normalization only changes output variants"),
        };
        return Err(deepagent_core::error::CoreError::invalid(format!(
            "Responses input item {index} ({kind}) has a call_id kind that does not match its call"
        )));
    }
    Ok(())
}

/// Convert the context projection into ordered Responses input items.
pub fn response_items_from_messages(messages: &[Message]) -> (Option<String>, Vec<ResponseItem>) {
    project_messages(messages, ResponseItem::from_function_call)
}

/// Rebuild a legacy compactor's message view without losing surviving native
/// call identities. Changed/new calls remain functions; only an exact mirror
/// can inherit its existing native type, raw input and argument spelling.
pub fn response_items_from_messages_with_native_calls(
    messages: &[Message],
    native: &[ResponseItem],
) -> (Option<String>, Vec<ResponseItem>) {
    project_messages(messages, |call| {
        native
            .iter()
            .find(|item| item.is_tool_call_projection_of(call))
            .cloned()
            .unwrap_or_else(|| ResponseItem::from_function_call(call))
    })
}

/// Read-only compatibility for old event logs with no native call types.
/// New programmatic custom callers must use explicit ResponseItems instead.
pub fn response_items_from_legacy_messages(
    messages: &[Message],
) -> (Option<String>, Vec<ResponseItem>) {
    project_messages(messages, ResponseItem::from_legacy_tool_call)
}

fn project_messages(
    messages: &[Message],
    call_item: impl Fn(&deepagent_core::message::ToolCall) -> ResponseItem,
) -> (Option<String>, Vec<ResponseItem>) {
    let mut instructions = Vec::new();
    let mut items = Vec::new();
    let custom_call_ids: std::collections::HashSet<&str> = messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .filter(|call| matches!(call_item(call), ResponseItem::CustomToolCall { .. }))
        .map(|call| call.id.as_str())
        .collect();
    for message in messages {
        match message.role {
            Role::System
                if items.is_empty()
                    && !deepagent_core::runtime_snapshot::RuntimeSnapshot::has_envelope_marker(
                        &ResponseItem::from_message_content(message),
                    ) =>
            {
                instructions.push(message.content.clone())
            }
            Role::Tool => {
                let call_id = message.tool_call_id.clone().unwrap_or_default();
                if custom_call_ids.contains(call_id.as_str()) {
                    items.push(ResponseItem::CustomToolCallOutput {
                        call_id,
                        output: deepagent_core::tool_content::ToolResultBody::from_message(message),
                    });
                } else {
                    items.push(ResponseItem::FunctionCallOutput {
                        call_id,
                        output: deepagent_core::tool_content::ToolResultBody::from_message(message),
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
                items.extend(message.tool_calls.iter().map(&call_item));
                if !message.content.is_empty() || message.tool_calls.is_empty() {
                    items.push(ResponseItem::from_message_content(message));
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
/// - preserve each contiguous group of provider calls before its outputs;
/// - preserve invalid/ambiguous input unchanged for dispatch validation;
/// - match each unique call's native output kind without changing its payload;
/// - repair unique orphaned outputs with a neutral placeholder call;
/// - synthesize one explicit failure output for each call that is still open.
///
/// This is a request-boundary guard, not a replacement for durable tool
/// completion events.  The runtime still records the real tool result whenever
/// execution reaches the tool pipeline.
pub fn repair_tool_call_pairs(items: &[ResponseInputItem]) -> Vec<ResponseInputItem> {
    // Constructors cannot return an error without changing their public API.
    // Keep every conflicting item so the shared dispatch validator can reject
    // it; silently retaining the first pair would hide executed work/results.
    let Ok(normalized) = normalize_tool_output_kinds(items) else {
        return items.to_vec();
    };
    let items = normalized.as_ref();
    let mut call_ids = HashSet::new();
    let mut custom_call_ids = HashSet::new();
    let mut search_call_ids = HashSet::new();
    let mut outputs = std::collections::HashMap::<String, ResponseInputItem>::new();
    let mut orphan_outputs = Vec::new();

    // Classify against the whole history, not just calls seen so far. A real
    // output can precede its call after import/replay and is not an orphan.
    // Identity validation above makes each association unambiguous.
    for item in items {
        if let Some(id) = item.client_tool_call_id() {
            call_ids.insert(id.to_string());
        }
        match item {
            ResponseInputItem::CustomToolCall { call_id, .. } => {
                custom_call_ids.insert(call_id.clone());
            }
            ResponseInputItem::ToolSearchCall {
                execution: ToolSearchExecution::Client,
                call_id: Some(id),
                ..
            } => {
                search_call_ids.insert(id.clone());
            }
            _ => {}
        }
    }
    // Reconstruction below retains each contiguous provider call group and
    // emits its real results after the group, regardless of input result order.
    for item in items {
        if let Some(call_id) = item.client_tool_output_call_id() {
            if call_ids.contains(call_id) {
                outputs.insert(call_id.to_string(), item.clone());
            } else {
                orphan_outputs.push(item.clone());
            }
        }
    }

    let mut repaired = Vec::with_capacity(items.len() + orphan_outputs.len());
    let mut emitted_outputs = HashSet::new();
    let mut pending_group = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(call_id) = item.client_tool_call_id() {
            repaired.push(item.clone());
            pending_group.push(call_id.to_string());
        } else if let Some(call_id) = item.client_tool_output_call_id() {
            // Outputs are emitted after their call group. An output that
            // has no call gets a placeholder pair below.
            if !emitted_outputs.contains(call_id) {
                tracing::debug!(call_id = %call_id, "deferring Responses tool output until its call");
            }
        } else {
            repaired.push(item.clone());
        }
        let next_is_call = items
            .get(index + 1)
            .is_some_and(|next| next.client_tool_call_id().is_some());
        if !next_is_call {
            for call_id in pending_group.drain(..) {
                if let Some(output) = outputs.get(&call_id) {
                    repaired.push(output.clone());
                } else {
                    tracing::warn!(call_id = %call_id, "missing Responses tool output; synthesizing failure result");
                    let output = r#"{"status":"error","error":"tool result missing; the previous tool execution was interrupted or its result was not persisted"}"#.to_string();
                    if search_call_ids.contains(&call_id) {
                        repaired.push(ResponseInputItem::ToolSearchOutput {
                            id: None,
                            call_id: Some(call_id.clone()),
                            status: "failed".into(),
                            execution: ToolSearchExecution::Client,
                            tools: Vec::new(),
                        });
                    } else if custom_call_ids.contains(&call_id) {
                        repaired.push(ResponseInputItem::CustomToolCallOutput {
                            call_id: call_id.clone(),
                            output: output.into(),
                        });
                    } else {
                        repaired.push(ResponseInputItem::FunctionCallOutput {
                            call_id: call_id.clone(),
                            output: output.into(),
                        });
                    }
                }
                emitted_outputs.insert(call_id);
            }
        }
    }

    for item in orphan_outputs {
        let call_id = item
            .client_tool_output_call_id()
            .expect("only client outputs are classified")
            .to_string();
        if !emitted_outputs.insert(call_id.clone()) {
            continue;
        }
        tracing::warn!(call_id = %call_id, "missing Responses tool call; synthesizing placeholder call");
        if matches!(item, ResponseInputItem::ToolSearchOutput { .. }) {
            repaired.push(ResponseInputItem::ToolSearchCall {
                id: None,
                call_id: Some(call_id.clone()),
                status: Some("completed".into()),
                execution: ToolSearchExecution::Client,
                arguments: serde_json::json!({}),
            });
        } else if matches!(item, ResponseInputItem::CustomToolCallOutput { .. }) {
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
        repaired.push(item);
    }
    repaired
}

/// Whether an un-repaired history can be projected to Chat Completions
/// without losing the provider's tool-call invariant.
///
/// The Responses repair pass is allowed to move outputs next to calls.  The
/// legacy Chat projection must instead see the original order so an assistant
/// narration item can be folded into the same assistant message.  We only use
/// that order when every call has exactly one later, matching-kind output and
/// there are no orphan/duplicate ids; malformed histories fall back to the
/// repaired sequence at the request boundary.
pub(crate) fn chat_projection_history_is_safe(items: &[ResponseInputItem]) -> bool {
    if validate_tool_call_history(items).is_err() {
        return false;
    }
    let mut pending_calls = HashSet::new();

    for item in items {
        if let Some(id) = item.client_tool_call_id() {
            pending_calls.insert(id);
        } else if let Some(id) = item.client_tool_output_call_id() {
            if !pending_calls.remove(id) {
                return false;
            }
        }
    }

    pending_calls.is_empty()
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
                            out: &mut Vec<Message>,
                            reasoning: &mut Option<String>| {
        if !calls.is_empty() {
            let mut message = Message::assistant("").with_tool_calls(std::mem::take(calls));
            message.reasoning_content = reasoning.take();
            out.push(message);
        }
    };

    for item in items {
        match item {
            ResponseItem::Reasoning { content, .. } => {
                if !content.is_empty() {
                    match pending_reasoning.as_mut() {
                        Some(reasoning) => reasoning.push_str(content),
                        None => pending_reasoning = Some(content.clone()),
                    }
                }
            }
            ResponseItem::Message { role, content } => {
                let role = match role.as_str() {
                    "system" => Role::System,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    _ => Role::User,
                };
                if role == Role::Assistant && !pending_tool_calls.is_empty() {
                    let mut message = Message::assistant(content.clone())
                        .with_tool_calls(std::mem::take(&mut pending_tool_calls));
                    message.reasoning_content = pending_reasoning.take();
                    out.push(message);
                    continue;
                }
                flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
                let mut message = Message::text(role, content.clone());
                if role == Role::Assistant {
                    message.reasoning_content = pending_reasoning.take();
                }
                out.push(message);
            }
            ResponseItem::InputMessage {
                role,
                content,
                attachments,
            } => {
                let role = match role.as_str() {
                    "system" => Role::System,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    _ => Role::User,
                };
                if role == Role::Assistant && !pending_tool_calls.is_empty() {
                    let mut message = Message::assistant(content.clone())
                        .with_tool_calls(std::mem::take(&mut pending_tool_calls));
                    message.reasoning_content = pending_reasoning.take();
                    message.attachments = attachments.clone();
                    out.push(message);
                    continue;
                }
                flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
                let mut message = Message::text(role, content.clone());
                if role == Role::Assistant {
                    message.reasoning_content = pending_reasoning.take();
                }
                message.attachments = attachments.clone();
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
                flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
                out.push(
                    Message::tool_result(call_id.clone(), output.text().into_owned())
                        .with_attachments(output.attachments().cloned().collect()),
                );
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution: ToolSearchExecution::Client,
                arguments,
                status,
                ..
            } if status.as_deref().is_none_or(|status| status == "completed") => {
                pending_tool_calls.push(deepagent_core::message::ToolCall {
                    id: call_id.clone(),
                    name: "tool_search".into(),
                    arguments: arguments.clone(),
                });
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                execution: ToolSearchExecution::Client,
                tools,
                status,
                ..
            } => {
                flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
                out.push(Message::tool_result(
                    call_id.clone(),
                    serde_json::json!({"status":status,"tools":tools}).to_string(),
                ));
            }
            ResponseItem::WebSearchCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. } => {
                flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
            }
        }
    }
    flush_tool_calls(&mut pending_tool_calls, &mut out, &mut pending_reasoning);
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
    fn audit_preserves_mid_history_system_messages_as_input_items() {
        let messages = vec![
            Message::system("stable leading rules"),
            Message::user("first question"),
            Message::assistant("first answer"),
            Message::system("updated runtime context"),
            Message::user("second question"),
        ];

        let (instructions, items) = response_items_from_messages(&messages);
        let request = crate::chat::ResponseRequest::new("deepseek-flash", messages.clone());
        let wire = serde_json::to_value(&request).expect("serialize Responses request");
        eprintln!(
            "mid-history system audit: instructions={instructions:?}, items={items:?}, wire={wire}"
        );

        assert_eq!(instructions.as_deref(), Some("stable leading rules"));
        assert_eq!(
            items,
            messages[1..]
                .iter()
                .map(ResponseItem::from_message_content)
                .collect::<Vec<_>>()
        );
        assert_eq!(wire["instructions"], "stable leading rules");
        assert_eq!(wire["input"], serde_json::to_value(&items).unwrap());
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
                if call_id == "call-missing" && output.text().contains("tool result missing")
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
    fn audit_repairs_mismatched_custom_output_kind_before_serialization() {
        let items = vec![
            ResponseItem::CustomToolCall {
                call_id: "call-patch".into(),
                name: "apply_patch".into(),
                input: "diff".into(),
            },
            // Deliberately malformed public-API input. This probe does not
            // establish that a production replay produces mixed output kinds.
            ResponseItem::FunctionCallOutput {
                call_id: "call-patch".into(),
                output: "patch applied".into(),
            },
        ];
        let request =
            crate::chat::ResponseRequest::from_response_items("deepseek-v4-flash", None, items);
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(
            wire["input"][1]["type"], "custom_tool_call_output",
            "the output kind must match the preceding custom_tool_call"
        );
    }

    #[test]
    fn reorders_interleaved_items_to_deepseek_adjacent_pairs() {
        let items = vec![
            ResponseItem::FunctionCall {
                call_id: "call-1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            },
            ResponseItem::Message {
                role: "user".into(),
                content: "hook context".into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".into(),
                output: "ok".into(),
            },
        ];
        let repaired = repair_tool_call_pairs(&items);
        assert!(matches!(
            (&repaired[0], &repaired[1]),
            (
                ResponseItem::FunctionCall { call_id: a, .. },
                ResponseItem::FunctionCallOutput { call_id: b, .. }
            ) if a == "call-1" && b == "call-1"
        ));
        assert!(matches!(
            &repaired[2],
            ResponseItem::Message { content, .. } if content == "hook context"
        ));
    }

    #[test]
    fn keeps_parallel_calls_together_before_their_outputs() {
        let items = vec![
            ResponseItem::Reasoning {
                id: Some("rs_1".into()),
                content: "call both tools".into(),
            },
            ResponseItem::FunctionCall {
                call_id: "call-1".into(),
                name: "first".into(),
                arguments: "{}".into(),
            },
            ResponseItem::FunctionCall {
                call_id: "call-2".into(),
                name: "second".into(),
                arguments: "{}".into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".into(),
                output: "one".into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-2".into(),
                output: "two".into(),
            },
        ];
        assert_eq!(repair_tool_call_pairs(&items), items);
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
    fn legacy_apply_patch_roundtrip_keeps_custom_tool_items() {
        let patch = "*** Begin Patch\n*** End Patch";
        let messages = vec![
            Message::assistant("").with_tool_calls(vec![ToolCall {
                id: "call-patch".into(),
                name: "apply_patch".into(),
                arguments: serde_json::json!({"patch": patch}),
            }]),
            Message::tool_result("call-patch", "patch applied"),
        ];

        let (_, items) = response_items_from_legacy_messages(&messages);

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
                if call_id == "call-patch" && output == &"patch applied".into()
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
        assert_eq!(messages[1].reasoning_content.as_deref(), Some("thinking"));
        assert!(messages[3].reasoning_content.is_none());
    }
}
