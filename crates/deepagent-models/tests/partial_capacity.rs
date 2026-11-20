//! H-296: discovered partial capacities reach real serializers, not just previews.

#[cfg(test)]
#[path = "support/metadata.rs"]
mod metadata_fixture;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message, Result};
use deepagent_models::discovery::ModelInfo;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest, WireMode};
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("h296 synthetic dispatch"))
    }
}

fn config(model: &str, context: Option<u64>, output: Option<u64>, mode: WireMode) -> ModelConfig {
    let entry: ModelInfo = serde_json::from_value(json!({
        "id":model,"context_window":context,"max_output_tokens":output
    }))
    .unwrap();
    let catalog = vec![entry];
    let restored: Vec<deepagent_models::ModelInfo> =
        serde_json::from_value(serde_json::to_value(catalog).unwrap()).unwrap();
    crate::metadata_fixture::config("synthetic-h296", &restored).with_wire_mode(mode)
}

#[tokio::test]
async fn h296_partial_capacities_bound_wire_without_inventing_request_defaults() {
    for (model, base_window, base_output) in [
        ("deepseek-flash", 1_000_000, 384_000),
        ("audit-capacity-model", 128_000, 32_000),
    ] {
        for context in [None, Some(32_768)] {
            for output in [None, Some(8_192)] {
                for mode in [WireMode::Responses, WireMode::ChatCompletions] {
                    for (default, requested) in [
                        (None, None),
                        (Some(65_536), None),
                        (Some(65_536), Some(1_024)),
                    ] {
                        let transport = Arc::new(Recorder::default());
                        let mut config = config(model, context, output, mode);
                        config.defaults.max_output_tokens = default;
                        let client = ModelClient::new(transport.clone(), config);
                        let mut request =
                            ResponseRequest::new(model, vec![Message::user("synthetic")]);
                        request.max_output_tokens = requested;
                        let expected = requested.or(default).map(|value| {
                            value
                                .min(output.unwrap_or(base_output) as u32)
                                .min(context.unwrap_or(base_window) as u32)
                        });
                        assert_eq!(
                            client
                                .prepare_response_request(request.clone())
                                .max_output_tokens,
                            expected
                        );
                        assert_eq!(
                            client
                                .stream_response(request)
                                .await
                                .unwrap_err()
                                .to_string(),
                            "h296 synthetic dispatch"
                        );
                        let bodies = transport.0.lock().unwrap();
                        assert_eq!(bodies.len(), 1);
                        let field = if mode == WireMode::Responses {
                            "max_output_tokens"
                        } else {
                            "max_tokens"
                        };
                        assert_eq!(
                            bodies[0].get(field).and_then(Value::as_u64),
                            expected.map(u64::from)
                        );
                        assert!(bodies[0].get("capability_source").is_none());
                        assert!(bodies[0].get("fallback_reason").is_none());
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h296_zero_partial_capacities_do_not_widen_configured_requests() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for (context, output) in [(None, Some(0)), (Some(0), None)] {
            let transport = Arc::new(Recorder::default());
            let client = ModelClient::new(
                transport.clone(),
                config("audit-capacity-model", context, output, mode),
            );
            let request =
                ResponseRequest::new("audit-capacity-model", vec![Message::user("synthetic")])
                    .with_max_output_tokens(1);
            assert!(matches!(
                client.stream_response(request).await,
                Err(CoreError::Invalid(_))
            ));
            assert!(transport.0.lock().unwrap().is_empty());
        }
    }
}
