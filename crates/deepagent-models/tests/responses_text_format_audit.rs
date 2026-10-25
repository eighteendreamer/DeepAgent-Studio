//! Final-provider-seam audit for documented Responses text.format shapes.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseDefaults, ResponseRequest};
use serde_json::{json, Value};

#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert!(request.url.ends_with("/responses"));
        let wire: Value = serde_json::from_str(&request.body)?;
        let model = wire["model"].clone();
        self.requests.lock().unwrap().push(wire);
        let item = json!({"type":"message","id":"msg-audit","role":"assistant",
            "status":"completed","content":[{"type":"output_text","text":"{}",
                "annotations":[]}]});
        // Synthetic success only lets the production serializer/accumulator
        // complete. It does not pretend the real provider accepts bad formats.
        for event in [
            json!({"type":"response.output_item.done","sequence_number":0,
                "output_index":0,"item":item}),
            json!({"type":"response.completed","sequence_number":1,
                "response":{"id":"resp-audit","object":"response","created_at":0,
                    "status":"completed","model":model,"output":[item],"usage":{
                        "input_tokens":10,"output_tokens":2,"total_tokens":12,
                        "input_tokens_details":{"cached_tokens":0},
                        "output_tokens_details":{"reasoning_tokens":0}}}}),
        ] {
            if sink.on_event(&event.to_string())? {
                break;
            }
        }
        Ok(())
    }
}

async fn observe(text: Value, via_defaults: bool, cancelled_entry: bool) -> (bool, Vec<Value>) {
    let transport = Arc::new(RecordingTransport::default());
    let mut config = ModelConfig::deepseek("offline-text-format-audit");
    let mut request = ResponseRequest::new(
        "deepseek-flash",
        vec![Message::user(
            "Reply with JSON for this offline boundary audit.",
        )],
    );
    if via_defaults {
        config.defaults = ResponseDefaults {
            text: Some(text),
            ..Default::default()
        };
    } else {
        request = request.with_text(text);
    }
    let client = ModelClient::new(transport.clone(), config);
    let result = if cancelled_entry {
        client
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
            .await
    } else {
        client.stream_response(request).await
    };
    let requests = transport.requests.lock().unwrap().clone();
    (result.is_err(), requests)
}

#[tokio::test]
async fn supported_responses_text_formats_reach_transport_unchanged() {
    let formats = [
        json!({"format":{"type":"text"}}),
        json!({"format":{"type":"json_object"}}),
        json!({"format":{"type":"json_schema","name":"answer","schema":{
            "type":"object","properties":{},"additionalProperties":false}}}),
    ];
    let mut count = 0;
    for text in formats {
        for via_defaults in [false, true] {
            for cancelled_entry in [false, true] {
                let (rejected, requests) =
                    observe(text.clone(), via_defaults, cancelled_entry).await;
                assert!(!rejected);
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0]["text"], text);
                count += 1;
            }
        }
    }
    eprintln!("text.format control: valid_request_paths={count}");
}

#[tokio::test]
#[ignore = "audit reproducer: malformed text.format must fail at the final provider seam"]
async fn malformed_responses_text_formats_are_rejected_before_transport() {
    let malformed = [
        ("format-not-object", json!({"format":"json_object"})),
        ("unknown-type", json!({"format":{"type":"xml"}})),
        (
            "schema-name-missing",
            json!({"format":{"type":"json_schema","schema":{}}}),
        ),
        (
            "schema-missing",
            json!({"format":{"type":"json_schema","name":"answer"}}),
        ),
        (
            "schema-not-object",
            json!({"format":{"type":"json_schema","name":"answer","schema":[]}}),
        ),
    ];
    let mut violations = Vec::new();
    for (name, text) in malformed {
        for via_defaults in [false, true] {
            for cancelled_entry in [false, true] {
                let (rejected, requests) =
                    observe(text.clone(), via_defaults, cancelled_entry).await;
                if let Some(wire) = requests.first() {
                    assert_eq!(wire["text"], text, "probe must observe the exact bad input");
                }
                if !rejected || !requests.is_empty() {
                    violations.push(json!({"case":name,"defaults":via_defaults,
                        "cancelled_entry":cancelled_entry,"rejected":rejected,
                        "transport_calls":requests.len()}));
                }
            }
        }
    }
    eprintln!("text.format audit: violations={}", json!(violations));
    assert!(
        violations.is_empty(),
        "all malformed format sources/entrypoints must reject locally with transport_calls=0"
    );
}
