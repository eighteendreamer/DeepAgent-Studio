//! Streaming response assembly (开发计划.md Phase 2 §4–§5).
//!
//! DeepSeek streams chat completions as Server-Sent Events. Each `data:` line
//! carries a [`ChatChunk`] with a *delta*: an incremental fragment of the
//! assistant message. The challenge (and the explicit acceptance criteria
//! "无 chunk 丢失" / "tool_calls merge") is that:
//!
//! - `content` and `reasoning_content` arrive as concatenated text fragments,
//! - `tool_calls` arrive as fragments **indexed by position**, where the `name`
//!   appears once and the `arguments` JSON string is streamed character-by-
//!   character across many chunks.
//!
//! [`DeltaAccumulator`] folds these chunks into a single, coherent
//! [`ChatResponse`], preserving the DeepSeek Thinking Mode `reasoning_content`
//! so it can be persisted and replayed.

use serde::Deserialize;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role, ToolCall};

use crate::chat::{ChatResponse, FinishReason, Usage};

/// One streamed chunk (the JSON object after `data:` in an SSE line).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatChunk {
    /// Choices array; we only consume index 0 (single-completion requests).
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// Usage, typically only present on the final chunk.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// A single choice within a chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkChoice {
    /// The incremental delta for this choice.
    #[serde(default)]
    pub delta: Delta,
    /// Finish reason, present on the terminal chunk.
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

/// The incremental delta carried by a chunk.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Delta {
    /// Visible content fragment.
    #[serde(default)]
    pub content: Option<String>,
    /// DeepSeek Thinking Mode reasoning fragment.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Tool-call fragments (indexed).
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

/// A fragment of a tool call. `index` identifies which call this fragment
/// belongs to; `function.arguments` is a partial JSON string to be concatenated.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolCallDelta {
    /// Position of this tool call within the message.
    #[serde(default)]
    pub index: usize,
    /// Provider call id (usually only on the first fragment).
    #[serde(default)]
    pub id: Option<String>,
    /// Function fragment.
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

/// A fragment of a function call.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FunctionDelta {
    /// Function name (usually only on the first fragment).
    #[serde(default)]
    pub name: Option<String>,
    /// Partial arguments JSON, to be concatenated across fragments.
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Observes streaming deltas as they arrive (for live UIs / event streams).
///
/// Distinct from the transport-level [`crate::transport::EventSink`] (raw SSE
/// payloads): this receives *semantic* fragments — visible content, Thinking
/// Mode reasoning, and tool-call starts — already decoded from each chunk. The
/// default no-op impl lets callers ignore deltas (the non-streaming path).
pub trait DeltaObserver: Send {
    /// A visible content fragment arrived.
    fn on_content(&mut self, _delta: &str) {}
    /// A Thinking Mode reasoning fragment arrived.
    fn on_reasoning(&mut self, _delta: &str) {}
    /// A tool call began (its name became known).
    fn on_tool_call(&mut self, _name: &str) {}
}

/// A `DeltaObserver` that ignores everything (the default for `stream_chat`).
pub struct NoopObserver;
impl DeltaObserver for NoopObserver {}

/// Accumulates streamed deltas into a final message.
#[derive(Debug, Default)]
pub struct DeltaAccumulator {
    content: String,
    reasoning: String,
    /// Tool calls under construction, keyed by their stream index. Kept in a
    /// Vec of (index, builder) so iteration order is stable / by-index.
    tool_calls: Vec<ToolCallBuilder>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
}

#[derive(Debug, Default)]
struct ToolCallBuilder {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

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
        }
        let Some(choice) = chunk.choices.first() else {
            return;
        };
        if let Some(reason) = choice.finish_reason {
            self.finish_reason = Some(reason);
        }
        let delta = &choice.delta;
        if let Some(c) = &delta.content {
            self.content.push_str(c);
            if !c.is_empty() {
                observer.on_content(c);
            }
        }
        if let Some(r) = &delta.reasoning_content {
            self.reasoning.push_str(r);
            if !r.is_empty() {
                observer.on_reasoning(r);
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
                    observer.on_tool_call(&name);
                }
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
                    ..Default::default()
                });
                self.tool_calls.last_mut().expect("just pushed a builder")
            }
        };
        if let Some(id) = &delta.id {
            builder.id = Some(id.clone());
        }
        if let Some(func) = &delta.function {
            if let Some(name) = &func.name {
                builder.name = Some(name.clone());
            }
            if let Some(args) = &func.arguments {
                builder.arguments.push_str(args);
            }
        }
    }

    /// Whether any tool calls were accumulated.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Finalize into a [`ChatResponse`]. Tool-call argument strings are parsed
    /// as JSON; an empty/blank argument string becomes `{}`.
    pub fn finish(mut self) -> Result<ChatResponse> {
        self.tool_calls.sort_by_key(|b| b.index);

        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for b in &self.tool_calls {
            let args_str = if b.arguments.trim().is_empty() {
                "{}"
            } else {
                b.arguments.trim()
            };
            let arguments: serde_json::Value = serde_json::from_str(args_str).map_err(|e| {
                CoreError::Serialization(format!(
                    "tool call {} has invalid argument JSON: {e}",
                    b.index
                ))
            })?;
            tool_calls.push(ToolCall {
                id: b.id.clone().unwrap_or_else(|| format!("call_{}", b.index)),
                name: b.name.clone().unwrap_or_default(),
                arguments,
            });
        }

        let mut message = Message::text(Role::Assistant, self.content);
        if !self.reasoning.is_empty() {
            message.reasoning_content = Some(self.reasoning);
        }
        message.tool_calls = tool_calls;

        Ok(ChatResponse {
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
        }
        impl DeltaObserver for Rec {
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
    fn invalid_tool_json_errors() {
        let mut acc = DeltaAccumulator::new();
        acc.push_chunk(&chunk(serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "name": "bad", "arguments": "{not json" } }
            ]}}]
        })));
        assert!(acc.finish().is_err());
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
}
