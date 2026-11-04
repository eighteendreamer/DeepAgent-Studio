//! H-219: initial capability evidence and admission have one durable owner.
use super::*;
use deepagent_core::clock::Timestamp;
use deepagent_core::id::SessionId;
use std::sync::{Arc, Barrier};

fn session(db: &Database) -> String {
    let id = SessionId::new();
    crate::event_store::EventStore::new(db)
        .create_session(id, None, Timestamp::from_millis(1))
        .unwrap();
    id.to_string()
}

fn snapshot(run: &str, session: &str) -> serde_json::Value {
    serde_json::json!({"version":1,"runId":run,"threadId":session,"snapshotId":"synthetic"})
}

fn admit(
    db: &Database,
    run: &str,
    session: &str,
    replaces: Option<&str>,
    snapshot: &serde_json::Value,
) -> Result<bool> {
    RunStore::new(db).admit(
        RunAdmission {
            run_id: run,
            session_id: session,
            task_id: None,
            replaces_run_id: replaces,
            capability_snapshot: Some(snapshot),
            scheduled: None,
        },
        2,
    )
}

fn assert_admitted(store: &RunStore<'_>, run: &str, data: &serde_json::Value) {
    let events = store.events_after(run, None).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "run_accepted");
    assert_eq!(events[0].sequence, 0);
    assert_eq!(events[1].event_type, "capability.snapshot");
    assert_eq!(events[1].sequence, 1);
    assert_eq!(events[1].data, *data);
}

#[test]
fn h219_admission_rolls_back_every_write_and_retries_the_same_identity() {
    for replacement in [false, true] {
        for (table, condition) in [
            ("runs", "NEW.id='new'"),
            (
                "run_events",
                "NEW.run_id='new' AND NEW.event_type='run_accepted'",
            ),
            ("run_events", "NEW.event_type='capability.snapshot'"),
            ("run_events", "NEW.event_type='continuation.created'"),
        ] {
            if !replacement && condition.contains("continuation.created") {
                continue;
            }
            let db = Database::open_in_memory().unwrap();
            let session = session(&db);
            let store = RunStore::new(&db);
            store.create("old", &session, None, 1).unwrap();
            let old = store.events_after("old", None).unwrap();
            let data = snapshot("new", &session);
            db.with_conn(|conn| {
                conn.execute_batch(&format!("CREATE TRIGGER h219_fail BEFORE INSERT ON {table} WHEN {condition} BEGIN SELECT RAISE(ABORT, 'h219 atomic admission failure'); END;"))
                    .map_err(map_sqlite)
            }).unwrap();
            let replaces = replacement.then_some("old");
            let error = admit(&db, "new", &session, replaces, &data).unwrap_err();
            assert!(error.to_string().contains("h219 atomic admission failure"));
            assert!(store.get("new").unwrap().is_none(), "{condition}");
            assert!(store.events_after("new", None).unwrap().is_empty());
            assert_eq!(store.events_after("old", None).unwrap(), old);
            db.with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER h219_fail;")
                    .map_err(map_sqlite)
            })
            .unwrap();
            assert!(admit(&db, "new", &session, replaces, &data).unwrap());
            assert_admitted(&store, "new", &data);
            assert_eq!(store.list_for_session(&session).unwrap().len(), 2);
            assert_eq!(
                store.events_after("old", None).unwrap().len(),
                if replacement { 2 } else { 1 }
            );
            assert!(store.bind_capability_snapshot("new", &data, 3).is_err());
            assert_admitted(&store, "new", &data);
        }
    }
}

#[test]
fn h219_admission_identity_failure_never_claims_the_predecessor() {
    let db = Database::open_in_memory().unwrap();
    let session = session(&db);
    let store = RunStore::new(&db);
    store.create("old", &session, None, 1).unwrap();
    for replacement in [false, true] {
        for field in ["runId", "threadId", "version", "missing-version"] {
            let mut data = snapshot("new", &session);
            match field {
                "version" => data["version"] = serde_json::json!(2),
                "missing-version" => {
                    data.as_object_mut().unwrap().remove("version");
                }
                other => data[other] = serde_json::json!("foreign"),
            }
            assert!(admit(&db, "new", &session, replacement.then_some("old"), &data).is_err());
            assert!(store.get("new").unwrap().is_none());
            assert!(store.events_after("new", None).unwrap().is_empty());
            assert_eq!(store.events_after("old", None).unwrap().len(), 1);
        }
    }
    let data = snapshot("new", &session);
    assert!(admit(&db, "new", &session, Some("old"), &data).unwrap());
    assert_admitted(&store, "new", &data);
}

#[test]
fn h219_scoped_admission_rejects_ineligible_predecessors_without_snapshot_writes() {
    let db = Database::open_in_memory().unwrap();
    let session_id = session(&db);
    let other = session(&db);
    let store = RunStore::new(&db);
    store.create("active", &session_id, None, 1).unwrap();
    store.create("foreign", &other, None, 1).unwrap();
    store.create("terminal", &session_id, None, 1).unwrap();
    store
        .finish_with_event(
            "terminal",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            2,
        )
        .unwrap();
    assert!(admit(
        &db,
        "claimed",
        &session_id,
        Some("active"),
        &snapshot("claimed", &session_id)
    )
    .unwrap());
    for old in ["missing", "foreign", "terminal", "active"] {
        let before = store.events_after(old, None).unwrap();
        assert!(!admit(
            &db,
            "refused",
            &session_id,
            Some(old),
            &snapshot("refused", &session_id)
        )
        .unwrap());
        assert!(store.get("refused").unwrap().is_none());
        assert!(store.events_after("refused", None).unwrap().is_empty());
        assert_eq!(store.events_after(old, None).unwrap(), before);
    }
}

#[test]
fn h219_independent_admissions_publish_only_the_winners_snapshot_and_survive_reopen() {
    for replacement in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("admission.sqlite");
        let db = Database::open(&path).unwrap();
        let session = session(&db);
        RunStore::new(&db).create("old", &session, None, 1).unwrap();
        let connections = (0..8)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(connections.len()));
        let threads = connections
            .into_iter()
            .enumerate()
            .map(|(index, connection)| {
                let barrier = barrier.clone();
                let session = session.clone();
                std::thread::spawn(move || {
                    let run = if replacement {
                        format!("new-{index}")
                    } else {
                        "new".into()
                    };
                    let mut data = snapshot(&run, &session);
                    data["candidate"] = serde_json::json!(index);
                    barrier.wait();
                    let result = admit(
                        &connection,
                        &run,
                        &session,
                        replacement.then_some("old"),
                        &data,
                    );
                    (run, data, result)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>();
        let winners = results
            .iter()
            .filter(|(_, _, result)| matches!(result, Ok(true)))
            .collect::<Vec<_>>();
        assert_eq!(winners.len(), 1);
        let (winner, data, _) = winners[0];
        let store = RunStore::new(&db);
        assert_admitted(&store, winner, data);
        assert_eq!(store.list_for_session(&session).unwrap().len(), 2);
        for (run, _, result) in &results {
            if replacement && !matches!(result, Ok(true)) {
                assert!(matches!(result, Ok(false)));
                assert!(store.get(run).unwrap().is_none());
                assert!(store.events_after(run, None).unwrap().is_empty());
            }
            if !replacement && !matches!(result, Ok(true)) {
                assert!(result.is_err());
            }
        }
        let old_events = store.events_after("old", None).unwrap();
        assert_eq!(old_events.len(), if replacement { 2 } else { 1 });
        if replacement {
            assert_eq!(old_events[1].data["newTurnId"], *winner);
        }
        let saved = store.events_after(winner, None).unwrap();
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after(winner, None).unwrap(), saved);
        assert_eq!(store.events_after("old", None).unwrap(), old_events);
        assert!(store.bind_capability_snapshot(winner, data, 3).is_err());
        assert_eq!(store.events_after(winner, None).unwrap(), saved);
    }
}

#[test]
fn h219_snapshot_admission_and_terminal_race_leave_no_partial_new_owner() {
    let root = tempfile::tempdir().unwrap();
    for round in 0..8 {
        let path = root.path().join(format!("terminal-{round}.sqlite"));
        let db = Database::open(&path).unwrap();
        let peer = Database::open(&path).unwrap();
        let session = session(&db);
        RunStore::new(&db).create("old", &session, None, 1).unwrap();
        let data = snapshot("new", &session);
        let barrier = Barrier::new(2);
        let claimed = std::thread::scope(|scope| {
            let claim = scope.spawn(|| {
                barrier.wait();
                admit(&peer, "new", &session, Some("old"), &data).unwrap()
            });
            barrier.wait();
            assert!(RunStore::new(&db)
                .finish_with_event(
                    "old",
                    "succeeded",
                    None,
                    &serde_json::json!({"kind":"succeeded"}),
                    None,
                    3,
                )
                .unwrap());
            claim.join().unwrap()
        });
        let store = RunStore::new(&db);
        assert_eq!(store.get("new").unwrap().is_some(), claimed);
        if claimed {
            assert_admitted(&store, "new", &data);
        } else {
            assert!(store.events_after("new", None).unwrap().is_empty());
        }
        let old = store.events_after("old", None).unwrap();
        assert_eq!(
            old.iter()
                .filter(|e| e.event_type == "continuation.created")
                .count(),
            usize::from(claimed)
        );
        assert_eq!(
            old.iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            old.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            (0..old.len() as u64).collect::<Vec<_>>()
        );
        if claimed {
            assert!(
                old.iter()
                    .position(|e| e.event_type == "continuation.created")
                    .unwrap()
                    < old
                        .iter()
                        .position(|e| e.event_type == "run_terminal")
                        .unwrap()
            );
        }
    }
}
