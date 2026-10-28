//! Formal workflow/Kernel consumer tests. The injected broker records typed
//! requests offline; actual DNS/HTTP policy is separately tested with local TCP.
use super::*;
use async_trait::async_trait;
use deepagent_builtins::http_request::{HttpRequest, HttpRequestClient};
use deepagent_core::error::Result;
use deepagent_tools::ToolExecutionContext;
use serde_json::{json, Value};

#[derive(Default)]
struct RecordingHttp {
    requests: Mutex<Vec<HttpRequest>>,
    wait_for_cancel: bool,
}

#[async_trait]
impl HttpRequestClient for RecordingHttp {
    fn validate(&self, request: &HttpRequest) -> Result<()> {
        request.validate()
    }

    async fn request(&self, request: HttpRequest, context: ToolExecutionContext) -> Result<Value> {
        self.requests.lock().unwrap().push(request);
        if self.wait_for_cancel {
            context.cancelled().await;
            return Err(deepagent_core::error::CoreError::other(
                "fixture observed cancellation",
            ));
        }
        Ok(
            json!({"body":{"payload":"x".repeat(20_000)},"status_code":201,"headers":{"x-fixture":"yes"}}),
        )
    }
}

fn request_workflow() -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start"},
            {"id":"request","kind":"http-request","config":{
                "httpUrl":"https://approved.fixture.invalid/submit",
                "httpMethod":"POST", "httpBody":"synthetic-payload",
                "httpHeaders":{"X-Fixture":"explicit","Authorization":"Bearer synthetic-h175-no-real-token"},"httpTimeout":7,"httpRetryCount":0
            }},
            {"id":"end","kind":"end"}
        ],"edges":[{"id":"a","source":"start","target":"request"},{"id":"b","source":"request","target":"end"}]}
    })).unwrap()
}

#[tokio::test]
async fn workflow_http_full_access_uses_the_same_broker_without_a_prompt() {
    use deepagent_app_core::settings::PermissionPreset;
    let (root, db, _original_chat, session) = workflow_fixture().await;
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::default()),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let client = Arc::new(RecordingHttp::default());
    // SettingsService owns an instance-local cache. Configure and run through
    // the same instance rather than mutating another instance's backing DB.
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    )
    .with_workflow_http_client(client.clone());
    chat.run_workflow(
        request_workflow(),
        "http-full-access".into(),
        session,
        |_| {},
        |_| panic!("FullAccess does not ask"),
    )
    .await
    .unwrap();
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(
        RunStore::new(&db)
            .get("http-full-access")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
}

#[tokio::test]
async fn workflow_http_isolated_backend_without_a_broker_never_falls_back_to_host() {
    for backend in ["sandboxie", "windows_sandbox"] {
        let (root, db, chat, session) = workflow_fixture().await;
        let config = root.path().join(".deepagent");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("settings.json"),
            json!({"sandbox_backend":backend}).to_string(),
        )
        .unwrap();
        let client = Arc::new(RecordingHttp::default());
        let chat = chat.with_workflow_http_client(client.clone());
        let error = chat
            .run_workflow(
                request_workflow(),
                "http-no-host-fallback".into(),
                session,
                |_| {},
                |_| panic!("unavailable backend must not ask"),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no backend registry is configured"),
            "{error}"
        );
        assert!(client.requests.lock().unwrap().is_empty());
        assert_eq!(
            RunStore::new(&db)
                .get("http-no-host-fallback")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("failed")
        );
    }
}

#[tokio::test]
async fn workflow_http_legacy_sandboxie_without_a_broker_never_falls_back_to_host() {
    use deepagent_app_core::{SandboxieExecutor, SandboxieService};
    let (_root, db, chat, session) = workflow_fixture().await;
    // Construct only: no status probe, Sandboxie initialization or OS process.
    let executor = Arc::new(SandboxieExecutor::new(Arc::new(SandboxieService::new(
        None,
    ))));
    let client = Arc::new(RecordingHttp::default());
    let chat = chat
        .with_local_command_executor(executor.clone())
        .with_sandboxie_executor(executor)
        .with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let result = chat
        .run_workflow(
            request_workflow(),
            "http-legacy-no-host-fallback".into(),
            session,
            |_| {},
            move |approval| {
                responder
                    .resolve_approval(&approval.call_id, true, "fixture")
                    .unwrap();
            },
        )
        .await;
    let run = RunStore::new(&db)
        .get("http-legacy-no-host-fallback")
        .unwrap()
        .unwrap();
    let requests = client.requests.lock().unwrap().len();
    eprintln!(
        "legacy Sandboxie: requests={requests}, terminal={:?}, result={result:?}",
        run.terminal_kind
    );
    assert_eq!(
        requests, 0,
        "legacy isolation must not select the host HTTP broker"
    );
    assert!(result.is_err());
    assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
}

#[tokio::test]
async fn workflow_http_direct_selection_overrides_unused_legacy_sandboxie() {
    use deepagent_app_core::{settings::PermissionPreset, SandboxieExecutor, SandboxieService};
    for (preset, backend, expected_approvals) in [
        (PermissionPreset::Default, Some("direct"), 1),
        (PermissionPreset::FullAccess, None, 0),
    ] {
        let (root, db, _original_chat, session) = workflow_fixture().await;
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::default()),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.set_active_permission_preset(preset).unwrap();
        if let Some(backend) = backend {
            let config = root.path().join(".deepagent");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(
                config.join("settings.json"),
                json!({"sandbox_backend":backend}).to_string(),
            )
            .unwrap();
        }
        let executor = Arc::new(SandboxieExecutor::new(Arc::new(SandboxieService::new(
            None,
        ))));
        let client = Arc::new(RecordingHttp::default());
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(MockTransport::default()),
            root.path(),
        )
        .with_local_command_executor(executor.clone())
        .with_sandboxie_executor(executor)
        .with_workflow_http_client(client.clone());
        let responder = chat.clone();
        let approvals = Arc::new(AtomicUsize::new(0));
        let count = approvals.clone();
        chat.run_workflow(
            request_workflow(),
            "http-legacy-direct".into(),
            session,
            |_| {},
            move |approval| {
                count.fetch_add(1, Ordering::SeqCst);
                responder
                    .resolve_approval(&approval.call_id, true, "fixture")
                    .unwrap();
            },
        )
        .await
        .unwrap();
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(approvals.load(Ordering::SeqCst), expected_approvals);
        assert_eq!(
            RunStore::new(&db)
                .get("http-legacy-direct")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("succeeded")
        );
    }
}

#[tokio::test]
async fn workflow_http_backend_registry_never_turns_isolation_into_host_network() {
    use deepagent_app_core::{
        settings::PermissionPreset, SandboxBackend, SandboxBackendKind, SandboxBackendRegistry,
        SandboxCapabilities, SandboxExecutionRequest, SandboxExecutionResult,
    };
    struct FixtureBackend(SandboxBackendKind);
    #[async_trait]
    impl SandboxBackend for FixtureBackend {
        fn capabilities(&self) -> SandboxCapabilities {
            SandboxCapabilities {
                kind: self.0,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: false,
                supports_network_toggle: true,
                supports_readonly_mapping: true,
                message: "offline fixture, no HTTP broker".into(),
            }
        }
        async fn execute(&self, _: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
            panic!("HTTP workflow must not execute an OS command")
        }
    }
    for kind in [
        SandboxBackendKind::Direct,
        SandboxBackendKind::Sandboxie,
        SandboxBackendKind::WindowsSandbox,
    ] {
        let (root, db, _original_chat, session) = workflow_fixture().await;
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::default()),
            Arc::new(MemorySecretStore::new()),
        ));
        settings
            .set_active_permission_preset(PermissionPreset::FullAccess)
            .unwrap();
        let client = Arc::new(RecordingHttp::default());
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(MockTransport::default()),
            root.path(),
        )
        .with_sandbox_backend_registry(Arc::new(SandboxBackendRegistry::new(
            kind,
            vec![Arc::new(FixtureBackend(kind)) as Arc<dyn SandboxBackend>],
        )))
        .with_workflow_http_client(client.clone());
        let result = chat
            .run_workflow(
                request_workflow(),
                "http-registry-policy".into(),
                session,
                |_| {},
                |_| panic!("FullAccess must not ask"),
            )
            .await;
        let direct = kind == SandboxBackendKind::Direct;
        assert_eq!(result.is_ok(), direct, "{kind:?}: {result:?}");
        assert_eq!(client.requests.lock().unwrap().len(), usize::from(direct));
        assert_eq!(
            RunStore::new(&db)
                .get("http-registry-policy")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(if direct { "succeeded" } else { "failed" })
        );
    }
}

#[tokio::test]
async fn workflow_http_generic_tool_alias_cannot_bypass_the_approval_pipeline() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(RecordingHttp::default());
    let chat = chat.with_workflow_http_client(client.clone());
    let mut request = request_workflow();
    let node = &mut request.definition.nodes[1];
    node.kind = "tool".into();
    node.config = json!({
        "toolName":"http_request",
        "toolArguments":{"url":"https://approved.fixture.invalid/submit","method":"POST"}
    })
    .as_object()
    .unwrap()
    .clone();
    // The legacy generic-tool adapter has no approval channel and already
    // refuses high-risk tools. Registering HTTP must not create an alias bypass.
    assert!(chat
        .run_workflow(
            request,
            "http-tool-alias".into(),
            session,
            |_| {},
            |_| panic!("generic tools must fail closed rather than invent approval"),
        )
        .await
        .is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(
        RunStore::new(&db)
            .get("http-tool-alias")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
}

#[tokio::test]
async fn workflow_http_explicit_deny_precedes_full_access_and_approval() {
    use deepagent_app_core::settings::PermissionPreset;
    use deepagent_hooks::PermissionRules;
    let (root, db, _original_chat, session) = workflow_fixture().await;
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::default()),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_permission_rules(PermissionRules::new(
            ["http_request".into()],
            ["http_request".into()],
            ["http_request".into()],
        ))
        .unwrap();
    let client = Arc::new(RecordingHttp::default());
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    )
    .with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    assert!(chat
        .run_workflow(
            request_workflow(),
            "http-policy-denied".into(),
            session,
            |_| {},
            move |request| {
                count.fetch_add(1, Ordering::SeqCst);
                responder
                    .resolve_approval(&request.call_id, true, "fixture")
                    .unwrap();
            }
        )
        .await
        .is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(approvals.load(Ordering::SeqCst), 0);
    let ledger = RunStore::new(&db)
        .events_after("http-policy-denied", None)
        .unwrap();
    assert!(!ledger
        .iter()
        .any(|event| event.event_type == "tool_started"));
    assert_eq!(
        RunStore::new(&db)
            .get("http-policy-denied")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
}

#[tokio::test]
async fn workflow_http_cancelled_while_awaiting_approval_never_dispatches() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(RecordingHttp::default());
    let chat = chat.with_workflow_http_client(client.clone());
    let worker_chat = chat.clone();
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        worker_chat
            .run_workflow(
                request_workflow(),
                "http-wait-cancel".into(),
                session,
                |_| {},
                move |request| {
                    approval_tx.send(request.call_id).unwrap();
                },
            )
            .await
    });
    let call_id = tokio::time::timeout(Duration::from_secs(3), approval_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(chat.cancel_session("http-wait-cancel"));
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap()
        .ok();
    let before = deepagent_persistence::run_control::RunControlStore::new(&db)
        .list_approvals("http-wait-cancel")
        .unwrap();
    assert!(
        matches!(chat.resolve_approval(&call_id, true, "late fixture"),
        Err(deepagent_core::error::CoreError::IllegalTransition { from, to })
        if from == "cancelled" && to == "approved")
    );
    assert_eq!(
        deepagent_persistence::run_control::RunControlStore::new(&db)
            .list_approvals("http-wait-cancel")
            .unwrap(),
        before
    );
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(
        RunStore::new(&db)
            .get("http-wait-cancel")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
}

#[tokio::test]
async fn workflow_http_approval_denial_never_dispatches_and_fails_the_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(RecordingHttp::default());
    let chat = chat.with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    let result = chat
        .run_workflow(
            request_workflow(),
            "http-denied".into(),
            session,
            |_| {},
            move |approval| {
                count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(approval.tool, "http_request");
                assert!(responder
                    .resolve_approval(&approval.call_id, false, "fixture")
                    .unwrap());
            },
        )
        .await;
    assert!(result.is_err());
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    assert!(client.requests.lock().unwrap().is_empty());
    let run = RunStore::new(&db).get("http-denied").unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
    let ledger = RunStore::new(&db).events_after(&run.id, None).unwrap();
    assert!(ledger
        .iter()
        .any(|row| row.event_type == "approval.requested"));
    assert!(!ledger.iter().any(|row| row.event_type == "tool_started"));
}

#[tokio::test]
async fn workflow_http_approved_tool_preserves_typed_output_and_one_action() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(RecordingHttp::default());
    let chat = chat.with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let captured = nodes.clone();
    chat.run_workflow(
        request_workflow(),
        "http-approved".into(),
        session,
        move |event| {
            if let RuntimeEvent::WorkflowNode { event } = event {
                captured.lock().unwrap().push(event);
            }
        },
        move |approval| {
            assert!(responder
                .resolve_approval(&approval.call_id, true, "fixture")
                .unwrap());
        },
    )
    .await
    .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, "synthetic-payload");
    assert_eq!(requests[0].headers["X-Fixture"], "explicit");
    assert_eq!(
        requests[0].headers["Authorization"],
        "Bearer synthetic-h175-no-real-token"
    );
    assert_eq!(requests[0].timeout_secs, 7);
    let nodes = nodes.lock().unwrap();
    let output = nodes
        .iter()
        .find(|node| node.node_id == "request" && node.status == NodeExecutionStatus::Completed)
        .unwrap()
        .outputs
        .as_ref()
        .unwrap();
    assert_eq!(output["status_code"], 201);
    assert_eq!(output["headers"]["x-fixture"], "yes");
    assert_eq!(
        output["body"]["payload"].as_str().unwrap().len(),
        20_000,
        "node gets bounded original JSON, not truncated model preview"
    );
    assert!(output.get("system_reminder").is_none());
    let run = RunStore::new(&db).get("http-approved").unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    let ledger = RunStore::new(&db).events_after(&run.id, None).unwrap();
    assert!(
        !serde_json::to_string(&ledger)
            .unwrap()
            .contains("synthetic-h175-no-real-token"),
        "canonical durable replay must retain its existing secret scrubber"
    );
    assert_eq!(
        ledger
            .iter()
            .filter(|row| row.event_type == "tool_started")
            .count(),
        1
    );
    assert_eq!(
        ledger
            .iter()
            .filter(|row| row.event_type == "run_terminal")
            .count(),
        1
    );
    db.with_conn(|connection| {
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM run_actions WHERE run_id='http-approved' AND state='completed' AND tool_name='http_request'", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn workflow_http_cancelled_during_request_never_completes_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(RecordingHttp {
        wait_for_cancel: true,
        ..Default::default()
    });
    let chat = chat.with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let worker_chat = chat.clone();
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let captured = nodes.clone();
    let worker = tokio::spawn(async move {
        worker_chat
            .run_workflow(
                request_workflow(),
                "http-cancelled".into(),
                session,
                move |event| {
                    if let RuntimeEvent::WorkflowNode { event } = event {
                        captured.lock().unwrap().push(event);
                    }
                },
                move |approval| {
                    responder
                        .resolve_approval(&approval.call_id, true, "fixture")
                        .unwrap();
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(chat.cancel_session("http-cancelled"));
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap()
        .ok();
    let run = RunStore::new(&db).get("http-cancelled").unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("cancelled"));
    let nodes = nodes.lock().unwrap();
    assert!(!nodes
        .iter()
        .any(|node| node.node_id == "request" && node.status == NodeExecutionStatus::Completed));
    assert!(nodes
        .iter()
        .any(|node| node.node_id == "request" && node.status == NodeExecutionStatus::Cancelled));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}
