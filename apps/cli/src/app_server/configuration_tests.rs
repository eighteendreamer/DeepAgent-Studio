//! H-04: exercise the dispatcher, authoritative session metadata and real run path.
use super::*;
use deepagent_models::DEEPSEEK_OFFICIAL_PROVIDER;

#[path = "config_read_tests.rs"]
mod config_read_tests;

fn configured_thread(state: &mut ServerState, workspace: &std::path::Path) -> Value {
    dispatch(
        state,
        "thread/start",
        json!({
            "cwd": workspace, "provider": DEEPSEEK_OFFICIAL_PROVIDER,
            "model":"deepseek-flash", "permissionProfile":"read_only", "sandboxBackend":"direct"
        }),
    )
}

fn start(state: &mut ServerState, thread_id: &str, extra: Value) -> TurnLaunch {
    let mut params = json!({"threadId":thread_id,"input":"exercise saved configuration"});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let (response, launch) = state.dispatch(rpc("turn/start", params), None);
    assert!(response.is_success(), "{response:?}");
    let launch = launch.unwrap();
    // H-219: cold resume/fork inherit configuration, never another run's
    // capability identity. Query the real admission, not a seeded projection.
    let snapshot = &response.result()["capabilitySnapshot"];
    assert_eq!(snapshot["threadId"], thread_id);
    assert_eq!(snapshot["runId"], launch.turn_id);
    if let Some(backend) = &launch.overrides.sandbox_backend {
        assert_eq!(snapshot["context"]["sandboxBackend"], *backend);
    }
    if let Some(profile) = &launch.overrides.permission_profile {
        assert_eq!(snapshot["context"]["sandboxMode"], *profile);
    }
    let scoped = dispatch(
        state,
        "tool/list",
        json!({
            "threadId":thread_id, "runId":launch.turn_id,
            "expectedSnapshotId":snapshot["snapshotId"]
        }),
    );
    assert_eq!(scoped["capabilitySnapshot"], *snapshot);
    launch
}

async fn finish_cancelled_metadata_launch(state: &Arc<Mutex<ServerState>>, launch: TurnLaunch) {
    // Metadata tests do not use a provider. Settle each admitted fixture through
    // the real executor before checking defaults on the next ordinary start.
    assert!(
        launch
            .chat
            .request_cancel(&launch.turn_id)
            .unwrap()
            .accepted
    );
    let worker = spawn_turn(state.clone(), Arc::new(|_| {}), launch);
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn configuration_cold_resume_fork_and_turn_defaults_are_independent() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let created = configured_thread(&mut fixture.state.lock().unwrap(), &fixture.workspace);
    let source = created["threadId"].as_str().unwrap();
    let expected = created["configuration"].clone();
    assert_eq!(expected["version"], 1);
    let reattached = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/resume",
        json!({"threadId":source}),
    );
    assert_eq!(reattached["configuration"], expected);

    let mut cold = fixture.reopened_state();
    assert!(!cold.threads.contains_key(source));
    // Fork before attaching the source: inheritance must come from the source
    // file, not an optional process-local ThreadState (H-216 readiness remains separate).
    let fork = dispatch(
        &mut cold,
        "thread/fork",
        json!({"threadId":source,"atSequence":0}),
    );
    let child = fork["threadId"].as_str().unwrap();
    assert_eq!(
        dispatch(&mut cold, "thread/resume", json!({"threadId":child}))["configuration"],
        expected
    );
    assert_eq!(
        dispatch(&mut cold, "thread/resume", json!({"threadId":source}))["configuration"],
        expected
    );
    let hot_fork = dispatch(
        &mut cold,
        "thread/fork",
        json!({"threadId":source,"atSequence":0}),
    );
    let hot_child = hot_fork["threadId"].as_str().unwrap();
    let hot_launch = start(&mut cold, hot_child, json!({}));
    assert_eq!(
        serde_json::to_value(&hot_launch.overrides).unwrap(),
        expected["overrides"]
    );
    let child_launch = start(&mut cold, child, json!({}));
    assert_eq!(
        serde_json::to_value(&child_launch.overrides).unwrap(),
        expected["overrides"]
    );
    let original = start(
        &mut cold,
        source,
        json!({
            "provider":"per-turn-provider", "model":"per-turn-model", "permissionProfile":"workspace_write",
            "sandboxBackend":"direct", "reasoningEffort":"high"
        }),
    );
    let (response, replacement) = cold.dispatch(
        rpc(
            "turn/steer",
            json!({
                "threadId":source,"turnId":original.turn_id,"input":"preserve all overrides"
            }),
        ),
        None,
    );
    assert!(response.is_success());
    let replacement = replacement.unwrap();
    assert_eq!(replacement.overrides, original.overrides);
    let state = Arc::new(Mutex::new(cold));
    for launch in [hot_launch, child_launch, original, replacement] {
        finish_cancelled_metadata_launch(&state, launch).await;
    }
    let next = start(&mut state.lock().unwrap(), source, json!({}));
    assert_eq!(
        serde_json::to_value(&next.overrides).unwrap(),
        expected["overrides"]
    );
    finish_cancelled_metadata_launch(&state, next).await;
    // Individual turn overrides are not promoted into the persistent thread contract.
    assert_eq!(
        EventStore::new(&state.lock().unwrap().database)
            .session_execution_config(source.parse().unwrap())
            .unwrap()
            .unwrap()
            .overrides,
        serde_json::from_value::<HarnessRunOverrides>(expected["overrides"].clone()).unwrap()
    );
}

#[tokio::test]
async fn configuration_legacy_resume_explicitly_reports_missing_snapshot() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let db = fixture.state.lock().unwrap().database.clone();
    let legacy = Session::create_in_project(
        &db,
        &SystemClock,
        None,
        deepagent_core::SessionMode::Normal,
        Some(&fixture.workspace.to_string_lossy()),
    )
    .unwrap()
    .id()
    .to_string();
    let response = dispatch(
        &mut fixture.reopened_state(),
        "thread/resume",
        json!({"threadId":legacy}),
    );
    assert!(response.as_object().unwrap().contains_key("configuration"));
    assert!(response["configuration"].is_null());
    let configured = Session::create_configured(
        &db,
        &SystemClock,
        None,
        deepagent_core::SessionMode::Normal,
        Some(&fixture.workspace.to_string_lossy()),
        Some(SessionExecutionConfig::new(HarnessRunOverrides {
            reasoning_effort: Some("low".into()),
            ..Default::default()
        })),
    )
    .unwrap()
    .id()
    .to_string();
    let mut cold = fixture.reopened_state();
    dispatch(&mut cold, "thread/resume", json!({"threadId":configured}));
    let first = start(&mut cold, &configured, json!({}));
    assert_eq!(first.overrides.reasoning_effort.as_deref(), Some("low"));
    let state = Arc::new(Mutex::new(cold));
    finish_cancelled_metadata_launch(&state, first).await;
    let second = start(
        &mut state.lock().unwrap(),
        &configured,
        json!({"reasoningEffort":"high"}),
    );
    assert_eq!(second.overrides.reasoning_effort.as_deref(), Some("high"));
    finish_cancelled_metadata_launch(&state, second).await;
}

struct WireTransport {
    bodies: Mutex<Vec<Value>>,
    script: ScriptedTransport,
}

#[async_trait::async_trait]
impl HttpTransport for WireTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> CoreResult<()> {
        self.bodies
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        self.script.stream(request, sink).await
    }
    async fn get_json(&self, _: &str, _: &str) -> CoreResult<String> {
        Ok(
            json!({"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-pro","object":"model","owned_by":"deepseek"}]})
            .to_string(),
        )
    }
}

async fn execute_with_approvals(fixture: &Fixture, launch: TurnLaunch) -> String {
    let id = launch.turn_id.clone();
    let chat = launch.chat.clone();
    let events = fixture.events.clone();
    let worker = spawn_turn(
        fixture.state.clone(),
        Arc::new(move |event| {
            // Approve through the normal durable route: read_only must still deny
            // the file write rather than merely relying on absent approval input.
            if let HarnessEvent::ApprovalRequested {
                approval_id: Some(approval_id),
                ..
            } = &event
            {
                chat.resolve_approval(approval_id, true, "h04-fixture")
                    .unwrap();
            }
            events
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
        }),
        launch,
    );
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    id
}

#[tokio::test]
async fn configuration_cold_resume_retains_wire_model_and_read_only_after_defaults_change() {
    let call = |call_id| {
        function_call(
            call_id,
            "write_file",
            json!({"path":"h04-canary.txt","content":"normal explicit write"}),
        )
    };
    let transport = Arc::new(WireTransport {
        bodies: Mutex::new(Vec::new()),
        script: ScriptedTransport {
            responses: vec![
                call("write-canary-denied"),
                completed_text("write denied"),
                call("write-canary-accepted"),
                completed_text("write accepted"),
            ],
            next_response: AtomicUsize::new(0),
        },
    });
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    let created = configured_thread(&mut fixture.state.lock().unwrap(), &fixture.workspace);
    fixture.thread_id = created["threadId"].as_str().unwrap().to_owned();
    fixture
        .settings
        .set_model(ModelRole::Chat, "deepseek-pro")
        .unwrap();
    fixture
        .settings
        .set_active_permission_preset(deepagent_app_core::PermissionPreset::FullAccess)
        .unwrap();
    // New ServerState/ChatService with the same settings and offline transport;
    // this is a cold attach, not a reused ThreadState or manually supplied overrides.
    let mut cold = {
        let old = fixture.state.lock().unwrap();
        ServerState::new(
            old.base_chat.clone(),
            fixture.workspace.clone(),
            old.capabilities.clone(),
        )
    };
    initialize(&mut cold);
    let resumed = dispatch(
        &mut cold,
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    assert_eq!(resumed["configuration"], created["configuration"]);
    fixture.state = Arc::new(Mutex::new(cold));
    let launch = start(
        &mut fixture.state.lock().unwrap(),
        &fixture.thread_id,
        json!({}),
    );
    let expected = launch.overrides.clone();
    let (response, replacement) = fixture.state.lock().unwrap().dispatch(rpc("turn/steer", json!({
        "threadId":fixture.thread_id,"turnId":launch.turn_id,"input":"keep the read-only configuration"
    })), None);
    assert!(response.is_success());
    let replacement = replacement.unwrap();
    assert_eq!(replacement.overrides, expected);
    let cancelled = execute_with_approvals(&fixture, launch).await;
    assert_durable_terminal(&fixture.read(), &cancelled, "cancelled");
    assert!(transport.bodies.lock().unwrap().is_empty());
    let restricted = execute_with_approvals(&fixture, replacement).await;
    assert_durable_terminal(&fixture.read(), &restricted, "succeeded");
    assert!(!fixture.workspace.join("h04-canary.txt").exists());
    let recorded = fixture.events.lock().unwrap().clone();
    // The existing modification hook denies this before tool execution. Its
    // mapper emits Error/tool_blocked without turn context, not ToolResult.
    assert!(recorded.iter().any(|event| event["type"] == "error"
        && event["code"] == "tool_blocked"
        && event["message"]
            .as_str()
            .is_some_and(|message| message.contains("write_file")
                && message.contains("modify is blocked by the current sandbox mode"))));
    assert_eq!(transport.bodies.lock().unwrap().len(), 2);
    assert!(transport
        .bodies
        .lock()
        .unwrap()
        .iter()
        .all(|body| body["model"] == "deepseek-flash"));

    let launch = start(
        &mut fixture.state.lock().unwrap(),
        &fixture.thread_id,
        json!({"permissionProfile":"full_access","model":"deepseek-pro"}),
    );
    let normal = execute_with_approvals(&fixture, launch).await;
    assert_durable_terminal(&fixture.read(), &normal, "succeeded");
    assert!(fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event["turnId"] == normal
            && event["type"] == "item.completed"
            && event["item"]["kind"] == "tool_result"
            && event["item"]["ok"] == true));
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("h04-canary.txt")).unwrap(),
        "normal explicit write"
    );
    let bodies = transport.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 4);
    // Both turns belong to one request history. Provider call identities must
    // stay distinct even though the canary tool and its arguments are equal.
    let final_ids: Vec<_> = bodies[3]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call")
        .filter_map(|item| item["call_id"].as_str())
        .collect();
    assert_eq!(final_ids, ["write-canary-denied", "write-canary-accepted"]);
    assert!(bodies[2..]
        .iter()
        .all(|body| body["model"] == "deepseek-pro"));
    assert_eq!(
        EventStore::new(&fixture.state.lock().unwrap().database)
            .session_execution_config(fixture.thread_id.parse().unwrap())
            .unwrap()
            .unwrap()
            .overrides
            .permission_profile
            .as_deref(),
        Some("read_only")
    );
}
