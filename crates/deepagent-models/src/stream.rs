//! DeepSeek Responses API semantic SSE assembly.
//!
//! The accumulator consumes typed Responses events, preserves visible and
//! reasoning deltas, correlates tool fragments by provider item/call id, and
//! requires an explicit completed, incomplete, or failed terminal event.

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::response_item::ResponseOutputItem;

use crate::chat::{FinishReason, Response, Usage};

#[path = "stream_text.rs"]
mod text;

#[path = "stream_terminal.rs"]
mod terminal;

#[path = "stream_sequence.rs"]
mod sequence;

#[path = "stream_metadata.rs"]
mod metadata;

#[path = "stream_tool_search.rs"]
mod tool_search;

/// Responses API semantic SSE accumulator. DeepSeek sends an event object in
/// each `data:` payload; the event's `type` selects the semantic delta.
#[derive(Debug, Default)]
pub struct ResponseAccumulator {
    sequence: sequence::Sequence,
    metadata: metadata::Metadata,
    text_items: text::TextItems,
    native_search: tool_search::ToolSearchItems,
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
        let (sequence_number, sequence_gap) = self.sequence.accept(&value)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let terminal_status = match kind {
            "response.completed" => Some("completed"),
            "response.incomplete" => Some("incomplete"),
            "response.failed" => Some("failed"),
            _ => None,
        };
        if let Some(status) = terminal_status {
            // Validate before copying any terminal identity to observers. An
            // invalid envelope must not leak its unaccepted provider metadata.
            terminal::validate(&value, kind, status)?;
        }
        self.metadata.observe(kind, &value);
        observer.on_event(ModelStreamEvent::ResponseStreamEvent {
            sequence_number: Some(sequence_number),
            sequence_gap,
            response_id: self.metadata.response_id.clone(),
            provider_model: self.metadata.provider_model.clone(),
            event_type: kind.to_string(),
            item_id: value
                .get("item_id")
                .or_else(|| value.get("call_id"))
                .or_else(|| value.get("item").and_then(|item| item.get("id")))
                .and_then(metadata::diagnostic_identifier),
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
                    self.text_items
                        .delta(&value, text, &mut self.output_items)?;
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
                let builder = self
                    .tool_calls
                    .iter_mut()
                    .find(|b| b.item_id.as_deref() == Some(item_id) || b.id == item_id);
                if let Some(builder) = builder {
                    if builder.completed_arguments.is_some() {
                        return Err(tool_identity_conflict(
                            builder.index,
                            "arguments after completion",
                        ));
                    }
                    builder.arguments.push_str(delta);
                    observer.on_event(ModelStreamEvent::ToolArgumentsDelta {
                        index: builder.index,
                        delta: delta.to_string(),
                    });
                }
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item") {
                    self.add_item(
                        item,
                        value
                            .get("output_index")
                            .and_then(serde_json::Value::as_u64),
                        observer,
                    )?;
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
                    let output_index = value
                        .get("output_index")
                        .and_then(serde_json::Value::as_u64);
                    let call_index = self.complete_item(item, output_index, observer)?;
                    self.record_output_item(item, call_index, output_index)?;
                }
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                let item_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if let Some(builder) = self
                    .tool_calls
                    .iter_mut()
                    .find(|b| b.item_id.as_deref() == Some(item_id) || b.id == item_id)
                {
                    if builder.custom != (kind == "response.custom_tool_call_input.done") {
                        return Err(tool_identity_conflict(builder.index, "call kind"));
                    }
                    if let Some(args) = value
                        .get("arguments")
                        .or_else(|| value.get("input"))
                        .and_then(serde_json::Value::as_str)
                    {
                        builder.complete_arguments(args)?;
                    } else {
                        builder.complete_arguments(&builder.arguments.clone())?;
                    }
                }
            }
            "response.completed" => {
                self.terminal = Some(FinishReason::Stop);
                self.observe_terminal_usage(value.get("response"), observer)?;
                self.complete_terminal_items(&value, observer)?;
                for index in 0..self.tool_calls.len() {
                    self.emit_tool_completed(index, observer);
                }
                // A terminal envelope alone does not accept its output items.
                self.saw_terminal = true;
                observer.on_event(ModelStreamEvent::Finished {
                    reason: self.terminal,
                });
                return Ok(true);
            }
            "response.incomplete" => {
                self.observe_terminal_usage(value.get("response"), observer)?;
                self.terminal = Some(terminal::incomplete_reason(&value)?);
                self.complete_terminal_items(&value, observer)?;
                // A truncated builder is not executable. Only explicit done
                // identities may survive an incomplete terminal, regardless
                // of whether its cause is length or content filtering.
                for index in 0..self.tool_calls.len() {
                    if self.tool_calls[index].completed_arguments.is_some() {
                        self.emit_tool_completed(index, observer);
                    }
                }
                self.saw_terminal = true;
                observer.on_event(ModelStreamEvent::Finished {
                    reason: self.terminal,
                });
                return Ok(true);
            }
            "response.failed" => {
                self.saw_terminal = true;
                self.observe_terminal_usage(value.get("response"), observer)?;
                let error = value
                    .get("response")
                    .and_then(|response| response.get("error"));
                return Err(crate::provider_error::from_json_error(
                    None,
                    error,
                    Some("response_failed"),
                    "Responses API response failed",
                    None,
                    None,
                ));
            }
            _ => {}
        }
        Ok(false)
    }

    fn observe_terminal_usage(
        &mut self,
        response: Option<&serde_json::Value>,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        // Even a failed response may report paid work. Validate and deliver
        // its numeric snapshot before returning the provider error.
        let raw = response
            .and_then(|value| value.get("usage"))
            .filter(|value| !value.is_null());
        if let Some(usage) = parse_response_usage(raw)? {
            self.raw_usage = raw.cloned();
            self.usage = Some(usage);
            observer.on_usage(usage, &crate::usage::sanitized_provider_usage(raw.unwrap()));
        }
        Ok(())
    }

    fn add_item(
        &mut self,
        item: &serde_json::Value,
        output_index: Option<u64>,
        observer: &mut dyn DeltaObserver,
    ) -> Result<Option<usize>> {
        let kind = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let item_id = item.get("id").and_then(serde_json::Value::as_str);
        if matches!(kind, "tool_search_call" | "tool_search_output") {
            if self.tool_calls.iter().any(|builder| {
                item_id.is_some_and(|id| builder.item_id.as_deref() == Some(id))
                    || item
                        .get("call_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| builder.id == id)
                    || output_index.is_some_and(|index| builder.output_index == Some(index))
            }) {
                return Err(CoreError::Serialization(
                    "native discovery conflicts with an existing function call".into(),
                ));
            }
            self.native_search
                .record(item, output_index, false, &mut self.output_items)?;
            return Ok(None);
        }
        // Output item ids locate stream fragments; only the explicit call_id
        // pairs an executable tool call with its output. Never synthesize one.
        let identity = if matches!(kind, "function_call" | "custom_tool_call") {
            Some((
                tool_identity_field(item, "call_id")?,
                tool_identity_field(item, "name")?,
            ))
        } else {
            None
        };
        let call_id = identity.map(|(id, _)| id);
        if identity.is_some() && self.native_search.conflicts(item_id, call_id, output_index) {
            return Err(CoreError::Serialization(
                "function call conflicts with native discovery identity".into(),
            ));
        }
        let matches: Vec<_> = self
            .tool_calls
            .iter()
            .filter(|builder| {
                item_id.is_some_and(|id| builder.item_id.as_deref() == Some(id))
                    || call_id.is_some_and(|id| builder.id == id)
                    || output_index.is_some_and(|index| builder.output_index == Some(index))
            })
            .map(|builder| builder.index)
            .collect();
        if let Some(&index) = matches.first() {
            let builder = &mut self.tool_calls[index];
            if matches.len() != 1
                || builder.custom != (kind == "custom_tool_call")
                || !matches!(kind, "function_call" | "custom_tool_call")
                || Some(builder.id.as_str()) != call_id
                || builder.item_id.as_deref() != item_id
                || Some(builder.name.as_str()) != identity.map(|(_, name)| name)
                || (builder.output_index.is_some()
                    && output_index.is_some()
                    && builder.output_index != output_index)
            {
                return Err(tool_identity_conflict(index, "item identity"));
            }
            builder.output_index = builder.output_index.or(output_index);
            return Ok(Some(index));
        }
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
            return Ok(None);
        }
        let Some((id, name)) = identity else {
            if kind == "message" {
                self.text_items
                    .start(item, output_index, &mut self.output_items)?;
            }
            return Ok(None);
        };
        let index = self.tool_calls.len();
        let item_id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let id = id.to_string();
        let name = name.to_string();
        self.tool_calls.push(ToolCallBuilder {
            index,
            output_index,
            item_id,
            id: id.clone(),
            name: name.clone(),
            arguments: item
                .get("arguments")
                .or_else(|| item.get("input"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            custom: kind == "custom_tool_call",
            completed_arguments: None,
            completed_emitted: false,
        });
        observer.on_event(ModelStreamEvent::ToolCallStarted {
            index,
            id: Some(id),
            name,
        });
        Ok(Some(index))
    }

    fn complete_item(
        &mut self,
        item: &serde_json::Value,
        output_index: Option<u64>,
        observer: &mut dyn DeltaObserver,
    ) -> Result<Option<usize>> {
        let index = self.add_item(item, output_index, observer)?;
        if let Some(index) = index {
            let builder = &mut self.tool_calls[index];
            let arguments = item
                .get(if builder.custom { "input" } else { "arguments" })
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&builder.arguments)
                .to_string();
            builder.complete_arguments(&arguments)?;
        }
        Ok(index)
    }

    fn complete_terminal_items(
        &mut self,
        event: &serde_json::Value,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        let Some(items) = event
            .get("response")
            .and_then(|response| response.get("output"))
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(());
        };
        let mut seen = std::collections::HashSet::new();
        for (output_index, item) in items.iter().enumerate() {
            if let Some(index) = self.add_item(item, Some(output_index as u64), observer)? {
                if !seen.insert(index) {
                    return Err(tool_identity_conflict(index, "duplicate terminal call"));
                }
                if self.is_incomplete() && self.tool_calls[index].completed_arguments.is_none() {
                    continue;
                }
                self.complete_item(item, Some(output_index as u64), observer)?;
                self.record_output_item(item, Some(index), Some(output_index as u64))?;
            } else if matches!(
                item.get("type").and_then(serde_json::Value::as_str),
                Some("message" | "tool_search_call" | "tool_search_output")
            ) {
                // As with function/custom calls, an incomplete terminal may
                // replay explicit done items but cannot promote a snapshot to
                // a newly executable host call.
                if self.is_incomplete()
                    && matches!(
                        item.get("type").and_then(serde_json::Value::as_str),
                        Some("tool_search_call" | "tool_search_output")
                    )
                    && !self.native_search.completed_at(output_index as u64)
                {
                    continue;
                }
                self.record_output_item(item, None, Some(output_index as u64))?;
            }
        }
        if let Some(builder) = self.tool_calls.iter().find(|builder| {
            !seen.contains(&builder.index)
                && (!self.is_incomplete() || builder.completed_arguments.is_some())
        }) {
            return Err(tool_identity_conflict(builder.index, "terminal output"));
        }
        Ok(())
    }

    fn emit_tool_completed(&mut self, index: usize, observer: &mut dyn DeltaObserver) {
        let Some(builder) = self.tool_calls.iter_mut().find(|b| b.index == index) else {
            return;
        };
        if builder.completed_emitted {
            return;
        }
        let name = builder.name.clone();
        let args = if builder.custom {
            builder.arguments.as_str()
        } else if builder.arguments.trim().is_empty() {
            "{}"
        } else {
            builder.arguments.trim()
        };
        let arguments = if builder.custom {
            serde_json::json!({"patch": args})
        } else {
            serde_json::from_str(args).unwrap_or_else(|e| serde_json::json!({"__invalid_tool_arguments__":true,"raw":args,"parse_error":e.to_string()}))
        };
        let id = builder.id.clone();
        builder.completed_emitted = true;
        observer.on_event(ModelStreamEvent::ToolCallCompleted {
            index,
            id,
            name,
            arguments,
        });
    }

    fn record_output_item(
        &mut self,
        item: &serde_json::Value,
        call_index: Option<usize>,
        output_index: Option<u64>,
    ) -> Result<()> {
        let Some(kind) = item.get("type").and_then(serde_json::Value::as_str) else {
            return Ok(());
        };
        if matches!(kind, "tool_search_call" | "tool_search_output") {
            return self
                .native_search
                .record(item, output_index, true, &mut self.output_items);
        }
        if kind == "message" {
            return self
                .text_items
                .complete(item, output_index, &mut self.output_items);
        }
        let parsed = match kind {
            "reasoning" => Some(ResponseOutputItem::Reasoning {
                id: item
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                content: reasoning_text_from_item(item).unwrap_or_default(),
            }),
            "function_call" | "custom_tool_call" => {
                call_index.map(|index| self.tool_calls[index].output_item())
            }
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
            if let ResponseOutputItem::FunctionCall { call_id, .. }
            | ResponseOutputItem::CustomToolCall { call_id, .. } = &parsed
            {
                if let Some(existing) = self.output_items.iter_mut().find(|existing| match existing
                {
                    ResponseOutputItem::FunctionCall { call_id: id, .. }
                    | ResponseOutputItem::CustomToolCall { call_id: id, .. } => id == call_id,
                    _ => false,
                }) {
                    *existing = parsed;
                    return Ok(());
                }
            }
            self.output_items.push(parsed);
        }
        Ok(())
    }

    fn is_incomplete(&self) -> bool {
        matches!(
            self.terminal,
            Some(FinishReason::Length | FinishReason::ContentFilter)
        )
    }

    pub fn finish(mut self) -> Result<Response> {
        self.sequence.ensure_valid()?;
        if !self.saw_terminal {
            return Err(CoreError::other(
                "Responses stream ended without a terminal response event",
            ));
        }
        let incomplete = self.is_incomplete();
        let mut fallback_items = Vec::new();
        for builder in self.tool_calls.drain(..) {
            // Responses exposes explicit argument/item completion events. An
            // incomplete terminal proves that any builder without one is only a
            // partial stream fragment, so it must not be synthesized into an
            // executable `{}`/partial JSON call.  For a completed response we
            // retain the historical fallback for providers that put the full
            // arguments on `output_item.added` but omit the separate done
            // event; that compatibility path cannot turn a truncated turn
            // into a call because it is disabled for `response.incomplete`.
            if incomplete && !builder.completed_emitted {
                continue;
            }
            fallback_items.push(builder.output_item());
        }
        let (mut output_items, text_fallback_start) = self.text_items.finish(self.output_items);
        if !self.reasoning.is_empty() {
            if let Some(ResponseOutputItem::Reasoning { content, .. }) = output_items
                .iter_mut()
                .find(|item| matches!(item, ResponseOutputItem::Reasoning { .. }))
            {
                if content.is_empty() {
                    *content = self.reasoning.clone();
                }
            } else {
                let reasoning = ResponseOutputItem::Reasoning {
                    id: None,
                    content: self.reasoning.clone(),
                };
                // Legacy reasoning deltas still precede their text fallback.
                // Completed native items keep their existing relative order.
                if let Some(index) = text_fallback_start {
                    output_items.insert(index, reasoning);
                } else {
                    fallback_items.insert(0, reasoning);
                }
            }
        }
        for item in fallback_items {
            if !response_item_already_present(&output_items, &item) {
                output_items.push(item);
            }
        }
        Ok(Response::from_parts(
            output_items,
            self.terminal,
            self.usage,
            self.raw_usage,
        ))
    }
}

fn response_item_already_present(
    items: &[ResponseOutputItem],
    candidate: &ResponseOutputItem,
) -> bool {
    match candidate {
        ResponseOutputItem::ToolSearchCall { .. } | ResponseOutputItem::ToolSearchOutput { .. } => {
            items.contains(candidate)
        }
        // Messages are reconciled by provider identity, never text equality.
        // InputMessage is input-only and is never produced as a model output.
        ResponseOutputItem::Message { .. } | ResponseOutputItem::InputMessage { .. } => false,
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

fn reasoning_text_from_item(item: &serde_json::Value) -> Option<String> {
    match item.get("content")? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if matches!(
                    part.get("type").and_then(serde_json::Value::as_str),
                    Some("reasoning_text" | "text")
                ) {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        out.push_str(text);
                    }
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn parse_response_usage(value: Option<&serde_json::Value>) -> Result<Option<Usage>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let prompt_tokens_raw = response_usage_count(value.get("input_tokens"), "input_tokens")?;
    let prompt_cache_hit_tokens_raw = response_usage_count(
        value
            .get("input_tokens_details")
            .and_then(|v| v.get("cached_tokens")),
        "input_tokens_details.cached_tokens",
    )?;
    if prompt_cache_hit_tokens_raw > prompt_tokens_raw {
        return Err(CoreError::invalid(format!(
            "Responses usage invariant violated: input_tokens_details.cached_tokens ({prompt_cache_hit_tokens_raw}) exceeds input_tokens ({prompt_tokens_raw})"
        )));
    }
    let prompt_tokens = checked_response_usage_count(prompt_tokens_raw, "input_tokens")?;
    let prompt_cache_hit_tokens = checked_response_usage_count(
        prompt_cache_hit_tokens_raw,
        "input_tokens_details.cached_tokens",
    )?;
    Ok(Some(Usage {
        prompt_tokens,
        completion_tokens: checked_response_usage_count(
            response_usage_count(value.get("output_tokens"), "output_tokens")?,
            "output_tokens",
        )?,
        reasoning_tokens: checked_response_usage_count(
            response_usage_count(
                value
                    .get("output_tokens_details")
                    .and_then(|v| v.get("reasoning_tokens")),
                "output_tokens_details.reasoning_tokens",
            )?,
            "output_tokens_details.reasoning_tokens",
        )?,
        total_tokens: checked_response_usage_count(
            response_usage_count(value.get("total_tokens"), "total_tokens")?,
            "total_tokens",
        )?,
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens: prompt_tokens.saturating_sub(prompt_cache_hit_tokens),
    }))
}

fn response_usage_count(value: Option<&serde_json::Value>, field: &str) -> Result<u64> {
    let Some(value) = value else {
        return Ok(0);
    };
    value.as_u64().ok_or_else(|| {
        CoreError::invalid(format!(
            "Responses usage field `{field}` must be a non-negative integer"
        ))
    })
}

fn checked_response_usage_count(value: u64, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| {
        CoreError::invalid(format!(
            "Responses usage field `{field}` exceeds host u32 range: {value}"
        ))
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
        /// Latest safe lifecycle snapshot for this physical response, if provided.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        /// Observed server model, not the requested model or billing authority.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence_number: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence_gap: Option<u64>,
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
    /// A provider usage snapshot arrived. The default preserves the existing
    /// semantic event; observers needing failure/cancellation accounting may
    /// retain the numeric report before stream assembly can fail.
    fn on_usage(&mut self, usage: Usage, _raw: &serde_json::Value) {
        self.on_event(ModelStreamEvent::Usage { usage });
    }

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

#[derive(Debug)]
struct ToolCallBuilder {
    index: usize,
    output_index: Option<u64>,
    item_id: Option<String>,
    id: String,
    name: String,
    arguments: String,
    custom: bool,
    completed_arguments: Option<String>,
    completed_emitted: bool,
}

impl ToolCallBuilder {
    fn output_item(&self) -> ResponseOutputItem {
        if self.custom {
            ResponseOutputItem::CustomToolCall {
                call_id: self.id.clone(),
                name: self.name.clone(),
                input: self.arguments.clone(),
            }
        } else {
            ResponseOutputItem::FunctionCall {
                call_id: self.id.clone(),
                name: self.name.clone(),
                arguments: if self.arguments.trim().is_empty() {
                    "{}".into()
                } else {
                    self.arguments.trim().to_string()
                },
            }
        }
    }

    fn complete_arguments(&mut self, arguments: &str) -> Result<()> {
        if let Some(previous) = &self.completed_arguments {
            let equivalent = if self.custom {
                previous == arguments
            } else {
                let json = |text: &str| {
                    serde_json::from_str::<serde_json::Value>(if text.trim().is_empty() {
                        "{}"
                    } else {
                        text
                    })
                };
                match (json(previous), json(arguments)) {
                    (Ok(previous), Ok(arguments)) => previous == arguments,
                    _ => previous.trim() == arguments.trim(),
                }
            };
            if !equivalent {
                return Err(tool_identity_conflict(self.index, "completed arguments"));
            }
        } else {
            self.completed_arguments = Some(arguments.to_string());
        }
        self.arguments = arguments.to_string();
        Ok(())
    }
}

fn tool_identity_field<'a>(item: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    item.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CoreError::provider(
                None,
                Some("malformed_tool_call".into()),
                format!("Responses tool call requires a non-empty string {field}"),
            )
        })
}

fn tool_identity_conflict(index: usize, field: &str) -> CoreError {
    // Do not include untrusted tool names or argument contents in diagnostics.
    CoreError::provider(
        None,
        Some("tool_call_identity_conflict".to_string()),
        format!("Responses tool call {index} has conflicting {field}"),
    )
}

#[cfg(test)]
#[path = "stream_identity_tests.rs"]
mod identity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_sequence::NumberedAccumulator as ResponseAccumulator;

    fn event(acc: &mut ResponseAccumulator, json: &str) {
        acc.push_sse_data_observed(json, &mut NoopObserver).unwrap();
    }

    fn complete(acc: &mut ResponseAccumulator, output: serde_json::Value) {
        event(
            acc,
            &serde_json::json!({"type":"response.completed","response":{
                "id":"fixture-response","object":"response","created_at":1,
                "status":"completed","model":"deepseek-flash","output":output
            }})
            .to_string(),
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
        assert!(
            rec.completed.is_empty(),
            "completion identity is provisional until terminal"
        );
        acc.push_sse_data_observed(
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[{"type":"function_call","id":"fc_0","call_id":"c0","name":"search","arguments":"{}"}],"status":"completed"}}"#,
            &mut rec,
        )
        .unwrap();

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
        complete(&mut acc, serde_json::json!([]));
        let resp = acc.finish().unwrap();
        assert_eq!(resp.output_text_projection(), "Hello, world");
        assert!(resp.reasoning_text_projection().is_none());
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
        complete(&mut acc, serde_json::json!([]));

        let resp = acc.finish().unwrap();
        assert_eq!(resp.output_text_projection(), delta);
        assert!(resp.output_text_projection().contains("```echarts"));
        assert!(resp.output_text_projection().contains("$E=mc^2$"));
        assert!(resp.output_text_projection().contains("\\ce{H2O}"));
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
        complete(&mut acc, serde_json::json!([]));
        let resp = acc.finish().unwrap();
        assert_eq!(
            resp.reasoning_text_projection().as_deref(),
            Some("Let me think... the answer is 4.")
        );
        assert_eq!(resp.output_text_projection(), "4");
    }

    #[test]
    fn preserves_reasoning_item_id_and_content_parts_for_tool_continuation() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.reasoning_text.delta","delta":"inspect first"}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_42","content":[{"type":"reasoning_text","text":"inspect first"}]}}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"fixture.txt\"}"}}"#,
        );
        complete(
            &mut acc,
            serde_json::json!([{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"fixture.txt\"}"}]),
        );

        let response = acc.finish().unwrap();
        let reasoning: Vec<_> = response
            .output_items
            .iter()
            .filter_map(|item| match item {
                ResponseOutputItem::Reasoning { id, content } => {
                    Some((id.as_deref(), content.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, vec![(Some("rs_42"), "inspect first")]);
    }

    #[test]
    fn completed_reasoning_items_do_not_receive_joined_global_fallback() {
        let mut acc = ResponseAccumulator::new();
        for (output_index, id, text) in [(0, "rs_1", "first "), (1, "rs_2", "second")] {
            event(
                &mut acc,
                &serde_json::json!({
                    "type":"response.output_item.added",
                    "output_index":output_index,
                    "item":{"type":"reasoning","id":id,"status":"in_progress","content":[]}
                })
                .to_string(),
            );
            event(
                &mut acc,
                &serde_json::json!({
                    "type":"response.reasoning_text.delta",
                    "item_id":id,
                    "output_index":output_index,
                    "content_index":0,
                    "delta":text
                })
                .to_string(),
            );
            event(
                &mut acc,
                &serde_json::json!({
                    "type":"response.output_item.done",
                    "output_index":output_index,
                    "item":{"type":"reasoning","id":id,"status":"completed",
                        "content":[{"type":"reasoning_text","text":text}]}
                })
                .to_string(),
            );
        }
        complete(&mut acc, serde_json::json!([]));

        let response = acc.finish().unwrap();
        let reasoning: Vec<_> = response
            .output_items
            .iter()
            .filter_map(|item| match item {
                ResponseOutputItem::Reasoning { id, content } => {
                    Some((id.as_deref(), content.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            reasoning,
            vec![(Some("rs_1"), "first "), (Some("rs_2"), "second")]
        );
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
        complete(
            &mut acc,
            serde_json::json!([{"type":"function_call","id":"fc_abc","call_id":"call_abc","name":"add","arguments":"{\"a\": 2, \"b\": 3}"}]),
        );

        let resp = acc.finish().unwrap();
        assert_eq!(resp.finish_reason, Some(FinishReason::Stop));
        let calls = resp.tool_invocations_from_items();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_abc");
        assert_eq!(calls[0].1, "add");
        assert_eq!(calls[0].2, serde_json::json!({"a": 2, "b": 3}));
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
        complete(
            &mut acc,
            serde_json::json!([{"type":"function_call","id":"fc_0","call_id":"c0","name":"f0","arguments":"{}"},{"type":"function_call","id":"fc_1","call_id":"c1","name":"f1","arguments":"{\"x\": 1}"}]),
        );
        let resp = acc.finish().unwrap();
        let calls = resp.tool_invocations_from_items();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "f0");
        assert_eq!(calls[1].2, serde_json::json!({"x": 1}));
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
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
                &mut NoopObserver,
            )
            .unwrap());
        let resp = acc.finish().unwrap();
        assert_eq!(resp.output_text_projection(), "hi");
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
        complete(
            &mut acc,
            serde_json::json!([{"type":"function_call","id":"fc","call_id":"c","name":"noargs","arguments":""}]),
        );
        let resp = acc.finish().unwrap();
        assert_eq!(
            resp.tool_invocations_from_items()[0].2,
            serde_json::json!({})
        );
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
        complete(
            &mut acc,
            serde_json::json!([{"type":"function_call","id":"fc","call_id":"bad-call","name":"bad","arguments":"{not json"}]),
        );
        let resp = acc.finish().unwrap();
        let calls = resp.tool_invocations_from_items();
        let arguments = &calls[0].2;
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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"status":"completed","model":"deepseek-flash","output":[],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#,
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
        complete(&mut acc, serde_json::json!([]));

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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"status":"completed","model":"deepseek-flash","output":[{"type":"custom_tool_call","id":"item_1","call_id":"call_1","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"}],"usage":{"input_tokens":3,"output_tokens":2,"output_tokens_details":{"reasoning_tokens":1},"total_tokens":5}}}"#,
            &mut NoopObserver,
        ).unwrap();
        let response = acc.finish().unwrap();
        let calls = response.tool_invocations_from_items();
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].2["patch"], "*** Begin Patch\n*** End Patch");
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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[{"type":"custom_tool_call","id":"item_1","call_id":"call_1","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"}],"status":"completed"}}"#,
            &mut NoopObserver,
        )
        .unwrap();

        let response = acc.finish().unwrap();
        assert_eq!(
            response.tool_invocations_from_items()[0].2["patch"],
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
                r#"{"type":"response.incomplete","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":4,"output_tokens":5,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":3},"total_tokens":9}}}"#,
                &mut rec,
            )
            .unwrap());

        assert!(rec.0.iter().any(|event| matches!(
            event,
            ModelStreamEvent::Usage { usage }
                if usage.prompt_tokens == 4
                    && usage.prompt_cache_hit_tokens == 2
                    && usage.prompt_cache_miss_tokens == 2
                    && usage.reasoning_tokens == 3
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

    #[derive(Default)]
    struct TerminalUsageRecorder {
        events: Vec<ModelStreamEvent>,
        snapshots: Vec<(Usage, serde_json::Value)>,
    }

    impl DeltaObserver for TerminalUsageRecorder {
        fn on_usage(&mut self, usage: Usage, raw: &serde_json::Value) {
            self.snapshots.push((usage, raw.clone()));
            self.on_event(ModelStreamEvent::Usage { usage });
        }

        fn on_event(&mut self, event: ModelStreamEvent) {
            self.events.push(event);
        }
    }

    #[test]
    fn responses_terminal_usage_preserves_numeric_snapshot_before_result() {
        use serde_json::json;
        for kind in [
            "response.completed",
            "response.incomplete",
            "response.failed",
        ] {
            let raw = json!({"input_tokens":9,"output_tokens":2,"total_tokens":11,
                "input_tokens_details":{"cached_tokens":4,"text":"provider-text-canary"},
                "output_tokens_details":{"reasoning_tokens":1},
                "unknown":"provider-text-canary"});
            let mut acc = ResponseAccumulator::new();
            let mut observer = TerminalUsageRecorder::default();
            let result = acc.push_sse_data_observed(
                &json!({"type":kind,"response":{"id":"fixture-response","object":"response","created_at":1,"status":kind.strip_prefix("response.").unwrap(),"model":"deepseek-flash","output":[],"usage":raw,
                    "error":{"code":"audit_failed","message":"controlled failure"},
                    "incomplete_details":if kind=="response.incomplete" {json!({"reason":"max_output_tokens"})} else {serde_json::Value::Null}}})
                .to_string(),
                &mut observer,
            );
            assert_eq!(observer.snapshots.len(), 1, "{kind}");
            let (usage, raw_observed) = &observer.snapshots[0];
            assert_eq!(
                *usage,
                Usage {
                    prompt_tokens: 9,
                    completion_tokens: 2,
                    total_tokens: 11,
                    prompt_cache_hit_tokens: 4,
                    prompt_cache_miss_tokens: 5,
                    reasoning_tokens: 1
                }
            );
            assert_eq!(*raw_observed, crate::usage::sanitized_provider_usage(&raw));
            assert!(!raw_observed.to_string().contains("provider-text-canary"));
            assert_eq!(
                observer
                    .events
                    .iter()
                    .filter(|event| matches!(event, ModelStreamEvent::Usage { .. }))
                    .count(),
                1
            );
            if kind == "response.failed" {
                assert!(matches!(result, Err(CoreError::Provider { code, .. })
                    if code.as_deref() == Some("audit_failed")));
                assert!(!observer
                    .events
                    .iter()
                    .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
            } else {
                assert!(result.unwrap());
                let response = acc.finish().unwrap();
                assert_eq!(response.usage, Some(*usage));
                assert_eq!(response.raw_usage, Some(raw));
                assert!(matches!(
                    observer.events.last(),
                    Some(ModelStreamEvent::Finished { .. })
                ));
            }
        }
    }

    #[test]
    fn responses_terminal_missing_or_null_usage_is_not_a_measured_zero() {
        use serde_json::{json, Value};
        for kind in [
            "response.completed",
            "response.incomplete",
            "response.failed",
        ] {
            for present in [false, true] {
                let mut response = json!({"id":"fixture-response","object":"response","created_at":1,"status":kind.strip_prefix("response.").unwrap(),"model":"deepseek-flash","output":[],"error":{"code":"audit_failed"}});
                if kind == "response.incomplete" {
                    response["incomplete_details"] = json!({"reason":"max_output_tokens"});
                }
                if present {
                    response["usage"] = Value::Null;
                }
                let mut acc = ResponseAccumulator::new();
                let mut observer = TerminalUsageRecorder::default();
                let result = acc.push_sse_data_observed(
                    &json!({"type":kind,"response":response}).to_string(),
                    &mut observer,
                );
                assert!(observer.snapshots.is_empty(), "{kind}, null={present}");
                assert!(!observer
                    .events
                    .iter()
                    .any(|event| matches!(event, ModelStreamEvent::Usage { .. })));
                if kind == "response.failed" {
                    assert!(result.is_err());
                } else {
                    assert!(result.unwrap());
                    let response = acc.finish().unwrap();
                    assert!(response.usage.is_none());
                    assert!(response.raw_usage.is_none());
                }
            }
        }
    }

    #[test]
    fn incomplete_response_drops_added_but_unfinished_tool_call() {
        let mut acc = ResponseAccumulator::new();
        event(
            &mut acc,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"item_cut","call_id":"call_cut","name":"write_file","arguments":""}}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.function_call_arguments.delta","item_id":"item_cut","delta":"{\"path\":"}"#,
        );
        event(
            &mut acc,
            r#"{"type":"response.incomplete","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        );

        let response = acc.finish().unwrap();
        assert_eq!(response.finish_reason, Some(FinishReason::Length));
        assert!(response.tool_invocations_from_items().is_empty());
        assert!(!response.output_items.iter().any(|item| matches!(
            item,
            ResponseOutputItem::FunctionCall { call_id, .. } if call_id == "call_cut"
        )));
    }

    #[test]
    fn responses_usage_without_cached_tokens_counts_all_input_as_miss() {
        let usage = parse_response_usage(Some(&serde_json::json!({
            "input_tokens": 37,
            "output_tokens": 5,
            "total_tokens": 42
        })))
        .unwrap()
        .unwrap();
        assert_eq!(usage.prompt_tokens, 37);
        assert_eq!(usage.prompt_cache_hit_tokens, 0);
        assert_eq!(usage.prompt_cache_miss_tokens, 37);

        let inconsistent = parse_response_usage(Some(&serde_json::json!({
            "input_tokens": 3,
            "input_tokens_details": {"cached_tokens": 8}
        })))
        .unwrap_err();
        assert!(inconsistent.to_string().contains("cached_tokens"));
    }

    #[test]
    fn responses_usage_accepts_zero_partial_and_full_cache() {
        for (cached, expected_miss) in [(0, 3), (2, 1), (3, 0)] {
            let usage = parse_response_usage(Some(&serde_json::json!({
                "input_tokens": 3,
                "input_tokens_details": {"cached_tokens": cached},
            })))
            .unwrap()
            .unwrap();
            assert_eq!(usage.prompt_cache_hit_tokens, cached);
            assert_eq!(usage.prompt_cache_miss_tokens, expected_miss);
        }
    }

    #[test]
    fn responses_incomplete_rejects_inconsistent_cached_usage() {
        let mut accumulator = ResponseAccumulator::new();
        let error = accumulator
            .push_sse_data_observed(
                r#"{"type":"response.incomplete","response":{"id":"fixture-response","object":"response","created_at":1,"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"model":"deepseek-flash","output":[],"usage":{"input_tokens":3,"input_tokens_details":{"cached_tokens":4}}}}"#,
                &mut NoopObserver,
            )
            .unwrap_err();
        assert!(error.to_string().contains("cached_tokens"));
    }

    #[test]
    fn responses_usage_rejects_counts_wider_than_host_type() {
        let oversized = u64::from(u32::MAX) + 1;
        for (field, usage) in [
            (
                "input_tokens",
                serde_json::json!({"input_tokens": oversized}),
            ),
            (
                "output_tokens",
                serde_json::json!({"output_tokens": oversized}),
            ),
            (
                "output_tokens_details.reasoning_tokens",
                serde_json::json!({
                    "output_tokens_details": {"reasoning_tokens": oversized}
                }),
            ),
            (
                "total_tokens",
                serde_json::json!({"total_tokens": oversized}),
            ),
            (
                "input_tokens_details.cached_tokens",
                serde_json::json!({
                    "input_tokens": u32::MAX,
                    "input_tokens_details": {"cached_tokens": oversized}
                }),
            ),
        ] {
            let error = parse_response_usage(Some(&usage)).unwrap_err();
            assert!(
                error.to_string().contains(field),
                "error for {field} did not identify the field: {error}"
            );
        }
    }

    #[test]
    fn responses_usage_accepts_u32_maximum_counts() {
        let maximum = u64::from(u32::MAX);
        let usage = parse_response_usage(Some(&serde_json::json!({
            "input_tokens": maximum,
            "input_tokens_details": {"cached_tokens": maximum},
            "output_tokens": maximum,
            "output_tokens_details": {"reasoning_tokens": maximum},
            "total_tokens": maximum,
        })))
        .unwrap()
        .unwrap();
        assert_eq!(usage.prompt_tokens, u32::MAX);
        assert_eq!(usage.prompt_cache_hit_tokens, u32::MAX);
        assert_eq!(usage.prompt_cache_miss_tokens, 0);
        assert_eq!(usage.completion_tokens, u32::MAX);
        assert_eq!(usage.reasoning_tokens, u32::MAX);
        assert_eq!(usage.total_tokens, u32::MAX);
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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"status":"completed","model":"deepseek-flash","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
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
