//! DeepSeek Chat Completions request serialization and streaming assembly.
//!
//! This module owns the provider wire format. The runtime consumes the
//! provider-neutral [`ModelStreamEvent`] and [`Response`] types, so adding the
//! Chat Completions route does not create a second agent loop or event model.

use std::collections::BTreeMap;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role};
use deepagent_core::response_item::ResponseOutputItem;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

use crate::chat::{FinishReason, Response, ThinkingToggle, ToolSchema, Usage};
use crate::stream::{DeltaObserver, ModelStreamEvent};

/// A DeepSeek Chat Completions request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub stream: bool,
    pub stream_options_include_usage: bool,
    pub thinking: Option<ThinkingToggle>,
    pub reasoning_effort: Option<String>,
    pub tools: Vec<ToolSchema>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub tool_choice: Option<serde_json::Value>,
    pub user: Option<String>,
}

impl Serialize for ChatCompletionRequest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("model", &self.model)?;
        map.serialize_entry("messages", &ChatMessages(&self.messages))?;
        map.serialize_entry("stream", &self.stream)?;
        if self.stream_options_include_usage {
            map.serialize_entry(
                "stream_options",
                &serde_json::json!({"include_usage": true}),
            )?;
        }
        if let Some(thinking) = &self.thinking {
            map.serialize_entry("thinking", thinking)?;
        }
        if let Some(effort) = &self.reasoning_effort {
            map.serialize_entry("reasoning_effort", effort)?;
        }
        if !self.tools.is_empty() {
            let tools = self
                .tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.function.name,
                            "description": tool.function.description,
                            "parameters": tool.function.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>();
            map.serialize_entry("tools", &tools)?;
        }
        if let Some(value) = self.temperature {
            map.serialize_entry("temperature", &value)?;
        }
        if let Some(value) = self.max_tokens {
            map.serialize_entry("max_tokens", &value)?;
        }
        if let Some(value) = self.top_p {
            map.serialize_entry("top_p", &value)?;
        }
        if let Some(value) = &self.tool_choice {
            map.serialize_entry("tool_choice", value)?;
        }
        if let Some(value) = &self.user {
            map.serialize_entry("user", value)?;
        }
        map.end()
    }
}

impl ChatCompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            stream_options_include_usage: true,
            thinking: None,
            reasoning_effort: None,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            top_p: None,
            tool_choice: None,
            user: None,
        }
    }

    pub fn streaming(mut self) -> Self {
        self.stream = true;
        self
    }

    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        let effort = effort.into();
        match effort.as_str() {
            "off" => {
                self.thinking = Some(ThinkingToggle {
                    kind: "disabled".to_string(),
                });
                self.reasoning_effort = None;
            }
            "low" | "high" | "max" => {
                self.thinking = Some(ThinkingToggle {
                    kind: "enabled".to_string(),
                });
                self.reasoning_effort = Some(effort);
            }
            _ => {
                self.thinking = None;
                self.reasoning_effort = Some(effort);
            }
        }
        self
    }

    pub fn with_thinking(mut self, enabled: bool) -> Self {
        self.thinking = Some(ThinkingToggle {
            kind: if enabled { "enabled" } else { "disabled" }.to_string(),
        });
        if !enabled {
            self.reasoning_effort = None;
        }
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolSchema>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_temperature(mut self, value: f32) -> Self {
        self.temperature = Some(value);
        self
    }

    pub fn with_max_tokens(mut self, value: u32) -> Self {
        self.max_tokens = Some(value);
        self
    }

    pub fn with_top_p(mut self, value: f32) -> Self {
        self.top_p = Some(value);
        self
    }

    pub fn with_tool_choice(mut self, value: serde_json::Value) -> Self {
        self.tool_choice = Some(value);
        self
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(effort) = self.reasoning_effort.as_deref() {
            if !matches!(effort, "low" | "high" | "max") {
                return Err(CoreError::invalid(format!(
                    "unsupported reasoning effort: {effort}"
                )));
            }
        }
        if self
            .thinking
            .as_ref()
            .is_some_and(|thinking| thinking.kind == "disabled")
            && self.reasoning_effort.is_some()
        {
            return Err(CoreError::invalid(
                "reasoning effort cannot be enabled when thinking is disabled",
            ));
        }
        if self.tools.iter().any(|tool| tool.kind != "function") {
            return Err(CoreError::invalid(
                "DeepSeek Chat Completions supports function tools only",
            ));
        }
        Ok(())
    }
}

struct ChatMessages<'a>(&'a [Message]);

impl Serialize for ChatMessages<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for message in self.0 {
            let value = serialize_message(message)
                .map_err(|error| serde::ser::Error::custom(error.to_string()))?;
            sequence.serialize_element(&value)?;
        }
        sequence.end()
    }
}

fn serialize_message(message: &Message) -> serde_json::Result<serde_json::Value> {
    let mut value = serde_json::Map::new();
    value.insert(
        "role".to_string(),
        serde_json::Value::String(message.role.as_str().to_string()),
    );
    match message.role {
        Role::Assistant => {
            value.insert(
                "content".to_string(),
                serde_json::Value::String(message.content.clone()),
            );
            if let Some(reasoning) = message
                .reasoning_content
                .as_deref()
                .filter(|reasoning| !reasoning.is_empty())
            {
                value.insert(
                    "reasoning_content".to_string(),
                    serde_json::Value::String(reasoning.to_string()),
                );
            }
            if !message.tool_calls.is_empty() {
                let calls = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        let arguments = serde_json::to_string(&call.arguments)?;
                        Ok(serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": arguments,
                            }
                        }))
                    })
                    .collect::<serde_json::Result<Vec<_>>>()?;
                value.insert("tool_calls".to_string(), serde_json::Value::Array(calls));
            }
        }
        Role::Tool => {
            value.insert(
                "tool_call_id".to_string(),
                serde_json::Value::String(message.tool_call_id.clone().unwrap_or_default()),
            );
            value.insert(
                "content".to_string(),
                serde_json::Value::String(if message.content.is_empty() {
                    "(no output)".to_string()
                } else {
                    message.content.clone()
                }),
            );
        }
        Role::System | Role::User => {
            value.insert(
                "content".to_string(),
                serde_json::Value::String(message.content.clone()),
            );
        }
    }
    Ok(serde_json::Value::Object(value))
}

#[derive(Debug, Default)]
pub struct ChatCompletionAccumulator {
    content: String,
    reasoning: String,
    tool_calls: BTreeMap<usize, ChatToolCallBuilder>,
    usage: Option<Usage>,
    raw_usage: Option<serde_json::Value>,
    finish_reason: Option<FinishReason>,
    saw_done: bool,
}

#[derive(Debug, Default)]
struct ChatToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    started: bool,
    completed: bool,
}

impl ChatCompletionAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_sse_data(&mut self, data: &str) -> Result<bool> {
        let mut observer = crate::stream::NoopObserver;
        self.push_sse_data_observed(data, &mut observer)
    }

    pub fn push_sse_data_observed(
        &mut self,
        data: &str,
        observer: &mut dyn DeltaObserver,
    ) -> Result<bool> {
        if self.saw_done {
            return Err(CoreError::invalid(
                "Chat Completions emitted data after [DONE]",
            ));
        }
        if data.trim() == "[DONE]" {
            self.saw_done = true;
            if self.finish_reason.is_none() {
                self.finish_reason = Some(FinishReason::Stop);
            }
            self.complete_tool_calls(observer);
            if self.content.is_empty() && self.reasoning.is_empty() && self.tool_calls.is_empty() {
                return Err(CoreError::provider(
                    None,
                    Some("empty_response".into()),
                    "model returned a completed response with no content",
                ));
            }
            observer.on_event(ModelStreamEvent::Finished {
                reason: self.finish_reason,
            });
            return Ok(true);
        }

        let value: serde_json::Value = serde_json::from_str(data.trim()).map_err(|error| {
            CoreError::Serialization(format!("bad Chat Completions chunk: {error}"))
        })?;
        observer.on_event(ModelStreamEvent::ResponseStreamEvent {
            event_type: "chat.completion.chunk".to_string(),
            item_id: value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            item_type: None,
            delta_chars: None,
        });

        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            let parsed = parse_chat_usage(usage);
            self.usage = Some(parsed);
            self.raw_usage = Some(usage.clone());
            observer.on_event(ModelStreamEvent::Usage { usage: parsed });
        }

        if let Some(choices) = value.get("choices").and_then(serde_json::Value::as_array) {
            for choice in choices {
                let delta = choice.get("delta").cloned().unwrap_or_default();
                if let Some(text) = delta
                    .get("reasoning_content")
                    .and_then(serde_json::Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    self.reasoning.push_str(text);
                    observer.on_event(ModelStreamEvent::ReasoningDelta {
                        text: text.to_string(),
                    });
                }
                if let Some(text) = delta
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    self.content.push_str(text);
                    observer.on_event(ModelStreamEvent::ContentDelta {
                        text: text.to_string(),
                    });
                }
                if let Some(tool_calls) = delta
                    .get("tool_calls")
                    .and_then(serde_json::Value::as_array)
                {
                    for tool_call in tool_calls {
                        self.push_tool_call(tool_call, observer)?;
                    }
                }
                if let Some(reason) = choice
                    .get("finish_reason")
                    .and_then(serde_json::Value::as_str)
                {
                    self.finish_reason = Some(map_finish_reason(reason)?);
                }
            }
        }
        Ok(false)
    }

    fn push_tool_call(
        &mut self,
        value: &serde_json::Value,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        let index = value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| CoreError::invalid("tool call delta is missing index"))?
            as usize;
        let builder = self.tool_calls.entry(index).or_default();
        if let Some(id) = value.get("id").and_then(serde_json::Value::as_str) {
            builder.id = Some(id.to_string());
        }
        let function = value.get("function").cloned().unwrap_or_default();
        if let Some(name) = function.get("name").and_then(serde_json::Value::as_str) {
            builder.name = Some(name.to_string());
        }
        if !builder.started {
            if let Some(name) = builder.name.clone() {
                builder.started = true;
                observer.on_event(ModelStreamEvent::ToolCallStarted {
                    index,
                    id: builder.id.clone(),
                    name,
                });
            }
        }
        let fragment = function
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !fragment.is_empty() {
            builder.arguments.push_str(fragment);
            observer.on_event(ModelStreamEvent::ToolArgumentsDelta {
                index,
                delta: fragment.to_string(),
            });
        }
        Ok(())
    }

    fn complete_tool_calls(&mut self, observer: &mut dyn DeltaObserver) {
        for (index, builder) in &mut self.tool_calls {
            if builder.completed {
                continue;
            }
            let id = builder
                .id
                .clone()
                .unwrap_or_else(|| format!("call_stream_{index}"));
            let name = builder.name.clone().unwrap_or_else(|| "tool".to_string());
            let arguments = parse_tool_arguments(&builder.arguments);
            builder.completed = true;
            observer.on_event(ModelStreamEvent::ToolCallCompleted {
                index: *index,
                id,
                name,
                arguments,
            });
        }
    }

    pub fn finish(self) -> Result<Response> {
        if !self.saw_done {
            return Err(CoreError::other(
                "Chat Completions stream ended without [DONE]",
            ));
        }
        let mut output_items = Vec::new();
        if !self.reasoning.is_empty() {
            output_items.push(ResponseOutputItem::Reasoning {
                id: None,
                content: self.reasoning,
            });
        }
        for (index, builder) in self.tool_calls {
            output_items.push(ResponseOutputItem::FunctionCall {
                call_id: builder.id.unwrap_or_else(|| format!("call_stream_{index}")),
                name: builder.name.unwrap_or_else(|| "tool".to_string()),
                arguments: if builder.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    builder.arguments
                },
            });
        }
        if !self.content.is_empty() || output_items.is_empty() {
            output_items.push(ResponseOutputItem::Message {
                role: "assistant".to_string(),
                content: self.content,
            });
        }
        Ok(Response::from_parts(
            output_items,
            self.finish_reason,
            self.usage,
            self.raw_usage,
        ))
    }
}

fn parse_tool_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|error| {
        serde_json::json!({
            "__invalid_tool_arguments__": true,
            "raw": raw,
            "parse_error": error.to_string(),
        })
    })
}

fn map_finish_reason(reason: &str) -> Result<FinishReason> {
    match reason {
        "stop" => Ok(FinishReason::Stop),
        "tool_calls" => Ok(FinishReason::ToolCalls),
        "length" => Ok(FinishReason::Length),
        other => Err(CoreError::provider(
            None,
            Some(other.to_string()),
            format!("model stopped with unsupported finish reason: {other}"),
        )),
    }
}

fn parse_chat_usage(value: &serde_json::Value) -> Usage {
    let prompt_tokens = value
        .get("prompt_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let cache_hit = value
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| value.get("prompt_cache_hit_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let cache_miss = value
        .get("prompt_cache_miss_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| prompt_tokens.saturating_sub(cache_hit) as u64)
        as u32;
    let completion_tokens = value
        .get("completion_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    Usage {
        prompt_tokens: prompt_tokens.saturating_sub(cache_hit),
        completion_tokens,
        reasoning_tokens: value
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        total_tokens: value
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or((prompt_tokens + completion_tokens) as u64) as u32,
        prompt_cache_hit_tokens: cache_hit,
        prompt_cache_miss_tokens: cache_miss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{DeltaObserver, ModelStreamEvent};
    use crate::{FinishReason, ToolSchema, Usage};
    use deepagent_core::message::{Message, ToolCall};

    #[test]
    fn serializes_deepseek_thinking_request_and_reasoning_passback() {
        let request = ChatCompletionRequest::new(
            "deepseek-v4-pro",
            vec![
                Message::system("Follow the policy."),
                Message::assistant("")
                    .with_reasoning("I should inspect the file first.")
                    .with_tool_calls(vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "README.md"}),
                    }]),
                Message::tool_result("call-1", "file contents"),
                Message::user("Continue."),
            ],
        )
        .with_reasoning_effort("high")
        .with_tools(vec![ToolSchema::function(
            "read_file",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )])
        .streaming();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "deepseek-v4-pro");
        assert_eq!(json["stream"], true);
        assert_eq!(json["stream_options"]["include_usage"], true);
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["messages"][1]["role"], "assistant");
        assert_eq!(
            json["messages"][1]["reasoning_content"],
            "I should inspect the file first."
        );
        assert_eq!(
            json["messages"][1]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"README.md\"}"
        );
        assert_eq!(json["messages"][2]["role"], "tool");
        assert_eq!(json["messages"][2]["tool_call_id"], "call-1");
    }

    #[test]
    fn off_reasoning_disables_thinking_without_serializing_off_effort() {
        let request = ChatCompletionRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .with_reasoning_effort("off");

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["thinking"]["type"], "disabled");
        assert!(json.get("reasoning_effort").is_none());
    }

    #[test]
    fn serializes_supported_reasoning_efforts() {
        for effort in ["low", "high", "max"] {
            let request =
                ChatCompletionRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
                    .with_reasoning_effort(effort);

            let json = serde_json::to_value(&request).unwrap();
            assert_eq!(json["thinking"]["type"], "enabled");
            assert_eq!(json["reasoning_effort"], effort);
            request.validate().unwrap();
        }
    }

    #[test]
    fn rejects_unsupported_reasoning_effort_before_network() {
        let request = ChatCompletionRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .with_reasoning_effort("medium");

        let error = request.validate().unwrap_err();
        assert!(error.to_string().contains("unsupported reasoning effort"));
    }

    #[test]
    fn accumulates_reasoning_content_and_parallel_tool_call_fragments() {
        let mut accumulator = ChatCompletionAccumulator::new();
        let mut observer = RecordingObserver::default();
        for payload in [
            r#"{"choices":[{"delta":{"reasoning_content":""},"finish_reason":null}]}"#
                .to_string(),
            r#"{"choices":[{"delta":{"reasoning_content":"think ","content":null},"finish_reason":null}]}"#
                .to_string(),
            r#"{"choices":[{"delta":{"reasoning_content":"first","content":"answer "},"finish_reason":null},{"delta":{"content":"now"},"finish_reason":null}]}"#
                .to_string(),
            serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [
                            {"index": 0, "id": "call-a", "type": "function", "function": {"name": "search", "arguments": "{\"q\":"}},
                            {"index": 1, "id": "call-b", "type": "function", "function": {"name": "read", "arguments": "{\"p\":"}}
                        ]
                    },
                    "finish_reason": null
                }]
            })
            .to_string(),
            serde_json::json!({
                "choices": [
                    {"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"rust\"}"}}]}, "finish_reason": null},
                    {"delta": {"tool_calls": [{"index": 1, "function": {"arguments": "\"a\"}"}}]}, "finish_reason": "tool_calls"}
                ]
            })
            .to_string(),
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":4}},"choices":[]}"#
                .to_string(),
            "[DONE]".to_string(),
        ] {
            accumulator
                .push_sse_data_observed(&payload, &mut observer)
                .unwrap();
        }

        let response = accumulator.finish().unwrap();
        assert_eq!(response.output_text_projection(), "answer now");
        assert_eq!(
            response.reasoning_text_projection().as_deref(),
            Some("think first")
        );
        assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(
            response.usage,
            Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 7,
                reasoning_tokens: 4,
                total_tokens: 17,
                prompt_cache_hit_tokens: 3,
                prompt_cache_miss_tokens: 7,
            })
        );
        let calls = response.tool_invocations_from_items();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "call-a");
        assert_eq!(calls[0].2, serde_json::json!({"q": "rust"}));
        assert_eq!(calls[1].0, "call-b");
        assert_eq!(calls[1].2, serde_json::json!({"p": "a"}));
        assert!(observer.events.iter().any(
            |event| matches!(event, ModelStreamEvent::ReasoningDelta { text } if text == "think ")
        ));
    }

    #[test]
    fn requires_done_sentinel_and_rejects_unknown_finish_reason() {
        let mut accumulator = ChatCompletionAccumulator::new();
        accumulator
            .push_sse_data(
                r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"content_filter"}]}"#,
            )
            .expect_err("content_filter must be surfaced as a provider failure");

        let mut closed = ChatCompletionAccumulator::new();
        closed
            .push_sse_data(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}"#)
            .unwrap();
        let error = closed.finish().unwrap_err();
        assert!(error.to_string().contains("[DONE]"));
    }

    #[test]
    fn maps_top_level_cache_hit_and_miss_usage() {
        let mut accumulator = ChatCompletionAccumulator::new();
        accumulator
            .push_sse_data(
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":5,"prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":5,"completion_tokens_details":{"reasoning_tokens":2}}}"#,
            )
            .unwrap();
        accumulator.push_sse_data("[DONE]").unwrap();

        let response = accumulator.finish().unwrap();
        assert_eq!(
            response.usage,
            Some(Usage {
                prompt_tokens: 5,
                completion_tokens: 5,
                reasoning_tokens: 2,
                total_tokens: 14,
                prompt_cache_hit_tokens: 4,
                prompt_cache_miss_tokens: 5,
            })
        );
        assert_eq!(
            response.raw_usage.as_ref().unwrap()["prompt_cache_hit_tokens"],
            4
        );
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<ModelStreamEvent>,
    }

    impl DeltaObserver for RecordingObserver {
        fn on_event(&mut self, event: ModelStreamEvent) {
            self.events.push(event);
        }
    }
}
