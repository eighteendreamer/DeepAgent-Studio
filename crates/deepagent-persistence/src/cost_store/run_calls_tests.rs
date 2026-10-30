use super::*;
use crate::event_store::EventStore;
use crate::run_store::RunStore;
use crate::Database;
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::id::SessionId;
use serde_json::json;

fn setup(db: &Database) -> String {
    let session = SessionId::new();
    EventStore::new(db)
        .create_session(session, None, SystemClock.now())
        .unwrap();
    let session = session.to_string();
    RunStore::new(db).create("run", &session, None, 1).unwrap();
    session
}

fn entry(session: &str) -> CostEntry<'_> {
    CostEntry {
        session_id: session,
        timestamp: 2,
        model: "flash",
        input_tokens: 1000,
        output_tokens: 100,
        cache_hit_tokens: 800,
        cache_miss_tokens: 200,
        total_tokens: 1100,
        cost_yuan: Some(0.001),
        input_cache_hit_price: None,
        input_cache_miss_price: None,
        output_price: None,
        pricing_source: None,
    }
}

fn call<'a>(session: &'a str, report: &'a Value) -> RunCallUsage<'a> {
    RunCallUsage {
        session_id: session,
        run_id: "run",
        call_id: "call-1",
        timestamp: 2,
        report,
    }
}

#[test]
fn cost_and_ownership_commit_once_across_reopen_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let session = setup(&db);
    let report = json!({"version":1,"model_id":"flash","input_tokens":1000});
    let first = CostStore::new(&db)
        .record_run_call(&call(&session, &report), Some(&entry(&session)))
        .unwrap();
    assert!(first.inserted);
    drop(db);
    let db = Database::open(path).unwrap();
    let again = CostStore::new(&db)
        .record_run_call(&call(&session, &report), Some(&entry(&session)))
        .unwrap();
    assert!(!again.inserted);
    assert_eq!(first.ledger_id, again.ledger_id);
    assert_eq!(
        CostStore::new(&db)
            .session_usage(&session)
            .unwrap()
            .input_tokens,
        1000
    );
    assert_eq!(
        CostStore::new(&db).run_call_ledger_ids(&session).unwrap(),
        HashSet::from([first.ledger_id.unwrap()])
    );
    let events = RunStore::new(&db).events_after("run", None).unwrap();
    assert_eq!(
        events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        RunStore::new(&db).events_after("run", Some(0)).unwrap(),
        events[1..]
    );
    assert_eq!(events[1].data["report"], report);
}

#[test]
fn conflicting_identity_and_wrong_parent_do_not_book() {
    let db = Database::open_in_memory().unwrap();
    let session = setup(&db);
    let report = json!({"model_id":"flash"});
    let store = CostStore::new(&db);
    store
        .record_run_call(&call(&session, &report), Some(&entry(&session)))
        .unwrap();
    assert!(store
        .record_run_call(
            &call(&session, &json!({"model_id":"pro"})),
            Some(&entry(&session))
        )
        .is_err());
    let mut wrong = call(&session, &report);
    wrong.session_id = "another-session";
    assert!(store.record_run_call(&wrong, None).is_err());
    wrong.session_id = &session;
    wrong.run_id = "missing-run";
    assert!(store.record_run_call(&wrong, None).is_err());
    assert_eq!(store.session_costs_detailed(&session).unwrap().len(), 1);
}

#[test]
fn insert_failure_rolls_back_both_sides_and_connection_remains_usable() {
    for table in ["costs", "run_events"] {
        let db = Database::open_in_memory().unwrap();
        let session = setup(&db);
        db.with_conn(|conn| conn.execute_batch(&format!(
            "CREATE TEMP TRIGGER fail_call BEFORE INSERT ON {table} BEGIN SELECT RAISE(FAIL,'controlled call write failure'); END;"
        )).map_err(map_sqlite)).unwrap();
        let report = json!({"model_id":"flash"});
        let store = CostStore::new(&db);
        assert!(store
            .record_run_call(&call(&session, &report), Some(&entry(&session)))
            .is_err());
        assert!(store.session_costs_detailed(&session).unwrap().is_empty());
        assert_eq!(
            RunStore::new(&db).events_after("run", None).unwrap().len(),
            1
        );
        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER fail_call")
                .map_err(map_sqlite)
        })
        .unwrap();
        assert!(
            store
                .record_run_call(&call(&session, &report), Some(&entry(&session)))
                .unwrap()
                .inserted
        );
        assert_eq!(store.session_usage(&session).unwrap().input_tokens, 1000);
    }
}

#[test]
fn missing_quote_is_unknown_but_unreported_usage_does_not_fabricate_a_row() {
    let db = Database::open_in_memory().unwrap();
    let session = setup(&db);
    let store = CostStore::new(&db);
    let report = json!({"usage_complete":true});
    let mut unknown = entry(&session);
    unknown.cost_yuan = None;
    let measured = store
        .record_run_call(&call(&session, &report), Some(&unknown))
        .unwrap();
    assert!(measured.ledger_id.is_some() && measured.cost_yuan.is_none());
    let unreported = json!({"usage_complete":false});
    let mut missing = call(&session, &unreported);
    missing.call_id = "call-2";
    let result = store.record_run_call(&missing, None).unwrap();
    assert!(result.ledger_id.is_none() && result.cost_yuan.is_none());
    assert_eq!(store.session_costs_detailed(&session).unwrap().len(), 1);
    assert_eq!(
        RunStore::new(&db).events_after("run", None).unwrap().len(),
        3
    );
}

#[test]
fn independent_connections_do_not_double_book_the_same_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let session = setup(&db);
    let other = Database::open(&path).unwrap();
    let report = json!({"usage_complete":true});
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            CostStore::new(&db)
                .record_run_call(&call(&session, &report), Some(&entry(&session)))
                .unwrap()
        });
        let second = scope.spawn(|| {
            CostStore::new(&other)
                .record_run_call(&call(&session, &report), Some(&entry(&session)))
                .unwrap()
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|r| r.inserted).count(), 1);
    assert_eq!(results[0].ledger_id, results[1].ledger_id);
    assert_eq!(
        CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn corrupt_or_unsupported_ownership_is_not_silently_positional() {
    let db = Database::open_in_memory().unwrap();
    let session = setup(&db);
    RunStore::new(&db)
        .append_event(
            "run",
            2,
            "usage",
            "completed",
            EVENT_TYPE,
            &json!({"version":99}),
        )
        .unwrap();
    assert!(CostStore::new(&db).run_call_ledger_ids(&session).is_err());
}
