//! Provider-neutral Responses API item primitives.
//!
//! DeepSeek's Responses API accepts and returns semantic input/output items
//! (`message`, `reasoning`, `function_call`, `function_call_output`,
//! `custom_tool_call`, `custom_tool_call_output`, and `web_search_call`).
//! Keeping this model in `deepagent-core` lets persistence, session recovery,
//! model requests, and runtime events share one typed contract instead of
//! storing provider protocol state as untyped JSON.

use serde::{Deserialize, Serialize};

use crate::message::{Message, MessageAttachment, ToolCall};
use crate::tool_content::ToolResultBody;

/// Local auxiliary user-data envelope, not a system-reminder or provider type.
/// It must travel with its source turn without acquiring an independent snip ID.
pub const HOOK_CONTEXT_PREFIX: &str = "[Hook output context (unprivileged)]\n";

/// Execution ownership of a native discovery item. Provider execution does
/// not authorize the host to invoke or register the returned tool definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSearchExecution {
    Client,
    Server,
}

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
        output: ToolResultBody,
    },
    CustomToolCall {
        call_id: String,
        name: String,
        input: String,
    },
    CustomToolCallOutput {
        call_id: String,
        output: ToolResultBody,
    },
    WebSearchCall {
        id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<serde_json::Value>,
    },
    /// Native compatibility vocabulary aligned with the inspected Codex
    /// reference, separate from an ordinary function named `tool_search`.
    ToolSearchCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        execution: ToolSearchExecution,
        arguments: serde_json::Value,
    },
    ToolSearchOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
        status: String,
        execution: ToolSearchExecution,
        #[serde(default)]
        tools: Vec<serde_json::Value>,
    },
}

pub type ResponseInputItem = ResponseItem;
pub type ResponseOutputItem = ResponseItem;

impl ResponseItem {
    /// An ordinary tool invocation is a function, regardless of its name.
    /// Custom calls must enter through the explicit native variant.
    pub fn from_function_call(call: &ToolCall) -> Self {
        Self::FunctionCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.to_string(),
        }
    }

    /// Check a UI/function-shaped mirror without guessing its native kind.
    /// A custom call projects only to its exact patch argument, not arbitrary
    /// same-name JSON. This also preserves legacy source-first native records.
    pub fn is_tool_call_projection_of(&self, call: &ToolCall) -> bool {
        match self {
            Self::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                call_id == &call.id
                    && name == &call.name
                    && serde_json::from_str::<serde_json::Value>(arguments)
                        .is_ok_and(|arguments| arguments == call.arguments)
            }
            Self::CustomToolCall {
                call_id,
                name,
                input,
            } => {
                call_id == &call.id
                    && name == &call.name
                    && call.arguments == serde_json::json!({"patch": input})
            }
            _ => false,
        }
    }

    /// Read-only migration of old, untyped events. Historical versions encoded
    /// custom apply_patch as a function-shaped ToolCall. Never use this for new
    /// requests or overwrite a persisted native call with this inference.
    pub fn from_legacy_tool_call(call: &ToolCall) -> Self {
        if call.name == "apply_patch" {
            Self::CustomToolCall {
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call
                    .arguments
                    .get("patch")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        } else {
            Self::from_function_call(call)
        }
    }

    /// Build a host discovery result from the real tool result and the host's
    /// authorized catalog. Provider-returned schemas are never a lookup source.
    pub fn native_tool_search_output(
        call_id: String,
        ok: bool,
        output: &serde_json::Value,
        mut schema: impl FnMut(&str) -> Option<serde_json::Value>,
    ) -> Self {
        let tools = output
            .get("matches")
            .and_then(serde_json::Value::as_array)
            .filter(|_| ok)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
            .filter_map(&mut schema)
            .collect();
        Self::ToolSearchOutput {
            id: None,
            call_id: Some(call_id),
            status: if ok { "completed" } else { "failed" }.into(),
            execution: ToolSearchExecution::Client,
            tools,
        }
    }

    /// Pairing identity of a host-executed call. Server discovery is provider
    /// data and must never acquire local execution or pairing ownership.
    pub fn client_tool_call_id(&self) -> Option<&str> {
        match self {
            Self::FunctionCall { call_id, .. } | Self::CustomToolCall { call_id, .. } => {
                Some(call_id)
            }
            Self::ToolSearchCall {
                call_id,
                execution: ToolSearchExecution::Client,
                ..
            } => call_id.as_deref(),
            _ => None,
        }
    }

    /// Identity paired with `client_tool_call_id`, shared by validation,
    /// history repair, snipping and compaction.
    pub fn client_tool_output_call_id(&self) -> Option<&str> {
        match self {
            Self::FunctionCallOutput { call_id, .. }
            | Self::CustomToolCallOutput { call_id, .. } => Some(call_id),
            Self::ToolSearchOutput {
                call_id,
                execution: ToolSearchExecution::Client,
                ..
            } => call_id.as_deref(),
            _ => None,
        }
    }

    /// References present in the canonical Surface, regardless of whether
    /// they originated in user input or in an executed tool result.
    pub fn image_attachments(&self) -> Vec<&MessageAttachment> {
        let attachments: Vec<_> = match self {
            Self::InputMessage { attachments, .. } => attachments.iter().collect(),
            Self::FunctionCallOutput { output, .. } | Self::CustomToolCallOutput { output, .. } => {
                output.attachments().collect()
            }
            _ => Vec::new(),
        };
        attachments
            .into_iter()
            .filter(|attachment| attachment.kind == "image")
            .collect()
    }
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
    fn h156_ordinary_calls_and_legacy_custom_mirrors_have_explicit_boundaries() {
        let mut call = ToolCall {
            id: "fixture".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"patch":"synthetic"}),
        };
        let function = ResponseItem::from_function_call(&call);
        let custom = ResponseItem::from_legacy_tool_call(&call);
        assert!(matches!(function, ResponseItem::FunctionCall { .. }));
        assert!(matches!(custom, ResponseItem::CustomToolCall { .. }));
        assert!(function.is_tool_call_projection_of(&call));
        assert!(custom.is_tool_call_projection_of(&call));
        call.arguments["extra"] = serde_json::json!(true);
        assert!(
            !custom.is_tool_call_projection_of(&call),
            "same name cannot hide additional arguments"
        );
        assert!(!function.is_tool_call_projection_of(&call));
        call.name = "other".into();
        assert!(matches!(
            ResponseItem::from_legacy_tool_call(&call),
            ResponseItem::FunctionCall { .. }
        ));
    }

    #[test]
    fn content_projection_preserves_refs_and_only_recognizes_exact_legacy_mirrors() {
        let text = Message::user("same text");
        let legacy = ResponseItem::from_message_content(&text);
        let image = text.clone().with_attachments(vec![MessageAttachment {
            sha256: Some("a".repeat(64)),
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
