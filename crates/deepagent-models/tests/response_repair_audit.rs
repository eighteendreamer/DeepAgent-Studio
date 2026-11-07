//! Audit that every public Responses history mutation keeps tool-call pairs valid.

#[path = "response_repair_audit/replacement.rs"]
mod replacement;
#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::{Message, ToolCall};
use deepagent_core::response_item::ResponseInputItem;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest};

#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn EventSink = &mut sequence_fixture::sink(sink);
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let events = if request.url.ends_with("/responses") {
            vec![
                r#"{"type":"response.output_text.delta","delta":"offline"}"#,
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
            ]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"offline"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        for event in events {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn dangling_tool_messages() -> Vec<Message> {
    vec![
        Message::user("Continue the interrupted operation."),
        Message::assistant("").with_tool_calls(vec![ToolCall {
            id: "audit-dangling-call".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "note.txt"}),
        }]),
    ]
}

#[tokio::test]
async fn audit_replace_messages_repairs_dangling_responses_calls_before_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    // `ResponseRequest::new` uses from_response_items and therefore supplies a
    // paired failure output. The public mutation method must preserve that
    // same provider-boundary invariant after replacing the message projection.
    let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("seed")]);
    request.replace_messages(&dangling_tool_messages());
    client.stream_response(request).await.unwrap();

    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "history repair must not redispatch a tool or model"
    );
    let input = requests[0]["input"].as_array().unwrap();
    let has_call = input
        .iter()
        .any(|item| item["type"] == "function_call" && item["call_id"] == "audit-dangling-call");
    let has_output = input.iter().any(|item| {
        item["type"] == "function_call_output" && item["call_id"] == "audit-dangling-call"
    });
    eprintln!(
        "Responses replace_messages repair audit: has_call={has_call} has_output={has_output} input={input:?}"
    );
    assert!(
        has_call && has_output,
        "dangling calls must be repaired before transport"
    );
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect();
    assert_eq!(outputs.len(), 1);
    let failure: serde_json::Value =
        serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    assert_eq!(failure["status"], "error");
}

#[test]
fn audit_late_tool_call_does_not_discard_real_earlier_output() {
    let items = vec![
        ResponseInputItem::FunctionCallOutput {
            call_id: "audit-late-call".into(),
            output: "REAL_TOOL_RESULT".into(),
        },
        ResponseInputItem::FunctionCall {
            call_id: "audit-late-call".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        },
    ];
    let request = ResponseRequest::from_response_items("deepseek-flash", None, items);
    let wire = serde_json::to_value(&request).unwrap();
    let input = wire["input"].as_array().unwrap();
    let output = input.iter().find(|item| {
        item["type"] == "function_call_output" && item["call_id"] == "audit-late-call"
    });
    eprintln!("late_tool_output_repair: input={input:?}");
    assert_eq!(
        output.and_then(|item| item["output"].as_str()),
        Some("REAL_TOOL_RESULT"),
        "repair must not replace a durable tool result with a synthetic failure"
    );
}
