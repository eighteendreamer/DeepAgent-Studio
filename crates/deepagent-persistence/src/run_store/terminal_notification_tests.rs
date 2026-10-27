use super::*;
use std::sync::Arc;

fn seed(db: &Database) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s', 1, 1)",
            [],
        )
        .map_err(map_sqlite)?;
        Ok(())
    })
    .unwrap();
    RunStore::new(db).create("r", "s", None, 1).unwrap();
}

fn finish(db: &Database, kind: &str, event: &str, now: i64) -> Result<bool> {
    RunStore::new(db).finish_with_notification(
        "r",
        RunTerminalUpdate {
            kind,
            reason: None,
            data: &serde_json::json!({"kind":kind}),
        },
        (event, &serde_json::json!({"outcome":kind,"message":"完成"})),
        Some("audit"),
        now,
    )
}

#[test]
fn notification_and_terminal_rejections_roll_back_the_entire_transaction() {
    for rejected in ["run_completed", "run_terminal", "update"] {
        let db = Database::open_in_memory().unwrap();
        seed(&db);
        db.with_conn(|conn| {
            let statement = if rejected == "update" {
                "CREATE TRIGGER reject_terminal BEFORE UPDATE ON runs WHEN NEW.finished_at IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'injected terminal update failure'); END;".into()
            } else {
                format!("CREATE TRIGGER reject_terminal BEFORE INSERT ON run_events WHEN NEW.event_type='{rejected}'
                        BEGIN SELECT RAISE(ABORT, 'injected terminal event failure'); END;")
            };
            conn.execute_batch(&statement).unwrap();
            Ok(())
        }).unwrap();
        assert!(finish(&db, "succeeded", "run_completed", 2).is_err());
        let run = RunStore::new(&db).get("r").unwrap().unwrap();
        assert!(run.finished_at.is_none() && run.terminal_kind.is_none());
        let events = RunStore::new(&db).events_after("r", None).unwrap();
        assert_eq!(
            events.len(),
            1,
            "all notification/cancel/terminal rows must roll back"
        );
        assert_eq!(events[0].event_type, "run_accepted");
    }
}

#[test]
fn committed_pair_survives_reopen_and_conflicting_duplicate_finalization() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("terminal.sqlite");
    let expected = {
        let db = Database::open(&path).unwrap();
        seed(&db);
        assert!(finish(&db, "succeeded", "run_completed", 2).unwrap());
        assert!(!finish(&db, "failed", "run_failed", 3).unwrap());
        let events = RunStore::new(&db).events_after("r", None).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            [
                "run_accepted",
                "cancel.requested",
                "cancel.propagated",
                "run_completed",
                "run_terminal"
            ]
        );
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event.sequence, sequence as u64);
        }
        events
    };
    let db = Database::open(path).unwrap();
    let store = RunStore::new(&db);
    assert_eq!(store.events_after("r", None).unwrap(), expected);
    assert_eq!(store.events_after("r", Some(2)).unwrap(), expected[3..]);
    let run = store.get("r").unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    assert_eq!(run.finished_at, Some(2));
}

#[test]
fn concurrent_finalizers_commit_exactly_one_matching_notification_pair() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    seed(&db);
    let results = std::thread::scope(|scope| {
        let handles = (0..16)
            .map(|index| {
                let db = db.clone();
                scope.spawn(move || {
                    let (kind, event) = if index % 2 == 0 {
                        ("succeeded", "run_completed")
                    } else {
                        ("failed", "run_failed")
                    };
                    finish(&db, kind, event, index + 2).unwrap()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|changed| **changed).count(), 1);
    let events = RunStore::new(&db).events_after("r", None).unwrap();
    let notifications = events
        .iter()
        .filter(|event| matches!(event.event_type.as_str(), "run_completed" | "run_failed"))
        .collect::<Vec<_>>();
    assert_eq!(notifications.len(), 1);
    let terminals = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    let run = RunStore::new(&db).get("r").unwrap().unwrap();
    assert_eq!(
        notifications[0].data["outcome"].as_str(),
        run.terminal_kind.as_deref()
    );
    assert_eq!(
        terminals[0].data["kind"].as_str(),
        run.terminal_kind.as_deref()
    );
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, sequence as u64);
    }
}
