//! H-108: capture real lifecycle stdin, not a fabricated serializer result.
use super::*;
use deepagent_hooks::{HookAction, HookCommandShell};
use deepagent_persistence::event_store::EventStore;

fn capture(path: &std::path::Path) -> HookAction {
    let path = path.to_string_lossy().replace('\'', "''");
    HookAction {
        shell: HookCommandShell::Powershell,
        timeout: Some(5),
        command: format!(
            "$payload = [Console]::In.ReadToEnd(); [System.IO.File]::AppendAllText('{path}', $payload + [Environment]::NewLine)"
        ),
        ..Default::default()
    }
}

fn captured(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn turn(chat: &ChatService, id: &str, text: &str) {
    chat.run_in_session(
        text,
        Some(id),
        None,
        None,
        vec![],
        None,
        false,
        None,
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn h108_real_lifecycle_metadata_is_truthful_and_does_not_change_dispatch_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lifecycle.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        super::tests::discovery_transport(),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    ));
    settings.initialize("sk-h108-synthetic").await.unwrap();
    let path = dir.path().join("lifecycle.jsonl");
    let action = capture(&path);
    settings
        .set_hooks_json(
            &serde_json::json!({"hooks":{
                "SessionStart":[{"hooks":[action.clone()]}],
                "SessionEnd":[{"hooks":[action]}]
            }})
            .to_string(),
        )
        .unwrap();
    let chat = ChatService::new(
        db.clone(),
        settings.clone(),
        super::tests::chat_transport(),
        dir.path(),
    );
    let id = chat.run("new conversation", |_| {}, |_| {}).await.unwrap();
    turn(&chat, &id, "ordinary continuation").await;
    turn(&chat, &id, "/clear").await;
    drop(chat);
    assert_eq!(captured(&path).len(), 1, "turn/clear/host drop are not End");
    let chat = ChatService::new(
        db.clone(),
        settings.clone(),
        super::tests::chat_transport(),
        dir.path(),
    );
    turn(&chat, &id, "continue after host replacement").await;
    settings.clear_key().unwrap();
    chat.end_session(&id).await.unwrap();
    chat.end_session(&id).await.unwrap();
    let events = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap();
    let ends = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::SessionEnded { reason } => Some(reason.as_deref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ends,
        [Some("explicit_end")],
        "do not rewrite durable reasons"
    );
    drop(chat);
    drop(settings);
    drop(db);
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        super::tests::discovery_transport(),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    ));
    let chat = ChatService::new(
        db.clone(),
        settings,
        super::tests::chat_transport(),
        dir.path(),
    );
    assert_eq!(
        EventStore::new(&db)
            .load_session(id.parse().unwrap())
            .unwrap(),
        events
    );
    chat.end_session(&id).await.unwrap();
    let after_retry = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap();
    // A newly admitted /end still has its own canonical run receipt. It must
    // not rewrite the prefix or append another session lifecycle/message.
    assert_eq!(&after_retry[..events.len()], events.as_slice());
    assert_eq!(after_retry.len(), events.len() + 1);
    assert!(matches!(
        &after_retry.last().unwrap().payload,
        EventPayload::RunTerminal { task_id: None, kind, .. } if kind == "succeeded"
    ));
    let payloads = captured(&path);
    assert_eq!(
        payloads.len(),
        2,
        "reopen/repeated End must not replay hooks"
    );
    assert_eq!(payloads[0]["session_id"], id);
    assert_eq!(payloads[1]["session_id"], id);
    assert_eq!(payloads[0]["hook_event_name"], "SessionStart");
    assert_eq!(payloads[1]["hook_event_name"], "SessionEnd");
    eprintln!("H108 real lifecycle: {payloads:?}");
    assert_eq!(payloads[0]["source"], "startup");
    assert_eq!(payloads[1]["reason"], "other");
}

#[tokio::test]
async fn h108_settings_test_uses_the_same_lifecycle_metadata_without_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings-lifecycle.jsonl");
    for event in ["SessionStart", "SessionEnd"] {
        let result = crate::hook_runtime::test_hook_action(event, None, capture(&path), None, None)
            .await
            .unwrap();
        assert_eq!(result.outcome, "continued", "{}", result.detail);
    }
    let payloads = captured(&path);
    assert_eq!(payloads.len(), 2);
    assert_eq!(payloads[0]["hook_event_name"], "SessionStart");
    assert_eq!(payloads[1]["hook_event_name"], "SessionEnd");
    eprintln!("H108 synthetic settings lifecycle: {payloads:?}");
    assert_eq!(payloads[0]["source"], "startup");
    assert_eq!(payloads[1]["reason"], "other");
}
