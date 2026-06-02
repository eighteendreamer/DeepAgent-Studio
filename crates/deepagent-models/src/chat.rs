//! Chat-completion request/response types (DeepSeek-compatible).
//!
//! These mirror the OpenAI-style chat-completions schema that DeepSeek exposes,
//! plus the DeepSeek-specific `reasoning_content` field used by Thinking Mode.
//! The types are provider-agnostic enough to target other OpenAI-compatible
//! backends, but the defaults and the [`ThinkingConfig`] are tuned for DeepSeek.

use serde::{Deserialize, Serialize};

use deepagent_core::message::Message;

/// DeepSeek Thinking Mode depth exposed to users.
///
/// DeepSeek maps low/medium efforts to `high`, so simple and medium both use
/// `reasoning_effort=high`; the runtime still differentiates them with output
/// token ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDepth {
    /// Reasoning enabled with a conservative output budget.
    Simple,
    /// Reasoning enabled with the normal agent output budget.
    Medium,
    /// Reasoning enabled at maximum effort.
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

impl Default for ThinkingDepth {
    fn default() -> Self {
        ThinkingDepth::Medium
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
                enabled: true,
                effort: Some("high".to_string()),
                max_tokens: Some(16_384),
            },
            ThinkingDepth::Medium => Self {
                enabled: true,
                effort: Some("high".to_string()),
                max_tokens: Some(32_768),
            },
            ThinkingDepth::Deep => Self {
                enabled: true,
                effort: Some("max".to_string()),
                max_tokens: Some(65_536),
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

/// A chat-completion request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatRequest {
    /// Target model id (e.g. `"deepseek-v4-flash"` / `"deepseek-v4-pro"`).
    pub model: String,
    /// Conversation so far. Serialized through [`crate::wire::serialize_messages`]
    /// so assistant tool calls take the API's required
    /// `{id, type:"function", function:{name, arguments}}` shape (with
    /// `arguments` JSON-stringified) rather than the kernel's internal flat form.
    #[serde(serialize_with = "crate::wire::serialize_messages")]
    pub messages: Vec<Message>,
    /// Whether to stream the response as SSE.
    pub stream: bool,
    /// Streaming options. When streaming, set `{include_usage: true}` so the
    /// provider (DeepSeek) emits a final usage chunk; omitted when not streaming.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// DeepSeek Thinking Mode toggle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingToggle>,
    /// DeepSeek Thinking effort (`high` / `max`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Advertised tools.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSchema>,
}

/// Streaming options (OpenAI/DeepSeek-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StreamOptions {
    /// Ask the provider to emit a final chunk carrying token usage.
    pub include_usage: bool,
}

impl ChatRequest {
    /// Build a non-streaming request for `model` with `messages`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            stream_options: None,
            temperature: None,
            max_tokens: None,
            thinking: None,
            reasoning_effort: None,
            tools: Vec::new(),
        }
    }

    /// Enable streaming (builder style). Also requests usage in the stream.
    pub fn streaming(mut self) -> Self {
        self.stream = true;
        self.stream_options = Some(StreamOptions {
            include_usage: true,
        });
        self
    }

    /// Attach tool schemas (builder style).
    pub fn with_tools(mut self, tools: Vec<ToolSchema>) -> Self {
        self.tools = tools;
        self
    }

    /// Set temperature (builder style).
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set max tokens (builder style).
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Apply a DeepSeek Thinking Mode profile to this request.
    pub fn with_thinking_depth(mut self, depth: ThinkingDepth) -> Self {
        let cfg = ThinkingConfig::for_depth(depth);
        self.thinking = Some(ThinkingToggle {
            kind: if cfg.enabled { "enabled" } else { "disabled" }.to_string(),
        });
        self.reasoning_effort = cfg.effort;
        if self.max_tokens.is_none() {
            self.max_tokens = cfg.max_tokens;
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
pub struct ChatResponse {
    /// The assistant message (content + reasoning_content + tool_calls).
    pub message: Message,
    /// Why generation finished, if reported.
    pub finish_reason: Option<FinishReason>,
    /// Token usage, if reported.
    pub usage: Option<Usage>,
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
                enabled: true,
                effort: Some("high".to_string()),
                max_tokens: Some(16_384)
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
        assert!(
            ThinkingConfig::for_depth(ThinkingDepth::Deep).max_tokens
                > ThinkingConfig::for_depth(ThinkingDepth::Medium).max_tokens
        );
    }

    #[test]
    fn request_builder_and_serialization() {
        let req = ChatRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
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
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["max_tokens"], 32768);
        assert!((json["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(json["tools"][0]["function"]["name"], "reverse");
    }

    #[test]
    fn usage_defaults_to_zero() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(u, Usage::default());
    }
}
