//! Audit the model-specific reasoning metadata at the Responses request seam.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    ModelCatalog, ModelClient, ModelConfig, ModelDiscovery, ModelRole, ReasoningEffort,
    ResponseDefaults, ResponseRequest,
};

const CATALOG: &str = r#"{"object":"list","data":[{
  "id":"deepseek-effort-limited","object":"model","owned_by":"deepseek",
  "context_window":1048576,"max_output_tokens":393216,
  "input_modalities":["text"],"output_modalities":["text"],
  "effort":{"supported_levels":["low"],"default_level":"low"},
  "api_capabilities":{}
}]}"#;

#[derive(Default)]
struct AuditTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for AuditTransport {
    async fn get_json(&self, url: &str, _key: &str) -> Result<String> {
        assert_eq!(url, "https://api.deepseek.com/models");
        Ok(CATALOG.to_string())
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        for event in [
            r#"{"type":"response.output_text.delta","delta":"offline audit"}"#,
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "audit reproducer: model-advertised effort levels must constrain the Responses request"]
async fn audit_model_effort_metadata_constrains_responses_request() {
    let transport = Arc::new(AuditTransport::default());
    let catalog = ModelDiscovery::deepseek(transport.clone())
        .discover("offline-audit-placeholder")
        .await
        .unwrap();
    let serialized_catalog = serde_json::to_value(&catalog).unwrap();
    let effort_metadata_retained = serialized_catalog["available"][0].get("effort").is_some();
    let restored: ModelCatalog = serde_json::from_value(serialized_catalog).unwrap();
    let config = ModelConfig::from_catalog("offline-audit-placeholder", &restored, ModelRole::Chat)
        .with_defaults(ResponseDefaults {
            reasoning_effort: Some(ReasoningEffort::High),
            ..ResponseDefaults::default()
        });
    let client = ModelClient::new(transport.clone(), config);
    let request = ResponseRequest::new(
        "deepseek-effort-limited",
        vec![Message::user("Reply briefly.")],
    );
    client.stream_response(request).await.unwrap();

    let requests = transport.requests.lock().unwrap();
    let actual_effort = requests[0]["reasoning"]["effort"].as_str();
    eprintln!(
        "model-effort audit: metadata_retained={effort_metadata_retained} advertised=[low] default=low wire_effort={actual_effort:?}"
    );
    assert!(
        effort_metadata_retained && actual_effort != Some("high"),
        "the discovered model declares only low effort, but the catalog discarded that capability and the Responses request still sent high"
    );
}
