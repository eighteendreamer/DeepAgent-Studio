use deepagent_harness_protocol::{validate_control_result, HarnessRequest};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/control_results_v1.json")).unwrap()
}

#[test]
fn h207_typed_receipts_accept_extensions_but_reject_missing_and_wrong_typed_fields() {
    for case in cases() {
        let request: HarnessRequest =
            serde_json::from_value(json!({"method":case["method"],"params":case["params"]}))
                .unwrap();
        let mut result = case["result"].clone();
        result["future"] = json!({"opaque":"retained"});
        let before = result.clone();
        validate_control_result(&request, &result).unwrap();
        assert_eq!(result, before, "validator does not normalize raw metadata");
        for field in case["result"].as_object().unwrap().keys() {
            let mut bad = result.clone();
            bad.as_object_mut().unwrap().remove(field);
            let error = validate_control_result(&request, &bad).unwrap_err();
            assert_eq!(error.method, case["method"].as_str().unwrap());
            assert!(!error.to_string().contains("opaque"));
        }
        for bad in [Value::Null, json!([]), json!({}), json!("PRIVATE_PAYLOAD")] {
            let error = validate_control_result(&request, &bad).unwrap_err();
            assert!(!error.to_string().contains("PRIVATE_PAYLOAD"));
        }
    }
}

#[test]
fn h207_full_u64_ack_and_both_approval_decisions_are_exact_echoes_not_new_grants() {
    let ack: HarnessRequest =
        serde_json::from_value(json!({"method":"event/ack","params":{"eventSequence":u64::MAX}}))
            .unwrap();
    validate_control_result(
        &ack,
        &json!({"acknowledgedEventSequence":u64::MAX,"status":"accepted"}),
    )
    .unwrap();
    assert!(validate_control_result(
        &ack,
        &json!({"acknowledgedEventSequence":u64::MAX-1,"status":"accepted"})
    )
    .is_err());
    for approved in [true, false] {
        let request:HarnessRequest=serde_json::from_value(json!({"method":"approval/respond","params":{"approvalId":"synthetic","approved":approved}})).unwrap();
        validate_control_result(
            &request,
            &json!({"approvalId":"synthetic","approved":approved,"status":"resolved"}),
        )
        .unwrap();
        assert!(validate_control_result(
            &request,
            &json!({"approvalId":"synthetic","approved":!approved,"status":"resolved"})
        )
        .is_err());
    }
}
