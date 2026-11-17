use deepagent_core::event::EventPayload;
use serde_json::json;

#[test]
fn legacy_model_items_keep_their_shape_without_origin_or_signature() {
    let legacy = json!({
        "type":"response_item_appended",
        "item":{"type":"reasoning","id":"legacy-reasoning","content":"legacy thought"}
    });
    let payload: EventPayload = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(payload).unwrap(), legacy);
}

#[test]
fn model_item_origin_and_thinking_signature_survive_event_round_trip() {
    let record = json!({
        "type":"response_item_appended",
        "item":{"type":"reasoning","content":"model thought","signature":"synthetic-signature"},
        "origin":{"provider_id":"alpha","model_name":"model-a","protocol":"messages"}
    });
    let payload: EventPayload = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(serde_json::to_value(payload).unwrap(), record);
}
