//! Typed Messages SSE assembly into the existing neutral model result.
use std::collections::HashSet;

use anthropic_sdk::{ContentBlock, ContentBlockDelta, MessageStreamEvent, StopReason};
use deepagent_core::error::{CoreError, Result};
use serde_json::Value;

use super::usage::UsageState;
use crate::stream::{DeltaObserver, ModelStreamEvent};
use crate::{FinishReason, Response, ResponseItem};

#[derive(Debug)]
struct Block {
    content: ContentBlock,
    arguments: String,
    closed: bool,
}

/// One physical Messages response. No transport, retries, permissions or store.
#[derive(Debug, Default)]
pub struct MessagesAccumulator {
    started: bool,
    stopped: bool,
    failed: bool,
    terminal: Option<FinishReason>,
    blocks: Vec<Block>,
    tool_ids: HashSet<String>,
    usage: UsageState,
}

impl MessagesAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one de-framed SSE data payload. True means message_stop was seen.
    pub fn push_sse_data_observed(
        &mut self,
        data: &str,
        observer: &mut dyn DeltaObserver,
    ) -> Result<bool> {
        let result = self.push(data, observer);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push(&mut self, data: &str, observer: &mut dyn DeltaObserver) -> Result<bool> {
        if self.failed || self.stopped {
            return Err(invalid("event after failed or stopped stream"));
        }
        let value: Value = serde_json::from_str(data).map_err(schema_error)?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("event requires a type"))?;
        if kind == "error" {
            return Err(crate::provider_error::from_json_error(
                None,
                value.get("error"),
                Some("messages_error"),
                "Messages stream reported a provider error",
                None,
                None,
            ));
        }
        // Anthropic specifies forward-compatible unknown event handling.
        if !matches!(
            kind,
            "message_start"
                | "message_delta"
                | "message_stop"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "ping"
        ) {
            return Ok(false);
        }
        let raw_usage = match kind {
            "message_start" => value.pointer("/message/usage"),
            "message_delta" => value.get("usage"),
            _ => None,
        }
        .map(crate::usage::sanitized_provider_usage);
        let event: MessageStreamEvent = serde_json::from_value(value).map_err(schema_error)?;
        match event {
            MessageStreamEvent::MessageStart { message } => {
                if self.started
                    || message.id.trim().is_empty()
                    || message.role != "assistant"
                    || message.type_name != "message"
                    || !message.content.is_empty()
                    || message.stop_reason.is_some()
                {
                    return Err(invalid("invalid or repeated message_start"));
                }
                self.started = true;
                self.observe_usage(raw_usage, observer)?;
            }
            MessageStreamEvent::ContentBlockStart {
                content_block,
                index,
            } => {
                self.require_content_phase()?;
                self.start_block(index, content_block, observer)?;
            }
            MessageStreamEvent::ContentBlockDelta { delta, index } => {
                self.require_content_phase()?;
                self.delta(index, delta, observer)?;
            }
            MessageStreamEvent::ContentBlockStop { index } => {
                self.require_content_phase()?;
                self.close_block(index, observer)?;
            }
            MessageStreamEvent::MessageDelta { delta, .. } => {
                if !self.started {
                    return Err(invalid("message_delta before message_start"));
                }
                if let Some(reason) = delta.stop_reason {
                    let reason = finish_reason(reason)?;
                    if self.terminal.is_some_and(|previous| previous != reason) {
                        return Err(invalid("conflicting stop reasons"));
                    }
                    self.terminal = Some(reason);
                }
                self.observe_usage(raw_usage, observer)?;
            }
            MessageStreamEvent::MessageStop => {
                let reason = self
                    .terminal
                    .ok_or_else(|| invalid("message_stop without stop reason"))?;
                if self.blocks.iter().any(|block| !block.closed) && reason != FinishReason::Length {
                    return Err(invalid("message_stop before content blocks stopped"));
                }
                if reason == FinishReason::ToolCalls && self.tool_ids.is_empty() {
                    return Err(invalid("tool_use stop reason without a tool call"));
                }
                self.stopped = true;
                observer.on_event(ModelStreamEvent::Finished {
                    reason: Some(reason),
                });
                return Ok(true);
            }
            MessageStreamEvent::Ping => {}
        }
        Ok(false)
    }

    fn require_content_phase(&self) -> Result<()> {
        if !self.started || self.terminal.is_some() {
            return Err(invalid("content event outside message content phase"));
        }
        Ok(())
    }

    fn observe_usage(
        &mut self,
        raw: Option<Value>,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        let usage = self
            .usage
            .apply(raw.ok_or_else(|| invalid("missing usage snapshot"))?)?;
        observer.on_usage(usage, &self.usage.raw());
        Ok(())
    }

    fn start_block(
        &mut self,
        index: usize,
        content: ContentBlock,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        if index != self.blocks.len() || self.blocks.iter().any(|block| !block.closed) {
            return Err(invalid(
                "invalid content block start index or overlapping blocks",
            ));
        }
        match &content {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    observer.on_event(ModelStreamEvent::ContentDelta { text: text.clone() });
                }
            }
            ContentBlock::Thinking { thinking, .. } => {
                if !thinking.is_empty() {
                    observer.on_event(ModelStreamEvent::ReasoningDelta {
                        text: thinking.clone(),
                    });
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                if id.trim().is_empty()
                    || name.trim().is_empty()
                    || !input.is_object()
                    || !self.tool_ids.insert(id.clone())
                {
                    return Err(invalid("invalid or repeated tool call identity"));
                }
                observer.on_event(ModelStreamEvent::ToolCallStarted {
                    index,
                    id: Some(id.clone()),
                    name: name.clone(),
                });
            }
            _ => {
                return Err(invalid(
                    "content block has no supported neutral representation",
                ))
            }
        }
        self.blocks.push(Block {
            content,
            arguments: String::new(),
            closed: false,
        });
        Ok(())
    }

    fn delta(
        &mut self,
        index: usize,
        delta: ContentBlockDelta,
        observer: &mut dyn DeltaObserver,
    ) -> Result<()> {
        let block = self
            .blocks
            .get_mut(index)
            .filter(|block| !block.closed)
            .ok_or_else(|| invalid("delta for absent or stopped content block"))?;
        match (&mut block.content, delta) {
            (ContentBlock::Text { text, .. }, ContentBlockDelta::TextDelta { text: value }) => {
                text.push_str(&value);
                observer.on_event(ModelStreamEvent::ContentDelta { text: value });
            }
            (
                ContentBlock::Thinking { thinking, .. },
                ContentBlockDelta::ThinkingDelta { thinking: value },
            ) => {
                thinking.push_str(&value);
                observer.on_event(ModelStreamEvent::ReasoningDelta { text: value });
            }
            (
                ContentBlock::Thinking { signature, .. },
                ContentBlockDelta::SignatureDelta { signature: value },
            ) => signature.push_str(&value),
            (
                ContentBlock::ToolUse { input, .. },
                ContentBlockDelta::InputJsonDelta { partial_json },
            ) => {
                if input.as_object().is_some_and(|input| !input.is_empty()) {
                    return Err(invalid("tool argument deltas conflict with initial input"));
                }
                block.arguments.push_str(&partial_json);
                observer.on_event(ModelStreamEvent::ToolArgumentsDelta {
                    index,
                    delta: partial_json,
                });
            }
            _ => return Err(invalid("delta does not match content block kind")),
        }
        Ok(())
    }

    fn close_block(&mut self, index: usize, observer: &mut dyn DeltaObserver) -> Result<()> {
        let block = self
            .blocks
            .get_mut(index)
            .filter(|block| !block.closed)
            .ok_or_else(|| invalid("stop for absent or stopped content block"))?;
        if let ContentBlock::ToolUse { id, name, input } = &mut block.content {
            if !block.arguments.is_empty() {
                *input = serde_json::from_str(&block.arguments).map_err(schema_error)?;
            }
            if !input.is_object() {
                return Err(invalid("tool input must be a JSON object"));
            }
            observer.on_event(ModelStreamEvent::ToolCallCompleted {
                index,
                id: id.clone(),
                name: name.clone(),
                arguments: input.clone(),
            });
        }
        block.closed = true;
        Ok(())
    }

    /// Require an explicit terminal. Incomplete tools never become executable.
    pub fn finish(self) -> Result<Response> {
        if self.failed || !self.stopped {
            return Err(invalid("stream ended without an accepted message_stop"));
        }
        let mut items = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let item = match block.content {
                ContentBlock::Text { text, .. } => ResponseItem::Message {
                    role: "assistant".into(),
                    content: text,
                },
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => ResponseItem::Reasoning {
                    id: None,
                    content: thinking,
                    signature: (!signature.is_empty()).then_some(signature),
                },
                ContentBlock::ToolUse { id, name, input } if block.closed => {
                    ResponseItem::FunctionCall {
                        call_id: id,
                        name,
                        arguments: input.to_string(),
                    }
                }
                ContentBlock::ToolUse { .. } => continue,
                _ => return Err(invalid("unsupported content block")),
            };
            items.push(item);
        }
        let (usage, raw) = self.usage.into_parts();
        Ok(Response::from_parts(items, self.terminal, usage, raw))
    }
}

fn finish_reason(reason: StopReason) -> Result<FinishReason> {
    match reason {
        StopReason::EndTurn | StopReason::StopSequence => Ok(FinishReason::Stop),
        StopReason::MaxTokens => Ok(FinishReason::Length),
        StopReason::ToolUse => Ok(FinishReason::ToolCalls),
        StopReason::Refusal => Ok(FinishReason::ContentFilter),
        StopReason::PauseTurn => Err(invalid(
            "pause_turn requires server-tool continuation support",
        )),
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::invalid(format!("Messages stream: {message}"))
}

fn schema_error(error: serde_json::Error) -> CoreError {
    // Serde data errors can embed provider strings. Preserve category/location,
    // never tool arguments, signatures, prompt text or credential-like values.
    CoreError::Serialization(format!(
        "invalid Messages event schema ({:?}, line {}, column {})",
        error.classify(),
        error.line(),
        error.column(),
    ))
}
