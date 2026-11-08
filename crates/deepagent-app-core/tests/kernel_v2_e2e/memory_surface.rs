//! H-53: production host, knowledge provider, durable Surface and next wire.
use super::*;
use deepagent_app_core::knowledge_service::{KnowledgeDraftDto, KnowledgeService};
use deepagent_context::memory_attachment::surfaced_memory_ids;
use deepagent_core::clock::SystemClock;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_core::response_item::ResponseInputItem;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;

struct MemoryWire {
    requests: Mutex<Vec<serde_json::Value>>,
    phase: usize,
}

#[async_trait]
impl HttpTransport for MemoryWire {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            requests.push(body);
            index
        };
        // Let the real keyword provider finish while the first model call is
        // in flight. Only a transport response is synthetic, not memory state.
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let output = if index == 0 {
            serde_json::json!([{"type":"function_call","id":format!("h53-read-item-{}", self.phase),"call_id":format!("h53-read-{}", self.phase),
                "name":"read_file","arguments":"{\"path\":\"synthetic.txt\"}"}])
        } else {
            serde_json::json!([{"type":"message","id":"h53-answer","role":"assistant",
                "content":[{"type":"output_text","text":"H53 synthetic completed","annotations":[]}]}])
        };
        emit_responses_events(sink, &[serde_json::json!({"type":"response.completed","response":{
            "id":format!("h53-{index}"),"object":"response","created_at":1,"model":"deepseek-flash",
            "status":"completed","output":output
        }}).to_string()])
    }
}

#[tokio::test]
async fn h53_host_recovers_visible_memory_then_retrieves_again_after_boundary() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("synthetic.sqlite");
    std::fs::write(root.path().join("synthetic.txt"), "synthetic control").unwrap();
    let knowledge =
        Arc::new(KnowledgeService::open(root.path(), &root.path().join("global-vault")).unwrap());
    // Exercise the supported background-only configuration. Passive knowledge
    // prepends a whole user reminder and has a separate query-selection guard.
    knowledge.set_passive_enabled(false);
    knowledge.set_auto_capture(false);
    knowledge
        .save(KnowledgeDraftDto {
            title: "H53 deployment configuration".into(),
            body: "H53 deployment configuration: H53_HOST_EXCERPT retain retry settings.".into(),
            kind: Some("pitfall".into()),
            tags: vec!["deployment".into(), "configuration".into()],
            scope: Some("project".into()),
            source_session: None,
            tier: None,
            importance: None,
        })
        .unwrap();
    let mut session_id = None;
    for phase in 0..3 {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("synthetic-control").await.unwrap();
        let transport = Arc::new(MemoryWire {
            requests: Mutex::new(Vec::new()),
            phase,
        });
        let chat = ChatService::new(db.clone(), settings.clone(), transport.clone(), root.path())
            .with_knowledge(knowledge.clone());
        if session_id.is_none() {
            session_id = Some(chat.create_session(None).unwrap());
        }
        let id = session_id.as_deref().unwrap();
        chat.run_in_session(
            "H53 deployment configuration",
            Some(id),
            None,
            None,
            vec![],
            None,
            false,
            Some(&format!("h53-host-{phase}")),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        let requests = transport.requests.lock().unwrap();
        assert!(requests.len() >= 2);
        assert_eq!(
            surfaced_memory_ids(
                &serde_json::from_value::<Vec<ResponseInputItem>>(requests[1]["input"].clone())
                    .unwrap()
            )
            .len(),
            1
        );
        drop(requests);
        let sid: SessionId = id.parse().unwrap();
        let events = EventStore::new(&db).load_session(sid).unwrap();
        assert_eq!(events.iter().filter(|event| matches!(&event.payload, EventPayload::ResponseItemAppended { item: ResponseInputItem::Message { content, .. } } if content.contains("<relevant-memories version="))).count(), if phase == 2 { 2 } else { 1 });
        let run = RunStore::new(&db)
            .get(&format!("h53-host-{phase}"))
            .unwrap()
            .unwrap();
        assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
        assert_eq!(
            RunStore::new(&db)
                .events_after(&run.id, None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        if phase == 1 {
            let mut session = deepagent_session::Session::recover(&db, &SystemClock, sid).unwrap();
            session
                .append(EventPayload::ContextCompacted {
                    tokens_before: 5000,
                    tokens_after: 20,
                    strategy: "h53-fixture-boundary".into(),
                    summary: Some("Earlier work completed without memory excerpt".into()),
                    first_kept_sequence: None,
                })
                .unwrap();
        }
        drop(chat);
        drop(settings);
        drop(db);
    }
}
