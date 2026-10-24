//! A damaged history file must not become a prerequisite for unrelated work.

use super::*;
use deepagent_app_core::{AppService, ProjectService};
use deepagent_core::{clock::Timestamp, event::EventPayload, id::SessionId, message::Message};
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;

const QUERIES: [&str; 3] = ["检", "缓存", "repairneedle"];

#[derive(Clone, Copy, Debug)]
enum Tail {
    Healthy,
    TornFrame,
    InvalidFrame,
}

struct FileFault {
    path: PathBuf,
    original: Vec<u8>,
}

impl FileFault {
    fn inject(db: &Database, session: SessionId, tail: Tail) -> Self {
        let mut files = std::fs::read_dir(db.session_transcript_dir(session))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().ends_with(".v1.jsonl.zst"));
        let path = files
            .next()
            .expect("the fixture must have a transcript file");
        assert!(files.next().is_none());
        let original = std::fs::read(&path).unwrap();
        let mut bytes = original.clone();
        bytes.extend_from_slice(match tail {
            Tail::Healthy => &[],
            Tail::TornFrame => &[0x28, 0xb5, 0x2f, 0xfd],
            Tail::InvalidFrame => &[0xff],
        });
        std::fs::write(&path, bytes).unwrap();
        Self { path, original }
    }
}

impl Drop for FileFault {
    fn drop(&mut self) {
        // Only a fixture-owned file in TempDir; always restore before cleanup.
        std::fs::write(&self.path, &self.original).unwrap();
    }
}

fn seed(db: &Database, project: &str, text: &str) -> SessionId {
    let id = SessionId::new();
    let store = EventStore::new(db);
    store
        .create_session_full(
            id,
            Some("synthetic repair isolation history"),
            Default::default(),
            Some(project),
            Timestamp::from_millis(100),
        )
        .unwrap();
    store
        .append(
            id,
            EventPayload::MessageAppended {
                message: Message::user(text),
            },
            Timestamp::from_millis(101),
        )
        .unwrap();
    id
}

struct HistoryFixture {
    workspace: tempfile::TempDir,
    db: Arc<Database>,
    good_project: String,
    bad_project: String,
    good: SessionId,
    bad: SessionId,
}

impl HistoryFixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(workspace.path().join("state.sqlite")).unwrap());
        let good_project = workspace.path().join("project-a");
        let bad_project = workspace.path().join("project-b");
        std::fs::create_dir_all(&good_project).unwrap();
        std::fs::create_dir_all(&bad_project).unwrap();
        let good_project = good_project.to_string_lossy().into_owned();
        let bad_project = bad_project.to_string_lossy().into_owned();
        let projects = ProjectService::new(db.clone());
        projects.add_project(&bad_project).unwrap();
        projects.add_project(&good_project).unwrap();
        let good = seed(&db, &good_project, "检 缓存 repairneedle earlier decision");
        let bad = seed(&db, &bad_project, "unrelated old history");
        Self {
            workspace,
            db,
            good_project,
            bad_project,
            good,
            bad,
        }
    }
}

fn observe_app_and_reopen(tail: Tail) -> (bool, bool) {
    let fixture = HistoryFixture::new();
    assert_ne!(fixture.good_project, fixture.bad_project);
    let app = AppService::from_shared(fixture.db.clone());
    for query in QUERIES {
        let before = app
            .search_sessions(query, Some(&fixture.good_project), 10)
            .unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].session.id, fixture.good.to_string());
    }
    let fault = FileFault::inject(&fixture.db, fixture.bad, tail);
    let mut searches_ok = true;
    for query in QUERIES {
        let result = app.search_sessions(query, Some(&fixture.good_project), 10);
        let found = match &result {
            Ok(hits) => hits.len() == 1 && hits[0].session.id == fixture.good.to_string(),
            Err(error) => {
                assert!(error.to_string().contains("invalid zstd frame"));
                false
            }
        };
        assert_eq!(
            EventStore::new(&fixture.db)
                .load_session(fixture.good)
                .unwrap()
                .len(),
            1
        );
        searches_ok &= found;
        eprintln!("repair isolation: tail={tail:?}, query={query}, scoped_to_good=true, good_history_readable=true, search_ok={found}");
    }
    // Search already repaired the torn tail. Reinject it to exercise startup's
    // own recovery, rather than reopening only an already healthy file.
    let startup_fault =
        matches!(tail, Tail::TornFrame).then(|| FileFault::inject(&fixture.db, fixture.bad, tail));
    // Release every Database owner before probing the real file-open lifecycle.
    drop(app);
    assert_eq!(Arc::strong_count(&fixture.db), 1);
    let path = fixture.workspace.path().join("state.sqlite");
    drop(fixture.db);
    let reopened = Database::open(&path);
    let open_ok = reopened.is_ok();
    if let Err(error) = &reopened {
        assert!(error.to_string().contains("invalid zstd frame"));
    }
    drop(reopened);
    drop(startup_fault);
    drop(fault);
    // Restoring only B's bytes recovers startup and A's unchanged search.
    let restored = AppService::open(&path).unwrap();
    for query in QUERIES {
        let hits = restored
            .search_sessions(query, Some(&fixture.good_project), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, fixture.good.to_string());
    }
    eprintln!("repair isolation reopen: tail={tail:?}, all_old_owners_closed=true, database_open_ok={open_ok}, restored_only_bad_file_recovers=true");
    (searches_ok, open_ok)
}

#[test]
#[ignore = "audit: an unrelated corrupt session must not block scoped search or database startup"]
fn audit_corrupt_other_project_does_not_block_search_or_startup() {
    let (search_ok, open_ok) = observe_app_and_reopen(Tail::InvalidFrame);
    assert!(search_ok && open_ok);
}

#[test]
fn healthy_and_recoverable_torn_other_session_do_not_block_search_or_startup() {
    for tail in [Tail::Healthy, Tail::TornFrame] {
        assert_eq!(observe_app_and_reopen(tail), (true, true));
    }
}

struct SearchModel {
    db: Arc<Database>,
    bad: SessionId,
    query: &'static str,
    tail: Tail,
    calls: AtomicUsize,
    fault: Mutex<Option<FileFault>>,
    output: Mutex<Option<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for SearchModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                // Damage B only after A has assembled its run and model request.
                *self.fault.lock().unwrap() =
                    Some(FileFault::inject(&self.db, self.bad, self.tail));
                response_function_call(
                    "audit-repair-isolation",
                    "session_search",
                    serde_json::json!({"query":self.query}),
                )
            }
            1 => {
                let raw = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == "audit-repair-isolation"
                    })
                    .expect("search observation must reach the next model request")["output"]
                    .as_str()
                    .unwrap();
                *self.output.lock().unwrap() = Some(serde_json::from_str(raw).unwrap());
                response_text("Offline isolation probe finished.")
            }
            _ => panic!("unexpected additional model request"),
        };
        emit_responses_events(sink, &events)
    }
}

async fn observe_tool(tail: Tail, query: &'static str) -> bool {
    let fixture = HistoryFixture::new();
    let settings = Arc::new(SettingsService::new(
        fixture.db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .set_model(ModelRole::Chat, "deepseek-flash")
        .unwrap();
    settings
        .set_model(ModelRole::Reasoner, "deepseek-flash")
        .unwrap();
    let model = Arc::new(SearchModel {
        db: fixture.db.clone(),
        bad: fixture.bad,
        query,
        tail,
        calls: AtomicUsize::new(0),
        fault: Mutex::new(None),
        output: Mutex::new(None),
    });
    let chat = ChatService::new(
        fixture.db.clone(),
        settings,
        model.clone(),
        fixture.workspace.path(),
    )
    .with_projects(Arc::new(ProjectService::new(fixture.db.clone())));
    let answer = chat
        .run_in_session(
            "Look up the earlier decision in this project.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-repair-isolation-run"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let current: SessionId = answer.parse().unwrap();
    assert_eq!(
        EventStore::new(&fixture.db)
            .get_session(current)
            .unwrap()
            .unwrap()
            .project,
        Some(fixture.good_project.clone())
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let output = model.output.lock().unwrap().clone().unwrap();
    let found = output["status"] == "ok"
        && output["result"]["results"].as_array().is_some_and(|hits| {
            hits.len() == 1 && hits[0]["session_id"] == fixture.good.to_string()
        });
    if !found {
        assert_eq!(output["status"], "error");
        assert!(output.to_string().contains("invalid zstd frame"));
    }
    let events = chat.run_events("audit-repair-isolation-run", None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    // End fault injection before releasing the fixture, and prove A still works.
    drop(model.fault.lock().unwrap().take());
    let restored = AppService::from_shared(fixture.db.clone())
        .search_sessions(query, Some(&fixture.good_project), 10)
        .unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].session.id, fixture.good.to_string());
    eprintln!("repair isolation tool: tail={tail:?}, query={query}, active_project=A, wire_has_good_history={found}, unique_succeeded=true, restore_recovers=true");
    found
}

#[tokio::test]
#[ignore = "audit: corrupt B must not make A's session_search return an error to the model"]
async fn audit_other_corrupt_session_does_not_fail_model_search() {
    let mut results = Vec::new();
    for query in QUERIES {
        results.push(observe_tool(Tail::InvalidFrame, query).await);
    }
    assert!(results.into_iter().all(|found| found));
}

#[tokio::test]
async fn healthy_and_torn_other_session_keep_model_search_usable() {
    for tail in [Tail::Healthy, Tail::TornFrame] {
        for query in QUERIES {
            assert!(observe_tool(tail, query).await);
        }
    }
}
