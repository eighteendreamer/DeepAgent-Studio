//! Plan restrictions must survive recovering the same persisted session.

use super::*;
use serde_json::{json, Value};
use std::path::Path;

const WRITTEN: &str = "synthetic-plan-lifecycle-write\n";

#[derive(Default)]
struct PlanModel(Mutex<Vec<Value>>, Vec<PlanOperation>);

enum PlanOperation {
    Select(bool),
    Write(&'static str),
}

#[async_trait]
impl HttpTransport for PlanModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let step = {
            let mut requests = self.0.lock().unwrap();
            requests.push(body);
            requests.len() - 1
        };
        let action = step / 2;
        let item = if let Some(operation) = self.1.get(step) {
            match operation {
                PlanOperation::Select(active) => super::responses_fixture::call(
                    &format!("audit-mode-{step}"),
                    if *active {
                        "enter_plan_mode"
                    } else {
                        "exit_plan_mode"
                    },
                    json!({}),
                ),
                PlanOperation::Write(path) => super::responses_fixture::call(
                    &format!("audit-mode-{step}"),
                    "write_file",
                    json!({"path":path,"content":WRITTEN}),
                ),
            }
        } else if self.1.is_empty() && step % 2 == 0 {
            super::responses_fixture::call(
                &format!("audit-plan-write-{action}"),
                "write_file",
                json!({"path":format!("generated-{action}.txt"),"content":WRITTEN}),
            )
        } else {
            super::responses_fixture::text(
                &format!("audit-plan-answer-{action}"),
                "The supplied tool result has been observed.",
            )
        };
        let mut events = super::responses_fixture::completed(
            &format!("audit-plan-response-{step}"),
            "deepseek-flash",
            item.clone(),
            json!({"input_tokens":10,"output_tokens":4,"total_tokens":14}),
        );
        if item["type"] == "message" {
            events.insert(
                2,
                json!({"type":"response.output_text.delta",
                "item_id":item["id"],"output_index":0,"content_index":0,
                "delta":item["content"][0]["text"]})
                .to_string(),
            );
        }
        for (sequence, event) in events.iter_mut().enumerate() {
            let mut value: Value = serde_json::from_str(event).unwrap();
            value["sequence_number"] = sequence.into();
            *event = value.to_string();
        }
        emit_responses_events(sink, &events)
    }
}

fn settings(db: Arc<Database>, secrets: Arc<MemorySecretStore>) -> Arc<SettingsService> {
    Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        secrets,
    ))
}

#[derive(Debug)]
struct WriteObservation {
    mode: bool,
    ok: bool,
    written: bool,
    prior_plan_in_wire: bool,
}

async fn attempt(
    chat: &ChatService,
    model: &PlanModel,
    workspace: &Path,
    session: &str,
    run: &str,
) -> WriteObservation {
    let start = model.0.lock().unwrap().len();
    assert_eq!(start % 2, 0);
    let action = start / 2;
    let path = workspace.join(format!("generated-{action}.txt"));
    assert!(!path.exists());
    let mode = chat.is_plan_mode(session).unwrap();
    let actual_session = chat
        .run_in_session(
            "Continue the supplied controlled local audit and report the tool result.",
            Some(session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(actual_session, session);
    let events = chat.run_events(run, None).unwrap();
    let terminals = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), start + 2);
    let call = format!("audit-plan-write-{action}");
    let input = requests[start + 1]["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|i| i["type"] == "function_call" && i["call_id"] == call)
            .count(),
        1
    );
    let outputs = input
        .iter()
        .filter(|i| i["type"] == "function_call_output" && i["call_id"] == call)
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    let result: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    let ok = result["status"] == "ok";
    if !ok {
        assert_eq!(result["status"], "error");
        assert!(result.to_string().contains("Plan mode"));
    }
    let prior_plan_in_wire = requests[start]["input"]
        .to_string()
        .contains("Entered Plan mode");
    let written = path.exists();
    if written {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WRITTEN);
    }
    assert_eq!(ok, written, "tool result must match the actual filesystem");
    WriteObservation {
        mode,
        ok,
        written,
        prior_plan_in_wire,
    }
}

async fn observe_reopen(slash: bool, reapply: bool) -> WriteObservation {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db_path = temp.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let model = Arc::new(PlanModel::default());
    let session = {
        let db = Arc::new(Database::open(&db_path).unwrap());
        let config = settings(db.clone(), secrets.clone());
        crate::registry_fixture::configure(&config, "offline-audit-placeholder")
            .await
            .unwrap();
        config
            .harness_models()
            .select_model("deepseek", "deepseek-flash")
            .unwrap();
        let chat = ChatService::new(db.clone(), config.clone(), model.clone(), &workspace);
        let session = if slash {
            chat.run("/plan", |_| {}, |_| {}).await.unwrap()
        } else {
            let session = chat.create_session(None).unwrap();
            assert!(chat.set_plan_mode(&session, true).unwrap());
            session
        };
        assert_eq!(model.0.lock().unwrap().len(), 0);
        for run in ["audit-plan-warm-1", "audit-plan-warm-2"] {
            let warm = attempt(&chat, &model, &workspace, &session, run).await;
            assert!(warm.mode);
            assert!(!warm.ok && !warm.written);
            assert_eq!(warm.prior_plan_in_wire, slash);
        }
        assert!(
            chat.wait_for_session_memory_passes(std::time::Duration::from_secs(1))
                .await
        );
        drop(chat);
        drop(config);
        assert_eq!(
            Arc::strong_count(&db),
            1,
            "the original database must close"
        );
        session
    };
    let db = Arc::new(Database::open(&db_path).unwrap());
    let config = settings(db.clone(), secrets);
    assert_eq!(
        config
            .harness_models()
            .selected_model()
            .unwrap()
            .unwrap()
            .model_name,
        "deepseek-flash"
    );
    let chat = ChatService::new(db, config, model.clone(), &workspace);
    let cold_mode_before_reapply = chat.is_plan_mode(&session).unwrap();
    if reapply {
        assert!(chat.set_plan_mode(&session, true).unwrap());
    }
    let cold = attempt(&chat, &model, &workspace, &session, "audit-plan-cold").await;
    assert_eq!(cold.prior_plan_in_wire, slash);
    eprintln!("plan lifecycle: slash={slash} reapply={reapply} cold_mode_before_reapply={cold_mode_before_reapply} observation={cold:?}");
    cold
}

#[tokio::test]
async fn plan_lifecycle_explicit_reapply_and_exit_controls() {
    for slash in [true, false] {
        let cold = observe_reopen(slash, true).await;
        assert!(cold.mode && !cold.ok && !cold.written);
    }
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(temp.path().join("state.sqlite")).unwrap());
    let config = settings(db.clone(), Arc::new(MemorySecretStore::new()));
    crate::registry_fixture::configure(&config, "offline-audit-placeholder")
        .await
        .unwrap();
    let model = Arc::new(PlanModel::default());
    let chat = ChatService::new(db, config, model.clone(), temp.path());
    let session = chat.run("/plan", |_| {}, |_| {}).await.unwrap();
    chat.run_in_session(
        "/execute",
        Some(&session),
        None,
        None,
        Vec::new(),
        None,
        false,
        None,
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let exited = attempt(
        &chat,
        &model,
        temp.path(),
        &session,
        "audit-plan-explicit-exit",
    )
    .await;
    assert!(!exited.mode && exited.ok && exited.written);
    let unrelated = chat.create_session(None).unwrap();
    assert!(chat.set_plan_mode(&session, true).unwrap());
    let normal = attempt(
        &chat,
        &model,
        temp.path(),
        &unrelated,
        "audit-plan-other-session",
    )
    .await;
    assert!(!normal.mode && normal.ok && normal.written);
}

#[tokio::test]
async fn audit_plan_mode_survives_database_and_service_reopen() {
    let mut observations = Vec::new();
    for slash in [true, false] {
        observations.push(observe_reopen(slash, false).await);
    }
    assert!(
        observations.iter().all(|v| v.mode && !v.ok && !v.written),
        "recovering the same session silently removed its plan restriction"
    );
}

#[tokio::test]
async fn plan_lifecycle_exit_does_not_grant_read_only_write_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(temp.path().join("state.sqlite")).unwrap());
    let config = settings(db.clone(), Arc::new(MemorySecretStore::new()));
    crate::registry_fixture::configure(&config, "offline-audit-placeholder")
        .await
        .unwrap();
    let model = Arc::new(PlanModel(
        Mutex::new(Vec::new()),
        vec![
            PlanOperation::Select(false),
            PlanOperation::Write("must-stay-absent.txt"),
        ],
    ));
    let chat = ChatService::new(db, config, model.clone(), temp.path());
    let id = chat.create_session(None).unwrap();
    assert!(chat.set_plan_mode(&id, true).unwrap());
    chat.run_in_session_with_overrides(
        "Execute the supplied synthetic permission control.",
        Some(&id),
        None,
        None,
        Vec::new(),
        None,
        false,
        None,
        deepagent_app_core::HarnessRunOverrides {
            permission_profile: Some("read_only".into()),
            ..Default::default()
        },
        |_| {},
        |_| panic!("read_only denial must not be an approval grant"),
    )
    .await
    .unwrap();
    assert!(!chat.is_plan_mode(&id).unwrap());
    assert!(!temp.path().join("must-stay-absent.txt").exists());
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let outputs: Vec<_> = requests[2]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == "audit-mode-1")
        .collect();
    assert_eq!(outputs.len(), 1);
    let result: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    assert_eq!(result["status"], "error");
    assert_eq!(result["error"]["error_type"], "hook_denied");
    assert_eq!(
        result["error"]["error"],
        "blocked by hook: modify is blocked by the current sandbox mode: 'must-stay-absent.txt'"
    );
}

#[tokio::test]
async fn plan_lifecycle_model_tools_commit_before_the_next_write_gate() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(temp.path().join("state.sqlite")).unwrap());
    let config = settings(db.clone(), Arc::new(MemorySecretStore::new()));
    crate::registry_fixture::configure(&config, "offline-audit-placeholder")
        .await
        .unwrap();
    let model = Arc::new(PlanModel(
        Mutex::new(Vec::new()),
        vec![
            PlanOperation::Select(true),
            PlanOperation::Write("while-active.txt"),
            PlanOperation::Select(false),
            PlanOperation::Write("after-exit.txt"),
        ],
    ));
    let chat = ChatService::new(db.clone(), config, model.clone(), temp.path());
    let id = chat
        .run(
            "Execute the supplied synthetic sequence and report tool results.",
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    assert!(!chat.is_plan_mode(&id).unwrap());
    assert!(!temp.path().join("while-active.txt").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("after-exit.txt")).unwrap(),
        WRITTEN
    );
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), 5);
    for (index, ok) in [true, false, true, true].into_iter().enumerate() {
        let call = format!("audit-mode-{index}");
        let outputs: Vec<_> = requests[index + 1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == call)
            .collect();
        assert_eq!(outputs.len(), 1);
        let output: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
        assert_eq!(output["status"] == "ok", ok);
    }
    let changes: Vec<_> = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            deepagent_core::event::EventPayload::PlanModeChanged { version: 1, active } => {
                Some(active)
            }
            _ => None,
        })
        .collect();
    assert_eq!(changes, [true, false]);
}

#[tokio::test]
async fn plan_lifecycle_initial_flag_is_durable_and_missing_state_fails_before_model_io() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let model = Arc::new(PlanModel::default());
    let secrets = Arc::new(MemorySecretStore::new());
    let id = {
        let db = Arc::new(Database::open(&path).unwrap());
        let config = settings(db.clone(), secrets.clone());
        crate::registry_fixture::configure(&config, "offline-audit-placeholder")
            .await
            .unwrap();
        let chat = ChatService::new(db.clone(), config.clone(), model.clone(), temp.path());
        let id = chat
            .run_in_session(
                "Try the supplied synthetic write and report the result.",
                None,
                None,
                None,
                Vec::new(),
                None,
                true,
                None,
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        assert!(chat.is_plan_mode(&id).unwrap());
        assert!(!temp.path().join("generated-0.txt").exists());
        assert!(
            chat.wait_for_session_memory_passes(std::time::Duration::from_secs(1))
                .await
        );
        drop(chat);
        drop(config);
        assert_eq!(Arc::strong_count(&db), 1);
        id
    };
    let db = Arc::new(Database::open(&path).unwrap());
    let config = settings(db.clone(), secrets);
    let chat = ChatService::new(db.clone(), config, model.clone(), temp.path());
    let cold = attempt(&chat, &model, temp.path(), &id, "initial-flag-cold").await;
    assert!(cold.mode && !cold.written && !cold.ok);
    let count = model.0.lock().unwrap().len();
    deepagent_persistence::event_store::EventStore::new(&db)
        .append(
            id.parse().unwrap(),
            deepagent_core::event::EventPayload::PlanModeChanged {
                version: 2,
                active: false,
            },
            deepagent_core::clock::Timestamp::from_millis(999),
        )
        .unwrap();
    assert!(chat.is_plan_mode(&id).is_err());
    assert!(chat.set_plan_mode(&id, false).is_err());
    assert!(chat
        .run_in_session(
            "Do not issue a model request on invalid state.",
            Some(&id),
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
            |_| {},
            |_| {}
        )
        .await
        .is_err());
    assert_eq!(model.0.lock().unwrap().len(), count);
    assert!(!temp.path().join("generated-2.txt").exists());
}
