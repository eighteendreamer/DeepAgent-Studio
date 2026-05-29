//! The DeepSeek model client.
//!
//! Wires together request building, a pluggable [`HttpTransport`], the
//! [`SseParser`] framing, and the [`DeltaAccumulator`] streaming assembly into
//! a single [`ModelClient::stream_chat`] call that returns a complete
//! [`ChatResponse`] with `reasoning_content` preserved.

use std::sync::Arc;

use deepagent_core::error::Result;

use crate::chat::{ChatRequest, ChatResponse};
use crate::stream::DeltaAccumulator;
use crate::transport::{HttpTransport, TransportRequest};

/// Connection / endpoint configuration.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Base URL (without the trailing path).
    pub base_url: String,
    /// Chat-completions path appended to `base_url`.
    pub chat_path: String,
    /// API key (bearer token).
    pub api_key: String,
}

impl ModelConfig {
    /// DeepSeek defaults; supply your API key.
    pub fn deepseek(api_key: impl Into<String>) -> Self {
        Self {
            base_url: "https://api.deepseek.com".to_string(),
            chat_path: "/chat/completions".to_string(),
            api_key: api_key.into(),
        }
    }

    /// The fully-qualified chat-completions endpoint.
    pub fn endpoint(&self) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            self.chat_path.trim_start_matches('/')
        )
    }
}

/// A model client over an arbitrary transport.
pub struct ModelClient {
    transport: Arc<dyn HttpTransport>,
    config: ModelConfig,
}

impl ModelClient {
    /// Build a client from a transport and config.
    pub fn new(transport: Arc<dyn HttpTransport>, config: ModelConfig) -> Self {
        Self { transport, config }
    }

    /// Send a chat request and assemble the streamed response.
    ///
    /// The request is forced into streaming mode. Each SSE payload is parsed and
    /// folded by a [`DeltaAccumulator`]; the final assembled [`ChatResponse`]
    /// retains `reasoning_content` for Thinking Mode persistence.
    pub async fn stream_chat(&self, mut request: ChatRequest) -> Result<ChatResponse> {
        request.stream = true;
        let body = serde_json::to_string(&request)?;
        let transport_req = TransportRequest {
            url: self.config.endpoint(),
            api_key: self.config.api_key.clone(),
            body,
        };

        let mut accumulator = DeltaAccumulator::new();

        // The transport's contract is to deliver already-de-framed SSE payloads
        // (the reqwest transport runs the SseParser; the mock yields payloads
        // directly). So we fold each payload straight into the accumulator.
        {
            let acc = &mut accumulator;
            let mut sink = move |data: &str| -> Result<bool> { acc.push_sse_data(data) };
            self.transport.stream(transport_req, &mut sink).await?;
        }

        accumulator.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use deepagent_core::message::Message;

    fn client_with(events: Vec<String>) -> ModelClient {
        let transport = Arc::new(MockTransport::new(events));
        ModelClient::new(transport, ModelConfig::deepseek("test-key"))
    }

    #[test]
    fn endpoint_is_well_formed() {
        let cfg = ModelConfig::deepseek("k");
        assert_eq!(cfg.endpoint(), "https://api.deepseek.com/chat/completions");
    }

    #[tokio::test]
    async fn streams_and_assembles_content() {
        let events = vec![
            r#"{"choices":[{"delta":{"reasoning_content":"thinking "}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"reasoning_content":"hard"}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"content":"Hello"}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"content":" there"}}]}"#.to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let client = client_with(events);
        let resp = client
            .stream_chat(ChatRequest::new(
                "deepseek-reasoner",
                vec![Message::user("hi")],
            ))
            .await
            .unwrap();
        assert_eq!(resp.message.content, "Hello there");
        assert_eq!(
            resp.message.reasoning_content.as_deref(),
            Some("thinking hard")
        );
    }

    #[tokio::test]
    async fn streams_tool_calls_end_to_end() {
        let events = vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"search","arguments":"{\"q\":"}}]}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"rust\"}"}}]}}]}"#.to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let client = client_with(events);
        let resp = client
            .stream_chat(ChatRequest::new(
                "deepseek-chat",
                vec![Message::user("find rust")],
            ))
            .await
            .unwrap();
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "search");
        assert_eq!(
            resp.message.tool_calls[0].arguments,
            serde_json::json!({"q": "rust"})
        );
    }
}
