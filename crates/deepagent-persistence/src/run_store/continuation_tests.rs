use super::*;

fn session(db: &Database, id: &str) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id,created_at,updated_at) VALUES (?1,1,1)",
            [id],
        )
        .map_err(map_sqlite)?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn h130_claim_and_admission_commit_together_and_persist_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let db = Database::open(&path).unwrap();
    session(&db, "s");
    let store = RunStore::new(&db);
    store.create("old", "s", None, 1).unwrap();
    assert!(store.create_continuation("old", "new", "s", 2).unwrap());
    assert!(!store
        .create_continuation("old", "duplicate", "s", 3)
        .unwrap());
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let store = RunStore::new(&reopened);
    assert!(!store
        .create_continuation("old", "cold-duplicate", "s", 4)
        .unwrap());
    assert_eq!(store.list_for_session("s").unwrap().len(), 2);
    let events = store.events_after("old", None).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].sequence, 1);
    assert_eq!(events[1].event_type, "continuation.created");
    assert_eq!(events[1].phase, "control");
    assert_eq!(events[1].status, "created");
    assert_eq!(
        events[1].data,
        serde_json::json!({"replacesTurnId":"old","newTurnId":"new","disposition":"new_turn"})
    );
    assert_eq!(
        store.events_after("new", None).unwrap()[0].event_type,
        "run_accepted"
    );
}

#[test]
fn h130_claim_rollback_covers_new_row_acceptance_and_old_event_failures() {
    for (table, condition) in [
        ("runs", "NEW.id='new'"),
        ("run_events", "NEW.event_type='run_accepted'"),
        ("run_events", "NEW.event_type='continuation.created'"),
    ] {
        let db = Database::open_in_memory().unwrap();
        session(&db, "s");
        let store = RunStore::new(&db);
        store.create("old", "s", None, 1).unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(&format!("CREATE TRIGGER h130_fail BEFORE INSERT ON {table} WHEN {condition} BEGIN SELECT RAISE(ABORT, 'h130 storage rollback'); END;")).map_err(map_sqlite)?;
            Ok(())
        }).unwrap();
        let error = store.create_continuation("old", "new", "s", 2).unwrap_err();
        assert!(error.to_string().contains("h130 storage rollback"));
        assert_eq!(store.list_for_session("s").unwrap().len(), 1);
        assert_eq!(store.events_after("old", None).unwrap().len(), 1);
        assert!(store.events_after("new", None).unwrap().is_empty());
        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER h130_fail")
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        assert!(store.create_continuation("old", "new", "s", 3).unwrap());
    }
}

#[test]
fn h130_claim_rejects_terminal_foreign_and_missing_run_but_not_other_threads() {
    let db = Database::open_in_memory().unwrap();
    session(&db, "s");
    session(&db, "other");
    let store = RunStore::new(&db);
    store.create("old", "s", None, 1).unwrap();
    store.create("other-old", "other", None, 1).unwrap();
    assert!(!store
        .create_continuation("old", "foreign", "other", 2)
        .unwrap());
    assert!(!store
        .create_continuation("missing", "absent", "s", 2)
        .unwrap());
    store
        .finish_with_event(
            "old",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            2,
        )
        .unwrap();
    assert!(!store.create_continuation("old", "late", "s", 3).unwrap());
    assert!(store
        .create_continuation("other-old", "other-new", "other", 3)
        .unwrap());
    assert_eq!(store.list_for_session("s").unwrap().len(), 1);
    assert_eq!(store.list_for_session("other").unwrap().len(), 2);
}

#[test]
fn h130_independent_connections_cannot_claim_the_same_predecessor_twice() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("race.db");
    let db = Database::open(&path).unwrap();
    session(&db, "s");
    RunStore::new(&db).create("old", "s", None, 1).unwrap();
    let connections = (0..8)
        .map(|_| Database::open(&path).unwrap())
        .collect::<Vec<_>>();
    let barrier = std::sync::Barrier::new(connections.len());
    let claimed = std::thread::scope(|scope| {
        let handles = connections
            .iter()
            .enumerate()
            .map(|(index, db)| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    RunStore::new(db)
                        .create_continuation("old", &format!("new-{index}"), "s", 2)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|claimed| *claimed)
            .count()
    });
    assert_eq!(claimed, 1);
    assert_eq!(RunStore::new(&db).list_for_session("s").unwrap().len(), 2);
    assert_eq!(
        RunStore::new(&db).events_after("old", None).unwrap().len(),
        2
    );
}

#[test]
fn h130_terminal_and_claim_race_has_one_linearized_decision_and_gapless_history() {
    let root = tempfile::tempdir().unwrap();
    for round in 0..16 {
        let path = root.path().join(format!("terminal-{round}.db"));
        let db = Database::open(&path).unwrap();
        let peer = Database::open(&path).unwrap();
        session(&db, "s");
        RunStore::new(&db).create("old", "s", None, 1).unwrap();
        let barrier = std::sync::Barrier::new(2);
        let (claimed, finished) = std::thread::scope(|scope| {
            let claim = scope.spawn(|| {
                barrier.wait();
                RunStore::new(&peer)
                    .create_continuation("old", "new", "s", 2)
                    .unwrap()
            });
            barrier.wait();
            let finished = RunStore::new(&db)
                .finish_with_event(
                    "old",
                    "succeeded",
                    None,
                    &serde_json::json!({"kind":"succeeded"}),
                    None,
                    3,
                )
                .unwrap();
            (claim.join().unwrap(), finished)
        });
        assert!(finished);
        let store = RunStore::new(&db);
        assert_eq!(
            store.list_for_session("s").unwrap().len(),
            if claimed { 2 } else { 1 }
        );
        let events = store.events_after("old", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "continuation.created")
                .count(),
            usize::from(claimed)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            (0..events.len() as u64).collect::<Vec<_>>()
        );
        assert!(!store.create_continuation("old", "late", "s", 4).unwrap());
    }
}
