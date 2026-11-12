//! H-107: exercise real control admission, kernel, durable session and command hooks.
use super::*;
use deepagent_core::clock::SystemClock;
use deepagent_persistence::{event_store::EventStore, run_store::RunStore};
use deepagent_session::Session;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountTransport(AtomicUsize);

#[async_trait::async_trait]
impl HttpTransport for CountTransport {
    async fn stream(
        &self,
        request: deepagent_models::transport::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        super::tests::chat_transport().stream(request, sink).await
    }
}

async fn turn(chat: &ChatService, id: &str, text: &str) -> Result<String> {
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
}

fn end_events(db: &Database, id: &str) -> usize {
    EventStore::new(db)
        .load_session(id.parse().unwrap())
        .unwrap()
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::SessionEnded { .. }))
        .count()
}

#[cfg(windows)]
fn configure_end(settings: &SettingsService, marker: &std::path::Path) {
    let path = marker.to_string_lossy().replace('\'', "''");
    settings.set_hooks_json(&serde_json::json!({"hooks":{"SessionEnd":[{"hooks":[{
        "type":"command", "shell":"powershell", "timeout":5,
        "command":format!("[System.IO.File]::AppendAllText('{path}', 'end' + [Environment]::NewLine)")
    }]}]}}).to_string()).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn h107_explicit_end_is_once_and_future_turns_are_read_only() {
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let id = chat.run("hello", |_| {}, |_| {}).await.unwrap();
    let fork_sequence = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    assert!(
        !marker.exists(),
        "a successful turn must not end the conversation"
    );
    turn(&chat, &id, "/end").await.unwrap();
    let end_run = RunStore::new(&db)
        .list_for_session(&id)
        .unwrap()
        .into_iter()
        .find(|run| {
            RunStore::new(&db)
                .events_after(&run.id, None)
                .unwrap()
                .iter()
                .any(|event| event.event_type == "session_end.requested")
        })
        .unwrap();
    assert!(chat
        .maybe_generate_session_title_for_run(&id, &end_run.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(
        std::fs::read_to_string(&marker)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["end"]
    );
    assert!(EventStore::new(&db)
        .get_session(id.parse().unwrap())
        .unwrap()
        .unwrap()
        .ended_at
        .is_some());
    let repeated_events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let live = repeated_events.clone();
    chat.run_in_session(
        "/end",
        Some(&id),
        None,
        None,
        vec![],
        None,
        false,
        Some("h107-repeat-notify"),
        move |event| live.lock().unwrap().push(event),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        repeated_events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::RunCompleted { .. }))
            .count(),
        1
    );
    assert_eq!(
        RunStore::new(&db)
            .events_after("h107-repeat-notify", None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);
    let before = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap();
    let runs = RunStore::new(&db).list_for_session(&id).unwrap();
    let error = turn(&chat, &id, "another normal turn").await.unwrap_err();
    assert!(error.to_string().contains("ended"), "{error}");
    assert_eq!(
        EventStore::new(&db)
            .load_session(id.parse().unwrap())
            .unwrap(),
        before
    );
    assert_eq!(RunStore::new(&db).list_for_session(&id).unwrap(), runs);
    assert_eq!(
        transport.0.load(Ordering::SeqCst),
        1,
        "/end must not call the main model"
    );
    assert!(RunStore::new(&db)
        .list_for_session(&id)
        .unwrap()
        .iter()
        .all(|r| r.finished_at.is_some()));
    let app = crate::AppService::from_shared(db.clone());
    let error = app
        .rewind_session(&id, fork_sequence)
        .expect_err("ended history must be read-only");
    assert!(error.to_string().contains("ended"), "{error}");
    assert_eq!(
        EventStore::new(&db)
            .load_session(id.parse().unwrap())
            .unwrap(),
        before
    );
    let fork = crate::AppService::from_shared(db.clone())
        .fork_session(&id, fork_sequence)
        .unwrap();
    turn(
        &chat,
        &fork.new_session_id,
        "continue from before the explicit end",
    )
    .await
    .unwrap();
    assert_eq!(transport.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        end_events(&db, &id),
        1,
        "fork must not reopen or rewrite its ended source"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn h107_fork_of_ended_history_starts_a_fresh_independent_lifecycle() {
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    chat.end_session(&id).await.unwrap();
    let source = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap();
    let fork = crate::AppService::from_shared(db.clone())
        .fork_session(&id, source.last().unwrap().sequence)
        .unwrap();
    let child_id = fork.new_session_id.parse().unwrap();
    let child = Session::recover(&db, &SystemClock, child_id).unwrap();
    assert!(!child.state().ended, "a fork has its own fresh lifecycle");
    assert!(child.record().unwrap().ended_at.is_none());
    let forked = EventStore::new(&db).load_session(child_id).unwrap();
    assert_eq!(forked.len(), source.len() + 1);
    for (old, copied) in source.iter().zip(&forked) {
        assert_eq!(old.payload, copied.payload);
        assert_eq!(old.timestamp, copied.timestamp);
    }
    assert!(matches!(
        forked.last().unwrap().payload,
        EventPayload::SessionStarted { .. }
    ));
    turn(&chat, &fork.new_session_id, "continue the independent fork")
        .await
        .unwrap();
    chat.end_session(&fork.new_session_id).await.unwrap();
    chat.end_session(&fork.new_session_id).await.unwrap();
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 2);
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(
        end_events(&db, &fork.new_session_id),
        2,
        "one inherited end and one independent end"
    );
    assert_eq!(
        EventStore::new(&db)
            .load_session(id.parse().unwrap())
            .unwrap(),
        source
    );
    assert_eq!(transport.0.load(Ordering::SeqCst), 1);
}

#[cfg(windows)]
#[tokio::test]
async fn h107_host_drop_and_reopen_do_not_end_a_persistent_conversation() {
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings.clone(), transport.clone(), dir.path());
    let id = chat.run("hello", |_| {}, |_| {}).await.unwrap();
    drop(chat);
    assert!(!marker.exists());
    assert_eq!(end_events(&db, &id), 0);
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    turn(&chat, &id, "resume after host replacement")
        .await
        .unwrap();
    assert!(!marker.exists());
    assert_eq!(end_events(&db, &id), 0);
    chat.end_session(&id).await.unwrap();
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(transport.0.load(Ordering::SeqCst), 2);
}

#[cfg(windows)]
#[tokio::test]
async fn h107_normal_turn_failure_and_cancellation_never_end_the_session() {
    struct FailedModel;
    #[async_trait::async_trait]
    impl HttpTransport for FailedModel {
        async fn stream(
            &self,
            _: deepagent_models::transport::TransportRequest,
            _: &mut dyn deepagent_models::transport::EventSink,
        ) -> Result<()> {
            Err(CoreError::invalid(
                "h107 synthetic invalid provider request",
            ))
        }
    }
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    let chat = ChatService::new(
        db.clone(),
        settings.clone(),
        Arc::new(FailedModel),
        dir.path(),
    );
    assert!(turn(&chat, &id, "ordinary failed turn").await.is_err());
    assert!(!marker.exists());
    assert_eq!(end_events(&db, &id), 0);
    let chat = ChatService::new(
        db.clone(),
        settings,
        super::tests::chat_transport(),
        dir.path(),
    );
    let prepared = chat
        .admit_prepared_input(
            "h107-cancel-normal",
            &id,
            "ordinary cancelled turn",
            HarnessRunOverrides::default(),
        )
        .unwrap();
    assert!(chat.request_cancel("h107-cancel-normal").unwrap().accepted);
    chat.run_prepared(prepared, "ordinary cancelled turn", |_| {}, |_| {})
        .await
        .unwrap();
    assert!(!marker.exists());
    assert_eq!(end_events(&db, &id), 0);
    assert_eq!(
        RunStore::new(&db)
            .get("h107-cancel-normal")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
    chat.end_session(&id).await.unwrap();
    assert_eq!(end_events(&db, &id), 1);
}

#[tokio::test]
async fn h107_busy_session_refuses_end_before_any_new_admission() {
    let (db, settings, dir) = super::tests::seeded().await;
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    chat.coordinator.admit("h107-busy", &id).unwrap();
    let before = RunStore::new(&db).list_for_session(&id).unwrap();
    let error = turn(&chat, &id, "/end").await.unwrap_err();
    assert!(error.to_string().contains("busy"), "{error}");
    assert_eq!(end_events(&db, &id), 0);
    assert_eq!(RunStore::new(&db).list_for_session(&id).unwrap(), before);
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn h107_cancelled_end_intent_stays_read_only_until_explicit_retry() {
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    let prepared = chat
        .admit_prepared_input(
            "h107-cancel-end",
            &id,
            "/end",
            HarnessRunOverrides::default(),
        )
        .unwrap();
    assert!(chat.request_cancel("h107-cancel-end").unwrap().accepted);
    chat.run_prepared(prepared, "/end", |_| {}, |_| {})
        .await
        .unwrap();
    assert!(!marker.exists());
    assert_eq!(end_events(&db, &id), 0);
    assert!(turn(&chat, &id, "must not silently reopen")
        .await
        .unwrap_err()
        .to_string()
        .contains("ended"));
    chat.end_session(&id).await.unwrap();
    chat.end_session(&id).await.unwrap();
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 1);
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn h107_failed_observational_hook_is_not_replayed_by_repeated_end() {
    let (db, settings, dir) = super::tests::seeded().await;
    let marker = dir.path().join("partial-hook.txt");
    let path = marker.to_string_lossy().replace('\'', "''");
    settings.set_hooks_json(&serde_json::json!({"hooks":{"SessionEnd":[{"hooks":[{
        "type":"command","shell":"powershell","timeout":5,
        "command":format!("[System.IO.File]::AppendAllText('{path}', 'effect' + [Environment]::NewLine); exit 17")
    }]}]}}).to_string()).unwrap();
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    // SessionEnd remains observational; a nonzero hook cannot undo closure.
    chat.end_session(&id).await.unwrap();
    chat.end_session(&id).await.unwrap();
    assert_eq!(end_events(&db, &id), 1);
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 1);
    let all = RunStore::new(&db)
        .list_for_session(&id)
        .unwrap()
        .iter()
        .flat_map(|run| RunStore::new(&db).events_after(&run.id, None).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        all.iter()
            .filter(|event| event.event_type == "hook_completed")
            .count(),
        1
    );
    assert!(all
        .iter()
        .any(|event| event.event_type == "hook_completed" && event.data["exit_code"] == 17));
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn h107_command_end_needs_no_login_and_uses_admitted_control() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("end.sqlite")).unwrap());
    let transport = Arc::new(CountTransport(AtomicUsize::new(0)));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        super::tests::discovery_transport(),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    ));
    settings.initialize("sk-h107-synthetic").await.unwrap();
    settings.clear_key().unwrap();
    let marker = dir.path().join("ends.txt");
    configure_end(&settings, &marker);
    let id = Session::create(&db, &SystemClock, None)
        .unwrap()
        .id()
        .to_string();
    let chat = ChatService::new(db.clone(), settings, transport.clone(), dir.path());
    let run_id = "h107-prepared-end";
    let prepared = chat
        .admit_run_input(run_id, &id, "/end")
        .unwrap()
        .expect("Desktop host must receive the exact admitted end control");
    chat.run_prepared(prepared, "/end", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    assert_eq!(end_events(&db, &id), 1);
    let run = RunStore::new(&db).get(run_id).unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    let events = RunStore::new(&db).events_after(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(events
        .iter()
        .any(|event| event.event_type == "hook_completed"));
    drop(chat);
    drop(db);
    let reopened = Database::open(dir.path().join("end.sqlite")).unwrap();
    assert_eq!(end_events(&reopened, &id), 1);
    assert_eq!(
        RunStore::new(&reopened).events_after(run_id, None).unwrap(),
        events
    );
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 1);
}
