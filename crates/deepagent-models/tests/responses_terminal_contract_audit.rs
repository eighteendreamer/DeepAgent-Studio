//! Audit that a Responses terminal event carries a valid provider response.

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
            r#"{"type":"response.output_text.delta","delta":"offline"}"#,
            r#"{"type":"response.completed","response":{}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "audit reproducer: malformed Responses terminal response must be rejected"]
async fn audit_completed_event_without_required_response_fields_is_rejected() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    let result = client
        .stream_response(ResponseRequest::new(
            "deepseek-flash",
            vec![Message::user("offline terminal contract audit")],
        ))
        .await;
    let requests = transport.requests.lock().unwrap();
    eprintln!(
        "Responses terminal contract audit: error={} transport_calls={} result={result:?}",
        result.is_err(),
        requests.len()
    );

    assert!(
        result.is_err(),
        "response.completed without required response.id/status must be rejected"
    );
}
