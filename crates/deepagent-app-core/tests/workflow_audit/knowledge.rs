//! H176: scope is an execution contract, not a hint for the model.
use super::*;
use serde_json::{json, Value};

fn note(title: &str, body: &str, scope: &str) -> KnowledgeDraftDto {
    KnowledgeDraftDto {
        title: title.into(),
        body: body.into(),
        scope: Some(scope.into()),
        kind: Some("note".into()),
        tags: Vec::new(),
        source_session: None,
        tier: None,
        importance: None,
    }
}

fn request(selector: Option<Value>) -> WorkflowRequest {
    let mut config = json!({"queryVariable":"scope-needle","knowledgeTopK":1,"scoreThreshold":0});
    if let Some(selector) = selector {
        config["knowledgeBaseId"] = selector;
    }
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"retrieve","kind":"knowledge-retrieval","config":config},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"retrieve"},{"id":"b","source":"retrieve","target":"end"}]}})).unwrap()
}

async fn run(
    chat: &ChatService,
    session: &str,
    w: WorkflowRequest,
    id: &str,
) -> (
    deepagent_core::Result<String>,
    Vec<deepagent_runtime::workflow::NodeExecutionEvent>,
) {
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    let result = chat
        .run_workflow(
            w,
            id.into(),
            session.into(),
            move |e| {
                if let RuntimeEvent::WorkflowNode { event } = e {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("knowledge fixture must never request approval"),
        )
        .await;
    let events = nodes.lock().unwrap().clone();
    (result, events)
}

#[tokio::test]
async fn h176_scopes_filter_before_candidates_and_persist_provenance_on_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("scope.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let knowledge =
        Arc::new(KnowledgeService::open(root.path(), &root.path().join("global")).unwrap());
    knowledge
        .save(note(
            "Private",
            "scope-needle project lower rank with many extra words",
            "project",
        ))
        .unwrap();
    // More than both contextual candidate_pool and KnowledgeBase's oversampling.
    for i in 0..80 {
        knowledge
            .save(note(&format!("Shared {i}"), "scope-needle", "global"))
            .unwrap();
    }
    let (root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let chat = chat.with_knowledge(knowledge.clone());
    let mut saved = Vec::new();
    for (i, (selector, scope)) in [
        (Some(json!("project")), "project"),
        (Some(json!("global")), "global"),
        (Some(json!("mixed")), "mixed"),
        (Some(json!("")), "mixed"),
        (None, "mixed"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("h176-scope-{i}");
        let mut w = request(selector);
        if scope == "mixed" {
            w.definition.nodes[1]
                .config
                .insert("knowledgeTopK".into(), json!(20));
        }
        let (result, events) = run(&chat, &session, w, &id).await;
        assert!(result.is_ok(), "{result:?}");
        let out = events
            .iter()
            .find(|e| e.node_id == "retrieve" && e.status == NodeExecutionStatus::Completed)
            .unwrap()
            .outputs
            .as_ref()
            .unwrap();
        assert_eq!(out["scope"], scope);
        assert_eq!(out["revision"].as_str().unwrap().len(), 64);
        let docs = out["documents"].as_array().unwrap();
        assert!(
            !docs.is_empty(),
            "eligible project hit must not be starved by global candidates"
        );
        if scope != "mixed" {
            assert!(docs.iter().all(|d| d["scope"] == scope));
        }
        for d in docs {
            assert!(d["id"]
                .as_str()
                .unwrap()
                .starts_with(&format!("{}:", d["scope"].as_str().unwrap())));
            assert!(d["updatedAt"].is_i64());
        }
        assert!(docs
            .windows(2)
            .all(|w| w[0]["score"].as_f64().unwrap() >= w[1]["score"].as_f64().unwrap()));
        let store = RunStore::new(&db);
        let before = store.events_after(&id, None).unwrap();
        let durable: Vec<deepagent_runtime::workflow::NodeExecutionEvent> = before
            .iter()
            .filter(|e| e.event_type == "workflow_node")
            .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
            .collect();
        // The store serializes JSON once. Compare the same JSON round trip,
        // not serde_json's in-memory f64 vs decimal parser representation.
        let projected: Vec<deepagent_runtime::workflow::NodeExecutionEvent> =
            serde_json::from_slice(&serde_json::to_vec(&events).unwrap()).unwrap();
        assert_eq!(durable, projected);
        assert_eq!(
            store.get(&id).unwrap().unwrap().terminal_kind.as_deref(),
            Some("succeeded")
        );
        saved.push((id, before));
    }
    drop(chat);
    drop(db);
    drop(knowledge);
    let reopened = Database::open(&path).unwrap();
    for (id, before) in saved {
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after(&id, None).unwrap(), before);
        let cursor = before[0].sequence;
        assert_eq!(
            store.events_after(&id, Some(cursor)).unwrap(),
            before
                .into_iter()
                .filter(|e| e.sequence > cursor)
                .collect::<Vec<_>>()
        );
    }
    drop(reopened);
    drop(root);
}

#[tokio::test]
async fn h176_unknown_and_malformed_scope_fail_before_any_node_even_without_backend() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (i, selector) in [
        json!("other-project"),
        json!("{{#start.scope#}}"),
        json!(" project "),
        json!(null),
        json!(1),
        json!({}),
        json!([]),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("h176-invalid-{i}");
        let (result, nodes) = run(&chat, &session, request(Some(selector)), &id).await;
        assert!(result.unwrap_err().to_string().contains("knowledgeBaseId"));
        assert!(nodes.is_empty());
        let events = RunStore::new(&db).events_after(&id, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(!events.iter().any(|e| e.event_type == "workflow_node"));
    }
    let (result, nodes) = run(
        &chat,
        &session,
        request(Some(json!("project"))),
        "h176-no-backend",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    let out = nodes
        .iter()
        .find(|e| e.node_id == "retrieve" && e.status == NodeExecutionStatus::Completed)
        .unwrap()
        .outputs
        .as_ref()
        .unwrap();
    assert_eq!(out["documents"], json!([]));
    assert_eq!(out["scope"], "project");
    assert!(out["revision"].is_null());
    assert_eq!(out["__note"], "no knowledge retriever configured");
}

#[tokio::test]
async fn h176_workflow_adapter_freezes_content_while_existing_tool_search_remains_live() {
    use deepagent_runtime::workflow::KnowledgeRetriever;
    let root = tempfile::tempdir().unwrap();
    let service =
        Arc::new(KnowledgeService::open(root.path(), &root.path().join("global")).unwrap());
    service
        .save(note("Private", "scope-needle FROZEN", "project"))
        .unwrap();
    let workflow =
        deepagent_app_core::knowledge_service::WorkflowKnowledgeRetriever::new(service.clone());
    service
        .save(note("Private", "scope-needle LATER", "project"))
        .unwrap();
    assert!(service
        .search("scope-needle", None, 10)
        .iter()
        .any(|h| h.excerpt.contains("LATER")));
    let query = || deepagent_runtime::workflow::KnowledgeQuery {
        query: "scope-needle",
        limit: 10,
        scope: deepagent_runtime::workflow::KnowledgeScope::Project,
    };
    let frozen = workflow.search(query()).await.unwrap();
    let hits = &frozen.documents;
    assert!(hits.iter().any(|h| h.content.contains("FROZEN")));
    assert!(!hits.iter().any(|h| h.content.contains("LATER")));
    let current =
        deepagent_app_core::knowledge_service::WorkflowKnowledgeRetriever::new(service.clone())
            .search(query())
            .await
            .unwrap();
    assert_ne!(current.revision, frozen.revision);
    assert!(current
        .documents
        .iter()
        .any(|h| h.content.contains("LATER")));
    // Global mutations do not perturb a project-only revision or BM25 corpus.
    service
        .save(note(
            "Shared",
            "scope-needle shared other content",
            "global",
        ))
        .unwrap();
    let updated =
        deepagent_app_core::knowledge_service::WorkflowKnowledgeRetriever::new(service.clone())
            .search(query())
            .await
            .unwrap();
    assert_eq!(updated.revision, current.revision);
    assert_eq!(updated.documents, current.documents);
    service
        .activate_project(&root.path().join("other"))
        .unwrap();
    service
        .save(note("Other", "scope-needle OTHER_PROJECT", "project"))
        .unwrap();
    let repeated = workflow.search(query()).await.unwrap();
    assert_eq!(repeated.revision, frozen.revision);
    assert_eq!(repeated.documents, frozen.documents);
    let reopened =
        Arc::new(KnowledgeService::open(root.path(), &root.path().join("global")).unwrap());
    let reloaded = deepagent_app_core::knowledge_service::WorkflowKnowledgeRetriever::new(reopened)
        .search(query())
        .await
        .unwrap();
    assert_eq!(reloaded.revision, current.revision);
    assert_eq!(reloaded.documents, current.documents);
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; real serializer and Kernel consumer"]
async fn h176_desktop_producer_kernel_and_consumer_preserve_scope_and_provenance() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let emitted = std::process::Command::new("node")
        .arg(&script)
        .arg("--emit-knowledge-fixtures")
        .output()
        .unwrap();
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&emitted.stdout).unwrap();
    assert_eq!(fixtures.len(), 6);
    for fixture in fixtures {
        let (root, _db, chat, session) = workflow_fixture().await;
        let knowledge =
            Arc::new(KnowledgeService::open(root.path(), &root.path().join("global")).unwrap());
        knowledge
            .save(note("Private", "scope-needle PROJECT", "project"))
            .unwrap();
        knowledge
            .save(note("Shared", "scope-needle GLOBAL", "global"))
            .unwrap();
        let chat = chat.with_knowledge(knowledge);
        let (result, events) = run(
            &chat,
            &session,
            serde_json::from_value(fixture["workflow"].clone()).unwrap(),
            "h176-desktop",
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        let expected = events
            .iter()
            .find(|e| e.node_id == "retrieve" && e.status == NodeExecutionStatus::Completed)
            .unwrap()
            .outputs
            .clone()
            .unwrap();
        let scope = match fixture["name"].as_str().unwrap() {
            "knowledge-project" | "knowledge-new" => "project",
            "knowledge-global" => "global",
            _ => "mixed",
        };
        assert_eq!(expected["scope"], scope);
        assert_eq!(
            expected["documents"].as_array().unwrap().len(),
            if scope == "mixed" { 2 } else { 1 }
        );
        let replay = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(replay.path(), serde_json::to_vec(&json!({"name":fixture["name"],"workflow":fixture["workflow"],"events":events,"expected":expected})).unwrap()).unwrap();
        let consumed = std::process::Command::new("node")
            .arg(&script)
            .arg("--consume-knowledge-events")
            .arg(replay.path())
            .output()
            .unwrap();
        assert!(
            consumed.status.success(),
            "{}",
            String::from_utf8_lossy(&consumed.stderr)
        );
    }
}

#[tokio::test]
async fn h176_empty_query_and_threshold_keep_selected_snapshot_identity_without_hits() {
    let (root, _db, chat, session) = workflow_fixture().await;
    let knowledge =
        Arc::new(KnowledgeService::open(root.path(), &root.path().join("global")).unwrap());
    knowledge
        .save(note("Private", "scope-needle PROJECT", "project"))
        .unwrap();
    let chat = chat.with_knowledge(knowledge);
    let mut revisions = Vec::new();
    for (i, query, threshold) in [(0, "", 0.0), (1, "scope-needle", 1.0)] {
        let mut workflow = request(Some(json!("project")));
        workflow.definition.nodes[1]
            .config
            .insert("queryVariable".into(), json!(query));
        workflow.definition.nodes[1]
            .config
            .insert("scoreThreshold".into(), json!(threshold));
        let (result, nodes) = run(&chat, &session, workflow, &format!("h176-empty-{i}")).await;
        assert!(result.is_ok(), "{result:?}");
        let out = nodes
            .iter()
            .find(|e| e.node_id == "retrieve" && e.status == NodeExecutionStatus::Completed)
            .unwrap()
            .outputs
            .as_ref()
            .unwrap();
        assert_eq!(out["scope"], "project");
        assert_eq!(out["documents"], json!([]));
        assert_eq!(out["content"], "");
        assert_eq!(out["revision"].as_str().unwrap().len(), 64);
        revisions.push(out["revision"].clone());
    }
    assert_eq!(revisions[0], revisions[1]);
}
