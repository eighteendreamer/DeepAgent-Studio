//! Complete, sequenced offline Responses fixtures shared by hook audit modules.

use serde_json::{json, Value};

pub(super) fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","id":format!("item-{id}"),"status":"completed",
        "call_id":id,"name":name,"arguments":arguments.to_string()})
}

pub(super) fn text(id: &str, value: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":value,"annotations":[]}]})
}

pub(super) fn completed(id: &str, model: &str, item: Value, usage: Value) -> Vec<String> {
    let mut added = item.clone();
    added["status"] = json!("in_progress");
    if item["type"] == "function_call" {
        added["arguments"] = json!("");
    } else {
        added["content"] = json!([]);
    }
    let mut events = vec![
        json!({"type":"response.created","response":{"id":id,"object":"response",
            "created_at":1,"model":model,"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":added}),
    ];
    if item["type"] == "function_call" {
        events.extend([
            json!({"type":"response.function_call_arguments.delta","output_index":0,
                "item_id":item["id"],"delta":item["arguments"]}),
            json!({"type":"response.function_call_arguments.done","output_index":0,
                "item_id":item["id"],"arguments":item["arguments"]}),
        ]);
    }
    events.extend([
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":id,"object":"response",
            "created_at":1,"model":model,"status":"completed","output":[item],
            "usage":usage,"error":null,"incomplete_details":null}}),
    ]);
    events
        .into_iter()
        .enumerate()
        .map(|(sequence, mut event)| {
            event["sequence_number"] = sequence.into();
            event.to_string()
        })
        .collect()
}
