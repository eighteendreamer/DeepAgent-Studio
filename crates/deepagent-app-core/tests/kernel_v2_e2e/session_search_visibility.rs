//! Search limits must count eligible results, not hidden/deleted candidates.

use super::*;
use deepagent_app_core::{AppService, ArchiveService, ProjectService};
use deepagent_core::{clock::Timestamp, event::EventPayload, id::SessionId, message::Message};
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;

const BODY: &str = "检 缓存 searchneedle earlier decision";
const QUERIES: [&str; 3] = ["检", "缓存", "searchneedle"];

#[derive(Clone, Copy, Debug)]
enum Hidden {
    Archived,
    Deleted,
    RemovedProject,
}

fn seed(db: &Database, project: Option<&str>, timestamp: i64) -> SessionId {
    let store = EventStore::new(db);
    let id = SessionId::new();
    store
        .create_session_full(
            id,
            Some("synthetic search history"),
            Default::default(),
            project,
            Timestamp::from_millis(timestamp),
        )
        .unwrap();
    store
        .append(
            id,
            EventPayload::MessageAppended {
                message: Message::user(BODY),
            },
            Timestamp::from_millis(timestamp + 1),
        )
        .unwrap();
    id
}

fn hide(db: &Arc<Database>, ids: &[SessionId], mode: Hidden, project: &str) {
    if matches!(mode, Hidden::RemovedProject) {
        assert!(ProjectService::new(db.clone())
            .remove_project(project)
            .unwrap());
        return;
    }
    let archive = ArchiveService::new(db.clone());
    for id in ids {
        assert!(archive.archive_session(&id.to_string()).unwrap());
        if matches!(mode, Hidden::Deleted) {
            assert!(archive.delete_archived_session(&id.to_string()).unwrap());
            assert!(EventStore::new(db).load_session(*id).unwrap().is_empty());
        }
    }
}

fn observe_app(mode: Hidden, limit: usize) -> Vec<usize> {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let projects = ProjectService::new(db.clone());
    let project = "/audit/synthetic-hidden-project";
    projects.add_project(project).unwrap();
    let live = seed(&db, None, 100);
    let hidden: Vec<_> = (0..limit)
        .map(|index| {
            seed(
                &db,
                matches!(mode, Hidden::RemovedProject).then_some(project),
                200 + index as i64 * 10,
            )
        })
        .collect();
    let app = AppService::from_shared(db.clone());
    for query in QUERIES {
        let before = app.search_sessions(query, None, limit).unwrap();
        assert_eq!(before.len(), limit);
        assert!(before.iter().all(|hit| hit.session.id != live.to_string()));
    }
    hide(&db, &hidden, mode, project);
    let visible = app.list_sessions().unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, live.to_string());
    QUERIES
        .iter()
        .map(|query| {
            let hits = app.search_sessions(query, None, limit).unwrap();
            assert!(hits.iter().all(|hit| hit.session.id == live.to_string()));
            // For the small fixture, a larger limit proves the live match exists.
            if limit == 1 {
                let wider = app.search_sessions(query, None, 2).unwrap();
                assert_eq!(wider.len(), 1);
                assert_eq!(wider[0].session.id, live.to_string());
            }
            assert_eq!(EventStore::new(&db).load_session(live).unwrap().len(), 1);
            eprintln!(
                "search visibility: mode={mode:?}, query={query}, limit={limit}, hidden={}, live_listed=true, hits={}",
                hidden.len(), hits.len()
            );
            hits.len()
        })
        .collect()
}

#[test]
#[ignore = "audit: hidden candidates consume the visible session search limit"]
fn audit_app_search_fills_limit_after_visibility_filter() {
    let mut counts = Vec::new();
    for mode in [Hidden::Archived, Hidden::Deleted, Hidden::RemovedProject] {
        for limit in [1, 50] {
            counts.extend(observe_app(mode, limit));
        }
    }
    assert!(counts.iter().all(|count| *count == 1));
}

struct SearchModel {
    query: &'static str,
    limit: usize,
    calls: AtomicUsize,
    output: Mutex<Option<String>>,
}

#[async_trait]
impl HttpTransport for SearchModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert!(body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "session_search"));
                let mut arguments = serde_json::json!({"query":self.query});
                if self.limit != 10 {
                    arguments["limit"] = serde_json::json!(self.limit);
                }
                response_function_call("audit-search-visibility", "session_search", arguments)
            }
            1 => {
                let output = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == "audit-search-visibility"
                    })
                    .expect("paired search output must reach the next model request")["output"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                *self.output.lock().unwrap() = Some(output);
                response_text("Offline lookup finished.")
            }
            _ => panic!("unexpected additional model request"),
        };
        emit_responses_events(sink, &events)
    }
}

async fn observe_tool(query: &'static str, limit: usize, deleted: bool) -> bool {
    let workspace = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(workspace.path().join("state.sqlite")).unwrap());
    let live = seed(&db, None, 100);
    let hidden: Vec<_> = (0..limit)
        .map(|index| seed(&db, None, 200 + index as i64 * 10))
        .collect();
    if deleted {
        hide(&db, &hidden, Hidden::Deleted, "unused");
    }
    let settings = Arc::new(SettingsService::new(
        db.clone(),
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
        query,
        limit,
        calls: AtomicUsize::new(0),
        output: Mutex::new(None),
    });
    let projects = Arc::new(ProjectService::new(db.clone()));
    let chat =
        ChatService::new(db, settings, model.clone(), workspace.path()).with_projects(projects);
    chat.run_in_session(
        "Look up the earlier decision.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-search-visibility-run"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let events = chat
        .run_events("audit-search-visibility-run", None)
        .unwrap();
    let completed: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed")
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].data["ok"], true);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let output = model.output.lock().unwrap().clone().unwrap();
    let expected_id = if deleted {
        live
    } else {
        *hidden.last().unwrap()
    };
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["status"], "ok");
    let results = parsed["result"]["results"]
        .as_array()
        .expect("structured search results");
    let found = results
        .iter()
        .any(|hit| hit["session_id"] == expected_id.to_string());
    assert_eq!(
        results.len(),
        if deleted { usize::from(found) } else { limit }
    );
    assert!(!deleted || hidden.iter().all(|id| !output.contains(&id.to_string())));
    eprintln!("search tool: query={query}, limit={limit}, deleted={deleted}, tool_ok=true, wire_results={}, next_request_has_expected_history={found}, unique_succeeded=true", results.len());
    found
}

#[tokio::test]
#[ignore = "audit: deleted search hits must not starve live history in the model request"]
async fn audit_deleted_candidates_do_not_starve_tool_search() {
    let mut observations = Vec::new();
    for query in QUERIES {
        for limit in [1, 10, 20] {
            observations.push(observe_tool(query, limit, true).await);
        }
    }
    assert!(observations.into_iter().all(|found| found));
}

#[tokio::test]
async fn visible_search_candidates_reach_next_model_request() {
    for query in QUERIES {
        for limit in [1, 10, 20] {
            let found = observe_tool(query, limit, false).await;
            // With healthy candidates the requested page is full, not just one hit.
            assert!(found);
        }
    }
}
