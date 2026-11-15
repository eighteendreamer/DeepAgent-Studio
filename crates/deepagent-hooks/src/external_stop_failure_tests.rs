//! H-106: native structured errors never conflate a veto with provider failure.
use super::*;
use crate::{CompletionHook, StopFailureError};
use deepagent_core::id::SessionId;
use serde_json::json;

fn context(error: StopFailureError) -> HookContext {
    HookContext::new(
        SessionId::nil(),
        HookPoint::StopFailure,
        HookData::StopFailure {
            content: "legacy candidate and diagnostic".into(),
            error,
        },
    )
}

#[test]
fn h106_rejected_completion_keeps_its_real_gate_and_reason() {
    for (hook_point, label) in [
        (CompletionHook::BeforeResponse, "before_response"),
        (CompletionHook::Stop, "stop"),
    ] {
        let ctx = context(StopFailureError::HookRejection {
            hook_point,
            message: "revise this candidate".into(),
        });
        let payload = build_hook_payload(HookEvent::StopFailure, &ctx);
        assert_eq!(payload["error_format"], "deepagent.stop_failure.v1");
        assert_eq!(payload["content"], "legacy candidate and diagnostic");
        assert_eq!(
            payload["error"],
            json!({
                "origin":"hook_rejection", "hook_point":label, "message":"revise this candidate"
            })
        );
    }
}

#[test]
fn h106_provider_facts_are_preserved_without_inventing_missing_metadata() {
    for present in [false, true] {
        let ctx = context(StopFailureError::Provider {
            message: "synthetic provider detail".into(),
            code: present.then(|| "rate_limit_error".into()),
            status: present.then_some(429),
            retry_after_ms: present.then_some(1_234),
            request_id: present.then(|| "synthetic-request".into()),
        });
        let payload = build_hook_payload(HookEvent::StopFailure, &ctx);
        let mut expected = json!({"origin":"provider", "message":"synthetic provider detail"});
        if present {
            expected["code"] = json!("rate_limit_error");
            expected["status"] = json!(429);
            expected["retry_after_ms"] = json!(1_234);
            expected["request_id"] = json!("synthetic-request");
        }
        assert_eq!(payload["error"], expected);
    }
}

#[test]
fn h106_legacy_and_mismatched_inputs_cannot_invent_a_failure() {
    let legacy = HookContext::new(
        SessionId::nil(),
        HookPoint::StopFailure,
        HookData::Response {
            content: "unknown legacy text".into(),
        },
    );
    assert!(build_hook_payload(HookEvent::StopFailure, &legacy)
        .get("error")
        .is_none());
    let mut ctx = context(StopFailureError::HookRejection {
        hook_point: CompletionHook::Stop,
        message: "real veto".into(),
    });
    assert!(build_hook_payload(HookEvent::Stop, &ctx)
        .get("error")
        .is_none());
    ctx.point = HookPoint::Stop;
    assert!(build_hook_payload(HookEvent::StopFailure, &ctx)
        .get("error")
        .is_none());
}

#[tokio::test]
async fn h106_every_action_backend_uses_the_same_versioned_failure_projection() {
    let errors = [
        StopFailureError::HookRejection {
            hook_point: CompletionHook::Stop,
            message: "synthetic veto".into(),
        },
        StopFailureError::HookRejection {
            hook_point: CompletionHook::BeforeResponse,
            message: "synthetic veto".into(),
        },
        StopFailureError::Provider {
            message: "synthetic provider error".into(),
            code: Some("invalid_request_error".into()),
            status: Some(400),
            retry_after_ms: None,
            request_id: Some("synthetic-request".into()),
        },
    ];
    let cases = errors.into_iter().map(|error| {
        let expected_error = serde_json::to_value(&error).unwrap();
        (HookEvent::StopFailure,
         HookData::StopFailure { content: "legacy text".into(), error },
         json!({"content":"legacy text", "error_format":"deepagent.stop_failure.v1", "error":expected_error}))
    }).collect();
    super::external_lifecycle_tests::assert_payload_matrix(None, cases).await;
}
