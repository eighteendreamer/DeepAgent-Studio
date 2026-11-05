//! H-222: thread reasoning must enter the existing durable/admitted configuration.
use super::*;
use deepagent_models::ReasoningEffort;

fn thread(fixture: &Fixture, effort: Option<&str>) -> Value {
    let mut params = json!({"cwd":fixture.workspace,"permissionProfile":"read_only"});
    if let Some(effort) = effort {
        params["reasoningEffort"] = json!(effort);
    }
    dispatch(&mut fixture.state.lock().unwrap(), "thread/start", params)
}

fn recorder(responses: Vec<Vec<String>>) -> Arc<WireTransport> {
    Arc::new(WireTransport {
        bodies: Mutex::new(Vec::new()),
        script: ScriptedTransport {
            responses,
            next_response: AtomicUsize::new(0),
        },
    })
}

#[tokio::test]
async fn h222_canonical_thread_defaults_and_turn_overrides_reach_real_wire() {
    let transport = recorder(vec![completed_text("synthetic response")]);
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    fixture
        .settings
        .set_reasoning_effort(ReasoningEffort::Low)
        .unwrap();
    let mut expected_wire = Vec::new();
    for effort in [None, Some("none"), Some("low"), Some("high"), Some("max")] {
        let created = thread(&fixture, effort);
        fixture.thread_id = created["threadId"].as_str().unwrap().to_owned();
        assert_eq!(
            created["configuration"]["overrides"]["reasoning_effort"],
            json!(effort)
        );
        let resolved = dispatch(
            &mut fixture.state.lock().unwrap(),
            "config/read",
            json!({"threadId":fixture.thread_id}),
        );
        let inherited = effort.unwrap_or("low");
        assert_eq!(resolved["reasoningEffort"], inherited);
        let override_value = if inherited == "none" { "max" } else { "none" };
        for (extra, expected) in [
            (json!({}), inherited),
            (json!({"reasoningEffort":override_value}), override_value),
            (json!({}), inherited),
        ] {
            let launch = start(
                &mut fixture.state.lock().unwrap(),
                &fixture.thread_id,
                extra,
            );
            assert_eq!(
                launch.overrides.reasoning_effort.as_deref(),
                if expected == inherited {
                    effort
                } else {
                    Some(override_value)
                }
            );
            let id = execute_with_approvals(&fixture, launch).await;
            assert_durable_terminal(&fixture.read(), &id, "succeeded");
            expected_wire.push(expected);
        }
        let stored = EventStore::new(&fixture.state.lock().unwrap().database)
            .session_execution_config(fixture.thread_id.parse().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(stored).unwrap(),
            created["configuration"],
            "per-turn effort must not mutate the thread"
        );
    }
    let bodies = transport.bodies.lock().unwrap();
    assert_eq!(bodies.len(), expected_wire.len());
    for (body, expected) in bodies.iter().zip(expected_wire) {
        assert_eq!(body["reasoning"]["effort"], expected);
        assert_eq!(body["model"], "deepseek-flash");
        assert!(
            body.get("thinking").is_none(),
            "Responses must not acquire Chat-only fields"
        );
    }
}

#[tokio::test]
async fn h222_cold_fork_resume_steer_and_tool_loop_keep_thread_effort() {
    let transport = recorder(vec![
        function_call("h222-read", "read_file", json!({"path":"h222.txt"})),
        completed_text("read done"),
    ]);
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    std::fs::write(fixture.workspace.join("h222.txt"), "synthetic normal read").unwrap();
    let created = thread(&fixture, Some("high"));
    let source = created["threadId"].as_str().unwrap().to_owned();
    // The generic metadata-only reopened_state uses an empty EnvSecretStore.
    // Keep the recording transport and synthetic MemorySecretStore for real
    // execution, while reopening the authoritative database and ThreadState.
    let database = Arc::new(Database::open(&fixture.database_path).unwrap());
    let chat = ChatService::new(
        database,
        fixture.settings.clone(),
        transport.clone(),
        fixture.workspace.clone(),
    );
    let capabilities = fixture.state.lock().unwrap().capabilities.clone();
    let mut cold = ServerState::new(chat, fixture.workspace.clone(), capabilities);
    initialize(&mut cold);
    assert!(!cold.threads.contains_key(&source));
    let fork = dispatch(
        &mut cold,
        "thread/fork",
        json!({"threadId":source,"atSequence":0}),
    );
    let child = fork["threadId"].as_str().unwrap().to_owned();
    for id in [&source, &child] {
        let resumed = dispatch(&mut cold, "thread/resume", json!({"threadId":id}));
        assert_eq!(resumed["configuration"], created["configuration"]);
    }
    fixture.state = Arc::new(Mutex::new(cold));
    fixture.thread_id = source.clone();
    fixture
        .settings
        .set_reasoning_effort(ReasoningEffort::Low)
        .unwrap();
    let original = start(&mut fixture.state.lock().unwrap(), &source, json!({}));
    assert_eq!(original.overrides.reasoning_effort.as_deref(), Some("high"));
    let (response, replacement) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({
                "threadId":source,"turnId":original.turn_id,"input":"keep the thread reasoning"
            }),
        ),
        None,
    );
    assert!(response.is_success(), "{response:?}");
    let replacement = replacement.unwrap();
    assert_eq!(replacement.overrides, original.overrides);
    // Another thread may be admitted concurrently with a different explicit
    // value. Neither it nor later host settings may mutate this accepted turn.
    let normal = thread(&fixture, Some("none"));
    let normal_id = normal["threadId"].as_str().unwrap();
    let normal_launch = start(&mut fixture.state.lock().unwrap(), normal_id, json!({}));
    fixture
        .settings
        .set_reasoning_effort(ReasoningEffort::Max)
        .unwrap();
    let cancelled = execute_with_approvals(&fixture, original).await;
    assert_durable_terminal(&fixture.read(), &cancelled, "cancelled");
    assert!(transport.bodies.lock().unwrap().is_empty());
    let id = execute_with_approvals(&fixture, replacement).await;
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    assert!(fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event["turnId"] == id
            && event["type"] == "item.completed"
            && event["item"]["kind"] == "tool_result"
            && event["item"]["ok"] == true));
    fixture.thread_id = normal_id.to_owned();
    let id = execute_with_approvals(&fixture, normal_launch).await;
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    fixture.thread_id = child;
    let launch = start(
        &mut fixture.state.lock().unwrap(),
        &fixture.thread_id,
        json!({}),
    );
    let id = execute_with_approvals(&fixture, launch).await;
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    let bodies = transport.bodies.lock().unwrap();
    let efforts: Vec<_> = bodies
        .iter()
        .map(|body| body["reasoning"]["effort"].as_str().unwrap())
        .collect();
    assert_eq!(efforts, ["high", "high", "none", "high"]);
}

#[tokio::test]
async fn h222_invalid_thread_effort_is_preserved_and_rejected_before_provider_dispatch() {
    let transport = recorder(vec![completed_text("must not dispatch")]);
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    // H-155 aliases remain a separate issue: never silently drop or replace
    // them with a canonical default while fixing this protocol seam.
    for effort in ["bogus", " high ", "minimal", "medium", "xhigh"] {
        let created = thread(&fixture, Some(effort));
        assert_eq!(
            created["configuration"]["overrides"]["reasoning_effort"],
            effort
        );
        fixture.thread_id = created["threadId"].as_str().unwrap().to_owned();
        let id = fixture.thread_id.as_str();
        let launch = {
            let mut state = fixture.state.lock().unwrap();
            let config = dispatch(&mut state, "config/read", json!({"threadId":id}));
            assert_eq!(config["resolutionStatus"], "invalid");
            let (response, launch) = state.dispatch(
                rpc(
                    "turn/start",
                    json!({"threadId":id,"input":"reject invalid effort"}),
                ),
                None,
            );
            // Existing lifecycle ACKs a turn with an invalid model plan, then
            // persists the executor's pre-provider failure. Do not introduce a
            // thread-only validator or a new admission contract for this fix.
            assert!(response.is_success(), "{response:?}");
            assert_eq!(
                response.result()["capabilitySnapshot"]["context"]["executionConfig"]
                    ["resolutionStatus"],
                "invalid"
            );
            launch.unwrap()
        };
        assert_eq!(launch.overrides.reasoning_effort.as_deref(), Some(effort));
        let failed = execute_with_approvals(&fixture, launch).await;
        assert_durable_terminal(&fixture.read(), &failed, "failed");
        assert!(transport.bodies.lock().unwrap().is_empty());
    }
    // Preserve the shared overlay's established empty-string-as-absent
    // compatibility. The protocol still saves the exact input; it does not
    // claim that an empty string is the canonical explicit "none" value.
    fixture
        .settings
        .set_reasoning_effort(ReasoningEffort::Low)
        .unwrap();
    for effort in ["", "  "] {
        let created = thread(&fixture, Some(effort));
        assert_eq!(
            created["configuration"]["overrides"]["reasoning_effort"],
            effort
        );
        let config = dispatch(
            &mut fixture.state.lock().unwrap(),
            "config/read",
            json!({"threadId":created["threadId"]}),
        );
        assert_eq!(config["reasoningEffort"], "low");
        assert_eq!(config["resolutionStatus"], "resolved");
    }
    let mut state = fixture.state.lock().unwrap();
    let count = state.threads.len();
    for value in [json!(7), json!({"effort":"high"})] {
        let (response, launch) =
            state.dispatch(rpc("thread/start", json!({"reasoningEffort":value})), None);
        assert!(!response.is_success());
        assert!(launch.is_none());
    }
    assert_eq!(state.threads.len(), count);
    assert!(state.turns.values().all(|turn| !turn.active));
    assert!(transport.bodies.lock().unwrap().is_empty());
}
