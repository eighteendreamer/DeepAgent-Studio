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
                legacy_projection_matches(&events[index].payload, expected, event)
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

/// A bounded reader need not infer a tool's kind from unbounded earlier
/// history: conservatively keep either exact output mirror with its source.
/// This does not change the replay/branch ownership validator above.
pub(crate) fn legacy_projection_matches(
    source: &EventPayload,
    expected: &ResponseItem,
    event: &Event,
) -> bool {
    let EventPayload::ResponseItemAppended { item, .. } = &event.payload else {
        return false;
    };
    if item == expected || item.is_legacy_text_projection_of(expected) {
        return true;
    }

    if let Some(id) = expected.client_tool_call_id() {
        let call = match source {
            EventPayload::ToolCallRequested { call } if call.id == id => Some(call),
            EventPayload::MessageAppended { message } => {
                message.tool_calls.iter().find(|call| call.id == id)
            }
            _ => None,
        };
        if let Some(call) = call {
            return item.is_tool_call_projection_of(call);
        }
    }

    let EventPayload::ToolCallCompleted {
        call_id,
        ok,
        output,
        content,
        ..
    } = source
    else {
        return false;
    };
    let expected_output = if *ok {
        serde_json::json!({"status":"ok","result":output}).to_string()
    } else {
        serde_json::json!({"status":"error","error":output}).to_string()
    };
    let expected_output =
        deepagent_core::tool_content::ToolResultBody::with_parts(expected_output, content);
    match item {
        ResponseItem::FunctionCallOutput {
            call_id: actual_call_id,
            output,
        }
        | ResponseItem::CustomToolCallOutput {
            call_id: actual_call_id,
            output,
        } => actual_call_id == call_id && output == &expected_output,
        _ => false,
    }
}

pub(crate) fn has_native_source(
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
            let EventPayload::ResponseItemAppended { item, .. } = &event.payload else {
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
    project_payload(history, payload, &|_| None, true)
}

pub(crate) fn response_items_with_tool_schemas(
    history: &[Event],
    payload: &EventPayload,
    schema: &dyn Fn(&str) -> Option<serde_json::Value>,
) -> Result<Vec<ResponseItem>> {
    project_payload(history, payload, schema, false)
}

// Only legacy readers infer an untyped historical call by its old reserved
// name. New writes always create function items unless a native call exists.
fn project_payload(
    history: &[Event],
    payload: &EventPayload,
    schema: &dyn Fn(&str) -> Option<serde_json::Value>,
    legacy: bool,
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
                    signature: None,
                });
            }
            items.extend(
                message
                    .tool_calls
                    .iter()
                    .map(|call| projected_call(history, call, legacy)),
            );
            if !message.content.is_empty() || message.tool_calls.is_empty() {
                items.push(ResponseItem::from_message_content(message));
            }
        }
        EventPayload::ToolCallRequested { call } => {
            items.push(projected_call(history, call, legacy));
        }
        EventPayload::ToolCallCompleted {
            call_id,
            ok,
            output,
            content,
            ..
        } => {
            if history.iter().any(|event| matches!(&event.payload,
                EventPayload::ResponseItemAppended { item: ResponseItem::ToolSearchCall {
                    call_id: Some(id), execution: deepagent_core::response_item::ToolSearchExecution::Client, ..
                }, .. } if id == call_id)) {
                items.push(ResponseItem::native_tool_search_output(call_id.clone(), *ok, output, schema));
                return Ok(items);
            }
            // Native call identity is authoritative even when a later UI
            // ToolCallRequested mirror has the same reserved name.
            let custom = history
                .iter()
                .rev()
                .find_map(|event| match &event.payload {
                    EventPayload::ResponseItemAppended {
                        item: ResponseItem::CustomToolCall { call_id: id, .. },
                        ..
                    } if id == call_id => Some(true),
                    EventPayload::ResponseItemAppended {
                        item: ResponseItem::FunctionCall { call_id: id, .. },
                        ..
                    } if id == call_id => Some(false),
                    _ => None,
                })
                .or_else(|| {
                    if !legacy {
                        return None;
                    }
                    history.iter().rev().find_map(|event| match &event.payload {
                        EventPayload::ToolCallRequested { call } if call.id == *call_id => {
                            Some(matches!(
                                ResponseItem::from_legacy_tool_call(call),
                                ResponseItem::CustomToolCall { .. }
                            ))
                        }
                        _ => None,
                    })
                })
                .unwrap_or(false);
            let output = if *ok {
                serde_json::json!({"status":"ok","result":output}).to_string()
            } else {
                serde_json::json!({"status":"error","error":output}).to_string()
            };
            let output = deepagent_core::tool_content::ToolResultBody::with_parts(output, content);
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

// A compatibility mirror may not override a provider-native function/custom
// call. Reuse a native item only if the source payload really is its projection;
// divergent legacy sources still fail the existing ownership validation.
fn projected_call(
    history: &[Event],
    call: &deepagent_core::message::ToolCall,
    legacy: bool,
) -> ResponseItem {
    let function = ResponseItem::from_function_call(call);
    let native = history.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ResponseItemAppended { item, .. }
            if item.client_tool_call_id() == Some(call.id.as_str()) =>
        {
            Some(item)
        }
        _ => None,
    });
    if let Some(item) = native {
        if item.is_tool_call_projection_of(call) {
            return item.clone();
        }
    }
    if legacy {
        ResponseItem::from_legacy_tool_call(call)
    } else {
        function
    }
}
