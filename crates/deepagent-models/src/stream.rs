//! DeepSeek Responses API semantic SSE assembly.
//!
//! The accumulator consumes typed Responses events, preserves visible and
//! reasoning deltas, correlates tool fragments by provider item/call id, and
//! requires an explicit completed, incomplete, or failed terminal event.

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role, ToolCall};

use crate::chat::{FinishReason, Response, Usage};

/// Responses API semantic SSE accumulator. DeepSeek sends an event object in
/// each `data:` payload; the event's `type` selects the semantic delta.
#[derive(Debug, Default)]
pub struct ResponseAccumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCallBuilder>,
    usage: Option<Usage>,
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

    pub fn finish(mut self) -> Result<Response> {
        if !self.saw_terminal {
            return Err(CoreError::other(
                "Responses stream ended without a terminal response event",
            ));
        }
        let mut message = Message::text(Role::Assistant, self.content);
        if !self.reasoning.is_empty() {
            message.reasoning_content = Some(self.reasoning);
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
                id,
                name: builder.name.unwrap_or_else(|| "tool".into()),
                arguments,
            });
        }
        Ok(Response {
            message,
            finish_reason: self.terminal,
            usage: self.usage,
        })
    }
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

// Kept test-only to exercise compatibility expectations against captured
// pre-migration failure samples. Production code only compiles and exports
// `ResponseAccumulator`.
#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<FinishReason>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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

#[cfg(test)]
#[derive(Debug, Default)]
struct DeltaAccumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCallBuilder>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
}

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
impl DeltaAccumulator {
    /// New empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one chunk into the accumulator.
    pub fn push_chunk(&mut self, chunk: &ChatChunk) {
        self.push_chunk_observed(chunk, &mut NoopObserver);
    }

    /// Fold one chunk and notify `observer` of the semantic fragments it carried
    /// (content / reasoning / new tool-call names).
    pub fn push_chunk_observed(&mut self, chunk: &ChatChunk, observer: &mut dyn DeltaObserver) {
        if let Some(usage) = chunk.usage {
            self.usage = Some(usage);
            observer.on_event(ModelStreamEvent::Usage { usage });
        }
        let Some(choice) = chunk.choices.first() else {
            return;
        };
        if let Some(reason) = choice.finish_reason {
            self.finish_reason = Some(reason);
            observer.on_event(ModelStreamEvent::Finished {
                reason: Some(reason),
            });
        }
        let delta = &choice.delta;
        if let Some(c) = &delta.content {
            self.content.push_str(c);
            if !c.is_empty() {
                observer.on_event(ModelStreamEvent::ContentDelta { text: c.clone() });
            }
        }
        if let Some(r) = &delta.reasoning_content {
            self.reasoning.push_str(r);
            if !r.is_empty() {
                observer.on_event(ModelStreamEvent::ReasoningDelta { text: r.clone() });
            }
        }
        for tc in &delta.tool_calls {
            // Detect a tool-call name becoming known for the first time.
            let known = self
                .tool_calls
                .iter()
                .find(|b| b.index == tc.index)
                .map(|b| b.name.is_some())
                .unwrap_or(false);
            self.merge_tool_call(tc);
            if !known {
                if let Some(name) = self
                    .tool_calls
                    .iter()
                    .find(|b| b.index == tc.index)
                    .and_then(|b| b.name.clone())
                {
                    observer.on_event(ModelStreamEvent::ToolCallStarted {
                        index: tc.index,
                        id: tc.id.clone(),
                        name,
                    });
                }
            }
            if let Some(arguments) = tc
                .function
                .as_ref()
                .and_then(|function| function.arguments.as_ref())
                .filter(|arguments| !arguments.is_empty())
            {
                observer.on_event(ModelStreamEvent::ToolArgumentsDelta {
                    index: tc.index,
                    delta: arguments.clone(),
                });
            }
            if let Some(event) = self.take_completed_event(tc.index) {
                observer.on_event(event);
            }
        }
    }

    /// Parse a single SSE `data:` payload and fold it. Returns `Ok(true)` if the
    /// payload was the `[DONE]` sentinel (stream finished).
    pub fn push_sse_data(&mut self, data: &str) -> Result<bool> {
        self.push_sse_data_observed(data, &mut NoopObserver)
    }

    /// Like [`DeltaAccumulator::push_sse_data`] but notifies `observer` of
    /// semantic fragments.
    pub fn push_sse_data_observed(
        &mut self,
        data: &str,
        observer: &mut dyn DeltaObserver,
    ) -> Result<bool> {
        let trimmed = data.trim();
        if trimmed == "[DONE]" {
            return Ok(true);
        }
        if trimmed.is_empty() {
            return Ok(false);
        }
        let chunk: ChatChunk = serde_json::from_str(trimmed)
            .map_err(|e| CoreError::Serialization(format!("bad chunk: {e}")))?;
        self.push_chunk_observed(&chunk, observer);
        Ok(false)
    }

    fn merge_tool_call(&mut self, delta: &ToolCallDelta) {
        // Find or create the builder for this index.
        let builder = match self.tool_calls.iter_mut().find(|b| b.index == delta.index) {
            Some(b) => b,
            None => {
                self.tool_calls.push(ToolCallBuilder {
                    index: delta.index,
                    item_id: None,
                    custom: false,
                    ..Default::default()
                });
                self.tool_calls.last_mut().expect("just pushed a builder")
            }
        };
        if let Some(id) = &delta.id {
            // Once a synthetic id has been observed it must stay stable so a
            // later provider fragment cannot orphan the UI/tool result pair.
            if builder.id.is_none() {
                builder.id = Some(id.clone());
            }
        }
        if let Some(func) = &delta.function {
            if let Some(name) = &func.name {
                builder.name = Some(name.clone());
            }
            if let Some(args) = &func.arguments {
                builder.arguments.push_str(args);
            }
        }
        if builder.name.is_some() && builder.id.is_none() {
            builder.id = Some(format!("call_stream_{}", builder.index));
        }
    }

    fn take_completed_event(&mut self, index: usize) -> Option<ModelStreamEvent> {
        let builder = self.tool_calls.iter_mut().find(|b| b.index == index)?;
        if builder.completed_emitted {
            return None;
        }
        let name = builder.name.as_ref()?.trim();
        if name.is_empty() {
            return None;
        }
        // An empty fragment is commonly the provider's tool-call preamble,
        // not proof that the call takes no arguments. Wait for actual JSON;
        // truly empty arguments are normalized to `{}` only at stream finish.
        if builder.arguments.trim().is_empty() {
            return None;
        }
        let arguments = serde_json::from_str::<serde_json::Value>(builder.arguments.trim()).ok()?;
        if !arguments.is_object() {
            return None;
        }
        let id = builder.id.clone()?;
        builder.completed_emitted = true;
        Some(ModelStreamEvent::ToolCallCompleted {
            index,
            id,
            name: name.to_string(),
            arguments,
        })
    }

    /// Whether any tool calls were accumulated.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Finalize into a [`Response`]. Tool-call argument strings are parsed
    /// as JSON; an empty/blank argument string becomes `{}`.
    pub fn finish(mut self) -> Result<Response> {
        if self.content.trim().is_empty()
            && self.reasoning.trim().is_empty()
            && self.tool_calls.is_empty()
        {
            return Err(CoreError::other(
                "empty model stream: provider returned no content, reasoning, or tool calls",
            ));
        }
        self.tool_calls.sort_by_key(|b| b.index);

        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        let mut call_ids = std::collections::HashSet::with_capacity(self.tool_calls.len());
        for b in &self.tool_calls {
            let args_str = if b.arguments.trim().is_empty() {
                "{}"
            } else {
                b.arguments.trim()
            };
            // Malformed argument JSON must NOT abort the whole turn: killing
            // the run for one bad provider fragment orphans every other tool
            // call in the batch. Degrade to a sentinel object the pipeline's
            // validation gate rejects deterministically — the model gets a
            // paired failure result and can retry (Claude Code parity).
            let arguments: serde_json::Value = match serde_json::from_str(args_str) {
                Ok(value) => value,
                Err(error) => serde_json::json!({
                    "__invalid_tool_arguments__": true,
                    "raw": args_str,
                    "parse_error": error.to_string(),
                }),
            };
            if !arguments.is_object() {
                return Err(CoreError::Serialization(format!(
                    "tool call {} arguments must be a JSON object",
                    b.index
                )));
            }
            let name = b.name.as_deref().unwrap_or_default().trim();
            if name.is_empty() {
                return Err(CoreError::Serialization(format!(
                    "tool call {} is missing a function name",
                    b.index
                )));
            }
            let id =
                b.id.clone()
                    .unwrap_or_else(|| format!("call_stream_{}", b.index));
            if !call_ids.insert(id.clone()) {
                return Err(CoreError::Serialization(format!(
                    "duplicate tool call id: {id}"
                )));
            }
            tool_calls.push(ToolCall {
                id,
                name: name.to_string(),
                arguments,
            });
        }

        let mut message = Message::text(Role::Assistant, self.content);
        if !self.reasoning.is_empty() {
            message.reasoning_content = Some(self.reasoning);
        }
        message.tool_calls = tool_calls;

        Ok(Response {
            message,
            finish_reason: self.finish_reason,
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(json: serde_json::Value) -> ChatChunk {
        serde_json::from_value(json).unwrap()
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

        let mut acc = DeltaAccumulator::new();
        let mut rec = Rec::default();
        acc.push_chunk_observed(
            &chunk(serde_json::json!({"choices":[{"delta":{"reasoning_content":"think "}}]})),
            &mut rec,
        );
        acc.push_chunk_observed(
            &chunk(serde_json::json!({"choices":[{"delta":{"content":"Hi"}}]})),
            &mut rec,
        );
        acc.push_chunk_observed(
            &chunk(serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":"c0","function":{"name":"search","arguments":""}}
            ]}}]})),
            &mut rec,
        );
        // A second arguments fragment for the same call must NOT re-fire on_tool_call.
        acc.push_chunk_observed(
            &chunk(serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"function":{"arguments":"{}"}}
            ]}}]})),
            &mut rec,
        );

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
        let mut acc = DeltaAccumulator::new();
        for frag in ["Hel", "lo, ", "world"] {
            acc.push_chunk(&chunk(serde_json::json!({
                "choices": [{ "delta": { "content": frag } }]
            })));
        }
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, "Hello, world");
        assert!(resp.message.reasoning_content.is_none());
    }

    #[test]
    fn preserves_markdown_latex_and_chart_characters() {
        let mut acc = DeltaAccumulator::new();
        let delta = r#"```echarts
{"title":{"text":"$E=mc^2$ \ce{H2O}"}}
```"#;
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "content": delta } }]
        })));

        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, delta);
        assert!(resp.message.content.contains("```echarts"));
        assert!(resp.message.content.contains("$E=mc^2$"));
        assert!(resp.message.content.contains("\\ce{H2O}"));
    }

    #[test]
    fn preserves_reasoning_content() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "reasoning_content": "Let me think... " } }]
        })));
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "reasoning_content": "the answer is 4." } }]
        })));
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "content": "4" } }]
        })));
        let resp = acc.finish().unwrap();
        assert_eq!(
            resp.message.reasoning_content.as_deref(),
            Some("Let me think... the answer is 4.")
        );
        assert_eq!(resp.message.content, "4");
    }

    #[test]
    fn merges_fragmented_tool_call_arguments() {
        // Simulates DeepSeek streaming: name in the first fragment, arguments
        // streamed char-by-char across subsequent fragments.
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call_abc",
                  "function": { "name": "add", "arguments": "" } }
            ]}}]
        })));
        for frag in ["{\"a\"", ": 2, ", "\"b\": 3}"] {
            acc.push_chunk(&chunk(serde_json::json!({
                "choices": [{ "delta": { "tool_calls": [
                    { "index": 0, "function": { "arguments": frag } }
                ]}}]
            })));
        }
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
        })));

        assert!(acc.has_tool_calls());
        let resp = acc.finish().unwrap();
        assert_eq!(resp.finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(resp.message.tool_calls.len(), 1);
        let call = &resp.message.tool_calls[0];
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.name, "add");
        assert_eq!(call.arguments, serde_json::json!({"a": 2, "b": 3}));
    }

    #[test]
    fn merges_multiple_parallel_tool_calls() {
        let mut acc = DeltaAccumulator::new();
        // Two calls interleaved by index.
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "c0", "function": { "name": "f0", "arguments": "{}" } },
                { "index": 1, "id": "c1", "function": { "name": "f1", "arguments": "{" } }
            ]}}]
        })));
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 1, "function": { "arguments": "\"x\": 1}" } }
            ]}}]
        })));
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.tool_calls.len(), 2);
        assert_eq!(resp.message.tool_calls[0].name, "f0");
        assert_eq!(
            resp.message.tool_calls[1].arguments,
            serde_json::json!({"x": 1})
        );
    }

    #[test]
    fn sse_done_sentinel_detected() {
        let mut acc = DeltaAccumulator::new();
        assert!(!acc
            .push_sse_data("{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}")
            .unwrap());
        assert!(acc.push_sse_data("[DONE]").unwrap());
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.content, "hi");
    }

    #[test]
    fn empty_sse_line_is_noop() {
        let mut acc = DeltaAccumulator::new();
        assert!(!acc.push_sse_data("").unwrap());
        assert!(!acc.push_sse_data("   ").unwrap());
    }

    #[test]
    fn blank_tool_arguments_become_empty_object() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "c", "function": { "name": "noargs", "arguments": "" } }
            ]}}]
        })));
        let resp = acc.finish().unwrap();
        assert_eq!(resp.message.tool_calls[0].arguments, serde_json::json!({}));
    }

    #[test]
    fn invalid_tool_json_degrades_to_rejectable_sentinel() {
        // Phase G behavior fix: malformed argument bytes no longer abort the
        // whole turn (which would orphan sibling tool calls). They become a
        // sentinel object the pipeline's validation gate rejects with a
        // paired failure result.
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "name": "bad", "arguments": "{not json" } }
            ]}}]
        })));
        let resp = acc.finish().unwrap();
        let arguments = &resp.message.tool_calls[0].arguments;
        assert_eq!(arguments["__invalid_tool_arguments__"], true);
        assert_eq!(arguments["raw"], "{not json");
        assert!(arguments["parse_error"].as_str().is_some());
    }

    #[test]
    fn duplicate_tool_call_ids_are_rejected() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "same", "function": { "name": "first", "arguments": "{}" } },
                { "index": 1, "id": "same", "function": { "name": "second", "arguments": "{}" } }
            ]}}]
        })));
        assert!(acc
            .finish()
            .unwrap_err()
            .to_string()
            .contains("duplicate tool call id"));
    }

    #[test]
    fn tool_call_without_name_is_rejected() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "c0", "function": { "arguments": "{}" } }
            ]}}]
        })));
        assert!(acc
            .finish()
            .unwrap_err()
            .to_string()
            .contains("missing a function name"));
    }

    #[test]
    fn captures_usage_from_final_chunk() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "content": "hi" } }]
        })));
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        })));
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
    }
}
