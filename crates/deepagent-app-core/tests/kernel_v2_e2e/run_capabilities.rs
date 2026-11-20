//! H-219: capability evidence must describe the real registry and provider wire.
use super::*;
use deepagent_app_core::{HarnessRunOverrides, SandboxMode};

#[derive(Default)]
struct CapabilityModel(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for CapabilityModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request: serde_json::Value = serde_json::from_str(&request.body)?;
        let count = {
            let mut requests = self.0.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if count == 1 {
            emit_responses_events(
                sink,
                &response_function_call(
                    "h219-read",
                    "read_file",
                    serde_json::json!({"path":"evidence.txt"}),
                ),
            )
        } else {
            emit_responses_events(sink, &response_text("Capability evidence complete."))
        }
    }
}

async fn settings(db: &Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-capability-placeholder")
        .await
        .unwrap();
    settings.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
    settings
}

async fn run(chat: &ChatService, run_id: &str) -> Result<String> {
    run_profile(chat, run_id, "read_only").await
}

async fn run_profile(chat: &ChatService, run_id: &str, profile: &str) -> Result<String> {
    chat.run_in_session_with_overrides(
        "Read the synthetic evidence file.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(run_id),
        HarnessRunOverrides {
            permission_profile: Some(profile.into()),
            sandbox_backend: Some("direct".into()),
            ..Default::default()
        },
        |_| {},
        |_| {},
    )
    .await
}

async fn check_prepared_input_replacement(
    input: Option<&str>,
    replacement: impl FnOnce(&str) -> String,
) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("evidence.txt"),
        "H219_SYNTHETIC_INPUT_OBSERVATION",
    )
    .unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
    let session = chat.create_session(None).unwrap();
    let foreign = chat.create_session(None).unwrap();
    let overrides = HarnessRunOverrides {
        permission_profile: Some("read_only".into()),
        sandbox_backend: Some("direct".into()),
        ..Default::default()
    };
    let run_id = "h219-input-replacement";
    let prepared = match input {
        Some(input) => chat.admit_prepared_input(run_id, &session, input, overrides),
        None => chat.admit_prepared_run(run_id, &session, overrides),
    }
    .unwrap();
    let initial = prepared.capability_snapshot().clone();
    let store = deepagent_persistence::event_store::EventStore::new(&db);
    let before = store.load_session(session.parse().unwrap()).unwrap();
    let foreign_before = store.load_session(foreign.parse().unwrap()).unwrap();
    let result = chat
        .run_prepared(prepared, &replacement(&foreign), |_| {}, |_| {})
        .await;
    assert!(
        result.is_err(),
        "a prepared execution cannot consume different input or change domain: {result:?}"
    );
    assert!(model.0.lock().unwrap().is_empty());
    let after = store.load_session(session.parse().unwrap()).unwrap();
    assert_eq!(
        after.len(),
        before.len() + 1,
        "only the existing terminal projection may be appended"
    );
    assert_eq!(after[..before.len()], before);
    assert!(matches!(&after.last().unwrap().payload,
        deepagent_core::event::EventPayload::RunTerminal { run_id:Some(owner), kind, .. }
        if owner == run_id && kind == "failed"));
    assert_eq!(
        store.load_session(foreign.parse().unwrap()).unwrap(),
        foreign_before
    );
    let events = chat.run_events(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "capability.snapshot")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .find(|event| event.event_type == "capability.snapshot")
            .unwrap()
            .data,
        initial
    );
    assert!(!events.iter().any(|event| matches!(
        event.event_type.as_str(),
        "tool_started" | "capability_wire_snapshot"
    )));
    let terminals = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "failed");
}

#[tokio::test]
async fn h219_prepared_model_input_rejects_replacement_with_builtin_control() {
    check_prepared_input_replacement(Some("Read the synthetic evidence file."), |_| {
        "/help".into()
    })
    .await;
}

#[tokio::test]
async fn h219_prepared_model_input_rejects_replacement_with_foreign_resume() {
    check_prepared_input_replacement(Some("Read the synthetic evidence file."), |foreign| {
        format!("/resume {foreign}")
    })
    .await;
}

#[tokio::test]
async fn h219_prepared_model_input_rejects_replacement_with_another_plain_prompt() {
    check_prepared_input_replacement(Some("Read the synthetic evidence file."), |_| {
        "A different synthetic input.".into()
    })
    .await;
}

#[tokio::test]
async fn h219_prepared_model_input_without_binding_cannot_switch_to_control() {
    check_prepared_input_replacement(None, |_| "/help".into()).await;
}

#[tokio::test]
async fn h219_prepared_model_input_control_replacement_keeps_the_existing_rejection() {
    check_prepared_input_replacement(Some("/help"), |_| "A synthetic model input.".into()).await;
}

#[tokio::test]
async fn h219_prepared_model_input_accepts_normalized_bound_and_unbound_model_text() {
    for bound in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("evidence.txt"),
            "H219_ACCEPTED_INPUT_OBSERVATION",
        )
        .unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = settings(&db).await;
        let model = Arc::new(CapabilityModel::default());
        let chat = ChatService::new(db, settings, model.clone(), root.path());
        let session = chat.create_session(None).unwrap();
        let overrides = HarnessRunOverrides {
            permission_profile: Some("read_only".into()),
            sandbox_backend: Some("direct".into()),
            ..Default::default()
        };
        let input = "Read the synthetic evidence file.";
        let prepared = if bound {
            chat.admit_prepared_input(
                "h219-valid-input",
                &session,
                &format!(" \n{input}\r\n"),
                overrides,
            )
        } else {
            chat.admit_prepared_run("h219-valid-input", &session, overrides)
        }
        .unwrap();
        let initial = prepared.capability_snapshot().clone();
        assert!(!initial["tools"].as_array().unwrap().is_empty());
        assert_eq!(
            chat.run_prepared(prepared, input, |_| {}, |_| {})
                .await
                .unwrap(),
            session
        );
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].to_string().contains(input));
        assert!(requests[1]
            .to_string()
            .contains("H219_ACCEPTED_INPUT_OBSERVATION"));
        let events = chat.run_events("h219-valid-input", None).unwrap();
        assert_eq!(
            events
                .iter()
                .find(|event| event.event_type == "capability.snapshot")
                .unwrap()
                .data,
            initial
        );
        let terminals = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], "succeeded");
    }
}

#[derive(Default)]
struct SkillCapabilityModel(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for SkillCapabilityModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request = serde_json::from_str(&request.body)?;
        let count = {
            let mut requests = self.0.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if count == 1 {
            emit_responses_events(
                sink,
                &response_function_call(
                    "h219-skill",
                    "skill",
                    serde_json::json!({"id":"snapshot-skill"}),
                ),
            )
        } else {
            emit_responses_events(sink, &response_text("Skill snapshot complete."))
        }
    }
}

async fn check_prepared_skill_snapshot(change_description: bool) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("skills/snapshot-skill");
    std::fs::create_dir_all(&source).unwrap();
    let write_skill = |description: &str, body: &str| {
        std::fs::write(
            source.join("SKILL.md"),
            format!("---\nname: Snapshot Skill\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    };
    write_skill("H219_INITIAL_SKILL_DESCRIPTION", "H219_INITIAL_SKILL_BODY");
    let skills = Arc::new(Mutex::new(
        deepagent_app_core::SkillsService::open_v2(deepagent_skills::SkillsRoots {
            builtin: root.path().join("builtin"),
            marketplace: root.path().join("marketplace"),
            user: root.path().join("user"),
            workspace: Some(root.path().join("skills")),
        })
        .unwrap(),
    ));
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    let model = Arc::new(SkillCapabilityModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path())
        .with_skills(skills.clone());
    let session = chat.create_session(None).unwrap();
    let overrides = || HarnessRunOverrides {
        sandbox_backend: Some("direct".into()),
        permission_profile: Some("read_only".into()),
        ..Default::default()
    };
    let prepared = chat
        .admit_prepared_run("h219-skill-frozen", &session, overrides())
        .unwrap();
    let initial = prepared.capability_snapshot().clone();
    write_skill(
        if change_description {
            "H219_REPLACEMENT_SKILL_DESCRIPTION"
        } else {
            "H219_INITIAL_SKILL_DESCRIPTION"
        },
        "H219_REPLACEMENT_SKILL_BODY",
    );
    skills.lock().unwrap().reload().unwrap();
    chat.run_prepared(
        prepared,
        "Invoke the synthetic snapshot skill.",
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    {
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = requests[0].to_string();
        assert!(first.contains("H219_INITIAL_SKILL_DESCRIPTION"));
        assert!(
            !first.contains("H219_REPLACEMENT_SKILL_DESCRIPTION"),
            "the reminder must describe the admitted skill registry"
        );
        assert!(requests[1].to_string().contains("H219_INITIAL_SKILL_BODY"));
        assert!(!requests[1]
            .to_string()
            .contains("H219_REPLACEMENT_SKILL_BODY"));
    }
    let next = chat
        .admit_prepared_run("h219-skill-next", &session, overrides())
        .unwrap();
    assert_ne!(initial["capabilityHash"], next.capability_snapshot()["capabilityHash"],
        "a changed skill body or enablement is a different capability even with identical tool schemas");
    assert_eq!(
        initial["toolSchemaHash"],
        next.capability_snapshot()["toolSchemaHash"]
    );
    assert!(
        !initial.to_string().contains("H219_INITIAL_SKILL"),
        "capability metadata must not persist skill prompts or bodies"
    );
    chat.request_cancel("h219-skill-next").unwrap();
    chat.run_prepared(next, "must not request another model", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(model.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn h219_skill_reminder_and_execution_consume_the_same_admitted_registry() {
    check_prepared_skill_snapshot(true).await;
}

#[tokio::test]
async fn h219_skill_body_changes_advance_capability_identity_without_schema_changes() {
    check_prepared_skill_snapshot(false).await;
}

#[tokio::test]
async fn h219_plugin_enablement_changes_future_capabilities_not_an_admitted_skill() {
    let root = tempfile::tempdir().unwrap();
    let roots = deepagent_app_core::PluginRoots {
        session: Vec::new(),
        builtin: root.path().join("plugins"),
        workspace: None,
        personal: root.path().join("personal-plugins"),
    };
    let plugin = roots.builtin.join("snapshot-plugin");
    std::fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
    std::fs::create_dir_all(plugin.join("skills/snapshot-skill")).unwrap();
    std::fs::write(
        plugin.join(".codex-plugin/plugin.json"),
        r#"{"name":"snapshot-plugin","version":"1.0.0","skills":"skills"}"#,
    )
    .unwrap();
    std::fs::write(plugin.join("skills/snapshot-skill/SKILL.md"),
        "---\nname: Snapshot Skill\ndescription: H219_FROZEN_PLUGIN_DESCRIPTION\n---\nH219_FROZEN_PLUGIN_BODY").unwrap();
    let plugins = Arc::new(deepagent_app_core::PluginService::new(
        roots,
        root.path().join("state"),
    ));
    let skills = Arc::new(Mutex::new(
        deepagent_app_core::SkillsService::open_v2(deepagent_skills::SkillsRoots {
            builtin: root.path().join("builtin-skills"),
            marketplace: root.path().join("marketplace"),
            user: root.path().join("user-skills"),
            workspace: None,
        })
        .unwrap(),
    ));
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    let model = Arc::new(SkillCapabilityModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path())
        .with_plugins(plugins.clone())
        .with_skills(skills.clone())
        .with_mcp(Arc::new(deepagent_app_core::McpService::new(db.clone())));
    chat.start_plugin_preparation(Arc::new(|| {}));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !skills
            .lock()
            .unwrap()
            .manager()
            .registry()
            .contains("snapshot-skill")
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the synthetic plugin must become ready");
    let session = chat.create_session(None).unwrap();
    let overrides = || HarnessRunOverrides {
        sandbox_backend: Some("direct".into()),
        ..Default::default()
    };
    let admitted = chat
        .admit_prepared_run("h219-plugin-frozen", &session, overrides())
        .unwrap();
    let initial = admitted.capability_snapshot().clone();
    plugins
        .set_enabled("snapshot-plugin@builtin", false)
        .unwrap();
    assert!(plugins.ready_snapshot().0.skill_roots.is_empty());
    chat.run_prepared(
        admitted,
        "Invoke the synthetic plugin skill.",
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    {
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0]
            .to_string()
            .contains("H219_FROZEN_PLUGIN_DESCRIPTION"));
        assert!(requests[1].to_string().contains("H219_FROZEN_PLUGIN_BODY"));
    }
    let next = chat
        .admit_prepared_run("h219-plugin-disabled", &session, overrides())
        .unwrap();
    assert_ne!(
        initial["capabilityHash"],
        next.capability_snapshot()["capabilityHash"]
    );
    assert_ne!(
        initial["context"]["skillCatalogHash"],
        next.capability_snapshot()["context"]["skillCatalogHash"]
    );
    assert_ne!(
        initial["context"]["pluginProjectionHash"],
        next.capability_snapshot()["context"]["pluginProjectionHash"]
    );
    assert_eq!(
        initial["toolSchemaHash"],
        next.capability_snapshot()["toolSchemaHash"]
    );
    assert!(!skills
        .lock()
        .unwrap()
        .manager()
        .registry()
        .contains("snapshot-skill"));
    assert!(!initial.to_string().contains("H219_FROZEN_PLUGIN"));
    chat.request_cancel("h219-plugin-disabled").unwrap();
    chat.run_prepared(next, "Do not dispatch after cancellation.", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(model.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn h219_prepared_run_consumes_admitted_tools_and_environment_once() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("evidence.txt"),
        "H219_PREPARED_OBSERVATION",
    )
    .unwrap();
    let agents = root.path().join(".deepagent").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    let agent = agents.join("h219_initial_agent.md");
    std::fs::write(&agent, "---\nname: h219_initial_agent\ndescription: H219_INITIAL_AGENT_DESCRIPTOR\ntools: Read\n---\nSynthetic test instruction.").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), root.path());
    let session = chat.create_session(None).unwrap();
    let run_id = "h219-prepared-consume";
    let prepared = chat
        .admit_prepared_run(
            run_id,
            &session,
            HarnessRunOverrides {
                permission_profile: Some("read_only".into()),
                sandbox_backend: Some("direct".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let initial = prepared.capability_snapshot().clone();
    assert!(model.0.lock().unwrap().is_empty());
    let before = chat.run_events(run_id, None).unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].event_type, "run_accepted");
    assert_eq!(before[1].data, initial);
    // Both the external source and live settings change after admission. A
    // second registry/environment build would advertise the replacement agent.
    std::fs::remove_file(&agent).unwrap();
    std::fs::write(agents.join("h219_replacement_agent.md"), "---\nname: h219_replacement_agent\ndescription: H219_REPLACEMENT_AGENT_DESCRIPTOR\n---\nSynthetic replacement.").unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    assert_eq!(
        chat.run_prepared(
            prepared,
            "Read the synthetic evidence file.",
            |_| {},
            |_| {}
        )
        .await
        .unwrap(),
        session
    );
    {
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1]
            .to_string()
            .contains("H219_PREPARED_OBSERVATION"));
        let task = requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "task")
            .unwrap();
        assert!(task["description"]
            .as_str()
            .unwrap()
            .contains("H219_INITIAL_AGENT_DESCRIPTOR"));
        assert!(!task["description"]
            .as_str()
            .unwrap()
            .contains("H219_REPLACEMENT_AGENT_DESCRIPTOR"));
    }
    let events = chat.run_events(run_id, None).unwrap();
    let snapshots = events
        .iter()
        .filter(|event| event.event_type == "capability.snapshot")
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].data, initial);
    assert_eq!(initial["context"]["sandboxMode"], "read_only");
    assert!(!initial
        .to_string()
        .contains("H219_INITIAL_AGENT_DESCRIPTOR"));
    let next = chat
        .admit_prepared_run(
            "h219-prepared-next",
            &session,
            HarnessRunOverrides {
                sandbox_backend: Some("direct".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let task_hash = |snapshot: &serde_json::Value| {
        snapshot["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "task")
            .unwrap()["schemaHash"]
            .clone()
    };
    assert_ne!(task_hash(next.capability_snapshot()), task_hash(&initial));
    assert_ne!(
        next.capability_snapshot()["snapshotId"],
        initial["snapshotId"]
    );
    assert_eq!(
        next.capability_snapshot()["context"]["sandboxMode"],
        "full_access"
    );
    chat.request_cancel("h219-prepared-next").unwrap();
    chat.run_prepared(next, "must not request another model", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(model.0.lock().unwrap().len(), 2);
}

#[cfg(feature = "web")]
#[tokio::test]
async fn h219_actual_search_function_wire_hash_is_not_the_internal_catalog_hash() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("evidence.txt"), "synthetic").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_web_search_settings(deepagent_app_core::WebSearchSettings::default())
        .unwrap();
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    run_profile(&chat, "h219-search-function", "full_access")
        .await
        .unwrap();
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let events = chat.run_events("h219-search-function", None).unwrap();
    let host = &events
        .iter()
        .find(|event| event.event_type == "capability.snapshot")
        .unwrap()
        .data;
    let wire = &events
        .iter()
        .find(|event| event.event_type == "capability_wire_snapshot")
        .unwrap()
        .data["wireSchema"];
    let actual = requests[0]["tools"].as_array().unwrap();
    let saved = wire["tools"].as_array().unwrap();
    let catalog = host["tools"].as_array().unwrap();
    assert_eq!(actual.len(), saved.len());
    // H66 always exposes callable search. Responses' flattened function DTO
    // still has a different hash domain from the internal nested catalog DTO.
    let index = saved
        .iter()
        .position(|entry| entry["name"] == "web_search")
        .unwrap();
    assert_eq!(actual[index]["type"], "function");
    assert_eq!(saved[index]["kind"], "function");
    let hash = canonical_schema_hash(&actual[index]);
    assert_eq!(saved[index]["schemaHash"], hash);
    assert_ne!(
        saved[index]["schemaHash"],
        catalog
            .iter()
            .find(|entry| entry["name"] == "web_search")
            .unwrap()["schemaHash"]
    );
    assert_eq!(actual[index]["parameters"]["type"], "object");
    assert!(actual[index]["parameters"]["properties"]["query"].is_object());
    assert!(!actual.iter().any(|tool| tool["type"] == "web_search"));
    assert!(actual
        .iter()
        .any(|tool| tool["type"] == "function" && tool["name"] == "read_file"));
}

#[tokio::test]
async fn h219_wire_durability_failure_prevents_provider_and_tool_execution() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db).await;
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_wire BEFORE INSERT ON run_events WHEN NEW.event_type='capability_wire_snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic wire failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    assert!(run(&chat, "h219-wire-failure").await.is_err());
    assert!(model.0.lock().unwrap().is_empty());
    let events = chat.run_events("h219-wire-failure", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "capability.snapshot")
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.event_type.as_str(),
        "tool_started" | "capability_wire_snapshot"
    )));
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "failed");
}

#[tokio::test]
async fn h219_actual_registry_snapshot_matches_wire_and_survives_database_reopen() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("evidence.txt"),
        "H219_SYNTHETIC_OBSERVATION",
    )
    .unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = settings(&db).await;
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), root.path());
    let run_id = "h219-actual-snapshot";
    let session = run(&chat, run_id).await.unwrap();
    let events = chat.run_events(run_id, None).unwrap();
    let snapshots: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "capability.snapshot")
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "one real run-owned capability snapshot, not a global catalog"
    );
    let snapshot = &snapshots[0].data;
    assert_eq!(snapshot["version"], 1);
    assert_eq!(snapshot["runId"], run_id);
    assert_eq!(snapshot["threadId"], session);
    assert_eq!(snapshot["context"]["sandboxBackend"], "direct");
    assert_eq!(snapshot["context"]["sandboxMode"], "read_only");
    assert_eq!(
        snapshot["context"]["workspace"],
        std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let requests = model.0.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "one actual read and a completing model turn"
    );
    assert!(requests[1]
        .to_string()
        .contains("H219_SYNTHETIC_OBSERVATION"));
    let wire = events
        .iter()
        .find(|event| event.event_type == "capability_wire_snapshot")
        .expect("the actual adapter wire schemas need their own explicitly named hash domain");
    assert_eq!(wire.data["snapshotId"], snapshot["snapshotId"]);
    assert_eq!(wire.data["wireSchema"]["generation"], 0);
    assert_eq!(wire.data["wireSchema"]["wireMode"], "responses");
    assert_eq!(wire.data["wireSchema"]["model"], requests[0]["model"]);
    assert_eq!(
        wire.data["wireSchema"]["schemaHash"],
        canonical_schema_hash(&requests[0]["tools"])
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "capability_wire_snapshot")
            .count(),
        1,
        "unchanged schemas and route must reuse the same generation"
    );
    let projected = snapshot["tools"].as_array().unwrap();
    let wire_refs = wire.data["wireSchema"]["tools"].as_array().unwrap();
    let source_refs = snapshot["modelSchema"]["tools"].as_array().unwrap();
    let actual_tools = requests[0]["tools"].as_array().unwrap();
    assert_eq!(actual_tools.len(), wire_refs.len());
    assert_eq!(actual_tools.len(), source_refs.len());
    for ((tool, wire_ref), source_ref) in actual_tools.iter().zip(wire_refs).zip(source_refs) {
        assert_eq!(wire_ref["name"], source_ref["name"]);
        assert_eq!(wire_ref["kind"], tool["type"]);
        assert_eq!(wire_ref["schemaHash"], canonical_schema_hash(tool));
        let entry = projected
            .iter()
            .find(|item| item["name"] == source_ref["name"])
            .expect("wire tool belongs to saved registry");
        assert_eq!(entry["schemaHash"], source_ref["schemaHash"]);
        if tool["type"] == "function" {
            let schema = serde_json::json!({"type":"function", "function":{
                "name":tool["name"], "description":tool["description"], "parameters":tool["parameters"]
            }});
            let hash = canonical_schema_hash(&schema);
            assert_eq!(entry["schemaHash"], hash);
        } else {
            assert_ne!(entry["schemaHash"], wire_ref["schemaHash"]);
        }
        assert_eq!(entry["initiallyVisible"], true);
        assert!(entry["risk"].is_string());
        assert!(entry["requiredPermissions"].is_array());
        assert!(entry["source"].is_string());
        assert!(entry["approval"]["evaluation"].is_string());
    }
    assert!(projected.iter().any(|t| t["name"] == "task"));
    assert!(projected.iter().any(|t| t["name"] == "snip_history"));
    assert!(!snapshot
        .to_string()
        .contains("offline-capability-placeholder"));
    let saved = snapshot.clone();
    let saved_wire = wire.data.clone();
    drop(requests);
    drop(chat);
    drop(settings);
    assert_eq!(Arc::strong_count(&db), 1);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let stored = deepagent_persistence::run_store::RunStore::new(&reopened)
        .events_after(run_id, None)
        .unwrap();
    assert_eq!(
        stored
            .iter()
            .find(|e| e.event_type == "capability.snapshot")
            .unwrap()
            .data,
        saved
    );
    assert_eq!(
        stored
            .iter()
            .find(|event| event.event_type == "capability_wire_snapshot")
            .unwrap()
            .data,
        saved_wire
    );
    assert_eq!(
        stored
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        model.0.lock().unwrap().len(),
        2,
        "history reads must not redispatch tools or models"
    );
}

#[tokio::test]
async fn h219_snapshot_durability_failure_prevents_model_and_tool_execution() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = settings(&db).await;
    db.with_conn(|connection| {
        connection.execute_batch("CREATE TRIGGER reject_capabilities BEFORE INSERT ON run_events WHEN NEW.event_type='capability.snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic snapshot failure'); END;")
            .map_err(|error| CoreError::Persistence(error.to_string()))
    }).unwrap();
    let model = Arc::new(CapabilityModel::default());
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    let outcome = run(&chat, "h219-snapshot-failure").await;
    assert!(
        outcome.is_err(),
        "a run without durable capability evidence must not succeed"
    );
    assert!(
        model.0.lock().unwrap().is_empty(),
        "no provider request before the snapshot durability barrier"
    );
    let events = chat.run_events("h219-snapshot-failure", None).unwrap();
    assert!(!events.iter().any(|e| e.event_type == "tool_started"));
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "failed");
}
