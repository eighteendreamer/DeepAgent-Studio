//! On-the-wire message encoding for the DeepSeek / OpenAI chat-completions API.
//!
//! The kernel's [`deepagent_core::message::Message`] / [`ToolCall`] are shaped
//! for the **event store and runtime** (flat, replayable, `arguments` as a
//! structured [`serde_json::Value`]). The chat-completions API on the wire,
//! however, demands a *different* shape for assistant tool calls:
//!
//! ```json
//! {
//!   "role": "assistant",
//!   "content": "",
//!   "tool_calls": [
//!     { "id": "call_1", "type": "function",
//!       "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" } }
//!   ]
//! }
//! ```
//!
//! Two differences from our internal form caused a real `400 Bad Request`
//! (`messages[..]: missing field 'type'`):
//!
//! 1. each tool call must carry `type: "function"` and nest `name`/`arguments`
//!    under a `function` object (our internal `ToolCall` is flat), and
//! 2. `arguments` must be a **JSON-encoded string**, not a JSON object.
//!
//! Rather than contort the core types (which the persistence layer round-trips
//! verbatim), we translate to these wire structs only at serialization time.
//! [`ChatRequest`](crate::chat::ChatRequest) serializes its `messages` through
//! [`serialize_messages`].

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use deepagent_core::message::{Message, ToolCall};

/// A chat message in DeepSeek/OpenAI wire form.
#[derive(Debug, Serialize)]
pub struct WireMessage<'a> {
    /// `system` / `user` / `assistant` / `tool`.
    pub role: &'static str,
    /// Visible text. Always present (empty string for pure tool-call turns),
    /// since the API rejects a missing `content` on most roles.
    pub content: &'a str,
    /// DeepSeek Thinking Mode trace, echoed back only when retained. Newer
    /// reasoner models require it on assistant turns that preceded tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<&'a str>,
    /// Assistant tool-call requests, in the API's nested form.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall<'a>>,
    /// For `role: "tool"` results, the id of the originating call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<&'a str>,
}

/// A tool call in the API's required `{id, type, function{...}}` shape.
#[derive(Debug, Serialize)]
pub struct WireToolCall<'a> {
    /// Provider-assigned call id.
    pub id: &'a str,
    /// Always `"function"` for current providers — the field whose absence
    /// triggered the `400 missing field 'type'`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The nested function name + stringified arguments.
    pub function: WireFunction<'a>,
}

/// The `function` object inside a wire tool call.
#[derive(Debug, Serialize)]
pub struct WireFunction<'a> {
    /// Function/tool name.
    pub name: &'a str,
    /// Arguments encoded as a JSON **string** (per the API contract).
    pub arguments: String,
}

impl<'a> From<&'a ToolCall> for WireToolCall<'a> {
    fn from(tc: &'a ToolCall) -> Self {
        // `arguments` is a structured Value internally; the wire wants a string.
        // Serializing a Value to a string is effectively infallible; fall back
        // to an empty object on the impossible error path.
        let arguments = serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string());
        Self {
            id: &tc.id,
            kind: "function",
            function: WireFunction {
                name: &tc.name,
                arguments,
            },
        }
    }
}

impl<'a> From<&'a Message> for WireMessage<'a> {
    fn from(m: &'a Message) -> Self {
        Self {
            role: m.role.as_str(),
            content: &m.content,
            reasoning_content: m.reasoning_content.as_deref(),
            tool_calls: m.tool_calls.iter().map(WireToolCall::from).collect(),
            tool_call_id: m.tool_call_id.as_deref(),
        }
    }
}

/// Serialize a slice of core [`Message`]s into the chat-completions wire form.
///
/// Used as `#[serde(serialize_with = "...")]` on
/// [`ChatRequest::messages`](crate::chat::ChatRequest).
pub fn serialize_messages<S>(messages: &[Message], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = serializer.serialize_seq(Some(messages.len()))?;
    for m in messages {
        seq.serialize_element(&WireMessage::from(m))?;
    }
    seq.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::message::{Message, Role};

    #[test]
    fn assistant_tool_call_has_type_and_nested_function() {
        let msg = Message::assistant("").with_tool_calls(vec![ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.rs"}),
        }]);
        let wire = WireMessage::from(&msg);
        let json = serde_json::to_value(&wire).unwrap();

        assert_eq!(json["role"], "assistant");
        let call = &json["tool_calls"][0];
        // The field whose absence caused the 400.
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "read_file");
        // arguments must be a STRING, not an object.
        let args = call["function"]["arguments"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(args).unwrap(),
            serde_json::json!({"path": "a.rs"})
        );
        // flat fields must NOT leak onto the wire tool call.
        assert!(call.get("name").is_none());
        assert!(call.get("arguments").is_none());
    }

    #[test]
    fn tool_result_message_carries_tool_call_id() {
        let msg = Message::tool_result("call_1", "{\"ok\":true}");
        let json = serde_json::to_value(WireMessage::from(&msg)).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert_eq!(json["content"], "{\"ok\":true}");
    }

    #[test]
    fn plain_messages_omit_optional_fields() {
        let json = serde_json::to_value(WireMessage::from(&Message::user("hi"))).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hi");
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
        assert!(json.get("reasoning_content").is_none());
    }

    #[test]
    fn reasoning_content_is_forwarded_when_present() {
        let msg = Message::text(Role::Assistant, "")
            .with_reasoning("let me think")
            .with_tool_calls(vec![ToolCall {
                id: "c1".into(),
                name: "f".into(),
                arguments: serde_json::json!({}),
            }]);
        let json = serde_json::to_value(WireMessage::from(&msg)).unwrap();
        assert_eq!(json["reasoning_content"], "let me think");
    }

    #[test]
    fn serialize_messages_helper_produces_array() {
        struct Holder(Vec<Message>);
        impl Serialize for Holder {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                serialize_messages(&self.0, s)
            }
        }
        let holder = Holder(vec![Message::system("s"), Message::user("u")]);
        let json = serde_json::to_value(&holder).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0]["role"], "system");
        assert_eq!(json[1]["role"], "user");
    }
}
