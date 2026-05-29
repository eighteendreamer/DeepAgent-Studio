//! Conversation primitives.
//!
//! These model the messages exchanged with the model provider. Critically,
//! the [`Message`] type carries an optional `reasoning_content` field to
//! support DeepSeek Thinking Mode persistence (see 开发提示词.md §8): when a
//! message contains tool calls, the reasoning content must be retained so it
//! can be replayed on resume / sub-agent handoff.

use serde::{Deserialize, Serialize};

/// The role of a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System / developer instruction.
    System,
    /// End-user input.
    User,
    /// Model output.
    Assistant,
    /// Result of a tool invocation fed back to the model.
    Tool,
}

impl Role {
    /// The wire string used by chat-completions style APIs.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A request from the model to invoke a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned id correlating the call with its result.
    pub id: String,
    /// The tool/function name.
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: serde_json::Value,
}

/// A single conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,

    /// The visible textual content. May be empty when the message is purely
    /// a set of tool calls.
    #[serde(default)]
    pub content: String,

    /// DeepSeek Thinking Mode reasoning trace. Persisted only when relevant
    /// (tool calls / resume / hand-off) — see [`Message::should_persist_reasoning`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,

    /// Tool calls requested by the assistant in this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,

    /// For [`Role::Tool`] messages, the id of the originating tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// A plain system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }

    /// A plain user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }

    /// A plain assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text(Role::Assistant, content)
    }

    /// Construct a textual message with the given role.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A tool-result message correlated to a prior [`ToolCall`].
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Attach reasoning content (builder style).
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning_content = Some(reasoning.into());
        self
    }

    /// Attach tool calls (builder style).
    pub fn with_tool_calls(mut self, calls: Vec<ToolCall>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Whether this message carries tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// DeepSeek Thinking Mode rule: reasoning_content is only meaningful to
    /// persist back into context when the assistant turn produced tool calls.
    /// Plain conversational turns drop the reasoning to save tokens.
    pub fn should_persist_reasoning(&self) -> bool {
        self.reasoning_content.is_some() && self.has_tool_calls()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_wire_strings() {
        assert_eq!(Role::Assistant.as_str(), "assistant");
        assert_eq!(Role::Tool.as_str(), "tool");
    }

    #[test]
    fn reasoning_persisted_only_with_tool_calls() {
        let plain = Message::assistant("hi").with_reasoning("thinking...");
        assert!(!plain.should_persist_reasoning());

        let with_tools = Message::assistant("")
            .with_reasoning("thinking...")
            .with_tool_calls(vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
            }]);
        assert!(with_tools.should_persist_reasoning());
    }

    #[test]
    fn reasoning_omitted_from_json_when_none() {
        let m = Message::user("hello");
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("reasoning_content"));
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn tool_result_roundtrip() {
        let m = Message::tool_result("c1", "ok");
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.tool_call_id.as_deref(), Some("c1"));
    }
}
