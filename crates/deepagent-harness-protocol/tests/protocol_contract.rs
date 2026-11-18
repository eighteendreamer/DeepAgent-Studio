use deepagent_harness_protocol::{
    project_runtime_event, ApprovalRespondRequest, ConfigReadRequest, EventAckRequest,
    EventContext, HarnessEvent, HarnessRequest, InitializeRequest, ItemPayload,
    SandboxStatusRequest, ThreadArchiveRequest, ThreadForkRequest, ThreadListRequest,
    ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, ToolListRequest,
    TurnInterruptRequest, TurnStartRequest, TurnSteerRequest,
};
use deepagent_runtime::RuntimeEvent;

#[test]
fn h138_client_metadata_remains_separate_in_harness_wire() {
    let runtime = RuntimeEvent::ToolCompleted {
        name: "mcp__synthetic__tool".into(),
        call_id: "h138-call".into(),
        ok: true,
        output: serde_json::json!({"content":[{"type":"text","text":"public-result"}]}),
        content: Vec::new(),
        duration_ms: 1,
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: Some(
            serde_json::json!({"client_metadata":{"mcp":{"_meta":{"detail":"h138-client-canary"}}}}),
        ),
    };
    let event = project_runtime_event(
        &runtime,
        &EventContext::new(Some("thread".into()), Some("turn".into())),
    )
    .unwrap();
    let wire = serde_json::to_value(&event).unwrap();
    assert!(!wire["item"]["output"]
        .to_string()
        .contains("h138-client-canary"));
    assert_eq!(
        wire["item"]["meta"]["client_metadata"]["mcp"]["_meta"]["detail"],
        "h138-client-canary"
    );
    assert_eq!(serde_json::from_value::<HarnessEvent>(wire).unwrap(), event);
}

#[test]
fn h127_tool_image_refs_survive_runtime_harness_and_wire_roundtrip() {
    use deepagent_core::message::MessageAttachment;
    use deepagent_core::tool_content::ToolResultPart;
    let content = vec![
        ToolResultPart::Text {
            text: "untrusted tool text".into(),
        },
        ToolResultPart::Image {
            attachment: MessageAttachment {
                id: "h127-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: Some("synthetic/image.png".into()),
                sha256: Some("a".repeat(64)),
            },
        },
    ];
    for include_content in [false, true] {
        let runtime = RuntimeEvent::ToolCompleted {
            name: "screenshot".into(),
            call_id: "image-call".into(),
            ok: true,
            output: serde_json::json!({"text":"image result"}),
            content: if include_content {
                content.clone()
            } else {
                Vec::new()
            },
            duration_ms: 1,
            tool_kind: None,
            file_path: None,
            summary: None,
            meta: None,
        };
        let event = project_runtime_event(
            &runtime,
            &EventContext::new(Some("thread".into()), Some("turn".into())),
        )
        .unwrap();
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "item.completed");
        assert_eq!(json["item"]["callId"], "image-call");
        assert_eq!(
            json["item"]["output"],
            serde_json::json!({"text":"image result"})
        );
        if include_content {
            let fixture: serde_json::Value =
                serde_json::from_str(include_str!("fixtures/tool-image-result-v1.json")).unwrap();
            assert_eq!(
                json, fixture,
                "SDK fixture must match the real RuntimeEvent projection"
            );
            assert_eq!(
                json["item"]["content"],
                serde_json::to_value(&content).unwrap()
            );
            assert!(!json.to_string().contains("base64"));
        } else {
            assert!(
                json["item"].get("content").is_none(),
                "text-only event wire is unchanged"
            );
        }
        assert_eq!(serde_json::from_value::<HarnessEvent>(json).unwrap(), event);
    }
}

#[test]
fn h221_config_scope_is_additive_and_admission_guard_round_trips() {
    assert_eq!(
        serde_json::to_value(ConfigReadRequest::default()).unwrap(),
        serde_json::json!({})
    );
    for params in [
        serde_json::json!({"threadId":"thread-1"}),
        serde_json::json!({"cwd":"synthetic-workspace"}),
    ] {
        let request: ConfigReadRequest = serde_json::from_value(params.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), params);
    }
    let request: HarnessRequest = serde_json::from_value(serde_json::json!({
        "method":"turn/start", "params":{"threadId":"thread-1","input":"hello","expectedConfigSnapshotId":"config-1"}
    })).unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap()["params"]["expectedConfigSnapshotId"],
        "config-1"
    );
}

#[test]
fn h212_response_constructors_own_the_same_wire_version_for_results_and_errors() {
    use deepagent_harness_protocol::{RpcResponse, JSONRPC_VERSION};
    assert_eq!(JSONRPC_VERSION, "2.0");
    for response in [
        RpcResponse::success(serde_json::json!(17), serde_json::json!({})),
        RpcResponse::error(serde_json::json!(18), -32004, "synthetic"),
    ] {
        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(serde_json::to_value(response).unwrap()["jsonrpc"], "2.0");
    }
}

#[test]
fn h222_thread_reasoning_round_trips_without_losing_explicit_none() {
    for effort in ["none", "low", "medium", "high", "max"] {
        let params = serde_json::json!({"reasoningEffort":effort});
        let request: ThreadStartRequest = serde_json::from_value(params.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), params);
    }
    for params in [
        serde_json::json!({}),
        serde_json::json!({"reasoningEffort":null}),
    ] {
        let request: ThreadStartRequest = serde_json::from_value(params).unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({})
        );
    }
}

#[test]
fn thread_start_request_uses_stable_machine_field_names() {
    let request = HarnessRequest::ThreadStart(ThreadStartRequest {
        cwd: Some("G:/workspace".into()),
        provider: Some("deepseek-official".into()),
        model: Some("deepseek-v4-flash".into()),
        reasoning_effort: None,
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
fn runtime_tool_diagnostics_and_terminal_events_keep_correlation() {
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
    let terminal = project_runtime_event(
        &RuntimeEvent::RunCancelled {
            edited_files: vec![],
        },
        &context,
    )
    .unwrap();

    assert!(matches!(
        started,
        HarnessEvent::ItemStarted {
            item: ItemPayload::ToolCall { call_id, name, .. },
            ..
        } if call_id == "call-1" && name == "read_file"
    ));
    assert!(matches!(
        approval,
        HarnessEvent::ItemUpdated {
            thread_id: Some(thread_id),
            turn_id: Some(turn_id),
            item: ItemPayload::Runtime { event_type, data },
            ..
        } if thread_id == "thread-1" && turn_id == "turn-1"
            && event_type == "tool_blocked" && data["needs_approval"] == true
            && data["name"] == "bash" && data["reason"] == "approval required"
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
fn h84_final_tool_denial_remains_an_error_not_an_approval() {
    let event = project_runtime_event(
        &RuntimeEvent::ToolBlocked {
            name: "delete_path".into(),
            reason: "denied by policy".into(),
            needs_approval: false,
        },
        &EventContext::default(),
    )
    .unwrap();
    assert!(matches!(event, HarnessEvent::Error { code, message, .. }
        if code == "tool_blocked" && message.contains("denied by policy")));
}

#[test]
fn turn_start_request_serializes_provider_and_sandbox_overrides() {
    let request = HarnessRequest::TurnStart(TurnStartRequest {
        expected_config_snapshot_id: None,
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
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        }),
        HarnessRequest::ThreadResume(ThreadResumeRequest {
            thread_id: "thread-1".into(),
        }),
        HarnessRequest::ThreadList(ThreadListRequest::default()),
        HarnessRequest::ThreadRead(ThreadReadRequest {
            thread_id: "thread-1".into(),
            page_size: None,
            max_bytes: None,
            cursor: None,
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
        HarnessRequest::ThreadUnarchive(deepagent_harness_protocol::ThreadUnarchiveRequest {
            thread_id: "thread-1".into(),
        }),
        HarnessRequest::TurnStart(TurnStartRequest {
            expected_config_snapshot_id: None,
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
        "thread/unarchive",
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
        assert!(HarnessRequest::is_known_method(expected_method));
        let decoded: HarnessRequest = serde_json::from_value(value).expect("decode request");
        assert_eq!(decoded, request);
    }
}

#[test]
fn h217_unknown_method_spelling_never_becomes_a_known_route() {
    for method in [
        "",
        "Thread/start",
        "thread/start ",
        " thread/start",
        "thread//start",
        "future/v2/extension",
        "rpc.extension",
        "harness/event",
        "does-not-exist",
    ] {
        assert!(!HarnessRequest::is_known_method(method), "{method:?}");
        assert!(serde_json::from_value::<HarnessRequest>(serde_json::json!({
            "method":method,"params":{}
        }))
        .is_err());
    }
}

#[test]
fn h217_current_request_dto_does_not_admit_batches_or_inbound_notifications() {
    use deepagent_harness_protocol::RpcRequest;
    // This change does not introduce batch or inbound-notification execution.
    // They remain outside the existing request DTO, not unknown-method routes.
    for value in [
        serde_json::json!([]),
        serde_json::json!([{"jsonrpc":"2.0","id":1,"method":"future/unknown"}]),
        serde_json::json!({"jsonrpc":"2.0","method":"future/unknown","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","method":"thread/start","params":{}}),
    ] {
        assert!(serde_json::from_value::<RpcRequest>(value).is_err());
    }
}

#[test]
fn thread_read_supports_stream_specific_cursors() {
    let request = HarnessRequest::ThreadRead(ThreadReadRequest {
        thread_id: "thread-1".into(),
        page_size: None,
        max_bytes: None,
        cursor: None,
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
fn h218_thread_read_page_options_are_additive_on_the_wire() {
    let legacy: ThreadReadRequest =
        serde_json::from_value(serde_json::json!({"threadId":"t"})).unwrap();
    assert_eq!(legacy.page_size, None);
    assert_eq!(legacy.max_bytes, None);
    assert_eq!(legacy.cursor, None);
    let value = serde_json::json!({"threadId":"t","pageSize":7,"maxBytes":8192,"cursor":"opaque"});
    let request: ThreadReadRequest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(request.page_size, Some(7));
    assert_eq!(request.max_bytes, Some(8192));
    assert_eq!(request.cursor.as_deref(), Some("opaque"));
    assert_eq!(serde_json::to_value(request).unwrap(), value);
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

#[test]
fn compaction_events_reach_machine_consumers_with_their_strategy() {
    // Contract: a context compaction must not be silently dropped on the
    // harness protocol, and the `strategy` must survive — it is the only thing
    // that tells a machine consumer whether the model view was cut by a session
    // note fold, a model summary, or a reactive trim.
    let context = EventContext::new(Some("thread-1".into()), Some("turn-1".into()));
    let projected = project_runtime_event(
        &RuntimeEvent::ContextCompacted {
            tokens_before: 180_000,
            tokens_after: 60_000,
            strategy: "session_memory".into(),
            summary: Some("everything through event 42".into()),
        },
        &context,
    )
    .expect("compaction must not be dropped");
    let value = serde_json::to_value(projected).unwrap();

    assert_eq!(value["type"], "item.updated");
    assert_eq!(value["item"]["kind"], "runtime");
    assert_eq!(value["item"]["eventType"], "context_compacted");
    assert_eq!(value["item"]["data"]["tokens_before"], 180_000);
    assert_eq!(value["item"]["data"]["tokens_after"], 60_000);
    assert_eq!(value["item"]["data"]["strategy"], "session_memory");
}

#[test]
fn memory_injection_events_reach_machine_consumers_as_runtime_items() {
    // Contract: a background memory injection must NOT be silently dropped on
    // the harness protocol. It has no first-class item type yet, so it must
    // arrive through the catch-all `runtime` payload with a stable event_type
    // and the ids the SDK needs for its own de-dup view.
    let context = EventContext::new(Some("thread-1".into()), Some("turn-1".into()));
    let projected = project_runtime_event(
        &RuntimeEvent::RelevantMemoriesInjected {
            count: 2,
            latency_ms: 42,
            ids: vec!["project:build-fix".into(), "global:cargo-j1".into()],
        },
        &context,
    )
    .expect("memory injection must not be dropped");
    let value = serde_json::to_value(projected).unwrap();

    assert_eq!(value["type"], "item.updated");
    assert_eq!(value["item"]["kind"], "runtime");
    assert_eq!(value["item"]["eventType"], "relevant_memories_injected");
    assert_eq!(value["item"]["data"]["count"], 2);
    assert_eq!(value["item"]["data"]["latency_ms"], 42);
    assert_eq!(value["item"]["data"]["ids"][0], "project:build-fix");
}

#[test]
fn tool_list_accepts_optional_run_scope_without_breaking_legacy_requests() {
    let scoped: HarnessRequest = serde_json::from_value(serde_json::json!({
        "method": "tool/list",
        "params": {"threadId": "thread-1", "runId": "run-1", "expectedSnapshotId":"snapshot-1"}
    }))
    .unwrap();
    let encoded = serde_json::to_value(scoped).unwrap();
    assert_eq!(encoded["params"]["threadId"], "thread-1");
    assert_eq!(encoded["params"]["runId"], "run-1");
    assert_eq!(encoded["params"]["expectedSnapshotId"], "snapshot-1");

    let legacy: HarnessRequest = serde_json::from_value(serde_json::json!({
        "method": "tool/list",
        "params": {}
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["params"],
        serde_json::json!({})
    );
}
