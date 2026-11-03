//! Audit DeepSeek Responses role validation for public native input items.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseInputItem, ResponseRequest};

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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn request(role: &str, attachments: bool) -> ResponseRequest {
    let item = if attachments {
        ResponseInputItem::InputMessage {
            role: role.to_owned(),
            content: "invalid role audit".to_owned(),
            attachments: Vec::new(),
        }
    } else {
        ResponseInputItem::Message {
            role: role.to_owned(),
            content: "invalid role audit".to_owned(),
        }
    };
    ResponseRequest::from_response_items("deepseek-flash", None, vec![item])
}

#[tokio::test]
#[ignore = "audit reproducer: invalid Responses message roles must be rejected before transport"]
async fn audit_invalid_responses_message_roles_are_rejected_before_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    let message_result = client.stream_response(request("moderator", false)).await;
    let input_message_result = client.stream_response(request("moderator", true)).await;
    let requests = transport.requests.lock().unwrap();
    eprintln!(
        "Responses role audit: message_error={} input_message_error={} transport_calls={} wire_roles={:?}",
        message_result.is_err(),
        input_message_result.is_err(),
        requests.len(),
        requests
            .iter()
            .map(|body| body["input"][0]["role"].clone())
            .collect::<Vec<_>>(),
    );

    assert!(
        message_result.is_err() && input_message_result.is_err(),
        "unrecognized Responses message roles must be rejected at the provider boundary"
    );
    assert_eq!(
        requests.len(),
        0,
        "invalid roles must not be sent to the provider transport"
    );
}

#[tokio::test]
#[ignore = "audit control: documented Responses message roles remain accepted"]
async fn audit_documented_responses_message_roles_reach_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    for role in ["user", "assistant", "system", "developer"] {
        client.stream_response(request(role, false)).await.unwrap();
    }
    let requests = transport.requests.lock().unwrap();
    let roles: Vec<&str> = requests
        .iter()
        .map(|body| body["input"][0]["role"].as_str().unwrap())
        .collect();
    eprintln!("Responses documented role control: {roles:?}");
    assert_eq!(roles, ["user", "assistant", "system", "developer"]);
}
