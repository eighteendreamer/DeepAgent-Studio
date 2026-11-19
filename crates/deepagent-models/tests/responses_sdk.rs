//! SDK migration must preserve the provider wire, especially non-OpenAI items.
use async_openai::types::responses as sdk;
use deepagent_core::{message::Message, response_item::ToolSearchExecution};
use deepagent_models::{ReasoningEffort, ResponseInputItem as Item, ResponseRequest, ToolSchema};
use serde_json::{json, Value};

#[test]
fn sampling_keeps_the_original_f32_wire_precision() {
    let mut request = ResponseRequest::new("fixture", vec![Message::user("hello")]);
    request.temperature = Some(0.2);
    request.top_p = Some(0.95);
    let wire: Value = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(wire["temperature"], json!(0.2));
    assert_eq!(wire["top_p"], json!(0.95));
}

#[test]
fn standard_request_round_trips_through_sdk_without_implicit_defaults() {
    let mut request = ResponseRequest::new("fixture", vec![Message::user("中文 🦀")])
        .with_tools(vec![ToolSchema::function(
            "read_file",
            "Read",
            json!({"type":"object"}),
        )])
        .with_reasoning_effort(ReasoningEffort::High);
    request.instructions = Some("instructions".into());
    request.temperature = Some(0.5);
    request.top_p = Some(0.9);
    request.top_logprobs = Some(3);
    request.max_output_tokens = Some(1024);
    request.tool_choice = Some(json!({"type":"function","name":"read_file"}));
    request.text = Some(json!({"format":{"type":"json_schema","name":"result",
        "schema":{"type":"object","properties":{"ok":{"type":"boolean"}}},"strict":true}}));
    let wire = serde_json::to_value(request).unwrap();
    let body: sdk::CreateResponse = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(body).unwrap(), wire);
    assert_eq!(wire["input"][0]["content"], "中文 🦀");
    for field in [
        "store",
        "background",
        "parallel_tool_calls",
        "stream_options",
        "safety_identifier",
    ] {
        assert!(wire.get(field).is_none(), "unexpected SDK default: {field}");
    }
}

#[test]
fn deepseek_extensions_keep_content_and_identity_instead_of_sdk_item_references() {
    let mut request = ResponseRequest::from_response_items(
        "fixture",
        None,
        vec![
            Item::Reasoning {
                id: Some("reasoning-1".into()),
                content: "retained reasoning".into(),
                signature: None,
            },
            Item::CustomToolCall {
                call_id: "patch-1".into(),
                name: "apply_patch".into(),
                input: "*** patch".into(),
            },
            Item::CustomToolCallOutput {
                call_id: "patch-1".into(),
                output: "applied".into(),
            },
            Item::WebSearchCall {
                id: "search-1".into(),
                status: "completed".into(),
                action: Some(json!({"type":"provider_search","extension":true})),
            },
            Item::ToolSearchCall {
                id: Some("discovery-1".into()),
                call_id: Some("discovery-call".into()),
                status: Some("completed".into()),
                execution: ToolSearchExecution::Client,
                arguments: json!({"query":"fixture"}),
            },
            Item::ToolSearchOutput {
                id: None,
                call_id: Some("discovery-call".into()),
                status: "completed".into(),
                execution: ToolSearchExecution::Client,
                tools: vec![json!({"type":"function","name":"read_file","extension":true})],
            },
            Item::Message {
                role: "user".into(),
                content: "continue".into(),
            },
        ],
    );
    request.user = Some("fixture-user".into());
    let mut tool = ToolSchema::function("apply_patch", "Apply patch", json!({"type":"object"}));
    tool.kind = "custom".into();
    request.tools = vec![tool];
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(
        value["input"][0],
        json!({"type":"reasoning","id":"reasoning-1",
        "content":[{"type":"reasoning_text","text":"retained reasoning"}]})
    );
    assert_eq!(
        value["input"][1],
        json!({"type":"custom_tool_call","call_id":"patch-1","name":"apply_patch","input":"*** patch"})
    );
    assert_eq!(value["input"][3]["action"]["extension"], true);
    assert_eq!(value["input"][4]["id"], "discovery-1");
    assert_eq!(value["input"][4]["arguments"]["query"], "fixture");
    assert_eq!(value["input"][5]["tools"][0]["extension"], true);
    assert_eq!(
        value["tools"],
        json!([{"type":"custom","name":"apply_patch"}])
    );
    assert_eq!(value["user"], "fixture-user");
    assert!(value.get("safety_identifier").is_none());
}

#[test]
fn function_pairs_keep_raw_arguments_and_sdk_output_shapes() {
    let arguments = "{\"path\":\"file.txt\", \"line\":1}";
    let request = ResponseRequest::from_response_items(
        "fixture",
        None,
        vec![
            Item::FunctionCall {
                call_id: "call-1".into(),
                name: "read_file".into(),
                arguments: arguments.into(),
            },
            Item::FunctionCallOutput {
                call_id: "call-1".into(),
                output: "contents".into(),
            },
            Item::Message {
                role: "user".into(),
                content: "continue".into(),
            },
        ],
    );
    let wire = serde_json::to_value(request).unwrap();
    assert_eq!(wire["input"][0]["arguments"], arguments);
    for item in wire["input"].as_array().unwrap().iter().take(2) {
        let typed: sdk::Item = serde_json::from_value(item.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), *item);
    }
    assert_eq!(wire["input"][1]["output"], Value::String("contents".into()));
}
