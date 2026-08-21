use deepagent_harness_protocol::{
    project_runtime_event, ApprovalRespondRequest, EventContext, HarnessEvent, HarnessRequest,
    InitializeRequest, ItemPayload, ThreadStartRequest, TurnStartRequest,
};
use deepagent_runtime::RuntimeEvent;

#[test]
fn thread_start_request_uses_stable_machine_field_names() {
    let request = HarnessRequest::ThreadStart(ThreadStartRequest {
        cwd: Some("G:/workspace".into()),
        provider: Some("deepseek-official".into()),
        model: Some("deepseek-v4-flash".into()),
        permission_profile: Some("workspace_write".into()),
        sandbox_backend: Some("windows_sandbox".into()),
    });

    let json = serde_json::to_value(request).unwrap();

    assert_eq!(json["method"], "thread/start");
    assert_eq!(json["params"]["cwd"], "G:/workspace");
    assert_eq!(json["params"]["provider"], "deepseek-official");
    assert_eq!(json["params"]["sandboxBackend"], "windows_sandbox");
}

#[test]
fn initialize_and_approval_requests_round_trip() {
    let initialize = HarnessRequest::Initialize(InitializeRequest {
        client_name: "deepagent-test".into(),
        client_version: "0.1.0".into(),
        protocol_version: 1,
    });
    let approval = HarnessRequest::ApprovalRespond(ApprovalRespondRequest {
        approval_id: "approval-1".into(),
        approved: true,
        scope: Some("turn".into()),
    });

    for request in [initialize, approval] {
        let json = serde_json::to_string(&request).unwrap();
        let decoded: HarnessRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }
}

#[test]
fn runtime_content_and_reasoning_project_to_item_updates() {
    let context = EventContext::new(Some("thread-1".into()), Some("turn-1".into()));

    let content = project_runtime_event(
        &RuntimeEvent::ContentDelta {
            text: "hello".into(),
        },
        &context,
    )
    .unwrap();
    let reasoning = project_runtime_event(
        &RuntimeEvent::ReasoningDelta {
            text: "think".into(),
        },
        &context,
    )
    .unwrap();

    assert!(matches!(
        content,
        HarnessEvent::ItemUpdated {
            item: ItemPayload::ContentDelta { text },
            ..
        } if text == "hello"
    ));
    assert!(matches!(
        reasoning,
        HarnessEvent::ItemUpdated {
            item: ItemPayload::ReasoningDelta { text },
            ..
        } if text == "think"
    ));
}

#[test]
fn content_event_matches_golden_json_shape() {
    let event = project_runtime_event(
        &RuntimeEvent::ContentDelta {
            text: "hello".into(),
        },
        &EventContext::new(Some("thread-1".into()), Some("turn-1".into())),
    )
    .unwrap();

    assert_eq!(
        serde_json::to_value(event).unwrap(),
        serde_json::json!({
            "type": "item.updated",
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "kind": "content_delta",
                "text": "hello"
            }
        })
    );
}

#[test]
fn runtime_tool_approval_and_terminal_events_keep_correlation() {
    let context = EventContext::new(Some("thread-1".into()), Some("turn-1".into()));
    let started = project_runtime_event(
        &RuntimeEvent::ToolStarted {
            name: "read_file".into(),
            call_id: "call-1".into(),
            arguments: serde_json::json!({"path": "README.md"}),
            tool_kind: Some("file_read".into()),
            file_path: Some("README.md".into()),
            summary: Some("read README".into()),
            meta: None,
        },
        &context,
    )
    .unwrap();
    let approval = project_runtime_event(
        &RuntimeEvent::ToolBlocked {
            name: "bash".into(),
            reason: "approval required".into(),
            needs_approval: true,
        },
        &context,
    )
    .unwrap();
    let terminal = project_runtime_event(&RuntimeEvent::RunCancelled, &context).unwrap();

    assert!(matches!(
        started,
        HarnessEvent::ItemStarted {
            item: ItemPayload::ToolCall { call_id, name, .. },
            ..
        } if call_id == "call-1" && name == "read_file"
    ));
    assert!(matches!(
        approval,
        HarnessEvent::ApprovalRequested {
            thread_id: Some(thread_id),
            turn_id: Some(turn_id),
            tool_name: Some(tool_name),
            ..
        } if thread_id == "thread-1" && turn_id == "turn-1" && tool_name == "bash"
    ));
    assert!(matches!(
        terminal,
        HarnessEvent::TurnInterrupted {
            thread_id: Some(thread_id),
            turn_id: Some(turn_id),
            ..
        } if thread_id == "thread-1" && turn_id == "turn-1"
    ));
}

#[test]
fn turn_start_request_serializes_provider_and_sandbox_overrides() {
    let request = HarnessRequest::TurnStart(TurnStartRequest {
        thread_id: "thread-1".into(),
        input: "inspect the repository".into(),
        provider: Some("deepseek-official".into()),
        model: Some("deepseek-v4-pro".into()),
        reasoning_effort: Some("high".into()),
        permission_profile: Some("read_only".into()),
        sandbox_backend: Some("direct".into()),
    });

    let json = serde_json::to_value(request).unwrap();

    assert_eq!(json["method"], "turn/start");
    assert_eq!(json["params"]["threadId"], "thread-1");
    assert_eq!(json["params"]["reasoningEffort"], "high");
}
