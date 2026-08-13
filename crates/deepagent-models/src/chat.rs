//! Provider-facing DeepSeek Responses request/response types.
//!
//! The internal `Message` projection remains provider-neutral for the runtime
//! and UI, while request serialization emits Responses input items.

use serde::{Deserialize, Serialize, Serializer};

use deepagent_core::message::{Message, Role, ToolCall};
use deepagent_core::response_item::{ResponseInputItem, ResponseOutputItem};

/// DeepSeek Thinking Mode depth exposed to users.
///
/// Aligned to the provider's official controls:
/// - `Simple` => thinking disabled
/// - `Medium` => thinking enabled with `reasoning_effort=high`
/// - `Deep` => thinking enabled with `reasoning_effort=max`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDepth {
    /// Thinking disabled.
    Simple,
    /// Thinking enabled at high effort.
    #[default]
    Medium,
    /// Thinking enabled at maximum effort.
    Deep,
}

impl ThinkingDepth {
    /// Stable label used in settings/UI.
    pub const fn label(&self) -> &'static str {
        match self {
            ThinkingDepth::Simple => "simple",
            ThinkingDepth::Medium => "medium",
            ThinkingDepth::Deep => "deep",
        }
    }
}

/// Concrete thinking parameters derived from a [`ThinkingDepth`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Whether reasoning is enabled.
    pub enabled: bool,
    /// Reasoning effort hint, when enabled (`"high"` / `"max"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Output token ceiling for this depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl ThinkingConfig {
    /// Derive the request-level config for a depth.
    pub fn for_depth(depth: ThinkingDepth) -> Self {
        match depth {
            ThinkingDepth::Simple => Self {
                enabled: false,
                effort: None,
                max_tokens: Some(8_192),
            },
            ThinkingDepth::Medium => Self {
                enabled: true,
                effort: Some("high".to_string()),
                max_tokens: Some(16_384),
            },
            ThinkingDepth::Deep => Self {
                enabled: true,
                effort: Some("max".to_string()),
                max_tokens: Some(32_768),
            },
        }
    }
}

/// DeepSeek OpenAI-format thinking toggle: `{"thinking":{"type":"enabled"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThinkingToggle {
    #[serde(rename = "type")]
    pub kind: String,
}

/// A tool/function the model may call, in JSON-schema form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Always `"function"` for current providers.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function definition.
    pub function: FunctionSchema,
}

impl ToolSchema {
    /// Build a function tool schema.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: FunctionSchema {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// A function definition advertised to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSchema {
    /// Function name.
    pub name: String,
    /// Natural-language description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
}

/// A DeepSeek Responses API request.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseRequest {
    /// Target model id (e.g. `"deepseek-v4-flash"` / `"deepseek-v4-pro"`).
    pub model: String,
    /// System/developer instructions. Serialized as Responses `instructions`.
    pub instructions: Option<String>,
    /// Ordered Responses input items (`message`, `reasoning`, function/custom
    /// tool calls and paired outputs, web search calls).
    pub input: Vec<ResponseInputItem>,
    /// Whether to stream the response as SSE.
    pub stream: bool,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Max output tokens.
    pub max_output_tokens: Option<u32>,
    /// Responses API reasoning effort (`high` / `max`).
    pub reasoning_effort: Option<String>,
    /// Advertised tools.
    pub tools: Vec<ToolSchema>,
    /// Responses API tool choice. Omitted means provider default (`auto`).
    pub tool_choice: Option<serde_json::Value>,
    /// Responses API nucleus sampling.
    pub top_p: Option<f32>,
    /// Responses API top logprobs.
    pub top_logprobs: Option<u8>,
    /// Responses API text configuration, e.g. json_schema.
    pub text: Option<serde_json::Value>,
    /// Optional end-user identifier.
    pub user: Option<String>,
}

impl Serialize for ResponseRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("model", &self.model)?;
        if let Some(value) = &self.instructions {
            map.serialize_entry("instructions", &value)?;
        }
        map.serialize_entry("input", &self.input)?;
        map.serialize_entry("stream", &self.stream)?;
        if let Some(value) = self.temperature {
            map.serialize_entry("temperature", &value)?;
        }
        if let Some(value) = self.top_p {
            map.serialize_entry("top_p", &value)?;
        }
        if let Some(value) = self.max_output_tokens {
            map.serialize_entry("max_output_tokens", &value)?;
        }
        if let Some(value) = self.top_logprobs {
            map.serialize_entry("top_logprobs", &value)?;
        }
        if let Some(value) = &self.reasoning_effort {
            map.serialize_entry("reasoning", &serde_json::json!({"effort": value}))?;
        }
        if !self.tools.is_empty() {
            let tools: Vec<serde_json::Value> = self.tools.iter().map(|tool| match tool.kind.as_str() {
                "function" => serde_json::json!({"type":"function","name":tool.function.name,"description":tool.function.description,"parameters":tool.function.parameters}),
                "custom" => serde_json::json!({"type":"custom","name":tool.function.name,"description":tool.function.description,"format":{"type":"text"}}),
                "web_search" => serde_json::json!({"type":"web_search"}),
                _ => serde_json::json!({"type": tool.kind}),
            }).collect();
            map.serialize_entry("tools", &tools)?;
        }
        if let Some(value) = &self.tool_choice {
            map.serialize_entry("tool_choice", value)?;
        }
        if let Some(value) = &self.text {
            map.serialize_entry("text", value)?;
        }
        if let Some(value) = &self.user {
            map.serialize_entry("user", value)?;
        }
        map.end()
    }
}

/// Streaming options (OpenAI/DeepSeek-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StreamOptions {
    /// Ask the provider to emit a final chunk carrying token usage.
    pub include_usage: bool,
}

impl ResponseRequest {
    /// Build a non-streaming request for `model` with `messages`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        let (instructions, input) = crate::responses::response_items_from_messages(&messages);
        Self::from_response_items(model, instructions, input)
    }

    /// Build a request from already-normalized Responses input items.
    pub fn from_response_items(
        model: impl Into<String>,
        instructions: Option<String>,
        input: Vec<ResponseInputItem>,
    ) -> Self {
        Self {
            model: model.into(),
            instructions,
            input,
            stream: false,
            temperature: None,
            max_output_tokens: None,
            reasoning_effort: None,
            tools: Vec::new(),
            tool_choice: None,
            top_p: None,
            top_logprobs: None,
            text: None,
            user: None,
        }
    }

    /// Enable streaming (builder style). Also requests usage in the stream.
    pub fn streaming(mut self) -> Self {
        self.stream = true;
        self
    }

    /// Attach tool schemas (builder style).
    pub fn with_tools(mut self, tools: Vec<ToolSchema>) -> Self {
        self.tools = tools;
        self
    }

    /// Refresh the Responses `instructions`/`input` projection after the
    /// runtime mutates its provider-neutral conversation state. This keeps
    /// retry-time changes such as `max_output_tokens` escalation intact.
    pub fn replace_messages(&mut self, messages: &[Message]) {
        let (instructions, input) = crate::responses::response_items_from_messages(messages);
        self.instructions = instructions;
        self.input = input;
    }

    /// Set temperature (builder style).
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_top_p(mut self, value: f32) -> Self {
        self.top_p = Some(value);
        self
    }

    pub fn with_top_logprobs(mut self, value: u8) -> Self {
        self.top_logprobs = Some(value);
        self
    }

    pub fn with_text(mut self, value: serde_json::Value) -> Self {
        self.text = Some(value);
        self
    }

    pub fn with_tool_choice(mut self, value: serde_json::Value) -> Self {
        self.tool_choice = Some(value);
        self
    }

    pub fn with_max_output_tokens(mut self, n: u32) -> Self {
        self.max_output_tokens = Some(n);
        self
    }

    /// Apply a DeepSeek Thinking Mode profile to this request.
    pub fn with_thinking_depth(mut self, depth: ThinkingDepth) -> Self {
        let cfg = ThinkingConfig::for_depth(depth);
        self.reasoning_effort = cfg.effort;
        if self.max_output_tokens.is_none() {
            self.max_output_tokens = cfg.max_tokens;
        }
        self
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural end of message.
    Stop,
    /// Hit the token limit.
    Length,
    /// Stopped to emit tool calls.
    ToolCalls,
    /// Filtered by content policy.
    ContentFilter,
}

/// Token accounting returned by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Completion tokens.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Reasoning tokens included in `completion_tokens` by Responses.
    /// Kept separately for diagnostics and UI; never added again for billing.
    #[serde(default)]
    pub reasoning_tokens: u32,
    /// Total tokens.
    #[serde(default)]
    pub total_tokens: u32,
    /// DeepSeek: prompt tokens served from the context cache (a "hit").
    #[serde(default)]
    pub prompt_cache_hit_tokens: u32,
    /// DeepSeek: prompt tokens NOT served from cache (a "miss").
    #[serde(default)]
    pub prompt_cache_miss_tokens: u32,
}

/// A fully assembled (non-streaming, or post-accumulation) response.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// Legacy UI/runtime compatibility projection. Prefer the item-native
    /// accessors (`output_items`, `output_text_projection`,
    /// `assistant_message_projection`) in new code.
    message: Message,
    /// Provider-native Responses output items. Runtime/UI may still use
    /// `message` as a compatibility projection, but persistence and recovery
    /// can retain exact item semantics.
    pub output_items: Vec<ResponseOutputItem>,
    /// Why generation finished, if reported.
    pub finish_reason: Option<FinishReason>,
    /// Token usage, if reported.
    pub usage: Option<Usage>,
    /// Raw provider Responses usage object, preserved for diagnostics and
    /// future billing fields without changing the existing UI projection.
    pub raw_usage: Option<serde_json::Value>,
}

impl Response {
    pub(crate) fn from_parts(
        message: Message,
        output_items: Vec<ResponseOutputItem>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        raw_usage: Option<serde_json::Value>,
    ) -> Self {
        Self {
            message,
            output_items,
            finish_reason,
            usage,
            raw_usage,
        }
    }

    /// Project the assistant's visible text from native Responses output items.
    ///
    /// This is the preferred helper for one-shot classification, title,
    /// compaction and review calls that only need text. It falls back to the
    /// legacy `message` projection for mocked/older tests that do not provide
    /// native output items.
    pub fn output_text_projection(&self) -> String {
        let mut content = String::new();
        for item in &self.output_items {
            if let ResponseOutputItem::Message {
                role,
                content: text,
            } = item
            {
                if role == "assistant" {
                    content.push_str(text);
                }
            }
        }
        if content.is_empty() {
            self.message.content.clone()
        } else {
            content
        }
    }

    /// Build the UI/runtime compatibility projection from native Responses
    /// output items. Model execution should prefer this centralized item
    /// projection over reading `message.tool_calls` directly.
    pub fn assistant_message_projection(&self) -> Message {
        let content = self.output_text_projection();
        let mut reasoning: Option<String> = None;
        let mut tool_calls = Vec::new();
        for item in &self.output_items {
            match item {
                ResponseOutputItem::Reasoning { content: text, .. } if !text.is_empty() => {
                    reasoning = Some(match reasoning.take() {
                        Some(mut existing) => {
                            existing.push_str(text);
                            existing
                        }
                        None => text.clone(),
                    });
                }
                ResponseOutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => tool_calls.push(ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: parse_function_arguments(arguments),
                }),
                ResponseOutputItem::CustomToolCall {
                    call_id,
                    name,
                    input,
                } => tool_calls.push(ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: serde_json::json!({ "patch": input }),
                }),
                _ => {}
            }
        }
        if reasoning.is_none() {
            reasoning = self.message.reasoning_content.clone();
        }
        if tool_calls.is_empty() {
            tool_calls = self.message.tool_calls.clone();
        }
        let mut message = Message::text(Role::Assistant, content);
        message.reasoning_content = reasoning;
        message.tool_calls = tool_calls;
        message
    }

    /// Extract local tool invocations from native Responses output items.
    pub fn tool_invocations_from_items(&self) -> Vec<(String, String, serde_json::Value)> {
        self.assistant_message_projection()
            .tool_calls
            .into_iter()
            .map(|call| (call.id, call.name, call.arguments))
            .collect()
    }
}

fn parse_function_arguments(raw: &str) -> serde_json::Value {
    let args = if raw.trim().is_empty() {
        "{}"
    } else {
        raw.trim()
    };
    serde_json::from_str(args).unwrap_or_else(|error| {
        serde_json::json!({
            "__invalid_tool_arguments__": true,
            "raw": args,
            "parse_error": error.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_depth_flags() {
        assert_eq!(ThinkingDepth::Simple.label(), "simple");
        assert_eq!(ThinkingDepth::Medium.label(), "medium");
        assert_eq!(ThinkingDepth::Deep.label(), "deep");
        assert_eq!(ThinkingDepth::default(), ThinkingDepth::Medium);
    }

    #[test]
    fn thinking_config_mapping() {
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Simple),
            ThinkingConfig {
                enabled: false,
                effort: None,
                max_tokens: Some(8_192)
            }
        );
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Medium)
                .effort
                .as_deref(),
            Some("high")
        );
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Deep)
                .effort
                .as_deref(),
            Some("max")
        );
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Medium).max_tokens,
            Some(16_384)
        );
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Deep).max_tokens,
            Some(32_768)
        );
    }

    #[test]
    fn request_builder_and_serialization() {
        let req = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .streaming()
            .with_thinking_depth(ThinkingDepth::Medium)
            .with_temperature(0.2)
            .with_tools(vec![ToolSchema::function(
                "reverse",
                "reverse text",
                serde_json::json!({"type": "object"}),
            )]);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "deepseek-v4-flash");
        assert_eq!(json["stream"], true);
        assert_eq!(json["reasoning"]["effort"], "high");
        assert_eq!(json["max_output_tokens"], 16_384);
        assert!((json["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(json["tools"][0]["name"], "reverse");
    }

    #[test]
    fn simple_thinking_disables_reasoning() {
        let req = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .with_thinking_depth(ThinkingDepth::Simple);
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("reasoning").is_none());
        assert_eq!(json["max_output_tokens"], 8_192);
    }

    #[test]
    fn usage_defaults_to_zero() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(u, Usage::default());
    }
}
