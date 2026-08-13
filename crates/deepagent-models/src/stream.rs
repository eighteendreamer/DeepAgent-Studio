//! DeepSeek Responses API semantic SSE assembly.
//!
//! The accumulator consumes typed Responses events, preserves visible and
//! reasoning deltas, correlates tool fragments by provider item/call id, and
//! requires an explicit completed, incomplete, or failed terminal event.

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role, ToolCall};
use deepagent_core::response_item::ResponseOutputItem;

use crate::chat::{FinishReason, Response, Usage};

/// Responses API semantic SSE accumulator. DeepSeek sends an event object in
/// each `data:` payload; the event's `type` selects the semantic delta.
#[derive(Debug, Default)]
pub struct ResponseAccumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCallBuilder>,
    output_items: Vec<ResponseOutputItem>,
    usage: Option<Usage>,
    raw_usage: Option<serde_json::Value>,
    terminal: Option<FinishReason>,
    saw_terminal: bool,
}

impl ResponseAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_sse_data_observed(
        &mut self,
        data: &str,
        observer: &mut dyn DeltaObserver,
    ) -> Result<bool> {
        let value: serde_json::Value = serde_json::from_str(data.trim())
            .map_err(|e| CoreError::Serialization(format!("bad Responses event: {e}")))?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        observer.on_event(ModelStreamEvent::ResponseStreamEvent {
            event_type: kind.to_string(),
            item_id: value
                .get("item_id")
                .or_else(|| value.get("call_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            item_type: value
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            delta_chars: value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .map(|delta| delta.chars().count()),
        });
        match kind {
            "response.output_text.delta" => {
                if let Some(text) = value.get("delta").and_then(serde_json::Value::as_str) {
                    self.content.push_str(text);
                    observer.on_event(ModelStreamEvent::ContentDelta {
                        text: text.to_string(),
                    });
                }
            }
            "response.reasoning_text.delta" => {
                if let Some(text) = value.get("delta").and_then(serde_json::Value::as_str) {
                    self.reasoning.push_str(text);
                    observer.on_event(ModelStreamEvent::ReasoningDelta {
                        text: text.to_string(),
                    });
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let item_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let delta = value
                    .get("delta")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let builder = self.tool_calls.iter_mut().find(|b| {
                    b.item_id.as_deref() == Some(item_id) || b.id.as_deref() == Some(item_id)
                });
                if let Some(builder) = builder {
                    builder.arguments.push_str(delta);
                    observer.on_event(ModelStreamEvent::ToolArgumentsDelta {
                        index: builder.index,
                        delta: delta.to_string(),
                    });
                }
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item") {
                    self.add_item(item, observer);
                }
            }
            "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.web_search_call.completed" => {
                // Native web-search lifecycle is intentionally provider-owned;
                // retain it in the structured runtime stream without exposing
                // search prompt/result text to diagnostics.
                if let Some(item_id) = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(serde_json::Value::as_str)
                {
                    let status = kind
                        .strip_prefix("response.web_search_call.")
                        .unwrap_or(kind);
                    observer.on_event(ModelStreamEvent::WebSearchCall {
                        id: item_id.to_string(),
                        status: status.to_string(),
                        action: None,
                    });
                }
            }
            "response.output_item.done" => {
                if let Some(item) = value.get("item") {
                    self.add_item(item, observer);
                    self.record_output_item(item);
                    if let Some(call_id) = item.get("call_id").and_then(serde_json::Value::as_str) {
                        if let Some(builder) = self
                            .tool_calls
                            .iter_mut()
                            .find(|b| b.id.as_deref() == Some(call_id))
                        {
                            if let Some(args) = item
                                .get("arguments")
                                .or_else(|| item.get("input"))
                                .and_then(serde_json::Value::as_str)
                            {
                                builder.arguments = args.to_string();
                            }
                            let index = builder.index;
                            let _ = builder;
                            self.emit_tool_completed(index, observer);
                        }
                    }
                }
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                let item_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if let Some(builder) = self.tool_calls.iter_mut().find(|b| {
                    b.item_id.as_deref() == Some(item_id) || b.id.as_deref() == Some(item_id)
                }) {
                    if let Some(args) = value
                        .get("arguments")
                        .or_else(|| value.get("input"))
                        .and_then(serde_json::Value::as_str)
                    {
                        builder.arguments = args.to_string();
                    }
                    let index = builder.index;
                    let _ = builder;
                    self.emit_tool_completed(index, observer);
                }
            }
            "response.completed" => {
                self.terminal = Some(FinishReason::Stop);
                self.saw_terminal = true;
                if let Some(usage) =
                    parse_response_usage(value.get("response").and_then(|v| v.get("usage")))
                {
                    self.raw_usage = value.get("response").and_then(|v| v.get("usage")).cloned();
                    self.usage = Some(usage);
                    observer.on_event(ModelStreamEvent::Usage { usage });
                }
                observer.on_event(ModelStreamEvent::Finished {
                    reason: self.terminal,
                });
                return Ok(true);
            }
            "response.incomplete" => {
                self.terminal = Some(FinishReason::Length);
                self.saw_terminal = true;
                if let Some(usage) =
                    parse_response_usage(value.get("response").and_then(|v| v.get("usage")))
                {
                    self.raw_usage = value.get("response").and_then(|v| v.get("usage")).cloned();
                    self.usage = Some(usage);
                    observer.on_event(ModelStreamEvent::Usage { usage });
                }
                observer.on_event(ModelStreamEvent::Finished {
                    reason: self.terminal,
                });
                return Ok(true);
            }
            "response.failed" => {
                self.saw_terminal = true;
                return Err(CoreError::provider(
                    None,
                    Some("response_failed".into()),
                    value
                        .get("response")
                        .and_then(|v| v.get("error"))
                        .and_then(|v| v.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Responses API response failed")
                        .to_string(),
                ));
            }
            _ => {}
        }
        Ok(false)
    }

    fn add_item(&mut self, item: &serde_json::Value, observer: &mut dyn DeltaObserver) {
        let kind = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if kind == "web_search_call" {
            if let Some(id) = item
                .get("id")
                .or_else(|| item.get("call_id"))
                .and_then(serde_json::Value::as_str)
            {
                observer.on_event(ModelStreamEvent::WebSearchCall {
                    id: id.to_string(),
                    status: item
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("in_progress")
                        .to_string(),
                    action: item.get("action").cloned(),
                });
            }
            return;
        }
        if kind != "function_call" && kind != "custom_tool_call" {
            return;
        }
        let index = self.tool_calls.len();
        let item_id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let id = item
            .get("call_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                item.get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool")
            .to_string();
        if self.tool_calls.iter().any(|b| b.id == id) {
            return;
        }
        self.tool_calls.push(ToolCallBuilder {
            index,
            item_id,
            id: id.clone(),
            name: Some(name.clone()),
            arguments: item
                .get("arguments")
                .or_else(|| item.get("input"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            custom: kind == "custom_tool_call",
            completed_emitted: false,
        });
        observer.on_event(ModelStreamEvent::ToolCallStarted { index, id, name });
    }

    fn emit_tool_completed(&mut self, index: usize, observer: &mut dyn DeltaObserver) {
        let Some(builder) = self.tool_calls.iter_mut().find(|b| b.index == index) else {
            return;
        };
        if builder.completed_emitted {
            return;
        }
        let Some(name) = builder.name.clone() else {
            return;
        };
        let args = if builder.arguments.trim().is_empty() {
            "{}"
        } else {
            builder.arguments.trim()
        };
        let arguments = if builder.custom {
            serde_json::json!({"patch": args})
        } else {
            serde_json::from_str(args).unwrap_or_else(|e| serde_json::json!({"__invalid_tool_arguments__":true,"raw":args,"parse_error":e.to_string()}))
        };
        let id = builder
            .id
            .clone()
            .unwrap_or_else(|| format!("call_stream_{}", index));
        builder.completed_emitted = true;
        observer.on_event(ModelStreamEvent::ToolCallCompleted {
            index,
            id,
            name,
            arguments,
        });
    }

    fn record_output_item(&mut self, item: &serde_json::Value) {
        let Some(kind) = item.get("type").and_then(serde_json::Value::as_str) else {
            return;
        };
        let parsed = match kind {
            "message" => {
                let role = item
                    .get("role")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("assistant")
                    .to_string();
                let content =
                    output_text_from_message_item(item).unwrap_or_else(|| self.content.clone());
                Some(ResponseOutputItem::Message { role, content })
            }
            "reasoning" => Some(ResponseOutputItem::Reasoning {
                id: item
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                content: item
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "function_call" => Some(ResponseOutputItem::FunctionCall {
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool")
                    .to_string(),
                arguments: item
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "custom_tool_call" => Some(ResponseOutputItem::CustomToolCall {
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("apply_patch")
                    .to_string(),
                input: item
                    .get("input")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "web_search_call" => Some(ResponseOutputItem::WebSearchCall {
                id: item
                    .get("id")
                    .or_else(|| item.get("call_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status: item
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("completed")
                    .to_string(),
                action: item.get("action").cloned(),
            }),
            _ => None,
        };
        if let Some(parsed) = parsed {
            self.output_items.push(parsed);
        }
    }

    pub fn finish(mut self) -> Result<Response> {
        if !self.saw_terminal {
            return Err(CoreError::other(
                "Responses stream ended without a terminal response event",
            ));
        }
        let mut message = Message::text(Role::Assistant, self.content);
        if !self.reasoning.is_empty() {
            message.reasoning_content = Some(self.reasoning.clone());
        }
        let mut fallback_items = Vec::new();
        if !self.reasoning.is_empty() {
            fallback_items.push(ResponseOutputItem::Reasoning {
                id: None,
                content: self.reasoning.clone(),
            });
        }
        for builder in self.tool_calls.drain(..) {
            let id = builder
                .id
                .unwrap_or_else(|| format!("call_stream_{}", builder.index));
            let args = if builder.arguments.trim().is_empty() {
                "{}"
            } else {
                builder.arguments.trim()
            };
            let arguments = if builder.custom {
                serde_json::json!({"patch": args})
            } else {
                serde_json::from_str(args).unwrap_or_else(|e| serde_json::json!({"__invalid_tool_arguments__":true,"raw":args,"parse_error":e.to_string()}))
            };
            message.tool_calls.push(ToolCall {
                id: id.clone(),
                name: builder.name.clone().unwrap_or_else(|| "tool".into()),
                arguments: arguments.clone(),
            });
            if builder.custom {
                fallback_items.push(ResponseOutputItem::CustomToolCall {
                    call_id: id,
                    name: builder.name.unwrap_or_else(|| "apply_patch".into()),
                    input: args.to_string(),
                });
            } else {
                fallback_items.push(ResponseOutputItem::FunctionCall {
                    call_id: id,
                    name: builder.name.unwrap_or_else(|| "tool".into()),
                    arguments: args.to_string(),
                });
            }
        }
        if !message.content.is_empty() || fallback_items.is_empty() {
            fallback_items.push(ResponseOutputItem::Message {
                role: "assistant".into(),
                content: message.content.clone(),
            });
        }
        let mut output_items = self.output_items;
        for item in fallback_items {
            if !response_item_already_present(&output_items, &item) {
                output_items.push(item);
            }
        }
        Ok(Response {
            message,
            output_items,
            finish_reason: self.terminal,
            usage: self.usage,
            raw_usage: self.raw_usage,
        })
    }
}

fn response_item_already_present(
    items: &[ResponseOutputItem],
    candidate: &ResponseOutputItem,
) -> bool {
    match candidate {
        ResponseOutputItem::Message { role, content } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::Message {
                    role: existing_role,
                    content: existing_content,
                } if existing_role == role && existing_content == content
            )
        }),
        ResponseOutputItem::Reasoning { id, content } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::Reasoning {
                    id: existing_id,
                    content: existing_content,
                } if existing_id == id && existing_content == content
            )
        }),
        ResponseOutputItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::FunctionCall {
                    call_id: existing_call_id,
                    name: existing_name,
                    arguments: existing_arguments,
                } if existing_call_id == call_id
                    && existing_name == name
                    && existing_arguments == arguments
            )
        }),
        ResponseOutputItem::CustomToolCall {
            call_id,
            name,
            input,
        } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::CustomToolCall {
                    call_id: existing_call_id,
                    name: existing_name,
                    input: existing_input,
                } if existing_call_id == call_id
                    && existing_name == name
                    && existing_input == input
            )
        }),
        ResponseOutputItem::FunctionCallOutput { call_id, output } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::FunctionCallOutput {
                    call_id: existing_call_id,
                    output: existing_output,
                } if existing_call_id == call_id && existing_output == output
            )
        }),
        ResponseOutputItem::CustomToolCallOutput { call_id, output } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::CustomToolCallOutput {
                    call_id: existing_call_id,
                    output: existing_output,
                } if existing_call_id == call_id && existing_output == output
            )
        }),
        ResponseOutputItem::WebSearchCall { id, status, action } => items.iter().any(|item| {
            matches!(
                item,
                ResponseOutputItem::WebSearchCall {
                    id: existing_id,
                    status: existing_status,
                    action: existing_action,
                } if existing_id == id && existing_status == status && existing_action == action
            )
        }),
    }
}

fn output_text_from_message_item(item: &serde_json::Value) -> Option<String> {
    let mut out = String::new();
    for part in item.get("content")?.as_array()? {
        if part.get("type").and_then(serde_json::Value::as_str) == Some("output_text") {
            if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                out.push_str(text);
            }
        }
    }
    Some(out)
}

fn parse_response_usage(value: Option<&serde_json::Value>) -> Option<Usage> {
    let value = value?;
    Some(Usage {
        prompt_tokens: value
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        completion_tokens: value
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        reasoning_tokens: value
            .get("output_tokens_details")
            .and_then(|v| v.get("reasoning_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        total_tokens: value
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        prompt_cache_hit_tokens: value
            .get("input_tokens_details")
            .and_then(|v| v.get("cached_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        prompt_cache_miss_tokens: 0,
    })
}

/// Provider-neutral semantic model stream used by Agent Kernel v2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamEvent {
    /// Sanitized metadata for every semantic Responses SSE event. No prompt,
    /// output text, reasoning text, or search query is included.
    ResponseStreamEvent {
        event_type: String,
        item_id: Option<String>,
        item_type: Option<String>,
        delta_chars: Option<usize>,
    },
    ContentDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallStarted {
        index: usize,
        id: Option<String>,
        name: String,
    },
    ToolArgumentsDelta {
        index: usize,
        delta: String,
    },
    /// A complete, schema-neutral JSON argument object is available for a
    /// tool call. This can arrive before the provider closes the assistant
    /// stream, allowing the query loop to prepare execution without guessing
    /// where a fragmented JSON value ends.
    ToolCallCompleted {
        index: usize,
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Provider-owned DeepSeek web-search lifecycle. This is not a local
    /// function call and must never enter the local execution pipeline.
    WebSearchCall {
        id: String,
        status: String,
        action: Option<serde_json::Value>,
    },
    Usage {
        usage: Usage,
    },
    Finished {
        reason: Option<FinishReason>,
    },
}

/// Observes streaming deltas as they arrive (for live UIs / event streams).
///
/// Distinct from the transport-level [`crate::transport::EventSink`] (raw SSE
/// payloads): this receives *semantic* fragments — visible content, Thinking
/// Mode reasoning, and tool-call starts — already decoded from each chunk. The
/// default no-op impl lets callers ignore deltas (the non-streaming path).
pub trait DeltaObserver: Send {
    /// Unified event entry point. Existing observers overriding the legacy
    /// callbacks continue to work through this default dispatcher.
    fn on_event(&mut self, event: ModelStreamEvent) {
        match event {
            ModelStreamEvent::ResponseStreamEvent { .. } => {}
            ModelStreamEvent::ContentDelta { text } => self.on_content(&text),
            ModelStreamEvent::ReasoningDelta { text } => self.on_reasoning(&text),
            ModelStreamEvent::ToolCallStarted { name, .. } => self.on_tool_call(&name),
            ModelStreamEvent::ToolArgumentsDelta { .. }
            | ModelStreamEvent::ToolCallCompleted { .. }
            | ModelStreamEvent::WebSearchCall { .. }
            | ModelStreamEvent::Usage { .. }
            | ModelStreamEvent::Finished { .. } => {}
        }
    }
    /// A visible content fragment arrived.
    fn on_content(&mut self, _delta: &str) {}
    /// A Thinking Mode reasoning fragment arrived.
    fn on_reasoning(&mut self, _delta: &str) {}
    /// A tool call began (its name became known).
    fn on_tool_call(&mut self, _name: &str) {}
}

/// A `DeltaObserver` that ignores everything (the default for `stream_response`).
pub struct NoopObserver;
impl DeltaObserver for NoopObserver {}

#[derive(Debug, Default)]
struct ToolCallBuilder {
    index: usize,
    item_id: Option<String>,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    custom: bool,
    completed_emitted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(acc: &mut ResponseAccumulator, json: &str) {
        acc.push_sse_data_observed(json, &mut NoopObserver).unwrap();
    }

    fn complete(acc: &mut ResponseAccumulator) {
        event(
            acc,
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
        );
    }

    #[test]
    fn observer_receives_semantic_deltas() {
        #[derive(Default)]
        struct Rec {
            content: String,
            reasoning: String,
            tools: Vec<String>,
            completed: Vec<(String, serde_json::Value)>,
        }
        impl DeltaObserver for Rec {
            fn on_event(&mut self, event: ModelStreamEvent) {
                if let ModelStreamEvent::ToolCallCompleted { id, arguments, .. } = &event {
                    self.completed.push((id.clone(), arguments.clone()));
                }
                match event {
                    ModelStreamEvent::ContentDelta { text } => self.on_content(&text),
                    ModelStreamEvent::ReasoningDelta { text } => self.on_reasoning(&text),
                    ModelStreamEvent::ToolCallStarted { name, .. } => self.on_tool_call(&name),
                    _ => {}
                }
            }
            fn on_content(&mut self, d: &str) {
                self.content.push_str(d);
            }
            fn on_reasoning(&mut self, d: &str) {
                self.reasoning.push_str(d);
            }
            fn on_tool_call(&mut self, name: &str) {
                self.tools.push(name.to_string());
            }
        }

        let mut acc = ResponseAccumulator::new();
        let mut rec = Rec::default();
        acc.push_sse_data_observed(
            r#"{"type":"response.reasoning_text.delta","delta":"think "}"#,
            &mut rec,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_text.delta","delta":"Hi"}"#,
            &mut rec,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_0","call_id":"c0","name":"search","arguments":""}}"#,
            &mut rec,
        )
        .unwrap();
        // Argument deltas for the same item must NOT re-fire on_tool_call.
        acc.push_sse_data_observed(
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_0","delta":"{}"}"#,
            &mut rec,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.function_call_arguments.done","item_id":"fc_0","arguments":"{}"}"#,
            &mut rec,
        )
        .unwrap();
        complete(&mut acc);

        assert_eq!(rec.content, "Hi");
        assert_eq!(rec.reasoning, "think ");
        assert_eq!(rec.tools, vec!["search"]); // fired exactly once
        assert_eq!(
            rec.completed,
            vec![("c0".to_string(), serde_json::json!({}))]
        );
    }

    #[test]
    fn merges_content_fragments_in_order() {
        let mut acc = ResponseAccumulator::new();
        for frag in ["Hel", "lo, ", "world"] {
            event(
                &mut acc,
                &serde_json::json!({"type":"response.output_text.delta","delta":frag}).to_string(),
            );
        }
        complete(&mut acc);
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, "Hello, world");
        assert!(resp.message.reasoning_content.is_none());
    }

    #[test]
    fn preserves_markdown_latex_and_chart_characters() {
        let mut acc = ResponseAccumulator::new();
        let delta = r#"```echarts
{"title":{"text":"$E=mc^2$ \ce{H2O}"}}
```"#;
        event(
            &mut acc,
            &serde_json::json!({"type":"response.output_text.delta","delta":delta}).to_string(),
        );
        complete(&mut acc);

        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, delta);
        assert!(resp.message.content.contains("```echarts"));
        assert!(resp.message.content.contains("$E=mc^2$"));
        assert!(resp.message.content.contains("\\ce{H2O}"));
    }

    #[test]
    fn preserves_reasoning_content() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.reasoning_text.delta","delta":"Let me think... "}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.reasoning_text.delta","delta":"the answer is 4."}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.output_text.delta","delta":"4"}"#,
        );
        complete(&mut acc);
        let resp = acc.finish().unwrap();
        assert_eq!(
            resp.message.reasoning_content.as_deref(),
            Some("Let me think... the answer is 4.")
        );
        assert_eq!(resp.message.content, "4");
    }

    #[test]
    fn merges_fragmented_tool_call_arguments() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_abc","call_id":"call_abc","name":"add","arguments":""}}"#,
        );
        for frag in ["{\"a\"", ": 2, ", "\"b\": 3}"] {
            event(
                &mut acc,
                &serde_json::json!({
                    "type":"response.function_call_arguments.delta",
                    "item_id":"fc_abc",
                    "delta": frag
                })
                .to_string(),
            );
        }
        event(
            &mut acc,
            r#"{"type":"response.function_call_arguments.done","item_id":"fc_abc","arguments":"{\"a\": 2, \"b\": 3}"}"#,
        );
        complete(&mut acc);

        let resp = acc.finish().unwrap();
        assert_eq!(resp.finish_reason, Some(FinishReason::Stop));
        assert_eq!(resp.message.tool_calls.len(), 1);
        let call = &resp.message.tool_calls[0];
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.name, "add");
        assert_eq!(call.arguments, serde_json::json!({"a": 2, "b": 3}));
    }

    #[test]
    fn merges_multiple_parallel_tool_calls() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_0","call_id":"c0","name":"f0","arguments":"{}"}}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"f1","arguments":"{"}}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"\"x\": 1}"}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","arguments":"{\"x\": 1}"}"#,
        );
        complete(&mut acc);
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.tool_calls.len(), 2);
        assert_eq!(resp.message.tool_calls[0].name, "f0");
        assert_eq!(
            resp.message.tool_calls[1].arguments,
            serde_json::json!({"x": 1})
        );
    }

    #[test]
    fn completed_terminal_event_finishes_stream() {
        let mut acc = ResponseAccumulator::new();
        assert!(!acc
            .push_sse_data_observed(
                r#"{"type":"response.output_text.delta","delta":"hi"}"#,
                &mut NoopObserver,
            )
            .unwrap());
        assert!(acc
            .push_sse_data_observed(
                r#"{"type":"response.completed","response":{"status":"completed"}}"#,
                &mut NoopObserver,
            )
            .unwrap());
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, "hi");
    }

    #[test]
    fn empty_sse_line_is_noop() {
        let mut acc = ResponseAccumulator::new();
        assert!(acc.push_sse_data_observed("", &mut NoopObserver).is_err());
        assert!(acc
            .push_sse_data_observed("   ", &mut NoopObserver)
            .is_err());
    }

    #[test]
    fn blank_tool_arguments_become_empty_object() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc","call_id":"c","name":"noargs","arguments":""}}"#,
        );
        complete(&mut acc);
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.tool_calls[0].arguments, serde_json::json!({}));
    }

    #[test]
    fn invalid_tool_json_degrades_to_rejectable_sentinel() {
        // Phase G behavior fix: malformed argument bytes no longer abort the
        // whole turn (which would orphan sibling tool calls). They become a
        // sentinel object the pipeline's validation gate rejects with a
        // paired failure result.
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc","call_id":"bad-call","name":"bad","arguments":"{not json"}}"#,
        );
        complete(&mut acc);
        let resp = acc.finish().unwrap();
        let arguments = &resp.message.tool_calls[0].arguments;
        assert_eq!(arguments["__invalid_tool_arguments__"], true);
        assert_eq!(arguments["raw"], "{not json");
        assert!(arguments["parse_error"].as_str().is_some());
    }

    #[test]
    fn stream_without_terminal_event_is_rejected() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_text.delta","delta":"partial"}"#,
        );
        assert!(acc
            .finish()
            .unwrap_err()
            .to_string()
            .contains("terminal response event"));
    }

    #[test]
    fn captures_usage_from_final_chunk() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#,
        );
        let resp = acc.finish().unwrap();
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn responses_web_search_uses_item_id_and_keeps_action_metadata() {
        #[derive(Default)]
        struct Rec(Vec<ModelStreamEvent>);
        impl DeltaObserver for Rec {
            fn on_event(&mut self, event: ModelStreamEvent) {
                self.0.push(event);
            }
        }
        let mut acc = ResponseAccumulator::new();
        let mut rec = Rec::default();
        acc.push_sse_data_observed(
            r#"{"type":"response.web_search_call.searching","item_id":"ws_1"}"#,
            &mut rec,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","queries":["redacted"]}}}"#,
            &mut rec,
        ).unwrap();
        assert!(rec.0.iter().any(|event| matches!(event,
            ModelStreamEvent::WebSearchCall { id, status, .. }
                if id == "ws_1" && status == "searching"
        )));
        assert!(rec.0.iter().any(|event| matches!(event,
            ModelStreamEvent::WebSearchCall { id, status, action: Some(action) }
                if id == "ws_1" && status == "completed"
                    && action["queries"].as_array().map_or(0, Vec::len) == 1
        )));
    }

    #[test]
    fn output_item_events_do_not_drop_delta_text_projection() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"query":"rust"}}}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.output_text.delta","delta":"found it"}"#,
        );
        complete(&mut acc);

        let response = acc.finish().unwrap();

        assert!(response.output_items.iter().any(|item| matches!(
            item,
            ResponseOutputItem::WebSearchCall { id, status, .. }
                if id == "ws_1" && status == "completed"
        )));
        assert!(response.output_items.iter().any(|item| matches!(
            item,
            ResponseOutputItem::Message { role, content }
                if role == "assistant" && content == "found it"
        )));
    }

    #[test]
    fn responses_custom_tool_deltas_correlate_by_item_id() {
        let mut acc = ResponseAccumulator::new();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.added","item":{"type":"custom_tool_call","id":"item_1","call_id":"call_1","name":"apply_patch","input":""}}"#,
            &mut NoopObserver,
        ).unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.custom_tool_call_input.delta","item_id":"item_1","delta":"*** Begin Patch"}"#,
            &mut NoopObserver,
        ).unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.custom_tool_call_input.done","item_id":"item_1","input":"*** Begin Patch\n*** End Patch"}"#,
            &mut NoopObserver,
        ).unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":2,"output_tokens_details":{"reasoning_tokens":1},"total_tokens":5}}}"#,
            &mut NoopObserver,
        ).unwrap();
        let response = acc.finish().unwrap();
        assert_eq!(response.message.tool_calls[0].id, "call_1");
        assert_eq!(
            response.message.tool_calls[0].arguments["patch"],
            "*** Begin Patch\n*** End Patch"
        );
        assert_eq!(response.usage.unwrap().reasoning_tokens, 1);
    }

    #[test]
    fn responses_custom_tool_done_item_supplies_final_input() {
        let mut acc = ResponseAccumulator::new();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.added","item":{"type":"custom_tool_call","id":"item_1","call_id":"call_1","name":"apply_patch","input":""}}"#,
            &mut NoopObserver,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.done","item":{"type":"custom_tool_call","id":"item_1","call_id":"call_1","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"}}"#,
            &mut NoopObserver,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
            &mut NoopObserver,
        )
        .unwrap();

        let response = acc.finish().unwrap();
        assert_eq!(
            response.message.tool_calls[0].arguments["patch"],
            "*** Begin Patch\n*** End Patch"
        );
        assert!(matches!(
            &response.output_items[0],
            ResponseOutputItem::CustomToolCall { call_id, input, .. }
                if call_id == "call_1" && input == "*** Begin Patch\n*** End Patch"
        ));
    }

    #[test]
    fn responses_incomplete_emits_usage_and_length_terminal() {
        #[derive(Default)]
        struct Rec(Vec<ModelStreamEvent>);
        impl DeltaObserver for Rec {
            fn on_event(&mut self, event: ModelStreamEvent) {
                self.0.push(event);
            }
        }

        let mut acc = ResponseAccumulator::new();
        let mut rec = Rec::default();
        assert!(acc
            .push_sse_data_observed(
                r#"{"type":"response.incomplete","response":{"status":"incomplete","usage":{"input_tokens":4,"output_tokens":5,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":3},"total_tokens":9}}}"#,
                &mut rec,
            )
            .unwrap());

        assert!(rec.0.iter().any(|event| matches!(
            event,
            ModelStreamEvent::Usage { usage }
                if usage.prompt_cache_hit_tokens == 2 && usage.reasoning_tokens == 3
        )));
        let response = acc.finish().unwrap();
        assert_eq!(response.finish_reason, Some(FinishReason::Length));
        assert_eq!(response.usage.unwrap().total_tokens, 9);
        assert_eq!(
            response
                .raw_usage
                .as_ref()
                .and_then(|usage| usage.get("input_tokens"))
                .and_then(serde_json::Value::as_u64),
            Some(4)
        );
    }

    #[test]
    fn responses_output_item_done_persists_message_item() {
        let mut acc = ResponseAccumulator::new();
        acc.push_sse_data_observed(
            r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello"}]}}"#,
            &mut NoopObserver,
        )
        .unwrap();
        acc.push_sse_data_observed(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
            &mut NoopObserver,
        )
        .unwrap();

        let response = acc.finish().unwrap();

        assert!(matches!(
            &response.output_items[0],
            ResponseOutputItem::Message { role, content }
                if role == "assistant" && content == "Hello"
        ));
        assert_eq!(response.raw_usage.as_ref().unwrap()["total_tokens"], 2);
    }
}
