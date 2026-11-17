//! Compatibility contract for `ResponseItem::Reasoning.signature` (plan §4.1).
//!
//! Session JSONL written before the field existed must still load, and records
//! without a signature must not gain a new key on disk.

use deepagent_core::response_item::ResponseItem;

#[test]
fn legacy_reasoning_record_without_signature_still_deserializes() {
    let legacy = r#"{"type":"reasoning","id":"rs_1","content":"think"}"#;

    let item: ResponseItem = serde_json::from_str(legacy).expect("legacy record must load");

    assert_eq!(
        item,
        ResponseItem::Reasoning {
            id: Some("rs_1".into()),
            content: "think".into(),
            signature: None,
        }
    );
}

#[test]
fn signature_round_trips_when_present() {
    let item = ResponseItem::Reasoning {
        id: None,
        content: "think".into(),
        signature: Some("sig-abc".into()),
    };

    let json = serde_json::to_value(&item).unwrap();
    assert_eq!(json["signature"], "sig-abc");

    let back: ResponseItem = serde_json::from_value(json).unwrap();
    assert_eq!(back, item);
}

#[test]
fn absent_signature_is_not_written_to_disk() {
    let item = ResponseItem::Reasoning {
        id: Some("rs_2".into()),
        content: "think".into(),
        signature: None,
    };

    let json = serde_json::to_value(&item).unwrap();

    assert!(json.get("signature").is_none(), "unexpected key in {json}");
}
