//! H-49: preparation and every dispatch entry use the active model's ceiling.
use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        request: TransportRequest,
        _: &mut dyn crate::transport::EventSink,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("synthetic-dispatched"))
    }
}

fn config(mode: WireMode, window: usize, limit: usize, configured: Option<u32>) -> ModelConfig {
    let mut config = ModelConfig::deepseek("synthetic").with_wire_mode(mode);
    config.defaults.max_output_tokens = configured;
    config.capabilities = vec![crate::ModelCapability {
        model_id: "active".into(),
        effort: None,
        thinking: None,
        context_window: window,
        max_output_tokens: limit,
        input_modalities: None,
        supports_tools: true,
        supports_thinking: true,
        supports_json_output: true,
        capability_source: crate::CapabilitySource::ProviderMetadata,
        fallback_reason: None,
    }];
    config
}

#[test]
fn h49_preparation_bounds_defaults_and_explicit_overrides_without_inventing_a_default() {
    for (window, limit, configured, requested, expected) in [
        (128_000, 8_192, Some(32_768), None, Some(8_192)),
        (128_000, 8_192, Some(32_768), Some(1_024), Some(1_024)),
        (4_096, 8_192, None, Some(32_768), Some(4_096)),
        (128_000, 8_192, None, None, None),
        (usize::MAX, usize::MAX, None, Some(u32::MAX), Some(u32::MAX)),
    ] {
        let client = ModelClient::new(
            Arc::new(Recorder::default()),
            config(WireMode::Responses, window, limit, configured),
        );
        let mut request = ResponseRequest::new("active", vec![Message::user("synthetic")]);
        request.max_output_tokens = requested;
        let request = client.prepare_response_request(request);
        assert_eq!(request.max_output_tokens, expected);
        assert_eq!(
            client.prepare_response_request(request).max_output_tokens,
            expected
        );
    }
}

#[tokio::test]
async fn h49_all_response_and_native_chat_entries_use_the_prepared_ceiling() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancelled_entry in [false, true] {
            for explicit in [None, Some(1_024), Some(65_536)] {
                let transport = Arc::new(Recorder::default());
                let client = ModelClient::new(
                    transport.clone(),
                    config(mode, 128_000, 8_192, Some(32_768)),
                );
                let mut request = ResponseRequest::new("active", vec![Message::user("synthetic")]);
                request.max_output_tokens = explicit;
                let prepared = client.prepare_response_request(request.clone());
                let cancel = Arc::new(AtomicBool::new(false));
                let result = if cancelled_entry {
                    client.stream_response_cancelled(request, cancel).await
                } else {
                    client.stream_response(request).await
                };
                assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
                let body = transport.0.lock().unwrap()[0].clone();
                let value = body
                    .get("max_output_tokens")
                    .or_else(|| body.get("max_tokens"))
                    .unwrap();
                assert_eq!(*value, json!(explicit.unwrap_or(32_768).min(8_192)));
                assert_eq!(*value, json!(prepared.max_output_tokens.unwrap()));

                let mut chat =
                    ChatCompletionRequest::new("active", vec![Message::user("synthetic")]);
                chat.max_tokens = explicit;
                let result = if cancelled_entry {
                    client
                        .stream_chat_completion_cancelled(chat, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_chat_completion(chat).await
                };
                assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
                assert_eq!(transport.0.lock().unwrap()[1]["max_tokens"], *value);
            }
        }
    }
}

#[tokio::test]
async fn h49_omission_is_preserved_but_zero_and_zero_capacity_are_rejected_before_dispatch() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(transport.clone(), config(mode, 128_000, 8_192, None));
        let request = ResponseRequest::new("active", vec![Message::user("synthetic")]);
        assert_eq!(
            client
                .stream_response(request.clone())
                .await
                .unwrap_err()
                .to_string(),
            "synthetic-dispatched"
        );
        assert!(transport.0.lock().unwrap()[0]
            .get("max_output_tokens")
            .is_none());
        assert!(transport.0.lock().unwrap()[0].get("max_tokens").is_none());
        let error = client
            .stream_response(request.clone().with_max_output_tokens(0))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)));
        assert_eq!(transport.0.lock().unwrap().len(), 1);
        let empty = ModelClient::new(transport.clone(), config(mode, 128_000, 0, Some(1)));
        assert!(matches!(
            empty.stream_response(request).await,
            Err(CoreError::Invalid(_))
        ));
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }
}
