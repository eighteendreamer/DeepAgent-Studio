//! Two real ChatService sessions interleave after assembly; only model I/O is
//! replaced. Tools, child registries, workflow, run ledgers and vault I/O are real.
use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{KnowledgeDraftDto, KnowledgeService, ProjectService};
use deepagent_runtime::{
    workflow::{NodeExecutionStatus, WorkflowRequest},
    RuntimeEvent,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Notify;

const QUERY: &str = "deployment workspace routing evidence";
const A: &str = "H51_RUN_A deployment workspace routing evidence";
const B: &str = "H51_RUN_B deployment workspace routing evidence";
const MODEL: &str = "deepseek-v4-flash";

#[derive(Default)]
struct InterleavedModel {
    a: AtomicUsize,
    b: AtomicUsize,
    child: AtomicUsize,
    entered: Notify,
    release: Notify,
    requests: Mutex<Vec<(String, Value)>>,
    isolated: bool,
}

#[async_trait]
impl HttpTransport for InterleavedModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let input = body["input"].to_string();
        let (route, item) = if input.contains("H51_RESUME_A") {
            ("resume", text("resume-answer", "Resumed original project."))
        } else if input.contains("H51_CHILD") && !input.contains("H51_RUN_A") {
            let step = self.child.fetch_add(1, Ordering::SeqCst);
            (
                "child",
                match step {
                    0 => call(
                        "child-search",
                        "knowledge_search",
                        json!({"query":QUERY,"limit":10}),
                    ),
                    1 => text("child-answer", "Child project lookup complete."),
                    _ => return Err(CoreError::other("unexpected child model call")),
                },
            )
        } else if input.contains("H51_RUN_B") {
            let step = self.b.fetch_add(1, Ordering::SeqCst);
            (
                "b",
                match step {
                    0 => call(
                        "b-write",
                        "memory_write",
                        json!({"tier":"workspace","scope":"project","content":"B_RUN_NOTE beta private write"}),
                    ),
                    // Omitted scope retains the existing user-global default.
                    1 => call(
                        "b-global",
                        "memory_write",
                        json!({"tier":"workspace","content":"SHARED_GLOBAL_NOTE deployment workspace routing evidence"}),
                    ),
                    2 => call(
                        "b-recall",
                        "memory_recall",
                        json!({"tier":"workspace","limit":20}),
                    ),
                    3 => text("b-answer", "Beta completed."),
                    _ => return Err(CoreError::other("unexpected beta model call")),
                },
            )
        } else {
            assert!(input.contains("H51_RUN_A"));
            let step = self.a.fetch_add(1, Ordering::SeqCst);
            if step == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            (
                "a",
                match step {
                    0 => call(
                        "a-write",
                        "memory_write",
                        json!({"tier":"workspace","scope":"project","content":"A_RUN_NOTE alpha private write"}),
                    ),
                    1 => call(
                        "a-recall",
                        "memory_recall",
                        json!({"tier":"workspace","limit":20}),
                    ),
                    2 => call(
                        "a-search",
                        "knowledge_search",
                        json!({"query":QUERY,"limit":10}),
                    ),
                    3 => call(
                        "a-child",
                        "task",
                        json!({"description":"project knowledge lookup","prompt":"H51_CHILD deployment workspace routing evidence",
                    "isolation":if self.isolated {"worktree"} else {"shared"}}),
                    ),
                    4 => text("a-answer", "Alpha completed."),
                    _ => return Err(CoreError::other("unexpected alpha model call")),
                },
            )
        };
        self.requests.lock().unwrap().push((route.into(), body));
        emit_responses_events(
            sink,
            &completed(
                item["id"].as_str().unwrap(),
                MODEL,
                item.clone(),
                json!({"input_tokens":20,"output_tokens":8,"total_tokens":28}),
            ),
        )
    }
}

fn seed(service: &KnowledgeService, owner: &str) {
    service
        .save(KnowledgeDraftDto {
            title: format!("{owner} deployment"),
            body: format!("{QUERY} {owner}"),
            kind: Some("memory".into()),
            scope: Some("project".into()),
            tier: Some("workspace".into()),
            tags: vec![],
            source_session: None,
            importance: Some(0.8),
        })
        .unwrap();
}

fn output(body: &Value, id: &str) -> String {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .unwrap_or_else(|| panic!("missing real tool result for {id}"))
        .to_string()
}

async fn run(chat: &ChatService, prompt: &str, session: &str, run: &str) {
    tokio::time::timeout(
        Duration::from_secs(20),
        chat.run_in_session(
            prompt,
            Some(session),
            None,
            None,
            vec![],
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        ),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn two_sessions_keep_project_memory_across_ui_switch_child_rebind_resume_and_workflow() {
    for isolated in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("alpha");
        let b = tmp.path().join("beta");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        if isolated {
            run_git(&a, &["init"]);
            run_git(
                &a,
                &[
                    "-c",
                    "user.name=Offline Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "fixture",
                ],
            );
        }
        let db_path = tmp.path().join("state.sqlite");
        let db = Arc::new(Database::open(&db_path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings
            .initialize("offline-fixture-placeholder")
            .await
            .unwrap();
        settings
            .set_active_permission_preset(PermissionPreset::FullAccess)
            .unwrap();
        settings
            .set_tool_search_mode(deepagent_builtins::ToolSearchMode::Disabled)
            .unwrap();
        let projects = Arc::new(ProjectService::new(db.clone()));
        let knowledge = Arc::new(KnowledgeService::open(&a, &tmp.path().join("global")).unwrap());
        knowledge.set_auto_capture(false);
        seed(&knowledge, "OWNER_ALPHA");
        let model = Arc::new(InterleavedModel {
            isolated,
            ..Default::default()
        });
        let chat = Arc::new(
            ChatService::new(db.clone(), settings, model.clone(), tmp.path())
                .with_projects(projects.clone())
                .with_knowledge(knowledge.clone()),
        );
        projects.add_project(a.to_str().unwrap()).unwrap();
        let session_a = chat.create_session(None).unwrap();
        let worker = chat.clone();
        let sa = session_a.clone();
        let alpha = tokio::spawn(async move { run(&worker, A, &sa, "h51-alpha").await });
        tokio::time::timeout(Duration::from_secs(15), model.entered.notified())
            .await
            .unwrap();

        // UI switches while A's assembled run is awaiting its model response.
        projects.add_project(b.to_str().unwrap()).unwrap();
        knowledge.activate_project(&b).unwrap();
        seed(&knowledge, "OWNER_BETA");
        let session_b = chat.create_session(None).unwrap();
        run(&chat, B, &session_b, "h51-beta").await;
        model.release.notify_one();
        tokio::time::timeout(Duration::from_secs(20), alpha)
            .await
            .unwrap()
            .unwrap();
        assert!(
            knowledge
                .list()
                .iter()
                .all(|entry| !entry.body.contains("A_RUN_NOTE")),
            "a run must not change the UI's selection"
        );

        {
            let requests = model.requests.lock().unwrap();
            let last = |route| {
                &requests
                    .iter()
                    .rev()
                    .find(|(name, _)| name == route)
                    .unwrap()
                    .1
            };
            for (route, id) in [
                ("a", "a-recall"),
                ("a", "a-search"),
                ("child", "child-search"),
            ] {
                let result = output(last(route), id);
                assert!(result.contains("OWNER_ALPHA"), "{route}/{id}: {result}");
                assert!(!result.contains("OWNER_BETA"), "{route}/{id}: {result}");
            }
            let recalled = output(last("a"), "a-recall");
            assert!(recalled.contains("A_RUN_NOTE"));
            assert!(
                recalled.contains("SHARED_GLOBAL_NOTE"),
                "global writes remain shared"
            );
            let beta = output(last("b"), "b-recall");
            assert!(beta.contains("OWNER_BETA") && beta.contains("B_RUN_NOTE"));
            assert!(!beta.contains("OWNER_ALPHA") && !beta.contains("A_RUN_NOTE"));
            let initial_a = &requests.iter().find(|(route, _)| route == "a").unwrap().1;
            let initial_b = &requests.iter().find(|(route, _)| route == "b").unwrap().1;
            // KnowledgeReminderProvider attaches to user input, not the leading
            // instructions. Assert the real wire surface rather than moving it.
            assert!(initial_a["input"].to_string().contains("OWNER_ALPHA"));
            assert!(!initial_a["input"].to_string().contains("OWNER_BETA"));
            assert!(initial_b["input"].to_string().contains("OWNER_BETA"));
            assert!(!initial_b["input"].to_string().contains("OWNER_ALPHA"));
        }
        // Removing A from the sidebar is not authority to retarget its session.
        projects.remove_project(a.to_str().unwrap()).unwrap();
        run(
            &chat,
            "H51_RESUME_A deployment workspace routing evidence",
            &session_a,
            "h51-resume",
        )
        .await;
        let resumed = model
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|(route, _)| route == "resume")
            .unwrap()
            .1
            .clone();
        assert!(resumed["input"].to_string().contains("OWNER_ALPHA"));
        assert!(!resumed["input"].to_string().contains("OWNER_BETA"));
        assert!(knowledge
            .list()
            .iter()
            .any(|entry| entry.body.contains("OWNER_BETA")));
        assert!(!knowledge
            .list()
            .iter()
            .any(|entry| entry.body.contains("OWNER_ALPHA")));

        let workflow: WorkflowRequest = serde_json::from_value(json!({"definition":{"version":1,
            "nodes":[{"id":"start","kind":"start"},{"id":"retrieve","kind":"knowledge-retrieval",
                "config":{"queryVariable":QUERY,"knowledgeTopK":10,"scoreThreshold":0}},{"id":"end","kind":"end"}],
            "edges":[{"id":"e1","source":"start","target":"retrieve"},{"id":"e2","source":"retrieve","target":"end"}]}})).unwrap();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let capture = observed.clone();
        chat.run_workflow(
            workflow,
            "h51-workflow".into(),
            session_a,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await
        .unwrap();
        let nodes = observed.lock().unwrap();
        let documents = nodes
            .iter()
            .find(|event| {
                event.node_id == "retrieve" && event.status == NodeExecutionStatus::Completed
            })
            .unwrap()
            .outputs
            .as_ref()
            .unwrap()
            .to_string();
        assert!(documents.contains("OWNER_ALPHA"));
        assert!(documents.contains("SHARED_GLOBAL_NOTE"));
        assert!(!documents.contains("OWNER_BETA"));
        drop(nodes);
        assert!(knowledge
            .list()
            .iter()
            .any(|entry| entry.body.contains("OWNER_BETA")));
        assert!(!knowledge
            .list()
            .iter()
            .any(|entry| entry.body.contains("OWNER_ALPHA")));
        for (path, expected, absent) in [
            (&a, "A_RUN_NOTE", "B_RUN_NOTE"),
            (&b, "B_RUN_NOTE", "A_RUN_NOTE"),
        ] {
            let reopened = KnowledgeService::open(path, &tmp.path().join("global")).unwrap();
            assert!(reopened
                .list()
                .iter()
                .any(|entry| entry.body.contains(expected)));
            assert!(!reopened
                .list()
                .iter()
                .any(|entry| entry.body.contains(absent)));
        }
        let reopened = Database::open(&db_path).unwrap();
        let store = deepagent_persistence::run_store::RunStore::new(&reopened);
        for run in ["h51-alpha", "h51-beta", "h51-resume", "h51-workflow"] {
            assert_eq!(
                store.get(run).unwrap().unwrap().terminal_kind.as_deref(),
                Some("succeeded")
            );
            assert_eq!(
                store
                    .events_after(run, None)
                    .unwrap()
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
        }
    }
}
