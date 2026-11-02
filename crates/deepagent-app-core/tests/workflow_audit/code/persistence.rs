//! Real SQLite/Kernel regressions. Only the explicit managed-Node opt-in test
//! below also exercises interpreter execution; normal CI needs no host SDK.
use super::*;
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::run_store::StoredRunEvent;

async fn disk_fixture() -> (tempfile::TempDir, Arc<Database>, ChatService, String) {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("code.db")).unwrap());
    super::super::workflow_fixture_with_database(root, db).await
}

fn assert_replay(db: &Database, run_id: &str, before: &[StoredRunEvent]) {
    let store = RunStore::new(db);
    let after = store.events_after(run_id, None).unwrap();
    assert_eq!(after, before);
    assert!(after
        .windows(2)
        .all(|events| events[0].sequence < events[1].sequence));
    let cursor = after[after.len() / 2].sequence;
    assert_eq!(
        store.events_after(run_id, Some(cursor)).unwrap(),
        after
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(!after.iter().any(|event| {
        let text = event.data.to_string();
        text.contains("H178_PRIVATE_SOURCE") || text.contains("H178_PRIVATE_INPUT")
    }));
}

#[tokio::test]
#[ignore = "requires explicit managed Node and unpackaged terminal; real success/error SQLite reopen/replay"]
async fn h178_real_approved_node_success_and_errors_survive_reopen_without_redispatch() {
    for (label, script, succeeds) in [
        (
            "success",
            "function main({input}) { /* H178_PRIVATE_SOURCE */ return {result:input*7}; }",
            true,
        ),
        ("syntax", "function main( { H178_PRIVATE_SOURCE", false),
        (
            "runtime",
            "function main({privateValue}) { throw new Error(privateValue); }",
            false,
        ),
        ("missing", "function main() { return {}; }", false),
        ("type", "function main() { return {result:'42'}; }", false),
    ] {
        let (root, db, chat, session) = disk_fixture().await;
        let (_active, chat) = with_real_node(chat);
        let responder = chat.clone();
        let approvals = Arc::new(AtomicUsize::new(0));
        let count = approvals.clone();
        let run_id = format!("h178-real-durable-{label}");
        let mut request = numeric_workflow(script);
        request.definition.nodes[1].config.insert("codeInputVariables".into(),
            json!([{"name":"input","value":6}, {"name":"privateValue","value":"H178_PRIVATE_INPUT"}]));
        let (result, events) = run(&chat, request, &run_id, session, move |approval| {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(approval.tool, "canvas_code_execute");
            assert!(responder
                .resolve_approval(&approval.call_id, true, "h178 synthetic SDK approval")
                .unwrap());
        })
        .await;
        assert_eq!(approvals.load(Ordering::SeqCst), 1);
        if succeeds {
            assert!(result.is_ok(), "actual SDK result required: {result:?}");
            let terminal: Vec<_> = events
                .iter()
                .filter(|event| event.node_id == "code" && event.status.is_terminal())
                .collect();
            assert_eq!(terminal.len(), 1);
            assert_eq!(terminal[0].status, NodeExecutionStatus::Completed);
            assert_eq!(terminal[0].outputs, Some(json!({"result":42})));
            assert_eq!(
                terminal[0].updates["codeExecution"]["isolation"],
                "windows_lpac_v1"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.node_id == "end"
                        && event.status == NodeExecutionStatus::Completed)
                    .count(),
                1
            );
        } else {
            assert_failed(&db, &run_id, &result, &events);
        }
        let before = RunStore::new(&db).events_after(&run_id, None).unwrap();
        assert_eq!(
            before
                .iter()
                .filter(|event| event.event_type == "tool_started")
                .count(),
            1
        );
        let approvals = RunControlStore::new(&db).list_approvals(&run_id).unwrap();
        assert_eq!(approvals.len(), 1);
        drop(chat);
        drop(db);
        let reopened = Database::open(root.path().join("code.db")).unwrap();
        assert_eq!(
            RunStore::new(&reopened)
                .get(&run_id)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(if succeeds { "succeeded" } else { "failed" })
        );
        assert_eq!(
            RunControlStore::new(&reopened)
                .list_approvals(&run_id)
                .unwrap(),
            approvals
        );
        assert_replay(&reopened, &run_id, &before);
        println!("production_approved_sqlite_reopen_replay={label} dispatch_count=1 terminal_count=1 private_arguments_absent=true");
    }
}

#[tokio::test]
async fn h178_approved_code_missing_sdk_failure_survives_reopen_without_redispatch() {
    let (root, db, chat, session) = disk_fixture().await;
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    let run_id = "h178-approved-sdk-failure";
    let (result, events) = run(
        &chat,
        request("javascript", SCRIPT),
        run_id,
        session,
        move |approval| {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(approval.tool, "canvas_code_execute");
            assert!(responder
                .resolve_approval(&approval.call_id, true, "h178 fixture approval")
                .unwrap());
        },
    )
    .await;
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    assert_failed(&db, run_id, &result, &events);
    assert!(events.iter().any(|event| event
        .error
        .as_deref()
        .is_some_and(|error| error.contains("CodeRuntimeUnavailable"))));
    let before = RunStore::new(&db).events_after(run_id, None).unwrap();
    assert_eq!(
        before
            .iter()
            .filter(|event| event.event_type == "tool_started")
            .count(),
        1
    );
    drop(chat);
    drop(db);
    let reopened = Database::open(root.path().join("code.db")).unwrap();
    assert_failed(&reopened, run_id, &result, &events);
    assert_replay(&reopened, run_id, &before);
}

#[tokio::test]
async fn h178_cancelled_code_approval_reopens_once_and_late_approval_cannot_dispatch() {
    let (root, db, chat, session) = disk_fixture().await;
    let canceller = chat.clone();
    let call_id = Arc::new(Mutex::new(None));
    let capture = call_id.clone();
    let run_id = "h178-cancelled-code-approval";
    let (result, events) = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            &chat,
            request("javascript", SCRIPT),
            run_id,
            session.clone(),
            move |approval| {
                assert_eq!(approval.tool, "canvas_code_execute");
                assert!(capture.lock().unwrap().replace(approval.call_id).is_none());
                assert!(canceller.cancel_session(run_id));
            },
        ),
    )
    .await
    .expect("cancellation must release the shared approval wait");
    // finish_admitted_run returns the session id for a committed cancellation;
    // the durable terminal and node events, not Result::is_ok, define success.
    assert_eq!(result.unwrap(), session);
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.node_id == "code" && event.status.is_terminal())
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].status, NodeExecutionStatus::Cancelled);
    assert!(terminal[0].outputs.is_none());
    assert!(!events
        .iter()
        .any(|event| matches!(event.node_id.as_str(), "code" | "end")
            && event.status == NodeExecutionStatus::Completed));
    let before = RunStore::new(&db).events_after(run_id, None).unwrap();
    assert!(!before
        .iter()
        .any(|event| event.event_type == "tool_started"));
    assert_eq!(
        RunStore::new(&db)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
    let approvals = RunControlStore::new(&db).list_approvals(run_id).unwrap();
    let call_id = call_id.lock().unwrap().take().unwrap();
    assert!(
        matches!(chat.resolve_approval(&call_id, true, "late h178 fixture approval"),
        Err(deepagent_core::error::CoreError::IllegalTransition { from, to })
        if from == "cancelled" && to == "approved")
    );
    assert_eq!(
        RunControlStore::new(&db).list_approvals(run_id).unwrap(),
        approvals
    );
    drop(chat);
    drop(db);
    let reopened = Database::open(root.path().join("code.db")).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        RunControlStore::new(&reopened)
            .list_approvals(run_id)
            .unwrap(),
        approvals
    );
    assert_replay(&reopened, run_id, &before);
}
