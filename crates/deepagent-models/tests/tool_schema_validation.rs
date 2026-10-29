//! H-159: direct requests cannot bypass the capability catalog's schema guard.
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{message::Message, Result};
use deepagent_models::transport::EventSink;
use deepagent_models::{
    ChatCompletionRequest, HttpTransport, ModelClient, ModelConfig, ResponseRequest, ToolSchema,
    TransportRequest, WireMode,
};
use serde_json::{json, Value};

#[derive(Default)]
struct RecordingTransport(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        if request.url.ends_with("/chat/completions") {
            sink.on_event(
                r#"{"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            )?;
            sink.on_event("[DONE]")?;
        } else {
            sink.on_event(r#"{"type":"response.completed","response":{"status":"completed"}}"#)?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn invalid_function_schemas_never_reach_either_provider_route() {
    for wire_mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancelled_entry in [false, true] {
            for parameters in [
                Value::Null,
                json!("not-a-schema"),
                json!([]),
                json!(true),
                json!({"type":"invalid-type"}),
                json!({"required":"not-an-array"}),
            ] {
                let transport = Arc::new(RecordingTransport::default());
                let client = ModelClient::new(
                    transport.clone(),
                    ModelConfig::deepseek("fixture-placeholder").with_wire_mode(wire_mode),
                );
                let request = ResponseRequest::new("deepseek-flash", vec![Message::user("hi")])
                    .with_tools(vec![ToolSchema::function("bad", "bad", parameters.clone())]);
                let result = if cancelled_entry {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                assert!(
                    result.is_err(),
                    "{wire_mode:?}/{cancelled_entry}: {parameters}"
                );
                assert!(transport.0.lock().unwrap().is_empty());

                // The public native Chat entrypoint has the same final guard.
                let request =
                    ChatCompletionRequest::new("deepseek-flash", vec![Message::user("hi")])
                        .with_tools(vec![ToolSchema::function("bad", "bad", parameters)]);
                let result = if cancelled_entry {
                    client
                        .stream_chat_completion_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_chat_completion(request).await
                };
                assert!(result.is_err());
                assert!(transport.0.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn complex_function_schema_is_preserved_on_both_wires() {
    let schema = json!({
        "type":"object",
        "definitions":{"entry":{"oneOf":[{"type":"string"},{"type":"integer"}]}},
        "properties":{"entries":{"type":"array","items":{"$ref":"#/definitions/entry"}}},
        "required":["entries"], "additionalProperties":false
    });
    for wire_mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancelled_entry in [false, true] {
            let transport = Arc::new(RecordingTransport::default());
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("fixture-placeholder").with_wire_mode(wire_mode),
            );
            let request =
                ResponseRequest::new("deepseek-flash", vec![Message::user("hi")]).with_tools(vec![
                    ToolSchema::function("healthy", "valid complex tool", schema.clone()),
                ]);
            if cancelled_entry {
                client
                    .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                    .await
                    .unwrap();
            } else {
                client.stream_response(request).await.unwrap();
            }
            let requests = transport.0.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let parameters = match wire_mode {
                WireMode::Responses => &requests[0]["tools"][0]["parameters"],
                WireMode::ChatCompletions => &requests[0]["tools"][0]["function"]["parameters"],
            };
            assert_eq!(parameters, &schema);
        }
    }
}
