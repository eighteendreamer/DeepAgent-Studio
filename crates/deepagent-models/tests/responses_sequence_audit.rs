//! Audit that DeepSeek Responses stream sequence numbers are enforced.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest};

#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        for event in [
            r#"{"type":"response.created","sequence_number":0,"response":{"id":"resp-sequence-audit","object":"response","created_at":0,"status":"in_progress","model":"deepseek-flash","output":[]}}"#,
            r#"{"type":"response.output_text.delta","sequence_number":0,"delta":"offline"}"#,
            r#"{"type":"response.output_text.done","sequence_number":4,"text":"offline"}"#,
            r#"{"type":"response.completed","sequence_number":2,"response":{"id":"resp-sequence-audit","object":"response","created_at":0,"status":"completed","model":"deepseek-flash","output":[]}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn audit_non_monotonic_responses_sequence_is_rejected() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    let result = client
        .stream_response(ResponseRequest::new(
            "deepseek-flash",
            vec![Message::user("offline sequence contract audit")],
        ))
        .await;
    let requests = transport.requests.lock().unwrap();
    eprintln!(
        "Responses sequence audit: error={} transport_calls={} result={result:?}",
        result.is_err(),
        requests.len()
    );

    assert!(
        result.is_err(),
        "duplicate, skipped, or descending sequence_number values must be rejected"
    );
}
