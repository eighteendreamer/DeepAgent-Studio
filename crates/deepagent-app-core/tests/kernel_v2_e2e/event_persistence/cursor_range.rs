//! Exercise the public reader left outside the H-218 paginated Harness path.
use super::*;
use deepagent_persistence::run_store::StoredRunEvent;

fn assert_reads(chat: &ChatService, expected: &[(String, Vec<StoredRunEvent>)]) {
    for (run, full) in expected {
        let tail = full.last().unwrap().sequence;
        for after in [
            None,
            Some(0),
            Some(1),
            Some(tail / 2),
            Some(tail),
            Some(tail + 1),
            Some(i64::MAX as u64),
            Some(i64::MAX as u64 + 1),
            Some(u64::MAX),
        ] {
            assert_eq!(
                chat.run_events(run, after).unwrap(),
                full.iter()
                    .filter(|event| after.map_or(true, |after| event.sequence > after))
                    .cloned()
                    .collect::<Vec<_>>(),
                "public ChatService reader: run={run}, after={after:?}"
            );
            assert_eq!(chat.run_events(run, None).unwrap(), *full);
            assert!(chat.run_events("missing-run", after).unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn h298_public_reader_never_rewinds_or_reexecutes_after_full_service_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cursor.sqlite");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let model = Arc::new(WriteModel {
        batch: false,
        requests: Mutex::new(Vec::new()),
    });
    let (session, expected, session_events, actions) = {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = settings(db.clone()).await;
        settings
            .set_active_permission_preset(deepagent_app_core::PermissionPreset::FullAccess)
            .unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
        let session = chat.create_session(None).unwrap();
        let mut expected = Vec::new();
        for run in ["h298-write", "h298-followup"] {
            tokio::time::timeout(
                Duration::from_secs(8),
                chat.run_in_session(
                    "Complete the synthetic local cursor check.",
                    Some(&session),
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(run),
                    |_| {},
                    |_| {},
                ),
            )
            .await
            .expect("finite offline run")
            .unwrap();
            expected.push((run.to_owned(), chat.run_events(run, None).unwrap()));
        }
        assert_eq!(model.requests.lock().unwrap().len(), 3);
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect-0.txt")).unwrap(),
            CONTENT
        );
        let session_events = serde_json::to_value(
            EventStore::new(&db)
                .load_session(session.parse().unwrap())
                .unwrap(),
        )
        .unwrap();
        let actions = serde_json::to_value(
            RunControlStore::new(&db)
                .list_actions("h298-write")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(actions.as_array().unwrap().len(), 1);
        assert_reads(&chat, &expected);
        (session, expected, session_events, actions)
    };
    // No original ChatService, SettingsService or Database handle survives.
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(r#"{"data":[]}"#)),
        Arc::new(MemorySecretStore::new()),
    ));
    let reopened = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    assert_reads(&reopened, &expected);
    assert_eq!(
        model.requests.lock().unwrap().len(),
        3,
        "reads must not call the model again"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("effect-0.txt")).unwrap(),
        CONTENT
    );
    assert_eq!(
        serde_json::to_value(
            EventStore::new(&db)
                .load_session(session.parse().unwrap())
                .unwrap()
        )
        .unwrap(),
        session_events
    );
    assert_eq!(
        serde_json::to_value(
            RunControlStore::new(&db)
                .list_actions("h298-write")
                .unwrap()
        )
        .unwrap(),
        actions
    );
}
