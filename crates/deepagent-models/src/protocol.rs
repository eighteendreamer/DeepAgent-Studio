//! Wire protocol factory (plan §5).
//!
//! `codec_for` is the single production dispatch point from a `WireMode` to
//! its complete codec: preparation, encoding, sending and semantic decoding.
//! `ModelClient` owns shared transport and delegates protocol behavior here.

use deepagent_core::error::Result;

use std::sync::{atomic::AtomicBool, Arc};

use crate::chat::{ReasoningEffort, Response, ResponseRequest};
use crate::client::{response_to_chat_completion, ModelClient, WireMode};
use crate::stream::DeltaObserver;
use crate::ToolSchema;

/// Protocol-specific behaviour behind one `WireMode`.
#[async_trait::async_trait]
pub(crate) trait WireCodec: Send + Sync {
    /// Stable protocol name for logs and diagnostics.
    fn name(&self) -> &'static str;

    fn auth(&self) -> crate::transport::TransportAuth {
        crate::transport::TransportAuth::Bearer
    }

    /// Serialize tools into this protocol's tool vocabulary.
    fn project_tools(&self, client: &ModelClient, tools: &[ToolSchema]) -> Vec<serde_json::Value>;

    fn tool_kind<'a>(&self, schema: &'a serde_json::Value) -> Result<&'a str> {
        schema
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| deepagent_core::error::CoreError::invalid("wire schema has no kind"))
    }

    fn tool_name<'a>(&self, schema: &'a serde_json::Value) -> Option<&'a str> {
        schema
            .get("name")
            .or_else(|| schema.get("function")?.get("name"))
            .and_then(serde_json::Value::as_str)
    }

    /// Own request preparation and stream decoding; the host owns transport/cancellation.
    async fn stream(
        &self,
        client: &ModelClient,
        request: ResponseRequest,
        observer: &mut dyn DeltaObserver,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Response>;

    fn decoder(&self) -> Box<dyn StreamDecoder>;

    fn validate_image_role(&self, role: &str) -> Result<()>;

    fn validate_tool_images(&self, _request: &ResponseRequest) -> Result<()> {
        Ok(())
    }

    fn sampling(
        &self,
        _client: &ModelClient,
        _request: &ResponseRequest,
    ) -> Option<crate::EffectiveSamplingConfig> {
        None
    }

    fn legacy_reasoning_default(
        &self,
        _capability: &crate::ModelCapability,
    ) -> Option<ReasoningEffort> {
        None
    }

    fn resolve_reasoning_effort(
        &self,
        capability: &crate::ModelCapability,
        requested: Option<ReasoningEffort>,
    ) -> Result<Option<ReasoningEffort>> {
        if capability.thinking.is_none() {
            Ok(requested)
        } else {
            capability.resolve_reasoning_effort(requested)
        }
    }

    /// Build the streaming request body for one prepared turn.
    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String>;
}

/// Stateful semantic stream, including protocol-owned usage and stop mapping.
pub(crate) trait StreamDecoder: Send {
    fn push(&mut self, data: &str, observer: &mut dyn DeltaObserver) -> Result<bool>;
    fn finish(self: Box<Self>) -> Result<Response>;
}

macro_rules! decoder {
    ($($accumulator:ty),+ $(,)?) => { $(
        impl StreamDecoder for $accumulator {
            fn push(&mut self, data: &str, observer: &mut dyn DeltaObserver) -> Result<bool> {
                self.push_sse_data_observed(data, observer)
            }
            fn finish(self: Box<Self>) -> Result<Response> { (*self).finish() }
        }
    )+ };
}
decoder!(
    crate::ResponseAccumulator,
    crate::ChatCompletionAccumulator,
    crate::messages::MessagesAccumulator
);

mod chat;
mod messages;
mod responses;
use chat::ChatCompletionsCodec;
use messages::MessagesCodec;
#[cfg(test)]
pub(crate) use responses::request::response_input_items_to_wire;
pub(crate) use responses::request::{
    response_tools_to_wire, serialize as serialize_response_request,
};
use responses::ResponsesCodec;

static RESPONSES: ResponsesCodec = ResponsesCodec;
pub(crate) static CHAT_COMPLETIONS: ChatCompletionsCodec = ChatCompletionsCodec;
static MESSAGES: MessagesCodec = MessagesCodec;

/// DeepSeek Harness provider.ts:48-53: configured protocols select a complete implementation.
static PROTOCOLS: &[(WireMode, &dyn WireCodec)] = &[
    (WireMode::Responses, &RESPONSES),
    (WireMode::ChatCompletions, &CHAT_COMPLETIONS),
    (WireMode::Messages, &MESSAGES),
];

pub(crate) fn codec_for(mode: WireMode) -> &'static dyn WireCodec {
    PROTOCOLS
        .iter()
        .find(|(protocol, _)| *protocol == mode)
        .map(|(_, codec)| *codec)
        .expect("every WireMode must be registered in PROTOCOLS")
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
        let client = ModelClient::new(
            std::sync::Arc::new(crate::MockTransport::default()),
            crate::ModelConfig::deepseek("synthetic"),
        );
        let tools = vec![ToolSchema::function(
            "read_file",
            "Read one file",
            json!({"type": "object", "properties": {}}),
        )];

        assert_eq!(
            codec_for(WireMode::Responses).project_tools(&client, &tools),
            crate::chat::response_tools_to_wire(&tools)
        );
        assert_eq!(
            codec_for(WireMode::ChatCompletions).project_tools(&client, &tools),
            crate::chat_completions::chat_tools_to_wire(&tools)
        );
    }
}
