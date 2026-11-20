//! Check the SDK boundary without dropping DeepSeek-specific wire fields.
use async_openai::types::chat as sdk;
use deepagent_core::message::Message;
use deepagent_models::{ChatCompletionRequest, ReasoningEffort, ToolSchema};
use serde_json::{json, Value};

#[test]
fn sampling_keeps_the_original_f32_wire_precision() {
    let request = ChatCompletionRequest::new("fixture", vec![Message::user("hello")])
        .with_temperature(0.2)
        .with_top_p(0.95);
    let wire: Value = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(wire["temperature"], json!(0.2));
    assert_eq!(wire["top_p"], json!(0.95));
}

#[test]
fn chat_standard_fields_round_trip_through_sdk() {
    let request = ChatCompletionRequest::new("fixture", vec![Message::user("hello")])
        .with_thinking(false)
        .with_tools(vec![ToolSchema::function(
            "read_file",
            "Read",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        )])
        .with_tool_choice(json!({"type":"function","function":{"name":"read_file"}}))
        .with_response_format(json!({"type":"json_object"}))
        .with_temperature(0.25)
        .with_top_p(0.75)
        .with_max_tokens(128);
    let wire = serde_json::to_value(&request).unwrap();
    let mut standard = wire.clone();
    assert_eq!(
        standard.as_object_mut().unwrap().remove("thinking"),
        Some(json!({"type":"disabled"}))
    );
    let typed: sdk::CreateChatCompletionRequest = serde_json::from_value(standard.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), standard);
    assert_eq!(wire["tool_choice"]["function"]["name"], "read_file");
    assert_eq!(wire["parallel_tool_calls"], false);
    assert_eq!(wire["messages"][0]["content"], "hello");
    assert_eq!(wire["response_format"]["type"], "json_object");
    assert_eq!(wire["max_tokens"], 128);
}

#[test]
fn chat_extensions_survive_sdk_projection() {
    let mut assistant = Message::assistant("answer");
    assistant.reasoning_content = Some("thinking".into());
    let mut request = ChatCompletionRequest::new(
        "fixture",
        vec![Message::user("hello"), assistant, Message::user("continue")],
    )
    .with_reasoning_effort(ReasoningEffort::Max);
    request.user = Some("fixture-user".into());
    let wire: Value = serde_json::to_value(request).unwrap();
    assert_eq!(wire["reasoning_effort"], "max");
    assert_eq!(wire["thinking"]["type"], "enabled");
    assert_eq!(wire["user_id"], "fixture-user");
    assert_eq!(wire["messages"][1]["reasoning_content"], "thinking");
    assert_eq!(wire["messages"][1]["content"], "answer");
    assert_eq!(wire["messages"][2]["content"], "continue");
}
