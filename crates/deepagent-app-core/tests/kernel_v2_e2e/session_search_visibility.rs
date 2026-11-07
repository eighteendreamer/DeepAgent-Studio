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
    cross_project: bool,
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
                if self.cross_project {
                    arguments["cross_project"] = serde_json::json!(true);
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
    observe_tool_visibility(query, limit, deleted.then_some(Hidden::Deleted)).await
}

async fn observe_tool_visibility(
    query: &'static str,
    limit: usize,
    visibility: Option<Hidden>,
) -> bool {
    let workspace = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(workspace.path().join("state.sqlite")).unwrap());
    let projects = Arc::new(ProjectService::new(db.clone()));
    let closed_project = "G:/synthetic/h228/tool-closed-project";
    let cross_project = matches!(visibility, Some(Hidden::RemovedProject));
    if cross_project {
        projects.add_project(closed_project).unwrap();
    }
    let deleted = matches!(visibility, Some(Hidden::Deleted));
    let live = seed(&db, None, 100);
    let hidden: Vec<_> = (0..limit)
        .map(|index| {
            seed(
                &db,
                cross_project.then_some(closed_project),
                200 + index as i64 * 10,
            )
        })
        .collect();
    if let Some(visibility) = visibility {
        hide(&db, &hidden, visibility, closed_project);
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
        cross_project,
        calls: AtomicUsize::new(0),
        output: Mutex::new(None),
    });
    let chat =
        ChatService::new(db, settings, model.clone(), workspace.path()).with_projects(projects);
    chat.run_in_session(
        "Look up the earlier decision; include all projects when requested by this synthetic fixture.",
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
    assert_eq!(parsed["result"]["complete"], true);
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
async fn h228_formal_model_tool_keeps_archived_and_explicit_cross_project_semantics() {
    for visibility in [Hidden::Archived, Hidden::RemovedProject] {
        for query in QUERIES {
            assert!(observe_tool_visibility(query, 1, Some(visibility)).await);
        }
    }
}

#[test]
fn h228_inflight_owner_snapshot_is_stable_and_next_app_search_refreshes_visibility() {
    use deepagent_persistence::event_store::{
        SessionSearchControl, SessionSearchFilter, SessionSearchScope,
    };
    let db = Arc::new(Database::open_in_memory().unwrap());
    let older = seed(&db, None, 100);
    let newest = seed(&db, None, 200);
    let excluded = ArchiveService::new(db.clone()).archived_ids().unwrap();
    let projects = ProjectService::new(db.clone()).registered_paths().unwrap();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let done_rx = Mutex::new(done_rx);
    let first = std::sync::atomic::AtomicBool::new(true);
    std::thread::scope(|scope| {
        let worker_db = db.clone();
        scope.spawn(move || {
            go_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            ArchiveService::new(worker_db)
                .archive_session(&newest.to_string())
                .unwrap();
            done_tx.send(()).unwrap();
        });
        let cancelled = || {
            if first.swap(false, Ordering::SeqCst) {
                go_tx.send(()).unwrap();
                done_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            false
        };
        let report = EventStore::new(&db)
            .search_with_control_filtered(
                "检",
                SessionSearchScope::All,
                1,
                &SessionSearchControl::new(std::time::Duration::from_secs(5), &cancelled),
                Some(SessionSearchFilter {
                    excluded_sessions: Some(&excluded),
                    allowed_projects: Some(&projects),
                }),
            )
            .unwrap();
        assert!(report.unavailable.is_empty());
        assert_eq!(report.hits[0].session.id, newest);
    });
    let app = AppService::from_shared(db);
    let fresh = app.search_sessions_with_status("检", None, 1).unwrap();
    assert!(fresh.complete);
    assert_eq!(fresh.results[0].session.id, older.to_string());
}

#[tokio::test]
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

#[test]
fn h228_interleaved_visibility_refresh_and_file_reopen_keep_the_visible_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("visibility.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let projects = ProjectService::new(db.clone());
    let open = "G:/synthetic/h228/open'[]";
    let closed = "G:/synthetic/h228/closed' OR 1=1 --";
    projects.add_project(open).unwrap();
    projects.add_project(closed).unwrap();
    let live = seed(&db, None, 100);
    let project_live = seed(&db, Some(open), 300);
    let archived = seed(&db, None, 200);
    let mut hidden = vec![archived];
    let archive = ArchiveService::new(db.clone());
    archive.archive_session(&archived.to_string()).unwrap();
    for i in 0..24 {
        let id = seed(
            &db,
            Some(if i % 2 == 0 { open } else { closed }),
            400 + i * 10,
        );
        hidden.push(id);
        if i % 2 == 0 {
            archive.archive_session(&id.to_string()).unwrap();
        }
    }
    projects.remove_project(closed).unwrap();
    let app = AppService::from_shared(db.clone());
    for query in QUERIES {
        for limit in [1, 2, 50] {
            let report = app.search_sessions_with_status(query, None, limit).unwrap();
            assert!(report.complete);
            let ids = report
                .results
                .iter()
                .map(|h| h.session.id.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                ids,
                [project_live, live]
                    .into_iter()
                    .take(limit.min(2))
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
            );
            assert!(report.unavailable.is_empty());
            assert!(ids
                .iter()
                .all(|id| !hidden.iter().any(|hidden| hidden.to_string() == *id)));
        }
        assert_eq!(app.search_sessions(query, Some(open), 50).unwrap().len(), 1);
        assert!(app
            .search_sessions(query, Some(closed), 50)
            .unwrap()
            .is_empty());
    }
    archive.archive_session(&project_live.to_string()).unwrap();
    assert_eq!(
        app.search_sessions("检", None, 1).unwrap()[0].session.id,
        live.to_string()
    );
    archive
        .unarchive_session(&project_live.to_string())
        .unwrap();
    projects.remove_project(open).unwrap();
    assert_eq!(
        app.search_sessions("检", None, 1).unwrap()[0].session.id,
        live.to_string()
    );
    projects.add_project(open).unwrap();
    assert_eq!(
        app.search_sessions("检", None, 1).unwrap()[0].session.id,
        project_live.to_string()
    );
    let authority = EventStore::new(&db).load_session(live).unwrap();
    drop(app);
    drop(archive);
    drop(projects);
    drop(db);
    let db = Arc::new(Database::open(&path).unwrap());
    let app = AppService::from_shared(db.clone());
    for query in QUERIES {
        let report = app.search_sessions_with_status(query, None, 50).unwrap();
        assert!(report.complete);
        assert_eq!(report.results.len(), 2);
    }
    assert_eq!(EventStore::new(&db).load_session(live).unwrap(), authority);
}

#[test]
fn h228_hidden_corrupt_authority_does_not_consume_visible_maintenance_or_health() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("health.sqlite")).unwrap());
    let live = seed(&db, None, 100);
    let hidden = seed(&db, None, 200);
    ArchiveService::new(db.clone())
        .archive_session(&hidden.to_string())
        .unwrap();
    let file = std::fs::read_dir(db.session_transcript_dir(hidden))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".v1.jsonl.zst"))
        .unwrap();
    let mut corrupt = std::fs::read(&file).unwrap();
    corrupt.extend_from_slice(&[0xff]);
    std::fs::write(&file, &corrupt).unwrap();
    let app = AppService::from_shared(db.clone());
    let report = app.search_sessions_with_status("检", None, 1).unwrap();
    assert!(report.complete);
    assert_eq!(report.results[0].session.id, live.to_string());
    let health:usize=db.with_conn(|c|c.query_row("SELECT count(*) FROM documents WHERE collection='session_file_health_v1' AND id=?1",[hidden.to_string()],|r|r.get(0)).map_err(|e|deepagent_core::CoreError::Persistence(e.to_string()))).unwrap();
    assert_eq!(
        health, 0,
        "ineligible authority must not be visited by the visible search's maintenance budget"
    );
    let raw = EventStore::new(&db)
        .search_with_status(
            "检",
            deepagent_persistence::event_store::SessionSearchScope::All,
            10,
        )
        .unwrap();
    assert!(raw
        .unavailable
        .iter()
        .any(|issue| issue.session_id == hidden));
    assert!(
        app.search_sessions_with_status("检", None, 1)
            .unwrap()
            .complete
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        corrupt,
        "search may not rewrite authority bytes"
    );
}
