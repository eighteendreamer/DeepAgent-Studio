//! H-219 admission must not outlive a failed initial capability write.
use super::*;
use deepagent_core::clock::FixedClock;

fn session(db: &Database) -> String {
    deepagent_session::Session::create(db, &FixedClock::new(1_000), None)
        .unwrap()
        .id()
        .to_string()
}

fn snapshot(run: &str, session: &str) -> serde_json::Value {
    serde_json::json!({"version":1,"runId":run,"threadId":session,"snapshotId":"synthetic"})
}

fn reject_snapshot(db: &Database, reject: bool) {
    db.with_conn(|conn| {
        conn.execute_batch(if reject {
            "CREATE TRIGGER h219_admission_fail BEFORE INSERT ON run_events WHEN NEW.event_type='capability.snapshot' BEGIN SELECT RAISE(ABORT, 'h219 admission snapshot failure'); END;"
        } else {
            "DROP TRIGGER h219_admission_fail;"
        }).map_err(|error| CoreError::Persistence(error.to_string()))
    }).unwrap();
}

// The red run exercised admit + bind separately. Keep the same inputs and
// rollback oracle, now using the coordinator's single storage boundary.
fn admit_snapshot(
    coordinator: &RunCoordinator,
    run: &str,
    session: &str,
    replaces: Option<&str>,
    snapshot: &serde_json::Value,
) -> Result<bool> {
    match replaces {
        Some(old) => coordinator.admit_continuation_with_snapshot(old, run, session, snapshot),
        None => {
            coordinator.admit_with_snapshot(run, session, snapshot)?;
            Ok(true)
        }
    }
}

#[test]
fn h219_failed_initial_snapshot_does_not_admit_or_publish_cancellation_owner() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let session = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    let data = snapshot("new", &session);
    reject_snapshot(&db, true);
    let error = admit_snapshot(&coordinator, "new", &session, None, &data).unwrap_err();
    assert!(error
        .to_string()
        .contains("h219 admission snapshot failure"));
    assert!(RunStore::new(&db).get("new").unwrap().is_none());
    assert!(RunStore::new(&db)
        .events_after("new", None)
        .unwrap()
        .is_empty());
    assert!(!coordinator.request_cancel("new").unwrap().accepted);
    assert!(coordinator.take_admission("new").is_err());
    assert_eq!(coordinator.readiness().unwrap().active_runs, 0);
    drop(
        EventStore::new(&db)
            .begin_control(session.parse().unwrap())
            .unwrap(),
    );
    reject_snapshot(&db, false);
    assert!(admit_snapshot(&coordinator, "new", &session, None, &data).unwrap());
    assert!(coordinator.request_cancel("new").unwrap().accepted);
    let events = RunStore::new(&db).events_after("new", None).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "run_accepted");
    assert_eq!(events[1].event_type, "capability.snapshot");
    assert_eq!(events[1].data, data);
    assert_eq!(events[1].sequence, 1);
    drop(coordinator.take_admission("new").unwrap());
    assert!(coordinator.take_admission("new").is_err());
}

#[test]
fn h219_failed_steer_snapshot_preserves_predecessor_claim_cancel_and_owner() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let session = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    let competitor = RunCoordinator::new(db.clone());
    coordinator.admit("old", &session).unwrap();
    let data = snapshot("new", &session);
    reject_snapshot(&db, true);
    let error = admit_snapshot(&coordinator, "new", &session, Some("old"), &data).unwrap_err();
    assert!(error
        .to_string()
        .contains("h219 admission snapshot failure"));
    assert!(!coordinator.cancellations.lock().unwrap()["old"].is_cancelled());
    assert!(RunStore::new(&db).get("new").unwrap().is_none());
    assert!(RunStore::new(&db)
        .events_after("new", None)
        .unwrap()
        .is_empty());
    assert_eq!(
        RunStore::new(&db).events_after("old", None).unwrap().len(),
        1
    );
    assert!(!coordinator.request_cancel("new").unwrap().accepted);
    assert!(coordinator.take_admission("new").is_err());
    assert_eq!(coordinator.readiness().unwrap().active_runs, 1);
    assert!(competitor.admit("foreign", &session).is_err());
    reject_snapshot(&db, false);
    assert!(admit_snapshot(&coordinator, "new", &session, Some("old"), &data).unwrap());
    assert!(coordinator.cancellations.lock().unwrap()["old"].is_cancelled());
    assert_eq!(
        RunStore::new(&db).events_after("new", None).unwrap()[1].data,
        data
    );
    assert_eq!(
        RunStore::new(&db).events_after("old", None).unwrap().len(),
        2
    );
    assert!(!admit_snapshot(
        &coordinator,
        "duplicate",
        &session,
        Some("old"),
        &snapshot("duplicate", &session)
    )
    .unwrap());
    assert!(RunStore::new(&db).get("duplicate").unwrap().is_none());
    assert!(!coordinator.request_cancel("duplicate").unwrap().accepted);
    drop(coordinator.take_admission("old").unwrap());
    assert!(competitor.admit("foreign", &session).is_err());
    drop(coordinator.take_admission("new").unwrap());
    competitor.admit("foreign", &session).unwrap();
}

#[test]
fn h219_preparation_uses_original_session_owner_and_releases_on_failure() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let session = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    let competitor = RunCoordinator::new(db.clone());
    let check_owner = || {
        assert!(RunStore::new(&db).get("prepared").unwrap().is_none());
        assert!(EventStore::new(&db)
            .begin_control(session.parse().unwrap())
            .is_err());
        assert!(competitor.admit("foreign", &session).is_err());
    };
    let failed: Result<Option<()>> =
        coordinator.admit_prepared("prepared", &session, None, None, || {
            check_owner();
            Err(CoreError::invalid("synthetic preparation failure"))
        });
    assert!(failed
        .unwrap_err()
        .to_string()
        .contains("synthetic preparation failure"));
    assert_eq!(coordinator.readiness().unwrap().active_runs, 0);
    assert!(!coordinator.request_cancel("prepared").unwrap().accepted);
    assert!(RunStore::new(&db).get("prepared").unwrap().is_none());
    drop(
        EventStore::new(&db)
            .begin_control(session.parse().unwrap())
            .unwrap(),
    );
    let data = snapshot("prepared", &session);
    let prepared = coordinator
        .admit_prepared("prepared", &session, None, None, || {
            check_owner();
            Ok(("one real preparation", Some(data.clone())))
        })
        .unwrap()
        .unwrap();
    assert_eq!(prepared, "one real preparation");
    assert_eq!(
        RunStore::new(&db).capability_snapshot("prepared").unwrap(),
        Some(data)
    );
    assert!(EventStore::new(&db)
        .begin_control(session.parse().unwrap())
        .is_err());
    drop(coordinator.take_admission("prepared").unwrap());
    drop(
        EventStore::new(&db)
            .begin_control(session.parse().unwrap())
            .unwrap(),
    );
}
