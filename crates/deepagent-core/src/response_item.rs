//! Provider-neutral Responses API item primitives.
//!
//! DeepSeek's Responses API accepts and returns semantic input/output items
//! (`message`, `reasoning`, `function_call`, `function_call_output`,
//! `custom_tool_call`, `custom_tool_call_output`, and `web_search_call`).
//! Keeping this model in `deepagent-core` lets persistence, session recovery,
//! model requests, and runtime events share one typed contract instead of
//! storing provider protocol state as untyped JSON.

use serde::{Deserialize, Serialize};

use crate::message::{Message, MessageAttachment};

/// A semantic Responses API item used for provider input/output persistence.
///
/// Source alignment:
/// - DeepSeek Responses API docs: supported input items include `message`,
///   `function_call`, `function_call_output`, `reasoning`, and
///   `web_search_call`; tool support includes `function`, `web_search`, and
///   custom `apply_patch`.
/// - Codex Rust reference keeps model-visible history as typed
///   `codex_protocol::models::ResponseItem` rather than a chat-only transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    Message {
        role: String,
        content: String,
    },
    /// A user message that carries file/image attachment *references* (never
    /// inline bytes). The provider wire layer materializes them into
    /// `input_image` content parts at request time, so the persisted item stays
    /// small and the event stream is not bloated by image payloads.
    InputMessage {
        role: String,
        #[serde(default)]
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<MessageAttachment>,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        content: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        /// Responses requires the raw JSON string, not a JSON object.
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
    CustomToolCall {
        call_id: String,
        name: String,
        input: String,
    },
    CustomToolCallOutput {
        call_id: String,
        output: String,
    },
    WebSearchCall {
        id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<serde_json::Value>,
    },
}

pub type ResponseInputItem = ResponseItem;
pub type ResponseOutputItem = ResponseItem;

impl ResponseItem {
    /// Older source-first user mirrors stored text only. Recognize that exact
    /// legacy shape, not an unrelated/native-only message or a different input.
    pub fn is_legacy_text_projection_of(&self, source: &Self) -> bool {
        matches!((self, source),
            (Self::Message { role, content }, Self::InputMessage {
                role: source_role, content: source_content, attachments,
            }) if role == "user" && role == source_role && content == source_content
                && !attachments.is_empty())
    }

    /// Project content without materializing attachment bytes. Durable admission
    /// and live/rebuilt model input share this rule; callers project tools and
    /// reasoning separately.
    pub fn from_message_content(message: &Message) -> Self {
        if message.attachments.is_empty() {
            Self::Message {
                role: message.role.as_str().to_string(),
                content: message.content.clone(),
            }
        } else {
            Self::InputMessage {
                role: message.role.as_str().to_string(),
                content: message.content.clone(),
                attachments: message.attachments.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_projection_preserves_refs_and_only_recognizes_exact_legacy_mirrors() {
        let text = Message::user("same text");
        let legacy = ResponseItem::from_message_content(&text);
        let image = text.clone().with_attachments(vec![MessageAttachment {
            id: "image".into(),
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: Some("image.png".into()),
        }]);
        let projected = ResponseItem::from_message_content(&image);
        assert!(
            matches!(&projected, ResponseItem::InputMessage { attachments, .. }
            if attachments == &image.attachments)
        );
        assert!(legacy.is_legacy_text_projection_of(&projected));
        assert!(!legacy.is_legacy_text_projection_of(&legacy));
        assert!(!projected.is_legacy_text_projection_of(&projected));
        for unrelated in [
            Message::user("different text"),
            Message::assistant("same text"),
        ] {
            assert!(!ResponseItem::from_message_content(&unrelated)
                .is_legacy_text_projection_of(&projected));
        }
        assert!(!serde_json::to_string(&projected)
            .unwrap()
            .contains("base64"));
    }
}
