//! A forced function is a single-function contract on both OpenAI wires.
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{message::Message, Result};
use deepagent_models::{
    transport::EventSink, HttpTransport, ModelClient, ModelConfig, ReasoningEffort,
    ResponseRequest, ToolSchema, TransportRequest, WireMode,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        if request.url.ends_with("/responses") {
            sink.on_event(
                &json!({"type":"response.completed","sequence_number":0,"response":{
                    "id":"fixture-response","object":"response","created_at":1,
                    "status":"completed","model":"fixture","output":[],
                    "usage":{"input_tokens":2,"output_tokens":0,"total_tokens":2}
                }})
                .to_string(),
            )?;
        } else {
            sink.on_event(r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#)?;
            sink.on_event("[DONE]")?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn forced_function_disables_parallel_calls_without_changing_choice_or_tools() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for default_choice in [false, true] {
            for cancelled_entry in [false, true] {
                for choice in [
                    json!({"type":"function","name":"read_file"}),
                    json!("auto"),
                    json!("none"),
                    json!("required"),
                ] {
                    let transport = Arc::new(Recorder::default());
                    let mut config = ModelConfig::deepseek("synthetic").with_wire_mode(mode);
                    let mut request =
                        ResponseRequest::new("fixture", vec![Message::user("fixture")])
                            .with_tools(
                                ["read_file", "other_file"]
                                    .map(|name| {
                                        ToolSchema::function(
                                            name,
                                            "fixture",
                                            json!({"type":"object"}),
                                        )
                                    })
                                    .to_vec(),
                            )
                            .with_reasoning_effort(ReasoningEffort::None);
                    if default_choice {
                        config.defaults.tool_choice = Some(choice.clone());
                    } else {
                        request.tool_choice = Some(choice.clone());
                    }
                    let client = ModelClient::new(transport.clone(), config);
                    if cancelled_entry {
                        client
                            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                            .await
                            .unwrap();
                    } else {
                        client.stream_response(request).await.unwrap();
                    }
                    let requests = transport.0.lock().unwrap();
                    assert_eq!(requests.len(), 1, "encoding compatibility must not retry");
                    let wire = &requests[0];
                    let expected_choice = if choice.is_object() && mode == WireMode::ChatCompletions
                    {
                        json!({"type":"function","function":{"name":"read_file"}})
                    } else {
                        choice.clone()
                    };
                    assert_eq!(wire["tool_choice"], expected_choice);
                    assert_eq!(wire["tools"].as_array().unwrap().len(), 2);
                    assert_eq!(
                        wire.get("parallel_tool_calls"),
                        choice.is_object().then_some(&Value::Bool(false))
                    );
                }
            }
        }
    }
}
