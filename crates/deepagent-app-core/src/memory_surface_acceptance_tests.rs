//! H-53: real session persistence, close/reopen, history projection and wire.
use super::*;
use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
use deepagent_context::memory_attachment::surfaced_memory_ids;
use deepagent_core::clock::SystemClock;
use deepagent_persistence::event_store::EventStore;
use deepagent_runtime::{
    Agent, ContextSurfacePersistence, ModelAgent, RelevantMemory, RelevantMemoryProvider,
};

#[derive(Default)]
struct H53Provider {
    calls: Mutex<Vec<Vec<String>>>,
    finished: tokio::sync::Notify,
}

#[async_trait]
impl RelevantMemoryProvider for H53Provider {
    async fn fetch_relevant(
        &self,
        _: &str,
        already: &[String],
        _: &[String],
    ) -> Result<Vec<RelevantMemory>> {
        self.calls.lock().unwrap().push(already.to_vec());
        self.finished.notify_one();
        Ok((!already.iter().any(|id| id == "project:h53"))
            .then(|| RelevantMemory {
                id: "project:h53".into(),
                block: "H53_REOPEN_EXCERPT".into(),
            })
            .into_iter()
            .collect())
    }
}

#[tokio::test]
async fn h53_memory_body_is_durable_before_dispatch_and_reopens_with_compaction() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("h53-synthetic.sqlite");
    let sid = {
        let db = Database::open(&path).unwrap();
        let mut session = deepagent_session::Session::create(&db, &SystemClock, None).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("inspect synthetic deployment settings"),
            })
            .unwrap();
        session.id()
    };
    for phase in 0..3 {
        let db = Arc::new(Database::open(&path).unwrap());
        let before = EventStore::new(&db).load_session(sid).unwrap();
        let history = crate::input_runtime::response_items_from_events(&before);
        assert_eq!(surfaced_memory_ids(&history).len(), usize::from(phase == 1));
        let provider = Arc::new(H53Provider::default());
        let (client, transport) = scripted_client_with_transport(vec![
            ScriptedTransport::call(
                "h53-read",
                "read_file",
                serde_json::json!({"path":"synthetic"}),
            ),
            ScriptedTransport::text("synthetic completed"),
        ]);
        let mut agent = ModelAgent::new(
            client,
            "deepseek-flash",
            "rules",
            "inspect synthetic deployment settings",
            vec![],
        )
        .with_response_history(history)
        .with_relevant_memory_provider(provider.clone())
        .with_surface_persistence(Arc::new(SessionSurfaceStore {
            db: db.clone(),
            session_id: sid,
        }));
        agent.think(0, &[]).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            provider.finished.notified(),
        )
        .await
        .unwrap();
        agent.think(1, &[]).await.unwrap();
        let wire: serde_json::Value = serde_json::from_str(&transport.last_prompt()).unwrap();
        assert_eq!(
            wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["content"]
                    .as_str()
                    .is_some_and(|s| s.contains("H53_REOPEN_EXCERPT")))
                .count(),
            1
        );
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), usize::from(phase == 1));
        let new_injections = agent.drain_memory_injections();
        assert_eq!(new_injections.len(), usize::from(phase != 1));
        drop(calls);
        let after = EventStore::new(&db).load_session(sid).unwrap();
        assert_eq!(
            surfaced_memory_ids(&crate::input_runtime::response_items_from_events(&after)),
            vec!["project:h53"]
        );
        assert_eq!(after.iter().filter(|e| matches!(&e.payload, EventPayload::ResponseItemAppended { item: ResponseInputItem::Message { content, .. } , .. } if content.contains("H53_REOPEN_EXCERPT"))).count(), if phase == 2 { 2 } else { 1 });
        // No loop-finalizer telemetry is needed to recover complete content.
        assert!(!after
            .iter()
            .any(|e| matches!(e.payload, EventPayload::MemoriesInjected { .. })));
        if phase == 1 {
            let mut session = deepagent_session::Session::recover(&db, &SystemClock, sid).unwrap();
            session
                .append(EventPayload::ContextCompacted {
                    tokens_before: 5000,
                    tokens_after: 20,
                    strategy: "h53-synthetic-boundary".into(),
                    summary: Some(
                        "completed earlier investigation; no memory identity retained".into(),
                    ),
                    first_kept_sequence: None,
                })
                .unwrap();
        }
        // All database owners, including the Surface writer, close each phase.
        drop(agent);
        drop(db);
    }
}

#[test]
fn h53_missing_session_rejects_attachment_persistence() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let store = SessionSurfaceStore {
        db,
        session_id: SessionId::nil(),
    };
    let item = deepagent_context::memory_attachment::memory_attachment(&[RelevantMemory {
        id: "project:h53".into(),
        block: "synthetic excerpt".into(),
    }])
    .unwrap();
    assert!(store.record_memory_attachment(&item).is_err());
}
