use deepagent_harness_protocol::{
    project_runtime_event, ApprovalRespondRequest, ConfigReadRequest, EventAckRequest,
    EventContext, HarnessEvent, HarnessRequest, InitializeRequest, ItemPayload,
    SandboxStatusRequest, ThreadArchiveRequest, ThreadForkRequest, ThreadListRequest,
    ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, ToolListRequest,
    TurnInterruptRequest, TurnStartRequest, TurnSteerRequest,
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
fn transport_turn_id_wins_over_internal_runtime_task_id() {
    let event = RuntimeEvent::RunStarted {
        task_id: "runtime-task".into(),
    };
    let projected = project_runtime_event(
        &event,
        &EventContext::new(Some("thread-1".into()), Some("turn-1".into())),
    )
    .expect("run start projects");

    let value = serde_json::to_value(projected).unwrap();
    assert_eq!(value["type"], "turn.started");
    assert_eq!(value["turnId"], "turn-1");
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

#[test]
fn every_harness_request_variant_has_a_stable_method_and_round_trips() {
    let requests = vec![
        HarnessRequest::Initialize(InitializeRequest {
            client_name: "test".into(),
            client_version: "1".into(),
            protocol_version: 1,
        }),
        HarnessRequest::ThreadStart(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        }),
        HarnessRequest::ThreadResume(ThreadResumeRequest {
            thread_id: "thread-1".into(),
        }),
        HarnessRequest::ThreadList(ThreadListRequest::default()),
        HarnessRequest::ThreadRead(ThreadReadRequest {
            thread_id: "thread-1".into(),
            after_sequence: None,
            session_after_sequence: None,
            run_after_sequence: None,
            run_after_sequences: None,
        }),
        HarnessRequest::ThreadFork(ThreadForkRequest {
            thread_id: "thread-1".into(),
            at_sequence: None,
        }),
        HarnessRequest::ThreadArchive(ThreadArchiveRequest {
            thread_id: "thread-1".into(),
        }),
        HarnessRequest::TurnStart(TurnStartRequest {
            thread_id: "thread-1".into(),
            input: "hello".into(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        }),
        HarnessRequest::TurnInterrupt(TurnInterruptRequest {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
        }),
        HarnessRequest::TurnSteer(TurnSteerRequest {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            input: "continue".into(),
        }),
        HarnessRequest::ApprovalRespond(ApprovalRespondRequest {
            approval_id: "approval-1".into(),
            approved: false,
            scope: None,
        }),
        HarnessRequest::EventAck(EventAckRequest { event_sequence: 7 }),
        HarnessRequest::ToolList(ToolListRequest::default()),
        HarnessRequest::ConfigRead(ConfigReadRequest::default()),
        HarnessRequest::SandboxStatus(SandboxStatusRequest::default()),
    ];
    let expected_methods = [
        "initialize",
        "thread/start",
        "thread/resume",
        "thread/list",
        "thread/read",
        "thread/fork",
        "thread/archive",
        "turn/start",
        "turn/interrupt",
        "turn/steer",
        "approval/respond",
        "event/ack",
        "tool/list",
        "config/read",
        "sandbox/status",
    ];

    assert_eq!(requests.len(), expected_methods.len());
    for (request, expected_method) in requests.into_iter().zip(expected_methods) {
        let value = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(value["method"], expected_method);
        let decoded: HarnessRequest = serde_json::from_value(value).expect("decode request");
        assert_eq!(decoded, request);
    }
}

#[test]
fn thread_read_supports_stream_specific_cursors() {
    let request = HarnessRequest::ThreadRead(ThreadReadRequest {
        thread_id: "thread-1".into(),
        after_sequence: None,
        session_after_sequence: Some(12),
        run_after_sequence: Some(7),
        run_after_sequences: Some(std::collections::BTreeMap::from([("run-1".into(), 4)])),
    });
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["params"]["sessionAfterSequence"], 12);
    assert_eq!(value["params"]["runAfterSequence"], 7);
    assert_eq!(value["params"]["runAfterSequences"]["run-1"], 4);
    assert!(value["params"].get("afterSequence").is_none());
}

#[test]
fn mcp_degradation_projects_to_typed_lifecycle_event() {
    let event = RuntimeEvent::McpLifecycle {
        server_id: "docs".into(),
        status: "degraded".into(),
        transport: Some("stdio".into()),
        config_hash: Some("config-hash".into()),
        tool_schema_hash: None,
        startup_attempt: 1,
        degradation_code: Some("connect_failed".into()),
        reason: Some("connection refused".into()),
        tool_count: 0,
    };
    let projected = project_runtime_event(
        &event,
        &EventContext::new(Some("thread-1".into()), Some("turn-1".into())),
    )
    .unwrap();
    let value = serde_json::to_value(projected).unwrap();

    assert_eq!(value["type"], "mcp.lifecycle");
    assert_eq!(value["serverId"], "docs");
    assert_eq!(value["degradationCode"], "connect_failed");
    assert_eq!(value["configHash"], "config-hash");
    assert_eq!(value["toolCount"], 0);
}
