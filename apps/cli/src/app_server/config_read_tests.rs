//! H-221: real dispatcher, persisted threads and recording provider transport.
use super::*;

fn read(state: &mut ServerState, params: Value) -> Value {
    dispatch(state, "config/read", params)
}

#[tokio::test]
async fn h89_thread_models_turn_overrides_and_unknowns_match_snapshot_wire_and_failed_terminal() {
    let transport = Arc::new(WireTransport {
        bodies: Mutex::new(Vec::new()),
        script: ScriptedTransport {
            responses: vec![completed_text("synthetic model choice")],
            next_response: AtomicUsize::new(0),
        },
    });
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    let created = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/start",
        json!({"cwd":fixture.workspace,"model":"deepseek-pro"}),
    );
    fixture.thread_id = created["threadId"].as_str().unwrap().into();
    for (extra, expected) in [
        (json!({}), Some("deepseek-pro")),
        (json!({"model":"deepseek-flash"}), Some("deepseek-flash")),
        (json!({"model":"h89-unknown"}), None),
        (json!({"model":"sonnet"}), None),
        (json!({}), Some("deepseek-pro")),
    ] {
        let before = transport.bodies.lock().unwrap().len();
        let launch = start(
            &mut fixture.state.lock().unwrap(),
            &fixture.thread_id,
            extra,
        );
        let context = &launch.prepared.capability_snapshot()["context"]["executionConfig"];
        assert_eq!(
            context["resolutionStatus"],
            if expected.is_some() {
                "resolved"
            } else {
                "invalid"
            }
        );
        assert_eq!(context["model"], json!(expected));
        let id = execute_with_approvals(&fixture, launch).await;
        assert_durable_terminal(
            &fixture.read(),
            &id,
            if expected.is_some() {
                "succeeded"
            } else {
                "failed"
            },
        );
        let bodies = transport.bodies.lock().unwrap();
        if let Some(expected) = expected {
            assert_eq!(bodies.len(), before + 1);
            assert_eq!(bodies.last().unwrap()["model"], expected);
        } else {
            assert_eq!(
                bodies.len(),
                before,
                "unknown selection must not bind or dispatch a replacement model"
            );
        }
        let persisted = dispatch(
            &mut fixture.state.lock().unwrap(),
            "thread/resume",
            json!({"threadId":fixture.thread_id}),
        );
        assert_eq!(
            persisted["configuration"]["overrides"]["model"],
            "deepseek-pro"
        );
    }
    let invalid = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/start",
        json!({"cwd":fixture.workspace,"model":"h89-unknown-thread"}),
    );
    fixture.thread_id = invalid["threadId"].as_str().unwrap().into();
    let config = read(
        &mut fixture.state.lock().unwrap(),
        json!({"threadId":fixture.thread_id}),
    );
    assert_eq!(config["resolutionStatus"], "invalid");
    let before = transport.bodies.lock().unwrap().len();
    let launch = start(
        &mut fixture.state.lock().unwrap(),
        &fixture.thread_id,
        json!({}),
    );
    let id = execute_with_approvals(&fixture, launch).await;
    assert_durable_terminal(&fixture.read(), &id, "failed");
    assert_eq!(transport.bodies.lock().unwrap().len(), before);
    let launch = start(
        &mut fixture.state.lock().unwrap(),
        &fixture.thread_id,
        json!({"model":"deepseek-flash"}),
    );
    let id = execute_with_approvals(&fixture, launch).await;
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    assert_eq!(
        transport.bodies.lock().unwrap().last().unwrap()["model"],
        "deepseek-flash"
    );
}

#[tokio::test]
async fn h89_unknown_project_model_is_reported_invalid_but_known_thread_selection_wins() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let path = fixture.workspace.join(".deepagent/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, r#"{"model":"not-in-catalog"}"#).unwrap();
    let mut state = fixture.state.lock().unwrap();
    let config = read(&mut state, json!({"threadId":fixture.thread_id}));
    assert_eq!(config["resolutionStatus"], "invalid");
    assert!(config["model"].is_null());
    assert!(
        !config.to_string().contains("not-in-catalog"),
        "read-only diagnostics retain redaction"
    );
    let known = configured_thread(&mut state, &fixture.workspace);
    let config = read(&mut state, json!({"threadId":known["threadId"]}));
    assert_eq!(config["resolutionStatus"], "resolved");
    assert_eq!(config["model"], "deepseek-flash");
}

#[tokio::test]
async fn h221_scoped_configuration_is_effective_redacted_and_read_only() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let project = fixture.workspace.join(".deepagent");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("settings.json"),
        json!({
            "model":"deepseek-flash", "permission_profile":"full_access",
            "instructions":"H221-PRIVATE-INSTRUCTIONS", "api_key":"H221-SYNTHETIC-SECRET"
        })
        .to_string(),
    )
    .unwrap();
    fixture
        .settings
        .set_reasoning_effort(deepagent_models::ReasoningEffort::Low)
        .unwrap();
    let mut state = fixture.state.lock().unwrap();
    let runs_before = RunStore::new(&state.database)
        .list_for_session(&fixture.thread_id)
        .unwrap()
        .len();
    let global = read(&mut state, json!({}));
    assert_eq!(global["configSnapshotVersion"], 1);
    assert_eq!(global["scope"]["kind"], "global");
    assert_eq!(global["scope"]["requiresThreadScope"], true);
    assert_eq!(global["model"], "deepseek-flash");
    assert_eq!(global["modelOverrideApplied"], true);
    assert_eq!(global["reasoningEffort"], "low");
    assert_eq!(global["projectTrusted"], false);
    assert_eq!(
        global["permissionProfile"]["sandboxMode"],
        "workspace_write"
    );
    let created = configured_thread(&mut state, &fixture.workspace);
    let thread_id = created["threadId"].as_str().unwrap();
    let scoped = read(&mut state, json!({"threadId":thread_id}));
    assert_eq!(scoped["scope"]["kind"], "thread");
    assert_eq!(scoped["model"], "deepseek-flash");
    assert_eq!(scoped["permissionProfile"]["sandboxMode"], "read_only");
    assert_eq!(scoped["sandboxBackend"], "direct");
    assert_ne!(global["configSnapshotId"], scoped["configSnapshotId"]);
    let cwd = read(&mut state, json!({"cwd":fixture.workspace}));
    assert_eq!(cwd["scope"]["kind"], "cwd");
    assert_eq!(cwd["configSnapshotId"], global["configSnapshotId"]);
    let encoded = scoped.to_string();
    for private in [
        "H221-PRIVATE-INSTRUCTIONS",
        "H221-SYNTHETIC-SECRET",
        "offline-audit-placeholder",
        fixture.workspace.to_str().unwrap(),
    ] {
        assert!(
            !encoded.contains(private),
            "config/read leaked a private value"
        );
    }
    assert_eq!(
        RunStore::new(&state.database)
            .list_for_session(&fixture.thread_id)
            .unwrap()
            .len(),
        runs_before
    );
    assert!(state.turns.is_empty());
    assert_eq!(read(&mut state, json!({"threadId":thread_id})), scoped);
    drop(state);
    let mut cold = fixture.reopened_state();
    // Cold read must use the authoritative session configuration without
    // loading a ThreadState or admitting a run.
    assert!(!cold.threads.contains_key(thread_id));
    assert_eq!(read(&mut cold, json!({"threadId":thread_id})), scoped);
    assert!(!cold.threads.contains_key(thread_id));
    let child = dispatch(
        &mut cold,
        "thread/fork",
        json!({"threadId":thread_id,"atSequence":0}),
    );
    let child_config = read(&mut cold, json!({"threadId":child["threadId"]}));
    assert_eq!(child_config["configSnapshotId"], scoped["configSnapshotId"]);
    assert_eq!(
        child_config["permissionProfile"],
        scoped["permissionProfile"]
    );
}

#[tokio::test]
async fn h221_admission_guard_rejects_stale_configuration_before_publishing_run() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let mut state = fixture.state.lock().unwrap();
    let before = read(&mut state, json!({"threadId":fixture.thread_id}));
    fixture
        .settings
        .set_reasoning_effort(deepagent_models::ReasoningEffort::Max)
        .unwrap();
    let count = RunStore::new(&state.database)
        .list_for_session(&fixture.thread_id)
        .unwrap()
        .len();
    let (response, launch) = state.dispatch(
        rpc(
            "turn/start",
            json!({
                "threadId":fixture.thread_id, "input":"ordinary synthetic request",
                "expectedConfigSnapshotId":before["configSnapshotId"]
            }),
        ),
        None,
    );
    assert!(
        !response.is_success(),
        "stale config/read must not grant admission"
    );
    assert!(launch.is_none());
    assert!(state.turns.is_empty());
    assert_eq!(
        RunStore::new(&state.database)
            .list_for_session(&fixture.thread_id)
            .unwrap()
            .len(),
        count
    );
    let after = read(&mut state, json!({"threadId":fixture.thread_id}));
    assert_ne!(before["configSnapshotId"], after["configSnapshotId"]);
    assert_eq!(after["reasoningEffort"], "max");
}

#[tokio::test]
async fn h221_admitted_snapshot_is_consumed_by_real_provider_after_settings_change() {
    let transport = Arc::new(WireTransport {
        bodies: Mutex::new(Vec::new()),
        script: ScriptedTransport {
            responses: vec![
                function_call("h221-read", "read_file", json!({"path":"h221.txt"})),
                completed_text("frozen model"),
            ],
            next_response: AtomicUsize::new(0),
        },
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    std::fs::write(fixture.workspace.join("h221.txt"), "synthetic normal read").unwrap();
    fixture
        .settings
        .set_reasoning_effort(deepagent_models::ReasoningEffort::Low)
        .unwrap();
    let config = read(
        &mut fixture.state.lock().unwrap(),
        json!({"threadId":fixture.thread_id}),
    );
    let (response, launch) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({
                "threadId":fixture.thread_id,"input":"run the frozen configuration",
                "expectedConfigSnapshotId":config["configSnapshotId"]
            }),
        ),
        None,
    );
    assert!(response.is_success(), "{response:?}");
    assert_eq!(
        response.result()["capabilitySnapshot"]["context"]["executionConfig"]["configSnapshotId"],
        config["configSnapshotId"]
    );
    fixture
        .settings
        .harness_models()
        .select_model("deepseek", "deepseek-pro")
        .unwrap();
    fixture
        .settings
        .set_reasoning_effort(deepagent_models::ReasoningEffort::Max)
        .unwrap();
    let turn_id = execute_with_approvals(&fixture, launch.unwrap()).await;
    assert_durable_terminal(&fixture.read(), &turn_id, "succeeded");
    let bodies = transport.bodies.lock().unwrap();
    assert_eq!(config["modelOverrideApplied"], false);
    assert_eq!(bodies.len(), 2);
    for body in bodies.iter() {
        assert_eq!(body["model"], config["model"]);
        assert_eq!(body["reasoning"]["effort"], config["reasoningEffort"]);
    }
}

#[tokio::test]
async fn h221_concurrent_settings_writer_and_guarded_admission_never_publish_a_mixed_snapshot() {
    let fixture = Fixture::new(completed_text("unused")).await;
    for index in 0..4 {
        let before = read(
            &mut fixture.state.lock().unwrap(),
            json!({"threadId":fixture.thread_id}),
        );
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let settings = fixture.settings.clone();
        let writer_barrier = barrier.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            settings
                .set_reasoning_effort(if index % 2 == 0 {
                    deepagent_models::ReasoningEffort::Low
                } else {
                    deepagent_models::ReasoningEffort::Max
                })
                .unwrap();
        });
        barrier.wait();
        let (response, launch) = fixture.state.lock().unwrap().dispatch(
            rpc(
                "turn/start",
                json!({
                    "threadId":fixture.thread_id,"input":"concurrent synthetic admission",
                    "expectedConfigSnapshotId":before["configSnapshotId"]
                }),
            ),
            None,
        );
        writer.join().unwrap();
        if let Some(launch) = launch {
            assert!(response.is_success());
            let actual = &response.result()["capabilitySnapshot"]["context"]["executionConfig"];
            assert_eq!(actual["configSnapshotId"], before["configSnapshotId"]);
            assert_eq!(actual["reasoningEffort"], before["reasoningEffort"]);
            finish_cancelled_metadata_launch(&fixture.state, launch).await;
        } else {
            assert_eq!(response.error_code(), Some(ERR_INVALID_PARAMS));
            assert!(!response.is_success());
        }
    }
}

#[tokio::test]
async fn h221_project_trust_and_policy_changes_invalidate_the_same_admission_guard() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let config_dir = fixture.workspace.join(".deepagent");
    std::fs::create_dir(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("settings.json"),
        r#"{"permission_profile":"read_only"}"#,
    )
    .unwrap();
    let mut state = fixture.state.lock().unwrap();
    let untrusted = read(&mut state, json!({"threadId":fixture.thread_id}));
    assert_eq!(
        untrusted["permissionProfile"]["sandboxMode"],
        "workspace_write"
    );
    let trust = deepagent_app_core::trust_service::TrustService::new(state.database.clone());
    trust.set_trusted(&fixture.workspace).unwrap();
    let trusted = read(&mut state, json!({"threadId":fixture.thread_id}));
    assert_eq!(trusted["projectTrusted"], true);
    assert_eq!(trusted["permissionProfile"]["sandboxMode"], "read_only");
    assert_ne!(trusted["configSnapshotId"], untrusted["configSnapshotId"]);
    let (response, launch) = state.dispatch(
        rpc(
            "turn/start",
            json!({
                "threadId":fixture.thread_id,"input":"trust changed",
                "expectedConfigSnapshotId":untrusted["configSnapshotId"]
            }),
        ),
        None,
    );
    assert!(!response.is_success());
    assert!(launch.is_none());
    trust.set_untrusted(&fixture.workspace).unwrap();
    assert_eq!(
        read(&mut state, json!({"threadId":fixture.thread_id})),
        untrusted
    );
    std::fs::write(
        config_dir.join("settings.json"),
        r#"{"sandbox_backend":"direct"}"#,
    )
    .unwrap();
    let changed = read(&mut state, json!({"threadId":fixture.thread_id}));
    assert_ne!(changed["configSnapshotId"], untrusted["configSnapshotId"]);
    assert_eq!(changed["sandboxBackend"], "direct");
}

#[tokio::test]
async fn h221_invalid_scopes_and_policy_diagnostics_do_not_disclose_private_values() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let mut state = fixture.state.lock().unwrap();
    for params in [
        json!({"threadId":fixture.thread_id,"cwd":fixture.workspace}),
        json!({"threadId":"H221-PRIVATE-NONEXISTENT"}),
        json!({"cwd":fixture.workspace.join("H221-PRIVATE-NONEXISTENT")}),
    ] {
        let (response, launch) = state.dispatch(rpc("config/read", params), None);
        assert!(!response.is_success());
        assert!(launch.is_none());
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("H221-PRIVATE-NONEXISTENT"));
        assert!(!encoded.contains(fixture.workspace.to_str().unwrap()));
    }
    let config_dir = fixture.workspace.join(".deepagent");
    std::fs::create_dir(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("settings.json"),
        r#"{"sandbox_backend":"H221-PRIVATE-POLICY"}"#,
    )
    .unwrap();
    let (response, _) = state.dispatch(rpc("config/read", json!({})), None);
    assert!(!response.is_success());
    assert!(!serde_json::to_string(&response)
        .unwrap()
        .contains("H221-PRIVATE-POLICY"));
    std::fs::write(
        config_dir.join("settings.json"),
        r#"{"provider":"H221-PRIVATE-PROVIDER"}"#,
    )
    .unwrap();
    let invalid = read(&mut state, json!({}));
    assert_eq!(invalid["resolutionStatus"], "invalid");
    assert!(invalid["model"].is_null());
    assert!(!invalid.to_string().contains("H221-PRIVATE-PROVIDER"));
}

struct NoSecrets;
impl deepagent_app_core::secret_store::SecretStore for NoSecrets {
    fn set(&self, _: &str, _: &str) -> CoreResult<()> {
        panic!("config/read must not write secrets")
    }
    fn get(&self, _: &str) -> CoreResult<Option<String>> {
        panic!("config/read must not read secrets")
    }
    fn delete(&self, _: &str) -> CoreResult<()> {
        panic!("config/read must not delete secrets")
    }
}

#[tokio::test]
async fn h221_read_does_not_bind_credentials_or_start_configured_mcp() {
    let transport = Arc::new(WireTransport {
        bodies: Mutex::new(Vec::new()),
        script: ScriptedTransport {
            responses: vec![],
            next_response: AtomicUsize::new(0),
        },
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let db = fixture.state.lock().unwrap().database.clone();
    let mcp = Arc::new(deepagent_app_core::McpService::new(db.clone()));
    mcp.upsert(
        serde_json::from_value(json!({
            "name":"h221-must-not-start","transport":"stdio","enabled":true,
            "command":"h221-synthetic-command-that-does-not-exist", "args":[],"env":{}
        }))
        .unwrap(),
    )
    .unwrap();
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        transport.clone(),
        Arc::new(NoSecrets),
    ));
    let chat = ChatService::new(db, settings, transport.clone(), fixture.workspace.clone())
        .with_mcp(mcp.clone());
    let first = chat
        .read_execution_config(&HarnessRunOverrides::default())
        .unwrap();
    assert_eq!(first["credentialChecked"], false);
    assert_eq!(first["resolutionStatus"], "resolved");
    assert!(first["mcpGeneration"].is_string());
    assert_eq!(
        chat.read_execution_config(&HarnessRunOverrides::default())
            .unwrap(),
        first
    );
    assert!(mcp.ready_registry().server_names().is_empty());
    assert!(transport.bodies.lock().unwrap().is_empty());
}
