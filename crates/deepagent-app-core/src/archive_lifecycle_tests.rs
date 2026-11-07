//! Archive is the same durable state for every host and every admission path.
use super::*;
use crate::run_coordinator::RunCoordinator;
use deepagent_core::clock::FixedClock;
use deepagent_persistence::run_store::RunStore;
use deepagent_session::Session;
use std::sync::atomic::{AtomicBool, Ordering};

fn session(db: &Database) -> String {
    Session::create(
        db,
        &FixedClock::new(1_000),
        Some("synthetic archive lifecycle"),
    )
    .unwrap()
    .id()
    .to_string()
}

#[test]
fn h215_archive_rejects_an_admitted_owner_without_marker_or_cancellation() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    coordinator.admit("synthetic-active", &id).unwrap();
    let archive = ArchiveService::new(db.clone());
    let result = archive.archive_session(&id);
    assert!(
        result.is_err(),
        "an active admission must exclude archive publication"
    );
    assert!(!archive.is_archived(&id).unwrap());
    let cancellation = coordinator.cancellation_map();
    assert!(!cancellation.lock().unwrap()["synthetic-active"].is_cancelled());
    assert!(RunStore::new(&db)
        .get("synthetic-active")
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    drop(coordinator.take_admission("synthetic-active").unwrap());
    assert!(archive.archive_session(&id).unwrap());
}

#[test]
fn h215_archived_state_rejects_all_new_preparation_before_work_or_admission() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    let archive = ArchiveService::new(db.clone());
    archive.archive_session(&id).unwrap();
    let coordinator = RunCoordinator::new(db.clone());
    let prepared = AtomicBool::new(false);
    let result = coordinator.admit_prepared("synthetic-blocked", &id, None, None, || {
        prepared.store(true, Ordering::SeqCst);
        Ok(((), None))
    });
    assert!(
        result.is_err(),
        "archived identity must fail at the shared admission boundary"
    );
    assert!(!prepared.load(Ordering::SeqCst));
    assert!(RunStore::new(&db)
        .get("synthetic-blocked")
        .unwrap()
        .is_none());
    assert!(
        !coordinator
            .request_cancel("synthetic-blocked")
            .unwrap()
            .accepted
    );
    assert!(archive.is_archived(&id).unwrap());
    assert!(archive.unarchive_session(&id).unwrap());
    coordinator.admit("synthetic-restored", &id).unwrap();
    drop(coordinator.take_admission("synthetic-restored").unwrap());
}

#[test]
fn h215_archive_and_admission_across_database_handles_have_one_linearized_winner() {
    for index in 0..6 {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.db");
        let first = Arc::new(Database::open(&path).unwrap());
        let id = session(&first);
        let second = Arc::new(Database::open(&path).unwrap());
        let coordinator = RunCoordinator::new(second);
        let gate = Arc::new(std::sync::Barrier::new(2));
        let run = format!("synthetic-race-{index}");
        let archived = std::thread::spawn({
            let gate = gate.clone();
            let id = id.clone();
            let db = first.clone();
            move || {
                gate.wait();
                ArchiveService::new(db).archive_session(&id)
            }
        });
        let admitted = std::thread::spawn({
            let id = id.clone();
            let run = run.clone();
            move || {
                gate.wait();
                let result = coordinator.admit(&run, &id);
                (coordinator, result)
            }
        });
        let archive_result = archived.join().unwrap();
        let (coordinator, admit_result) = admitted.join().unwrap();
        assert_ne!(
            archive_result.is_ok(),
            admit_result.is_ok(),
            "archive/admission cannot both publish (or both fail) for a healthy idle session"
        );
        assert_eq!(
            ArchiveService::new(first.clone()).is_archived(&id).unwrap(),
            archive_result.is_ok()
        );
        assert_eq!(
            RunStore::new(&first).get(&run).unwrap().is_some(),
            admit_result.is_ok()
        );
        if admit_result.is_ok() {
            drop(coordinator.take_admission(&run).unwrap());
        }
    }
}

#[test]
fn h215_equivalent_session_spelling_cannot_bypass_archive_identity() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    let alias = format!(
        "ses_{}",
        id.strip_prefix("ses_").unwrap().to_ascii_uppercase()
    );
    assert_eq!(
        id.parse::<deepagent_core::id::SessionId>().unwrap(),
        alias.parse().unwrap()
    );
    let archive = ArchiveService::new(db.clone());
    archive.archive_session(&id).unwrap();
    assert!(archive.is_archived(&alias).unwrap());
    let coordinator = RunCoordinator::new(db.clone());
    assert!(coordinator.admit("synthetic-alias", &alias).is_err());
    assert!(RunStore::new(&db).get("synthetic-alias").unwrap().is_none());
    assert!(archive.unarchive_session(&alias).unwrap());
    assert!(!archive.is_archived(&id).unwrap());
}

#[test]
fn h215_corrupt_archive_body_still_hides_identity_and_blocks_admission() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    DocumentStore::new(&db)
        .put(ARCHIVE_COLLECTION, &id, "{invalid", None, SystemClock.now())
        .unwrap();
    let archive = ArchiveService::new(db.clone());
    assert!(archive.is_archived(&id).unwrap());
    assert!(
        archive.archived_ids().unwrap().contains(&id),
        "listing must not drop the archive gate with its malformed display metadata"
    );
    assert!(RunCoordinator::new(db)
        .admit("synthetic-corrupt", &id)
        .is_err());
}

#[test]
fn h215_failed_archive_write_leaves_no_marker_and_releases_control() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    db.with_conn(|conn| conn.execute_batch(
        "CREATE TRIGGER h215_reject_archive BEFORE INSERT ON documents WHEN NEW.collection='archived_conversations' BEGIN SELECT RAISE(ABORT, 'synthetic archive write failure'); END;"
    ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    let archive = ArchiveService::new(db.clone());
    assert!(archive
        .archive_session(&id)
        .unwrap_err()
        .to_string()
        .contains("synthetic archive write failure"));
    assert!(!archive.is_archived(&id).unwrap());
    let coordinator = RunCoordinator::new(db);
    coordinator.admit("synthetic-write-control", &id).unwrap();
    drop(
        coordinator
            .take_admission("synthetic-write-control")
            .unwrap(),
    );
}

#[test]
fn h215_project_archive_refuses_live_member_before_publishing_any_marker() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let ids = (0..2)
        .map(|_| {
            Session::create_in_project(
                &db,
                &clock,
                None,
                Default::default(),
                Some("/synthetic/project"),
            )
            .unwrap()
            .id()
            .to_string()
        })
        .collect::<Vec<_>>();
    let coordinator = RunCoordinator::new(db.clone());
    coordinator
        .admit("synthetic-project-active", &ids[1])
        .unwrap();
    let archive = ArchiveService::new(db.clone());
    assert!(archive.archive_project("/synthetic/project").is_err());
    assert!(archive.archived_ids().unwrap().is_empty());
    drop(
        coordinator
            .take_admission("synthetic-project-active")
            .unwrap(),
    );
    assert_eq!(
        archive
            .archive_project("/synthetic/project")
            .unwrap()
            .archived_count,
        2
    );
}

#[test]
fn h215_failed_unarchive_delete_preserves_execution_gate_and_restored_history() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    let archive = ArchiveService::new(db.clone());
    archive.archive_session(&id).unwrap();
    let count = EventStore::new(&db)
        .event_count(id.parse().unwrap())
        .unwrap();
    db.with_conn(|conn| conn.execute_batch(
        "CREATE TRIGGER h215_reject_restore BEFORE DELETE ON documents WHEN OLD.collection='archived_conversations' BEGIN SELECT RAISE(ABORT, 'synthetic unarchive write failure'); END;"
    ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    assert!(archive
        .unarchive_session(&id)
        .unwrap_err()
        .to_string()
        .contains("synthetic unarchive write failure"));
    assert!(archive.is_archived(&id).unwrap());
    assert_eq!(
        EventStore::new(&db)
            .event_count(id.parse().unwrap())
            .unwrap(),
        count
    );
    assert!(RunCoordinator::new(db.clone())
        .admit("synthetic-restore-failed", &id)
        .is_err());
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER h215_reject_restore;")
            .map_err(|error| CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    assert!(archive.unarchive_session(&id).unwrap());
}
