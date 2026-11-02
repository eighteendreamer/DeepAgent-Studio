//! Source/native projection and branch boundaries share one rule.

use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::response_item::ResponseItem;

use crate::session_files::ProjectionSpan;

pub(crate) fn complete_boundary(sequence: u64, spans: &[ProjectionSpan]) -> u64 {
    spans
        .iter()
        .find(|span| span.start <= sequence && sequence < span.end)
        .map_or(sequence, |span| span.end)
}

/// Explicit records own their projection decision. Legacy records are accepted
/// only when their source-first group or native-first mirror is identifiable;
/// an incomplete post-native source must not silently disappear on recovery.
pub(crate) fn projection_spans(
    events: &[Event],
    explicit: &[ProjectionSpan],
    through: Option<u64>,
) -> Result<Vec<ProjectionSpan>> {
    let first_native = events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ResponseItemAppended { .. }))
        .unwrap_or(events.len());
    let mut spans = Vec::new();
    let mut explicit = explicit.iter().peekable();
    let mut claimed = std::collections::HashSet::new();
    let mut index = 0;
    while index < events.len() {
        if through.is_some_and(|sequence| events[index].sequence > sequence) {
            break;
        }
        if let Some(span) = explicit
            .peek()
            .filter(|span| span.start == events[index].sequence)
        {
            claimed.extend(span.start + 1..=span.end);
            spans.push(**span);
            index = span.end as usize + 1;
            explicit.next();
            continue;
        }
        let expected = response_items_for_payload(&events[..index], &events[index].payload)?;
        if expected.is_empty() {
            index += 1;
            continue;
        }
        let matched = expected
            .iter()
            .zip(&events[index + 1..])
            .take_while(|(expected, event)| {
                matches!(&event.payload, EventPayload::ResponseItemAppended { item }
                    if item == *expected || item.is_legacy_text_projection_of(expected))
            })
            .count();
        if matched == expected.len() {
            if has_native_source(&events[..index], &events[index].payload, &claimed)? {
                return Err(CoreError::EventLog(format!(
                    "ambiguous legacy source/native ownership at sequence {}",
                    events[index].sequence
                )));
            }
            let span = ProjectionSpan {
                start: events[index].sequence,
                end: events[index + matched].sequence,
            };
            claimed.extend(span.start + 1..=span.end);
            spans.push(span);
            index += matched + 1;
            continue;
        }
        if matched != 0
            || (index >= first_native
                && !has_native_source(&events[..index], &events[index].payload, &claimed)?)
        {
            return Err(CoreError::EventLog(format!("incomplete or ambiguous legacy response projection at sequence {}; refusing to discard source content", events[index].sequence)));
        }
        index += 1;
    }
    Ok(spans)
}

fn has_native_source(
    history: &[Event],
    source: &EventPayload,
    claimed: &std::collections::HashSet<u64>,
) -> Result<bool> {
    let candidates: Vec<_> = history
        .iter()
        .rev()
        .take_while(|event| !matches!(event.payload, EventPayload::MessageAppended { .. }))
        .filter_map(|event| {
            if claimed.contains(&event.sequence) {
                return None;
            }
            let EventPayload::ResponseItemAppended { item } = &event.payload else {
                return None;
            };
            Some(item)
        })
        .collect();
    Ok(response_items_for_payload(history, source)?
        .iter()
        .all(|expected| candidates.iter().any(|item| native_matches(expected, item))))
}

fn native_matches(expected: &ResponseItem, actual: &ResponseItem) -> bool {
    if expected == actual || actual.is_legacy_text_projection_of(expected) {
        return true;
    }
    match (expected, actual) {
        (
            ResponseItem::Reasoning { content: a, .. },
            ResponseItem::Reasoning { content: b, .. },
        ) => a == b,
        (
            ResponseItem::Message {
                role: a,
                content: text_a,
            },
            ResponseItem::InputMessage {
                role: b,
                content: text_b,
                ..
            },
        ) => a == b && text_a == text_b,
        (
            ResponseItem::FunctionCall {
                call_id: a,
                name: name_a,
                arguments: args_a,
            },
            ResponseItem::FunctionCall {
                call_id: b,
                name: name_b,
                arguments: args_b,
            },
        ) => {
            a == b
                && name_a == name_b
                && matches!((serde_json::from_str::<serde_json::Value>(args_a), serde_json::from_str::<serde_json::Value>(args_b)), (Ok(a), Ok(b)) if a == b)
        }
        _ => false,
    }
}

pub(crate) fn response_items_for_payload(
    history: &[Event],
    payload: &EventPayload,
) -> Result<Vec<ResponseItem>> {
    let mut items = Vec::new();
    match payload {
        EventPayload::MessageAppended { message } => {
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
                    items.push(ResponseItem::FunctionCall {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    });
                }
            }
            if !message.content.is_empty() || message.tool_calls.is_empty() {
                items.push(ResponseItem::from_message_content(message));
            }
        }
        EventPayload::ToolCallRequested { call } => {
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
                items.push(ResponseItem::FunctionCall {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: serde_json::to_string(&call.arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                });
            }
        }
        EventPayload::ToolCallCompleted {
            call_id,
            ok,
            output,
            ..
        } => {
            // Model-backed runs persist the provider-native call before the
            // legacy ToolCallRequested projection. Resolve the call kind from
            // whichever representation is newest so a completed custom call
            // (apply_patch) receives CustomToolCallOutput rather than a
            // mismatched FunctionCallOutput.
            let custom = history
                .iter()
                .rev()
                .find_map(|event| match &event.payload {
                    EventPayload::ToolCallRequested { call } if call.id == *call_id => {
                        Some(call.name == "apply_patch")
                    }
                    EventPayload::ResponseItemAppended {
                        item: ResponseItem::CustomToolCall { call_id: id, .. },
                    } if id == call_id => Some(true),
                    EventPayload::ResponseItemAppended {
                        item: ResponseItem::FunctionCall { call_id: id, .. },
                    } if id == call_id => Some(false),
                    _ => None,
                })
                .unwrap_or(false);
            let output = if *ok {
                serde_json::json!({"status":"ok","result":output}).to_string()
            } else {
                serde_json::json!({"status":"error","error":output}).to_string()
            };
            if custom {
                items.push(ResponseItem::CustomToolCallOutput {
                    call_id: call_id.clone(),
                    output,
                });
            } else {
                items.push(ResponseItem::FunctionCallOutput {
                    call_id: call_id.clone(),
                    output,
                });
            }
        }
        _ => {}
    }
    Ok(items)
}
