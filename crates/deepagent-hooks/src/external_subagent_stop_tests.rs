//! H-109: SubagentStop uses an event-specific, schema-complete payload.
use super::*;
use crate::{HookData, HookPoint};
use deepagent_core::id::SessionId;
use serde_json::json;

fn stop_data() -> HookData {
    HookData::SubagentStop {
        agent_id: "sub_h109".into(),
        agent_type: "review".into(),
        summary: Some("finished".into()),
        stop_hook_active: false,
        agent_transcript_path: r"G:\synthetic\sub_h109.json".into(),
    }
}

#[test]
fn h109_stop_payload_contains_required_fields_without_changing_start() {
    let stop = HookContext::new(SessionId::nil(), HookPoint::SubagentStop, stop_data());
    assert_eq!(
        build_hook_payload(HookEvent::SubagentStop, &stop),
        json!({
            "session_id": SessionId::nil().to_string(),
            "hook_event_name": "SubagentStop",
            "agent_id": "sub_h109",
            "agent_type": "review",
            "last_assistant_message": "finished",
            "stop_hook_active": false,
            "agent_transcript_path": r"G:\synthetic\sub_h109.json",
        })
    );

    let start = HookContext::new(
        SessionId::nil(),
        HookPoint::SubagentStart,
        HookData::Subagent {
            agent_id: "sub_h109".into(),
            agent_type: "review".into(),
            summary: None,
        },
    );
    let start_payload = build_hook_payload(HookEvent::SubagentStart, &start);
    assert_eq!(start_payload["agent_id"], "sub_h109");
    assert!(start_payload.get("stop_hook_active").is_none());
    assert!(start_payload.get("agent_transcript_path").is_none());
}

#[test]
fn h109_mismatched_stop_data_cannot_invent_fields() {
    let context = HookContext::new(SessionId::nil(), HookPoint::SubagentStart, stop_data());
    let payload = build_hook_payload(HookEvent::SubagentStop, &context);
    assert!(payload.get("stop_hook_active").is_none());
    assert!(payload.get("agent_transcript_path").is_none());
}

#[tokio::test]
async fn h109_all_external_backends_receive_the_same_stop_projection() {
    super::external_lifecycle_tests::assert_payload_matrix(
        None,
        vec![(
            HookEvent::SubagentStop,
            stop_data(),
            json!({
                "agent_id": "sub_h109",
                "agent_type": "review",
                "last_assistant_message": "finished",
                "stop_hook_active": false,
                "agent_transcript_path": r"G:\synthetic\sub_h109.json",
            }),
        )],
    )
    .await;
}
