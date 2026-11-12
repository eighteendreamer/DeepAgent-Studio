//! H-107: lifecycle admission and publication use the existing authority/ledger.
use deepagent_core::{clock::Timestamp, event::EventPayload, id::SessionId};
use deepagent_persistence::{
    event_store::EventStore,
    run_store::{RunAdmission, RunStore},
    Database,
};
use std::sync::{Arc, Barrier};

fn seed(db: &Database) -> SessionId {
    let id = SessionId::new();
    let store = EventStore::new(db);
    store
        .create_session(id, None, Timestamp::from_millis(1))
        .unwrap();
    store
        .append(
            id,
            EventPayload::SessionStarted {
                title: None,
                mode: Default::default(),
            },
            Timestamp::from_millis(1),
        )
        .unwrap();
    id
}

fn admit(db: &Database, id: SessionId, run: &str, ending: bool) -> deepagent_core::Result<bool> {
    let id = id.to_string();
    let admission = RunAdmission {
        run_id: run,
        session_id: &id,
        task_id: None,
        replaces_run_id: None,
        capability_snapshot: None,
        scheduled: None,
    };
    let store = RunStore::new(db);
    if ending {
        store.admit_session_end(admission, 2)
    } else {
        store.admit(admission, 2)
    }
}

fn ends(db: &Database, id: SessionId) -> usize {
    EventStore::new(db)
        .load_session(id)
        .unwrap()
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::SessionEnded { .. }))
        .count()
}

#[test]
fn end_intent_fences_turns_after_failure_and_retry_publishes_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("end.sqlite");
    let db = Database::open(&path).unwrap();
    let id = seed(&db);
    admit(&db, id, "end", true).unwrap();
    assert!(admit(&db, id, "normal", false)
        .unwrap_err()
        .to_string()
        .contains("ended"));
    assert!(admit(&db, id, "duplicate-active", true)
        .unwrap_err()
        .to_string()
        .contains("busy"));
    RunStore::new(&db)
        .finish(
            "end",
            "failed",
            Some("synthetic pre-publication failure"),
            3,
        )
        .unwrap();
    assert_eq!(ends(&db, id), 0);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert!(admit(&db, id, "normal-after-reopen", false).is_err());
    admit(&db, id, "retry", true).unwrap();
    assert!(EventStore::new(&db)
        .end_session_for_run(id, "retry", Some("explicit_end"), Timestamp::from_millis(4))
        .unwrap());
    assert!(!EventStore::new(&db)
        .end_session_for_run(id, "retry", Some("explicit_end"), Timestamp::from_millis(5))
        .unwrap());
    RunStore::new(&db)
        .finish("retry", "succeeded", None, 6)
        .unwrap();
    assert_eq!(ends(&db, id), 1);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(ends(&db, id), 1);
    assert!(EventStore::new(&db)
        .get_session(id)
        .unwrap()
        .unwrap()
        .ended_at
        .is_some());
    assert!(admit(&db, id, "ordinary-reopen", false).is_err());
    admit(&db, id, "repeat-closed", true).unwrap();
    assert!(!EventStore::new(&db)
        .end_session_for_run(id, "repeat-closed", None, Timestamp::from_millis(7))
        .unwrap());
}

#[test]
fn failed_end_admission_rolls_back_intent_and_does_not_freeze_session() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db);
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_end BEFORE INSERT ON run_events WHEN NEW.event_type='session_end.requested' BEGIN SELECT RAISE(ABORT,'synthetic end intent failure'); END;")
            .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))
    }).unwrap();
    assert!(admit(&db, id, "end", true)
        .unwrap_err()
        .to_string()
        .contains("synthetic end intent failure"));
    assert!(RunStore::new(&db).get("end").unwrap().is_none());
    assert!(RunStore::new(&db)
        .events_after("end", None)
        .unwrap()
        .is_empty());
    assert_eq!(ends(&db, id), 0);
    admit(&db, id, "normal", false).unwrap();
}

#[test]
fn real_independent_admissions_cannot_race_end_past_a_normal_turn() {
    for _ in 0..8 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("race.sqlite");
        let first = Database::open(&path).unwrap();
        let id = seed(&first);
        let second = Database::open(&path).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let one = std::thread::spawn(move || {
            b.wait();
            admit(&first, id, "end", true)
        });
        let two = std::thread::spawn(move || {
            barrier.wait();
            admit(&second, id, "normal", false)
        });
        let ended = one.join().unwrap().is_ok();
        let normal = two.join().unwrap().is_ok();
        assert_ne!(ended, normal, "exactly one competing admission may win");
        let db = Database::open(&path).unwrap();
        assert_eq!(
            RunStore::new(&db)
                .list_for_session(&id.to_string())
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn a_normal_run_or_forged_capability_snapshot_cannot_claim_end_authority() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db);
    admit(&db, id, "normal", false).unwrap();
    assert!(EventStore::new(&db)
        .end_session_for_run(id, "normal", None, Timestamp::from_millis(3))
        .is_err());
    assert!(EventStore::new(&db)
        .end_session(id, None, Timestamp::from_millis(3))
        .unwrap_err()
        .to_string()
        .contains("busy"));
    RunStore::new(&db)
        .finish("normal", "succeeded", None, 4)
        .unwrap();
    assert!(EventStore::new(&db)
        .end_session(id, None, Timestamp::from_millis(5))
        .unwrap());
    assert!(!EventStore::new(&db)
        .end_session(id, None, Timestamp::from_millis(6))
        .unwrap());
    let sid = id.to_string();
    let forged = serde_json::json!({"version":1,"runId":"forged","threadId":sid,"context":{"executionDomain":"session_end"}});
    assert!(RunStore::new(&db)
        .admit(
            RunAdmission {
                run_id: "forged",
                session_id: &sid,
                task_id: None,
                replaces_run_id: None,
                capability_snapshot: Some(&forged),
                scheduled: None
            },
            7
        )
        .is_err());
    assert_eq!(ends(&db, id), 1);
}

#[test]
fn committed_end_survives_projection_failure_without_republication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("end-projection.sqlite");
    let db = Database::open(&path).unwrap();
    let id = seed(&db);
    admit(&db, id, "end", true).unwrap();
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER fail_end_projection BEFORE UPDATE OF ended_at ON sessions BEGIN SELECT RAISE(ABORT,'synthetic end projection failure'); END;")
        .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))).unwrap();
    // The file is authoritative; EventStore's existing contract preserves its
    // committed event even when the rebuildable SQLite projection fails.
    assert!(EventStore::new(&db)
        .end_session_for_run(id, "end", None, Timestamp::from_millis(3))
        .unwrap());
    assert_eq!(ends(&db, id), 1);
    assert!(EventStore::new(&db)
        .get_session(id)
        .unwrap()
        .unwrap()
        .ended_at
        .is_none());
    RunStore::new(&db)
        .finish("end", "failed", Some("synthetic caller failure"), 4)
        .unwrap();
    assert!(admit(&db, id, "normal", false).is_err());
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER fail_end_projection;")
            .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(ends(&db, id), 1);
    assert!(EventStore::new(&db)
        .get_session(id)
        .unwrap()
        .unwrap()
        .ended_at
        .is_some());
    admit(&db, id, "retry", true).unwrap();
    assert!(!EventStore::new(&db)
        .end_session_for_run(id, "retry", None, Timestamp::from_millis(5))
        .unwrap());
    assert_eq!(ends(&db, id), 1);
}
