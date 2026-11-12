use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use deepagent_models::{
    transport::EventSink, ChatCompletionRequest, HttpTransport, ModelClient, ModelConfig,
    ResponseRequest, ToolSchema, TransportRequest,
};
use serde_json::{json, Value};

fn function(name: &str) -> ToolSchema {
    ToolSchema::function(name, "fixture", json!({"type":"object","properties":{}}))
}

fn request(choice: Value, tools: Vec<ToolSchema>) -> ResponseRequest {
    ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")])
        .with_tools(tools)
        .with_tool_choice(choice)
}

fn invalid_choices() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(1),
        json!([]),
        json!("invalid-mode"),
        json!({}),
        json!({"type":"custom","name":"apply_patch"}),
        json!({"type":"web_search"}),
        json!({"type":"function"}),
        json!({"type":"function","name":42}),
        json!({"type":"function","name":""}),
        json!({"type":"function","name":"private/value"}),
        json!({"type":"function","name":"x".repeat(129)}),
        json!({"type":"function","name":"read_file","extra":"private-value"}),
        json!({"type":"function","function":{"name":"read_file"}}),
    ]
}

#[test]
fn responses_choice_shape_and_final_reference_gate_validation_and_direct_serde() {
    for choice in invalid_choices() {
        let request = request(choice, vec![function("read_file")]);
        assert!(request.validate().is_err());
        assert!(serde_json::to_value(request).is_err());
    }
    for tools in [
        vec![],
        vec![function("different")],
        {
            let mut custom = function("read_file");
            custom.kind = "custom".into();
            vec![custom]
        },
        vec![function("read_file"), function("read_file")],
    ] {
        let request = request(json!({"type":"function","name":"read_file"}), tools);
        assert!(request.validate().is_err());
        assert!(serde_json::to_value(request).is_err());
    }
    for choice in [
        json!("auto"),
        json!("none"),
        json!("required"),
        json!({"type":"function","name":"read_file"}),
    ] {
        let request = request(choice.clone(), vec![function("read_file")]);
        request.validate().unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["tool_choice"],
            choice
        );
    }
    let request = ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")]);
    request.validate().unwrap();
    assert!(serde_json::to_value(request)
        .unwrap()
        .get("tool_choice")
        .is_none());
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        request: TransportRequest,
        _: &mut dyn EventSink,
    ) -> deepagent_core::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("synthetic-dispatched"))
    }
}

#[tokio::test]
async fn responses_choices_are_validated_after_defaults_and_tool_projection_on_both_entries() {
    for defaults in [false, true] {
        for cancel_entry in [false, true] {
            let mut cases: Vec<_> = invalid_choices()
                .into_iter()
                .map(|choice| (choice, vec![function("read_file")], false))
                .collect();
            cases.extend([
                (
                    json!({"type":"function","name":"missing"}),
                    vec![function("read_file")],
                    false,
                ),
                (
                    json!({"type":"function","name":"apply_patch"}),
                    vec![function("apply_patch")],
                    false,
                ),
                (
                    json!({"type":"function","name":"read_file"}),
                    vec![function("read_file")],
                    true,
                ),
                (json!("auto"), vec![function("apply_patch")], true),
                (json!("none"), vec![], true),
                (json!("required"), vec![function("read_file")], true),
            ]);
            for (choice, tools, valid) in cases {
                let transport = Arc::new(Recorder::default());
                let mut config = ModelConfig::deepseek("synthetic");
                let mut request = request(choice.clone(), tools);
                if defaults {
                    request.tool_choice = None;
                    config.defaults.tool_choice = Some(choice.clone());
                }
                let client = ModelClient::new(transport.clone(), config);
                let result = if cancel_entry {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                let error = result.unwrap_err();
                let wire = transport.0.lock().unwrap();
                if valid {
                    assert_eq!(error.to_string(), "synthetic-dispatched");
                    assert_eq!(wire.len(), 1);
                    assert_eq!(wire[0]["tool_choice"], choice);
                } else {
                    assert!(matches!(error, CoreError::Invalid(_)));
                    assert!(!error.to_string().contains("private"));
                    assert!(wire.is_empty());
                }
            }
        }
    }
}

#[tokio::test]
async fn explicit_choice_overrides_invalid_default_and_chat_keeps_its_own_wire_shape() {
    let transport = Arc::new(Recorder::default());
    let mut config = ModelConfig::deepseek("synthetic");
    config.defaults.tool_choice = Some(json!({"type":"custom","name":"apply_patch"}));
    let client = ModelClient::new(transport.clone(), config);
    let result = client.stream_response(request(json!("auto"), vec![])).await;
    assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
    assert_eq!(transport.0.lock().unwrap()[0]["tool_choice"], "auto");

    let choice = json!({"type":"function","function":{"name":"read_file"}});
    let chat = ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
        .with_tools(vec![function("read_file")])
        .with_tool_choice(choice.clone());
    assert_eq!(serde_json::to_value(chat).unwrap()["tool_choice"], choice);
}
