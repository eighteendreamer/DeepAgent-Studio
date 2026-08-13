//! The DeepSeek model client.
//!
//! Wires together request building, a pluggable [`HttpTransport`], the
//! [`SseParser`] framing, and the [`ResponseAccumulator`] streaming assembly into
//! a single [`ModelClient::stream_response`] call that returns a complete
//! [`Response`] with `reasoning_content` preserved.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deepagent_core::error::Result;

use crate::chat::{Response, ResponseRequest};
use crate::stream::ResponseAccumulator;
use crate::transport::{HttpTransport, TransportRequest};

/// Connection / endpoint configuration.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Base URL (without the trailing path).
    pub base_url: String,
    /// API key (bearer token).
    pub api_key: String,
    /// Global provider-level overrides applied to every request built by this client.
    pub defaults: ResponseDefaults,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseDefaults {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_logprobs: Option<u8>,
    pub reasoning_effort: Option<String>,
    pub text: Option<serde_json::Value>,
    pub tool_choice: Option<serde_json::Value>,
    pub user: Option<String>,
    pub native_web_search: bool,
}

impl ModelConfig {
    /// DeepSeek defaults (base URL is hard-coded); supply your API key.
    pub fn deepseek(api_key: impl Into<String>) -> Self {
        Self {
            base_url: crate::discovery::DEEPSEEK_BASE_URL.to_string(),
            api_key: api_key.into(),
            defaults: ResponseDefaults::default(),
        }
    }

    /// Build a config from a discovered [`ModelCatalog`] + API key, targeting a
    /// specific role's model. This is how the system wires the user's key to the
    /// auto-selected model after project initialization.
    pub fn from_catalog(
        api_key: impl Into<String>,
        catalog: &crate::discovery::ModelCatalog,
        role: crate::discovery::ModelRole,
    ) -> Self {
        let _ = catalog.model_for(role); // role resolution lives in the catalog
        Self {
            base_url: catalog.base_url.clone(),
            api_key: api_key.into(),
            defaults: ResponseDefaults::default(),
        }
    }

    /// The fully-qualified DeepSeek Responses endpoint.
    pub fn endpoint(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    pub fn with_defaults(mut self, defaults: ResponseDefaults) -> Self {
        self.defaults = defaults;
        self
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

    /// Send a Responses request and assemble the streamed response.
    ///
    /// The request is forced into streaming mode. Each SSE payload is parsed and
    /// folded by a [`ResponseAccumulator`]; the final assembled [`Response`]
    /// retains `reasoning_content` for Thinking Mode persistence.
    pub async fn stream_response(&self, request: ResponseRequest) -> Result<Response> {
        self.stream_response_observed(request, &mut crate::stream::NoopObserver)
            .await
    }

    /// Responses API name for the cancel-aware streaming entrypoint.
    pub async fn stream_response_cancelled(
        &self,
        request: ResponseRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<Response> {
        self.stream_response_observed_cancelled(request, &mut crate::stream::NoopObserver, cancel)
            .await
    }

    /// Like [`ModelClient::stream_response`] but forwards each semantic delta
    /// (content / reasoning / tool-call start) to `observer` as it arrives, for
    /// live token streaming to a UI or event bus.
    pub async fn stream_response_observed(
        &self,
        mut request: ResponseRequest,
        observer: &mut dyn crate::stream::DeltaObserver,
    ) -> Result<Response> {
        request.stream = true;
        self.apply_defaults(&mut request);
        // Ask the provider to include a final usage chunk in the stream.
        let body = serde_json::to_string(&request)?;
        let transport_req = TransportRequest {
            url: self.config.endpoint(),
            api_key: self.config.api_key.clone(),
            body,
        };

        let mut accumulator = ResponseAccumulator::new();

        // The transport's contract is to deliver already-de-framed SSE payloads
        // (the reqwest transport runs the SseParser; the mock yields payloads
        // directly). So we fold each payload straight into the accumulator,
        // notifying the observer of each semantic fragment.
        {
            let acc = &mut accumulator;
            let mut sink =
                move |data: &str| -> Result<bool> { acc.push_sse_data_observed(data, observer) };
            self.transport.stream(transport_req, &mut sink).await?;
        }

        accumulator.finish()
    }

    /// Like [`ModelClient::stream_response_observed`], but aborts promptly when
    /// `cancel` is set. The real reqwest transport uses this to stop an
    /// in-flight SSE body read; mock/default transports still preserve the old
    /// behavior unless they opt in.
    pub async fn stream_response_observed_cancelled(
        &self,
        mut request: ResponseRequest,
        observer: &mut dyn crate::stream::DeltaObserver,
        cancel: Arc<AtomicBool>,
    ) -> Result<Response> {
        request.stream = true;
        self.apply_defaults(&mut request);
        let body = serde_json::to_string(&request)?;
        let transport_req = TransportRequest {
            url: self.config.endpoint(),
            api_key: self.config.api_key.clone(),
            body,
        };

        let mut accumulator = ResponseAccumulator::new();
        {
            let acc = &mut accumulator;
            let mut sink =
                move |data: &str| -> Result<bool> { acc.push_sse_data_observed(data, observer) };
            self.transport
                .stream_cancelled(transport_req, &mut sink, cancel)
                .await?;
        }

        accumulator.finish()
    }

    fn apply_defaults(&self, request: &mut ResponseRequest) {
        if request.temperature.is_none() {
            request.temperature = self.config.defaults.temperature;
        }
        if request.top_p.is_none() {
            request.top_p = self.config.defaults.top_p;
        }
        if request.max_output_tokens.is_none() {
            request.max_output_tokens = self.config.defaults.max_output_tokens;
        }
        if request.top_logprobs.is_none() {
            request.top_logprobs = self.config.defaults.top_logprobs;
        }
        if request.reasoning_effort.is_none() {
            request.reasoning_effort = self.config.defaults.reasoning_effort.clone();
        }
        if request.text.is_none() {
            request.text = self.config.defaults.text.clone();
        }
        if request.tool_choice.is_none() {
            request.tool_choice = self.config.defaults.tool_choice.clone();
        }
        if request.user.is_none() {
            request.user = self.config.defaults.user.clone();
        }
        if self.config.defaults.native_web_search {
            for tool in &mut request.tools {
                if tool.function.name == "web_search" {
                    tool.kind = "web_search".to_string();
                }
            }
        }
        for tool in &mut request.tools {
            if tool.function.name == "apply_patch" {
                tool.kind = "custom".to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
    use async_trait::async_trait;
    use deepagent_core::message::Message;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn client_with(events: Vec<String>) -> ModelClient {
        let transport = Arc::new(MockTransport::new(events));
        ModelClient::new(transport, ModelConfig::deepseek("test-key"))
    }

    #[test]
    fn endpoint_is_well_formed() {
        let cfg = ModelConfig::deepseek("k");
        assert_eq!(cfg.endpoint(), "https://api.deepseek.com/responses");
    }

    #[tokio::test]
    async fn streams_and_assembles_content() {
        let events = vec![
            r#"{"type":"response.created","response":{"id":"r1","status":"in_progress"}}"#.to_string(),
            r#"{"type":"response.reasoning_text.delta","delta":"thinking "}"#.to_string(),
            r#"{"type":"response.reasoning_text.delta","delta":"hard"}"#.to_string(),
            r#"{"type":"response.output_text.delta","delta":"Hello"}"#.to_string(),
            r#"{"type":"response.output_text.delta","delta":" there"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#.to_string(),
        ];
        let client = client_with(events);
        let resp = client
            .stream_response(ResponseRequest::new(
                "deepseek-v4-pro",
                vec![Message::user("hi")],
            ))
            .await
            .unwrap();
        assert_eq!(resp.output_text_projection(), "Hello there");
        assert_eq!(
            resp.assistant_message_projection()
                .reasoning_content
                .as_deref(),
            Some("thinking hard")
        );
    }

    struct WaitingTransport {
        entered: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl HttpTransport for WaitingTransport {
        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            panic!("cancelled stream should use stream_cancelled");
        }

        async fn stream_cancelled(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
            cancel: Arc<AtomicBool>,
        ) -> Result<()> {
            self.entered.notify_waiters();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(deepagent_core::error::CoreError::other("request cancelled"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }

    #[tokio::test]
    async fn stream_response_cancelled_interrupts_transport() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let cancel = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(WaitingTransport {
            entered: entered.clone(),
        });
        let client = ModelClient::new(transport, ModelConfig::deepseek("test-key"));
        let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")]);

        let run = client.stream_response_cancelled(request, cancel.clone());
        tokio::pin!(run);
        tokio::select! {
            result = &mut run => {
                panic!("stream completed before cancellation: {result:?}");
            }
            _ = entered.notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                panic!("transport did not enter stream_cancelled");
            }
        }
        cancel.store(true, Ordering::Relaxed);

        let err = tokio::time::timeout(std::time::Duration::from_secs(1), run)
            .await
            .expect("stream should stop promptly")
            .expect_err("cancelled stream should return an error");
        assert!(err.to_string().contains("request cancelled"));
    }

    #[tokio::test]
    async fn streams_tool_calls_end_to_end() {
        let events = vec![
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"item1","call_id":"c1","name":"search","arguments":""}}"#.to_string(),
            r#"{"type":"response.function_call_arguments.delta","item_id":"item1","delta":"{\"q\":"}"#.to_string(),
            r#"{"type":"response.function_call_arguments.delta","item_id":"item1","delta":"\"rust\"}"}"#.to_string(),
            r#"{"type":"response.function_call_arguments.done","item_id":"item1","arguments":"{\"q\":\"rust\"}"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#.to_string(),
        ];
        let client = client_with(events);
        let resp = client
            .stream_response(ResponseRequest::new(
                "deepseek-v4-flash",
                vec![Message::user("find rust")],
            ))
            .await
            .unwrap();
        let calls = resp.tool_invocations_from_items();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "search");
        assert_eq!(calls[0].2, serde_json::json!({"q": "rust"}));
    }
}
