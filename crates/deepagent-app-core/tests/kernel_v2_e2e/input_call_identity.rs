//! Legacy native history must not send blank tool identities after reopening.

use super::*;
use deepagent_core::{clock::Timestamp, event::EventPayload, response_item::ResponseItem};
use deepagent_models::ModelRole;
use deepagent_persistence::{
    event_store::EventStore, run_control::RunControlStore, run_store::RunStore,
};

#[derive(Default)]
struct HistoryModel(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for HistoryModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        emit_responses_events(sink, &response_text("History continued."))
    }
}

async fn observe(id: &str, custom: bool, reject: bool) {
    const RUN: &str = "h148-reopened-history";
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = temporary.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let discovery = Arc::new(MockTransport::with_get_json(
        r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
    ));
    // Seed a legacy native pair through the real append-only event API, then
    // discard the complete host. This intentionally bypasses today's output
    // parser, which H-151 already protects, not the request/replay boundary.
    let (session, original) = {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery.clone(),
            secrets.clone(),
        ));
        settings.initialize("offline-placeholder").await.unwrap();
        for role in [ModelRole::Chat, ModelRole::Reasoner] {
            settings.set_model(role, "deepseek-flash").unwrap();
        }
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(HistoryModel::default()),
            &workspace,
        );
        let session = chat.create_session(None).unwrap();
        let pair = if custom {
            vec![
                ResponseItem::CustomToolCall {
                    call_id: id.into(),
                    name: "apply_patch".into(),
                    input: "PRIVATE_ARGUMENTS".into(),
                },
                ResponseItem::CustomToolCallOutput {
                    call_id: id.into(),
                    output: "PRIVATE_OUTPUT".into(),
                },
            ]
        } else {
            vec![
                ResponseItem::FunctionCall {
                    call_id: id.into(),
                    name: "write_file".into(),
                    arguments: r#"{"path":"never.txt","content":"PRIVATE_ARGUMENTS"}"#.into(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: id.into(),
                    output: "PRIVATE_OUTPUT".into(),
                },
            ]
        };
        let store = EventStore::new(&db);
        for item in pair {
            store
                .append(
                    session.parse().unwrap(),
                    EventPayload::ResponseItemAppended { item },
                    Timestamp::from_millis(1000),
                )
                .unwrap();
        }
        let original = store.load_session(session.parse().unwrap()).unwrap();
        (session, original)
    };

    let db = Arc::new(Database::open(&path).unwrap());
    assert_eq!(
        EventStore::new(&db)
            .load_session(session.parse().unwrap())
            .unwrap(),
        original
    );
    let settings = Arc::new(SettingsService::new(db.clone(), discovery, secrets));
    let model = Arc::new(HistoryModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        chat.run_in_session(
            "Continue the old conversation without executing any tool.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            |_| {},
            |_| panic!("history replay must not request tool approval"),
        ),
    )
    .await
    .expect("bounded offline history run");
    if reject {
        let error =
            result.expect_err("blank historical call_id must fail before provider transport");
        assert!(error.to_string().contains("call_id"), "{error}");
        assert!(!error.to_string().contains("PRIVATE_"));
        assert!(
            model.0.lock().unwrap().is_empty(),
            "invalid histories must not retry or send"
        );
    } else {
        assert_eq!(result.unwrap(), session);
        let bodies = model.0.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let ids: Vec<_> = bodies[0]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["call_id"].as_str())
            .collect();
        assert_eq!(
            ids,
            [id, id],
            "replay must preserve the existing valid identity"
        );
    }
    assert!(!workspace.join("never.txt").exists());
    assert!(RunControlStore::new(&db)
        .list_actions(RUN)
        .unwrap()
        .is_empty());
    assert!(RunControlStore::new(&db)
        .list_approvals(RUN)
        .unwrap()
        .is_empty());
    let kind = if reject { "failed" } else { "succeeded" };
    let events = chat.run_events(RUN, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], kind);
    let history = EventStore::new(&db)
        .load_session(session.parse().unwrap())
        .unwrap();
    assert!(
        history.starts_with(&original),
        "validation must not rewrite legacy raw events"
    );
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(
                &event.payload,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCall { .. }
                        | ResponseItem::FunctionCallOutput { .. }
                        | ResponseItem::CustomToolCall { .. }
                        | ResponseItem::CustomToolCallOutput { .. }
                }
            ))
            .count(),
        2,
        "no synthetic identity/pair may enter the durable history"
    );
    drop(chat);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(session.parse().unwrap())
            .unwrap(),
        history
    );
    assert_eq!(
        RunStore::new(&reopened)
            .get(RUN)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some(kind)
    );
}

#[tokio::test]
async fn h148_reopened_blank_native_history_fails_without_sending_or_rewriting() {
    for custom in [false, true] {
        for id in ["", " \t"] {
            observe(id, custom, true).await;
        }
    }
}

#[tokio::test]
async fn h148_reopened_valid_function_and_custom_history_remains_usable() {
    for custom in [false, true] {
        observe("h148-valid-history", custom, false).await;
    }
}
