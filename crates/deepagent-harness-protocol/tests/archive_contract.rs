use deepagent_harness_protocol::{validate_control_result, HarnessRequest};
use serde_json::json;

#[test]
fn h215_explicit_unarchive_has_an_identity_checked_false_receipt() {
    let request: HarnessRequest = serde_json::from_value(json!({
        "method":"thread/unarchive", "params":{"threadId":"synthetic-thread"}
    }))
    .expect("explicit restore is a supported protocol operation");
    validate_control_result(
        &request,
        &json!({"threadId":"synthetic-thread","archived":false}),
    )
    .unwrap();
    for result in [
        json!({}),
        json!({"threadId":"other","archived":false}),
        json!({"threadId":"synthetic-thread","archived":true}),
        json!({"threadId":"synthetic-thread","archived":null}),
    ] {
        assert!(validate_control_result(&request, &result).is_err());
    }
}

#[test]
fn h215_list_visibility_is_an_explicit_typed_request_with_active_default() {
    let active: HarnessRequest =
        serde_json::from_value(json!({"method":"thread/list","params":{}})).unwrap();
    assert_eq!(
        serde_json::to_value(active).unwrap(),
        json!({"method":"thread/list","params":{}})
    );
    let archived: HarnessRequest =
        serde_json::from_value(json!({"method":"thread/list","params":{"archived":true}})).unwrap();
    assert_eq!(
        serde_json::to_value(archived).unwrap(),
        json!({"method":"thread/list","params":{"archived":true}})
    );
    assert!(serde_json::from_value::<HarnessRequest>(
        json!({"method":"thread/list","params":{"archived":"true"}})
    )
    .is_err());
}

#[test]
fn h215_explicit_list_views_reject_missing_or_conflicting_projection_markers() {
    for archived in [true, false] {
        let request: HarnessRequest =
            serde_json::from_value(json!({"method":"thread/list","params":{"archived":archived}}))
                .unwrap();
        validate_control_result(
            &request,
            &json!({"threads":[{"threadId":"synthetic","archived":archived}]}),
        )
        .unwrap();
        for result in [
            json!({}),
            json!({"threads":[{"threadId":"synthetic"}]}),
            json!({"threads":[{"threadId":"synthetic","archived":!archived}]}),
        ] {
            assert!(validate_control_result(&request, &result).is_err());
        }
    }
}
