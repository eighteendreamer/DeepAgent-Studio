//! Chat-completion request/response types (DeepSeek-compatible).
//!
//! These mirror the OpenAI-style chat-completions schema that DeepSeek exposes,
//! plus the DeepSeek-specific `reasoning_content` field used by Thinking Mode.
//! The types are provider-agnostic enough to target other OpenAI-compatible
//! backends, but the defaults and the [`ThinkingConfig`] are tuned for DeepSeek.

use serde::{Deserialize, Serialize};

use deepagent_core::message::Message;

/// Thinking-mode depth, per 开发提示词.md §9.
///
/// The runtime exposes a small, intent-level knob; [`ThinkingConfig::for_depth`]
/// maps it onto concrete request parameters. `Max` additionally signals the
/// runtime to wrap the call in a recursive reflective loop (handled by the
/// verification/reflection engine, not the request itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDepth {
    /// Thinking disabled — fastest, no reasoning trace.
    Fast,
    /// Reasoning enabled at high effort.
    Balanced,
    /// Reasoning enabled at maximum effort.
    Deep,
    /// Maximum effort + caller-driven recursive reflection.
    Max,
}

impl ThinkingDepth {
    /// Whether this depth enables the model's reasoning trace.
    pub const fn reasoning_enabled(&self) -> bool {
        !matches!(self, ThinkingDepth::Fast)
    }

    /// Whether the runtime should drive a recursive reflective loop on top.
    pub const fn recursive_reflection(&self) -> bool {
        matches!(self, ThinkingDepth::Max)
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
}

impl ThinkingConfig {
    /// Derive the request-level config for a depth.
    pub fn for_depth(depth: ThinkingDepth) -> Self {
        match depth {
            ThinkingDepth::Fast => Self {
                enabled: false,
                effort: None,
            },
            ThinkingDepth::Balanced => Self {
                enabled: true,
                effort: Some("high".to_string()),
            },
            // Deep and Max both request max effort at the API level; Max differs
            // only in the runtime-side reflective loop.
            ThinkingDepth::Deep | ThinkingDepth::Max => Self {
                enabled: true,
                effort: Some("max".to_string()),
            },
        }
    }
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
    /// Target model id (e.g. `"deepseek-chat"` / `"deepseek-reasoner"`).
    pub model: String,
    /// Conversation so far.
    pub messages: Vec<Message>,
    /// Whether to stream the response as SSE.
    pub stream: bool,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Advertised tools.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSchema>,
}

impl ChatRequest {
    /// Build a non-streaming request for `model` with `messages`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
        }
    }

    /// Enable streaming (builder style).
    pub fn streaming(mut self) -> Self {
        self.stream = true;
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
        assert!(!ThinkingDepth::Fast.reasoning_enabled());
        assert!(ThinkingDepth::Balanced.reasoning_enabled());
        assert!(ThinkingDepth::Max.recursive_reflection());
        assert!(!ThinkingDepth::Deep.recursive_reflection());
    }

    #[test]
    fn thinking_config_mapping() {
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Fast),
            ThinkingConfig {
                enabled: false,
                effort: None
            }
        );
        assert_eq!(
            ThinkingConfig::for_depth(ThinkingDepth::Balanced)
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
    }

    #[test]
    fn request_builder_and_serialization() {
        let req = ChatRequest::new("deepseek-chat", vec![Message::user("hi")])
            .streaming()
            .with_temperature(0.2)
            .with_tools(vec![ToolSchema::function(
                "reverse",
                "reverse text",
                serde_json::json!({"type": "object"}),
            )]);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "deepseek-chat");
        assert_eq!(json["stream"], true);
        assert!((json["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(json["tools"][0]["function"]["name"], "reverse");
        // max_tokens omitted when None.
        assert!(json.get("max_tokens").is_none());
    }

    #[test]
    fn usage_defaults_to_zero() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(u, Usage::default());
    }
}
