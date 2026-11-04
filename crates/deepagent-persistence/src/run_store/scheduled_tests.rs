//! H96: origin evidence is part of the existing admission, not a second queue.
use super::*;
use deepagent_core::{clock::Timestamp, id::SessionId};
use std::sync::{Arc, Barrier};

fn session(db: &Database) -> String {
    let id = SessionId::new();
    crate::event_store::EventStore::new(db)
        .create_session(id, None, Timestamp::from_millis(1))
        .unwrap();
    id.to_string()
}

fn origin() -> ScheduledRunOrigin {
    ScheduledRunOrigin {
        version: 1,
        occurrence_id: "scheduled".into(),
        task_id: "task".into(),
        schedule_anchor_ms: 1,
        scheduled_for: "2026-11-04 12:00:00".into(),
        accepted_at_ms: 2,
    }
}

fn admit(db: &Database, session: &str, origin: &ScheduledRunOrigin) -> Result<bool> {
    RunStore::new(db).admit(
        RunAdmission {
            run_id: "scheduled",
            session_id: session,
            task_id: None,
            replaces_run_id: None,
            capability_snapshot: Some(
                &serde_json::json!({"version":1,"runId":"scheduled","threadId":session}),
            ),
            scheduled: Some(origin),
        },
        2,
    )
}

#[test]
fn h96_origin_failure_rolls_back_run_snapshot_and_retries_without_losing_evidence() {
    let db = Database::open_in_memory().unwrap();
    let session = session(&db);
    let store = RunStore::new(&db);
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER h96 BEFORE INSERT ON run_events WHEN NEW.event_type='schedule.accepted' BEGIN SELECT RAISE(ABORT,'h96 origin failure'); END;").map_err(map_sqlite)).unwrap();
    assert!(admit(&db, &session, &origin()).is_err());
    assert!(store.get("scheduled").unwrap().is_none());
    assert!(store.events_after("scheduled", None).unwrap().is_empty());
    assert!(store.scheduled_origin("scheduled").unwrap().is_none());
    db.with_conn(|conn| conn.execute_batch("DROP TRIGGER h96").map_err(map_sqlite))
        .unwrap();
    assert!(admit(&db, &session, &origin()).unwrap());
    let events = store.events_after("scheduled", None).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        ["run_accepted", "capability.snapshot", "schedule.accepted"]
    );
    assert_eq!(store.scheduled_origin("scheduled").unwrap(), Some(origin()));
    assert!(store
        .finish(
            "scheduled",
            "interrupted",
            Some("unknown execution outcome"),
            3
        )
        .unwrap());
    assert_eq!(
        store.scheduled_origin("scheduled").unwrap(),
        Some(origin()),
        "a terminal is not permission to redispatch"
    );
}

#[test]
fn h96_missing_ambiguous_and_invalid_origins_never_authorize_redispatch() {
    for mode in [
        "missing",
        "duplicate",
        "invalid-json",
        "identity",
        "version",
        "timestamp",
    ] {
        let db = Database::open_in_memory().unwrap();
        let session = session(&db);
        let store = RunStore::new(&db);
        assert!(admit(&db, &session, &origin()).unwrap());
        db.with_conn(|conn| {
            match mode {
                "missing" => { conn.execute("DELETE FROM run_events WHERE event_type='schedule.accepted'", []).map_err(map_sqlite)?; }
                "duplicate" => { conn.execute("INSERT INTO run_events (run_id,sequence,timestamp,phase,status,event_type,data) SELECT run_id,sequence+1,timestamp,phase,status,event_type,data FROM run_events WHERE event_type='schedule.accepted'", []).map_err(map_sqlite)?; }
                _ => {
                    let mut bad = serde_json::to_value(origin()).unwrap();
                    match mode {
                        "identity" => bad["occurrenceId"] = serde_json::json!("other"),
                        "version" => bad["version"] = serde_json::json!(99),
                        "timestamp" => bad["acceptedAtMs"] = serde_json::json!(1),
                        _ => {}
                    }
                    let data = if mode == "invalid-json" { "{".into() } else { bad.to_string() };
                    conn.execute("UPDATE run_events SET data=?1 WHERE event_type='schedule.accepted'", [data]).map_err(map_sqlite)?;
                }
            }
            Ok(())
        }).unwrap();
        assert!(store.scheduled_origin("scheduled").is_err(), "{mode}");
        assert!(
            admit(&db, &session, &origin()).is_err(),
            "{mode}: identity remains reserved"
        );
    }
}

#[test]
fn h96_independent_connections_have_only_one_durable_admission_owner() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("runs.sqlite");
    let db = Database::open(&path).unwrap();
    let session = session(&db);
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let db = Database::open(&path).unwrap();
            let session = session.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                admit(&db, &session, &origin()).is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().expect("admission worker must not panic"))
            .filter(|won| *won)
            .count(),
        1
    );
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .scheduled_origin("scheduled")
            .unwrap(),
        Some(origin())
    );
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("scheduled", None)
            .unwrap()
            .len(),
        3
    );
}
