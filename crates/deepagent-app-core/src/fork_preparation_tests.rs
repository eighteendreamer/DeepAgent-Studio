//! Host readiness participates in the existing fork/checkpoint transaction.
use super::*;
use deepagent_core::{clock::FixedClock, event::EventPayload, message::Message};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct Fixture {
    db: Arc<Database>,
    id: SessionId,
    target: std::path::PathBuf,
    _workspace: tempfile::TempDir,
    _storage: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let target = workspace.path().join("synthetic-file");
        std::fs::write(&target, "before").unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("synthetic fork preparation")).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("synthetic edit"),
            })
            .unwrap();
        let id = session.id();
        let sequence = EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .last()
            .unwrap()
            .sequence;
        RunStore::new(&db)
            .create("h216-checkpoint", &id.to_string(), None, 1_000)
            .unwrap();
        let checkpoint = CheckpointManager::new(
            db.clone(),
            "h216-checkpoint",
            sequence as i64,
            workspace.path(),
            storage.path(),
        )
        .unwrap();
        checkpoint.capture_before(&target).unwrap();
        checkpoint.commit().unwrap();
        std::fs::write(&target, "current").unwrap();
        Self {
            db,
            id,
            target,
            _workspace: workspace,
            _storage: storage,
        }
    }
    fn history(&self) -> serde_json::Value {
        serde_json::to_value(EventStore::new(&self.db).load_session(self.id).unwrap()).unwrap()
    }
}

#[test]
fn h216_failed_host_preparation_is_inside_control_before_workspace_or_child_publication() {
    let f = Fixture::new();
    let before = f.history();
    let service = AppService::from_shared(f.db.clone());
    let result: Result<(ForkResultDto, ())> =
        service.fork_session_prepared(&f.id.to_string(), 0, || {
            assert!(
                EventStore::new(&f.db).acquire_activity(f.id).is_err(),
                "preparation must retain source control"
            );
            assert_eq!(std::fs::read_to_string(&f.target).unwrap(), "current");
            Err(CoreError::invalid("synthetic host preparation failure"))
        });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("synthetic host preparation failure"));
    assert_eq!(f.history(), before);
    assert_eq!(std::fs::read_to_string(&f.target).unwrap(), "current");
    assert_eq!(EventStore::new(&f.db).list_sessions().unwrap().len(), 1);
    // Failure releases the same guard. A subsequent legitimate fork works.
    let (fork, prepared) = service
        .fork_session_prepared(&f.id.to_string(), 0, || Ok("prepared"))
        .unwrap();
    assert_eq!(prepared, "prepared");
    assert_ne!(fork.new_session_id, f.id.to_string());
    assert_eq!(std::fs::read_to_string(&f.target).unwrap(), "before");
    assert_eq!(f.history(), before);
    assert_eq!(EventStore::new(&f.db).list_sessions().unwrap().len(), 2);
}

#[test]
fn h216_fork_commit_failure_discards_prepared_owner_and_compensates_workspace() {
    struct Owner(Arc<AtomicBool>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let f = Fixture::new();
    let before = f.history();
    f.db.with_conn(|conn|conn.execute_batch(
        "CREATE TRIGGER h216_reject_fork BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'synthetic fork commit failure'); END;"
    ).map_err(|error|CoreError::Persistence(error.to_string()))).unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let service = AppService::from_shared(f.db.clone());
    let result = service.fork_session_prepared(&f.id.to_string(), 0, || Ok(Owner(dropped.clone())));
    assert!(result.is_err());
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(std::fs::read_to_string(&f.target).unwrap(), "current");
    assert_eq!(f.history(), before);
    assert_eq!(EventStore::new(&f.db).list_sessions().unwrap().len(), 1);
    assert!(EventStore::new(&f.db).acquire_activity(f.id).is_ok());
}

#[test]
fn h216_archived_or_busy_source_rejects_before_any_host_preparation() {
    let f = Fixture::new();
    let service = AppService::from_shared(f.db.clone());
    let archive = ArchiveService::new(f.db.clone());
    archive.archive_session(&f.id.to_string()).unwrap();
    let called = AtomicBool::new(false);
    assert!(service
        .fork_session_prepared(&f.id.to_string(), 0, || {
            called.store(true, Ordering::SeqCst);
            Ok(())
        })
        .is_err());
    assert!(!called.load(Ordering::SeqCst));
    archive.unarchive_session(&f.id.to_string()).unwrap();
    let _activity = EventStore::new(&f.db).acquire_activity(f.id).unwrap();
    assert!(service
        .fork_session_prepared(&f.id.to_string(), 0, || {
            called.store(true, Ordering::SeqCst);
            Ok(())
        })
        .is_err());
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(std::fs::read_to_string(&f.target).unwrap(), "current");
    assert_eq!(EventStore::new(&f.db).list_sessions().unwrap().len(), 1);
}
