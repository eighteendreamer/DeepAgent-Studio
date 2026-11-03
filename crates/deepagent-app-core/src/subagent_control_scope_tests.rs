//! H-241: durable conversation authority precedes the shared cancellation flag.
use super::tests::audit_child_runner_with_db;
use super::*;
use deepagent_builtins::SubagentRunner;
use deepagent_models::transport::MockTransport;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::{SubagentRunRecord, SubagentRunStore};
use std::sync::atomic::Ordering;

struct Fixture {
    root: tempfile::TempDir,
    runner: ChatSubagentRunner,
    flag: Arc<AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("state.db")).unwrap());
        let runner = audit_child_runner_with_db(
            root.path(),
            Arc::new(MockTransport::new(Vec::<String>::new())),
            db.clone(),
        );
        let runs = RunStore::new(&db);
        let owner_session = runs.get("audit-parent").unwrap().unwrap().session_id;
        runs.create("later", &owner_session, None, 2).unwrap();
        let foreign = Session::create(&db, &SystemClock, None).unwrap();
        runs.create("foreign", &foreign.id().to_string(), None, 2)
            .unwrap();
        SubagentRunStore::new(&db)
            .create(&SubagentRunRecord {
                id: "child".into(),
                parent_run_id: "audit-parent".into(),
                origin_parent_run_id: "audit-parent".into(),
                state: "running".into(),
                agent_type: "general".into(),
                transcript_path: None,
                worktree_path: None,
                summary: None,
                created_at: 2,
                updated_at: 2,
                finished_at: None,
                resume_count: 0,
            })
            .unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        runner
            .background
            .lock()
            .unwrap()
            .insert("child".into(), flag.clone());
        Self { root, runner, flag }
    }

    fn caller(&self, parent: &str) -> ChatSubagentRunner {
        let mut caller = self.runner.clone();
        caller.parent_run_id = parent.into();
        caller
    }
}

#[tokio::test]
async fn h241_foreign_and_missing_callers_cannot_touch_a_live_flag() {
    let f = Fixture::new();
    for parent in ["foreign", "missing-parent"] {
        assert!(f.caller(parent).cancel("child").await.is_err());
        assert!(!f.flag.load(Ordering::Acquire));
    }
    assert_eq!(
        SubagentRunStore::new(&f.runner.db)
            .get("child")
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    assert!(f.caller("later").cancel("child").await.unwrap());
    assert!(!f.caller("later").cancel("child").await.unwrap());
}

#[tokio::test]
async fn h241_unknown_and_dangling_control_handles_are_not_authority() {
    let f = Fixture::new();
    assert!(!f.runner.cancel("absent").await.unwrap());
    let orphan = Arc::new(AtomicBool::new(false));
    f.runner
        .background
        .lock()
        .unwrap()
        .insert("orphan".into(), orphan.clone());
    assert!(!f.runner.cancel("orphan").await.unwrap());
    assert!(!orphan.load(Ordering::Acquire));
    assert!(!f.flag.load(Ordering::Acquire));
    f.runner.background.lock().unwrap().remove("child");
    assert!(!f.runner.cancel("child").await.unwrap());
    assert_eq!(
        SubagentRunStore::new(&f.runner.db)
            .get("child")
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
}

#[tokio::test]
async fn h241_storage_failure_is_closed_and_does_not_consume_a_retry() {
    let f = Fixture::new();
    // Fault injection affects only this fixture's file database, never a user DB.
    f.runner
        .db
        .with_conn(|conn| {
            conn.execute_batch("ALTER TABLE subagent_runs RENAME TO h241_unavailable")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(f.runner.cancel("child").await.is_err());
    assert!(!f.flag.load(Ordering::Acquire));
    f.runner
        .db
        .with_conn(|conn| {
            conn.execute_batch("ALTER TABLE h241_unavailable RENAME TO subagent_runs")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(f.runner.cancel("child").await.unwrap());
}

#[tokio::test]
async fn h241_terminal_child_rejects_foreign_authority_and_ignores_a_stale_flag() {
    let f = Fixture::new();
    SubagentRunStore::new(&f.runner.db)
        .finish("child", "succeeded", Some("normal result"), 3)
        .unwrap();
    assert!(f.caller("foreign").cancel("child").await.is_err());
    assert!(!f.runner.cancel("child").await.unwrap());
    assert!(!f.flag.load(Ordering::Acquire));
    assert_eq!(
        SubagentRunStore::new(&f.runner.db)
            .get("child")
            .unwrap()
            .unwrap()
            .summary
            .as_deref(),
        Some("normal result")
    );
}

#[tokio::test]
async fn h241_resumed_child_uses_durable_origin_after_database_reopen() {
    let f = Fixture::new();
    let store = SubagentRunStore::new(&f.runner.db);
    store.finish("child", "succeeded", Some("done"), 3).unwrap();
    store.resume("child", "later", 4).unwrap();
    let mut later = f.caller("later");
    later.db = Arc::new(Database::open(f.root.path().join("state.db")).unwrap());
    let mut foreign = later.clone();
    foreign.parent_run_id = "foreign".into();
    assert!(foreign.cancel("child").await.is_err());
    assert!(!f.flag.load(Ordering::Acquire));
    assert!(later.cancel("child").await.unwrap());
    let record = store.get("child").unwrap().unwrap();
    assert_eq!(record.parent_run_id, "later");
    assert_eq!(record.origin_parent_run_id, "audit-parent");
    assert_eq!(record.resume_count, 1);
    // Admission only signals: no forged durable terminal or result overwrite.
    assert_eq!(record.state, "running");
    assert_eq!(record.finished_at, None);
}

#[test]
fn h241_concurrent_foreign_requests_cannot_win_the_owner_cancellation() {
    let f = Fixture::new();
    for parent in ["foreign", "later"] {
        let caller = f.caller(parent);
        let barrier = std::sync::Barrier::new(16);
        let accepted = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let caller = &caller;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        futures::executor::block_on(caller.cancel("child"))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|result| match parent {
                    "foreign" => {
                        assert!(result.is_err());
                        false
                    }
                    _ => *result.as_ref().unwrap(),
                })
                .count()
        });
        assert_eq!(accepted, usize::from(parent == "later"));
        assert_eq!(f.flag.load(Ordering::Acquire), parent == "later");
    }
}
