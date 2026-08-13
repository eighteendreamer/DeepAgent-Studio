//! Provider-neutral Responses API item primitives.
//!
//! DeepSeek's Responses API accepts and returns semantic input/output items
//! (`message`, `reasoning`, `function_call`, `function_call_output`,
//! `custom_tool_call`, `custom_tool_call_output`, and `web_search_call`).
//! Keeping this model in `deepagent-core` lets persistence, session recovery,
//! model requests, and runtime events share one typed contract instead of
//! storing provider protocol state as untyped JSON.

use serde::{Deserialize, Serialize};

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
