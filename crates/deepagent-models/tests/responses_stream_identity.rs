//! H-200: provider identity is diagnostic data, not the requested model or a run cursor.
use std::sync::{atomic::AtomicBool, Arc};

use deepagent_core::message::Message;
use deepagent_models::{
    DeltaObserver, ModelClient, ModelConfig, ModelStreamEvent, ResponseAccumulator, ResponseRequest,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Seen(Vec<Value>);
impl DeltaObserver for Seen {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(serde_json::to_value(event).unwrap());
    }
}
impl Seen {
    fn metadata(&self) -> Vec<&Value> {
        self.0
            .iter()
            .filter(|value| value["type"] == "response_stream_event")
            .collect()
    }
}

fn terminal(kind: &str, sequence: u64) -> Value {
    json!({"type":format!("response.{kind}"),"sequence_number":sequence,"response":{
        "id":"response-B","object":"response","created_at":1,"status":kind,
        "model":"server/model@2026-11-06","output":[],
        "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5},
        "incomplete_details":if kind=="incomplete" {json!({"reason":"max_output_tokens"})} else {Value::Null},
        "error":if kind=="failed" {json!({"code":"synthetic_failure","message":"controlled failure"})} else {Value::Null}
    }})
}

#[test]
fn h200_lifecycle_and_cross_item_events_keep_server_identity_and_sequence() {
    for kind in ["completed", "incomplete", "failed"] {
        let mut acc = ResponseAccumulator::new();
        let mut seen = Seen::default();
        for event in [
            json!({"type":"response.created","sequence_number":0,"response":{
                "id":"response-B","model":"server/model@2026-11-06",
                "instructions":"PRIVATE_BODY","metadata":{"secret":"PRIVATE_BODY"}}}),
            json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,
                "item":{"type":"reasoning","id":"reasoning-1","content":[]}}),
            json!({"type":"response.reasoning_text.delta","sequence_number":2,
                "item_id":"reasoning-1","delta":"PRIVATE_REASONING"}),
            json!({"type":"response.output_item.added","sequence_number":3,"output_index":1,
                "item":{"type":"message","role":"assistant","id":"message-1","content":[]}}),
            json!({"type":"response.output_text.delta","sequence_number":6,
                "output_index":1,"item_id":"message-1","delta":"normal text"}),
        ] {
            acc.push_sse_data_observed(&event.to_string(), &mut seen)
                .unwrap();
        }
        let result = acc.push_sse_data_observed(&terminal(kind, 7).to_string(), &mut seen);
        assert_eq!(result.is_err(), kind == "failed");
        let metadata = seen.metadata();
        assert_eq!(metadata.len(), 6);
        assert!(metadata
            .iter()
            .all(|event| event["response_id"] == "response-B"
                && event["provider_model"] == "server/model@2026-11-06"));
        assert_eq!(metadata[1]["item_id"], "reasoning-1");
        assert_eq!(metadata[3]["item_id"], "message-1");
        assert_eq!(metadata[4]["sequence_gap"], 2);
        assert_eq!(metadata[5]["sequence_number"], 7);
        assert!(!serde_json::to_string(&metadata)
            .unwrap()
            .contains("PRIVATE"));
        if kind != "failed" {
            let response = acc.finish().unwrap();
            assert_eq!(response.output_text_projection(), "normal text");
            assert_eq!(response.usage.unwrap().total_tokens, 5);
        }
    }
}

#[test]
fn h200_optional_snapshots_do_not_invent_identity_or_copy_unrelated_response_fields() {
    let mut acc = ResponseAccumulator::new();
    let mut seen = Seen::default();
    for event in [
        json!({"type":"response.created","sequence_number":0}),
        json!({"type":"response.in_progress","sequence_number":1,
            "response":{"id":"response-B"}}),
        json!({"type":"response.future_event","sequence_number":2,
            "response":{"id":"PRIVATE_EXTENSION","model":"PRIVATE_EXTENSION"}}),
        json!({"type":"response.in_progress","sequence_number":3,
            "response":{"model":"server/model@2026-11-06"}}),
        terminal("completed", 4),
    ] {
        acc.push_sse_data_observed(&event.to_string(), &mut seen)
            .unwrap();
    }
    let events = seen.metadata();
    assert!(events[0].get("response_id").is_none());
    assert!(events[0].get("provider_model").is_none());
    assert_eq!(events[1]["response_id"], "response-B");
    assert!(events[1].get("provider_model").is_none());
    assert_eq!(events[2]["response_id"], "response-B");
    assert_eq!(events[3]["provider_model"], "server/model@2026-11-06");
    assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
    assert!(acc.finish().is_ok());
}

#[test]
fn h200_diagnostic_identity_is_bounded_and_unsafe_values_clear_stale_snapshots() {
    for unsafe_value in [
        Value::Null,
        json!(2),
        json!({"private":"PRIVATE_BODY"}),
        json!("PRIVATE TEXT\r\n"),
        json!("PRIVATE=TEXT"),
        json!("sk-PRIVATE"),
        json!("私密内容"),
        json!("X".repeat(257)),
    ] {
        let mut acc = ResponseAccumulator::new();
        let mut seen = Seen::default();
        for event in [
            json!({"type":"response.created","sequence_number":0,
                "response":{"id":"old-response","model":"old-model"}}),
            json!({"type":"response.in_progress","sequence_number":1,
                "response":{"id":unsafe_value,"model":unsafe_value}}),
            json!({"type":"response.output_text.delta","sequence_number":2,"delta":"normal"}),
            terminal("completed", 3),
        ] {
            acc.push_sse_data_observed(&event.to_string(), &mut seen)
                .unwrap();
        }
        let events = seen.metadata();
        for event in &events[1..3] {
            assert!(event.get("response_id").is_none(), "{event}");
            assert!(event.get("provider_model").is_none(), "{event}");
        }
        assert_eq!(events[3]["response_id"], "response-B");
        assert_eq!(acc.finish().unwrap().output_text_projection(), "normal");
    }
}

#[tokio::test]
async fn h200_each_physical_request_starts_without_previous_identity_in_both_entries() {
    for cancel_aware in [false, true] {
        let client = ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new([
                json!({"type":"response.output_text.delta","sequence_number":0,"delta":"normal"})
                    .to_string(),
                terminal("completed", 1).to_string(),
            ])),
            ModelConfig::deepseek("synthetic"),
        );
        for _ in 0..2 {
            let mut seen = Seen::default();
            let request = ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]);
            let response = if cancel_aware {
                client
                    .stream_response_observed_cancelled(
                        request,
                        &mut seen,
                        Arc::new(AtomicBool::new(false)),
                    )
                    .await
            } else {
                client.stream_response_observed(request, &mut seen).await
            }
            .unwrap();
            let metadata = seen.metadata();
            assert!(metadata[0].get("response_id").is_none());
            assert!(metadata[0].get("provider_model").is_none());
            assert_eq!(metadata[1]["response_id"], "response-B");
            assert_eq!(metadata[1]["provider_model"], "server/model@2026-11-06");
            assert_eq!(response.output_text_projection(), "normal");
        }
    }
}

#[test]
fn h200_old_model_metadata_json_remains_readable() {
    let value = json!({"type":"response_stream_event","event_type":"response.created",
        "item_id":null,"item_type":null,"delta_chars":null});
    let event: ModelStreamEvent = serde_json::from_value(value).unwrap();
    let value = serde_json::to_value(event).unwrap();
    assert!(value.get("response_id").is_none());
    assert!(value.get("provider_model").is_none());
}

#[test]
fn h200_new_identity_cannot_inherit_a_previous_model_and_new_created_resets_snapshot() {
    let mut acc = ResponseAccumulator::new();
    let mut seen = Seen::default();
    for event in [
        json!({"type":"response.created","sequence_number":0,
            "response":{"id":"old-response","model":"old-model"}}),
        json!({"type":"response.in_progress","sequence_number":1,
            "response":{"id":"new-response"}}),
        json!({"type":"response.created","sequence_number":2}),
        terminal("completed", 3),
    ] {
        acc.push_sse_data_observed(&event.to_string(), &mut seen)
            .unwrap();
    }
    let events = seen.metadata();
    assert_eq!(events[1]["response_id"], "new-response");
    assert!(events[1].get("provider_model").is_none());
    assert!(events[2].get("response_id").is_none());
    assert!(events[2].get("provider_model").is_none());
    assert_eq!(events[3]["response_id"], "response-B");
    assert!(acc.finish().is_ok());
}

#[tokio::test]
async fn h200_chat_completions_does_not_acquire_responses_identity_fields() {
    let client = ModelClient::new(
        Arc::new(deepagent_models::MockTransport::new([
            json!({"id":"chat-id","model":"chat-server-model",
                "choices":[{"delta":{"content":"normal"},"finish_reason":"stop"}]})
            .to_string(),
            "[DONE]".into(),
        ])),
        ModelConfig::deepseek_chat("synthetic"),
    );
    let mut seen = Seen::default();
    let response = client
        .stream_response_observed(
            ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic")]),
            &mut seen,
        )
        .await
        .unwrap();
    assert_eq!(response.output_text_projection(), "normal");
    assert_eq!(seen.metadata().len(), 1);
    assert_eq!(seen.metadata()[0]["item_id"], "chat-id");
    assert!(seen.metadata()[0].get("response_id").is_none());
    assert!(seen.metadata()[0].get("provider_model").is_none());
}

#[test]
fn h200_diagnostic_filter_does_not_reject_answers_or_weaken_terminal_validation() {
    for id in ["I".repeat(256), "I".repeat(257), "PRIVATE TEXT".into()] {
        let mut acc = ResponseAccumulator::new();
        let mut seen = Seen::default();
        let mut event = terminal("completed", 0);
        event["response"]["id"] = json!(id);
        acc.push_sse_data_observed(&event.to_string(), &mut seen)
            .unwrap();
        let metadata = seen.metadata();
        if id.len() == 256 {
            assert_eq!(metadata[0]["response_id"], id);
        } else {
            assert!(metadata[0].get("response_id").is_none());
        }
        assert!(acc.finish().is_ok());
    }
    let mut acc = ResponseAccumulator::new();
    let mut event = terminal("completed", 0);
    event["response"].as_object_mut().unwrap().remove("id");
    assert!(acc
        .push_sse_data_observed(&event.to_string(), &mut Seen::default())
        .is_err());
    assert!(acc.finish().is_err());
}

#[test]
fn h200_rejected_terminal_cannot_publish_identity_before_envelope_validation() {
    for kind in ["completed", "incomplete", "failed"] {
        for field in ["id", "object", "created_at", "status", "model", "output"] {
            let mut acc = ResponseAccumulator::new();
            let mut seen = Seen::default();
            let mut event = terminal(kind, 0);
            event["response"]["id"] = json!("PRIVATE_UNACCEPTED_IDENTITY");
            event["response"].as_object_mut().unwrap().remove(field);
            assert!(acc
                .push_sse_data_observed(&event.to_string(), &mut seen)
                .is_err());
            assert!(
                seen.0.is_empty(),
                "rejected envelope projected: {:?}",
                seen.0
            );
            assert!(acc.finish().is_err());
        }
    }
}
