//! Wire protocol factory (plan §5).
//!
//! `codec_for` is the single production dispatch point from a `WireMode` to
//! its codec. Codecs own protocol-specific projection; `ModelClient` must not
//! carry `WireMode::<variant> =>` match arms.

use deepagent_core::error::Result;

use crate::chat::ResponseRequest;
use crate::client::{request_body, response_to_chat_completion, ModelClient, WireMode};
use crate::ToolSchema;

/// Protocol-specific behaviour behind one `WireMode`.
pub(crate) trait WireCodec: Send + Sync {
    /// Stable protocol name for logs and diagnostics.
    fn name(&self) -> &'static str;

    /// Serialize tools into this protocol's tool vocabulary.
    fn project_tools(&self, tools: &[ToolSchema]) -> Vec<serde_json::Value>;

    /// Build the streaming request body for one prepared turn.
    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String>;
}

struct ResponsesCodec;

impl WireCodec for ResponsesCodec {
    fn name(&self) -> &'static str {
        "responses"
    }

    fn project_tools(&self, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::chat::response_tools_to_wire(tools)
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        request_body::encode(&client.prepare_response_request(request))
    }
}

struct ChatCompletionsCodec;

impl WireCodec for ChatCompletionsCodec {
    fn name(&self) -> &'static str {
        "chat_completions"
    }

    fn project_tools(&self, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::chat_completions::chat_tools_to_wire(tools)
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        let mut chat = response_to_chat_completion(request)?;
        chat.stream = true;
        client.apply_chat_defaults(&mut chat)?;
        request_body::encode(&chat)
    }
}

struct MessagesCodec;

impl WireCodec for MessagesCodec {
    fn name(&self) -> &'static str {
        "messages"
    }

    fn project_tools(&self, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::messages::project_tools(tools)
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        let request = client.prepare_response_request(request);
        let max_tokens = request.max_output_tokens.ok_or_else(|| {
            deepagent_core::error::CoreError::invalid(
                "Messages requests require a positive max_output_tokens value",
            )
        })?;
        crate::messages::encode_request(&request, max_tokens)
    }
}

static RESPONSES: ResponsesCodec = ResponsesCodec;
static CHAT_COMPLETIONS: ChatCompletionsCodec = ChatCompletionsCodec;
static MESSAGES: MessagesCodec = MessagesCodec;

/// The one production match on wire protocol. Add a protocol here and nowhere
/// else in the client path.
pub(crate) fn codec_for(mode: WireMode) -> &'static dyn WireCodec {
    match mode {
        WireMode::Responses => &RESPONSES,
        WireMode::ChatCompletions => &CHAT_COMPLETIONS,
        WireMode::Messages => &MESSAGES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolSchema;
    use serde_json::json;

    #[test]
    fn each_wire_mode_resolves_to_a_named_codec() {
        assert_eq!(codec_for(WireMode::Responses).name(), "responses");
        assert_eq!(
            codec_for(WireMode::ChatCompletions).name(),
            "chat_completions"
        );
        assert_eq!(codec_for(WireMode::Messages).name(), "messages");
    }

    #[test]
    fn codec_tool_projection_matches_the_existing_serializers() {
        let tools = vec![ToolSchema::function(
            "read_file",
            "Read one file",
            json!({"type": "object", "properties": {}}),
        )];

        assert_eq!(
            codec_for(WireMode::Responses).project_tools(&tools),
            crate::chat::response_tools_to_wire(&tools)
        );
        assert_eq!(
            codec_for(WireMode::ChatCompletions).project_tools(&tools),
            crate::chat_completions::chat_tools_to_wire(&tools)
        );
    }

    #[test]
    fn client_production_code_has_no_protocol_match_arms() {
        // Audit only the production half of client.rs (before its test modules).
        let production = include_str!("client.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("client.rs has a production section");
        for arm in ["WireMode::Responses =>", "WireMode::ChatCompletions =>"] {
            assert!(
                !production.contains(arm),
                "client.rs still dispatches on `{arm}`; route it through protocol::codec_for"
            );
        }
    }
}
