//! Native discovery compatibility, distinct from the existing function tool.
//! Item vocabulary follows the inspected Codex protocol reference; this is not
//! a claim that DeepSeek's undocumented native request declaration is enabled.
use super::*;
use crate::ResponseItem;
use deepagent_core::message::Message;
use serde_json::{json, Value};

fn call(execution: &str, call_id: Value) -> Value {
    json!({"type":"tool_search_call", "id":"tsc-h152", "call_id":call_id,
        "execution":execution, "status":"completed",
        "arguments":{"query":"calendar create", "max_results":1}})
}

async fn response(items: Vec<Value>, added: bool) -> Result<crate::Response> {
    let mut events = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if added {
            let mut start = item.clone();
            start["status"] = json!("in_progress");
            events.push(
                json!({"type":"response.output_item.added",
                "output_index":index, "item":start})
                .to_string(),
            );
        }
        events.push(
            json!({"type":"response.output_item.done",
            "output_index":index, "item":item})
            .to_string(),
        );
    }
    events.push(
        json!({"type":"response.completed", "response":{
            "id":"h152-response", "object":"response", "created_at":1,
            "model":"deepseek-flash", "status":"completed", "output":items,
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }})
        .to_string(),
    );
    ModelClient::new(
        Arc::new(crate::test_sequence::mock(events)),
        ModelConfig::deepseek("synthetic-h152"),
    )
    .stream_response(ResponseRequest::new(
        "deepseek-flash",
        vec![Message::user("Discover one tool")],
    ))
    .await
}

#[tokio::test]
async fn client_native_call_is_preserved_once_and_uses_existing_tool_search_invocation() {
    let item = call("client", json!("h152-call"));
    let result = response(vec![item.clone()], true).await.unwrap();
    assert_eq!(
        serde_json::to_value(&result.output_items).unwrap(),
        json!([item])
    );
    assert_eq!(
        result.tool_invocations_from_items(),
        vec![(
            "h152-call".into(),
            "tool_search".into(),
            json!({"query":"calendar create", "max_results":1}),
        )]
    );
}

#[tokio::test]
async fn server_native_call_and_catalog_are_preserved_without_client_redispatch() {
    let items = vec![
        call("server", Value::Null),
        json!({
            "type":"tool_search_output", "id":"tso-h152", "call_id":null,
            "execution":"server", "status":"completed", "tools":[{
                "type":"function", "name":"calendar_create", "description":"Synthetic tool",
                "parameters":{"type":"object", "properties":{}}
            }]
        }),
    ];
    let result = response(items.clone(), true).await.unwrap();
    assert_eq!(
        serde_json::to_value(&result.output_items).unwrap(),
        json!(items)
    );
    assert!(result.tool_invocations_from_items().is_empty());
}

#[test]
fn native_call_and_catalog_round_trip_through_shared_surface_and_request_boundary() {
    let values = vec![
        call("client", json!("h152-call")),
        json!({
            "type":"tool_search_output", "id":"tso-h152", "call_id":"h152-call",
            "execution":"client", "status":"completed", "tools":[{
                "type":"function", "name":"calendar_create",
                "description":"Synthetic tool", "parameters":{"type":"object"}
            }]
        }),
    ];
    let items: Vec<ResponseItem> = serde_json::from_value(json!(values)).unwrap();
    assert_eq!(serde_json::to_value(&items).unwrap(), json!(values));
    let request = ResponseRequest::from_response_items("deepseek-flash", None, items);
    let wire = serde_json::to_value(request).unwrap();
    assert_eq!(wire["input"], json!(values));
}

#[test]
fn native_pairs_use_existing_repair_and_reject_ambiguous_cross_kind_identities() {
    let call: ResponseItem = serde_json::from_value(call("client", json!("search"))).unwrap();
    let repaired = crate::responses::repair_tool_call_pairs(std::slice::from_ref(&call));
    assert!(
        matches!(&repaired[1], ResponseItem::ToolSearchOutput { call_id: Some(id), status, tools, .. }
        if id == "search" && status == "failed" && tools.is_empty())
    );
    let orphan = crate::responses::repair_tool_call_pairs(&repaired[1..]);
    assert!(
        matches!(&orphan[0], ResponseItem::ToolSearchCall { call_id: Some(id), .. } if id == "search")
    );
    assert_eq!(orphan[1], repaired[1]);
    let mut interleaved = vec![
        call.clone(),
        ResponseItem::Message {
            role: "user".into(),
            content: "hook context".into(),
        },
        repaired[1].clone(),
    ];
    let ordered = crate::responses::repair_tool_call_pairs(&interleaved);
    assert_eq!(ordered[0], call);
    assert_eq!(ordered[1], repaired[1]);
    interleaved.push(call.clone());
    assert!(crate::responses::validate_tool_call_ids(&interleaved).is_err());
    for incompatible in [
        ResponseItem::FunctionCall {
            call_id: "search".into(),
            name: "tool_search".into(),
            arguments: "{}".into(),
        },
        ResponseItem::FunctionCallOutput {
            call_id: "search".into(),
            output: "wrong kind".into(),
        },
    ] {
        assert!(
            crate::responses::normalize_tool_output_kinds(&[call.clone(), incompatible]).is_err()
        );
    }
    let server = serde_json::from_value::<ResponseItem>(call_value_server()).unwrap();
    assert_eq!(
        crate::responses::repair_tool_call_pairs(std::slice::from_ref(&server)),
        [server]
    );
}

fn call_value_server() -> Value {
    call("server", Value::Null)
}

#[tokio::test]
async fn malformed_native_client_identity_never_becomes_successful_empty_output() {
    for invalid in [Value::Null, json!(""), json!(12)] {
        let result = response(vec![call("client", invalid)], false).await;
        assert!(
            result.is_err(),
            "client discovery requires a usable call identity"
        );
    }
}

#[tokio::test]
async fn incomplete_snapshot_cannot_dispatch_a_native_call_without_an_item_done() {
    for explicit_done in [false, true] {
        let item = call("client", json!("native-truncated"));
        let mut events = Vec::new();
        if explicit_done {
            events.push(
                json!({"type":"response.output_item.done", "output_index":0, "item":item})
                    .to_string(),
            );
        }
        events.push(
            json!({"type":"response.incomplete", "response":{
                "id":"h152-incomplete", "object":"response", "created_at":1,
                "model":"deepseek-flash", "status":"incomplete", "output":[item],
                "incomplete_details":{"reason":"max_output_tokens"}
            }})
            .to_string(),
        );
        let response = ModelClient::new(
            Arc::new(crate::test_sequence::mock(events)),
            ModelConfig::deepseek("synthetic-h152"),
        )
        .stream_response(ResponseRequest::new(
            "deepseek-flash",
            vec![Message::user("discover")],
        ))
        .await
        .unwrap();
        assert_eq!(
            response.tool_invocations_from_items().len(),
            usize::from(explicit_done),
            "only an explicit completed item may dispatch after a truncated terminal"
        );
    }
}

#[tokio::test]
async fn ordinary_function_tool_search_keeps_its_original_protocol_kind() {
    let item = json!({"type":"function_call", "id":"fc-h152", "call_id":"h152-function",
        "name":"tool_search", "arguments":"{\"query\":\"calendar\",\"max_results\":1}"});
    let result = response(vec![item], true).await.unwrap();
    assert_eq!(
        serde_json::to_value(&result.output_items).unwrap()[0]["type"],
        "function_call"
    );
    assert_eq!(result.tool_invocations_from_items()[0].1, "tool_search");
}
