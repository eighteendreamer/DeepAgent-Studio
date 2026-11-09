//! H-298: both projections must implement the same exclusive u64 lower bound.
use super::*;
use serde_json::json;

fn populate(db: &Database) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, created_at, updated_at) VALUES ('cursor-session', 1, 1)",
            [],
        )
        .map_err(map_sqlite)?;
        Ok(())
    })
    .unwrap();
    let store = RunStore::new(db);
    for (run, count) in [("short", 3), ("long", 7)] {
        store.create(run, "cursor-session", None, 1).unwrap();
        for index in 0..count {
            store
                .append_event(
                    run,
                    2 + index,
                    "running",
                    "completed",
                    "synthetic.cursor",
                    &json!({"run":run,"index":index}),
                )
                .unwrap();
        }
        store
            .finish_with_event(run, "succeeded", None, &json!({}), None, 20)
            .unwrap();
    }
}

fn paged(store: &RunStore<'_>, run: &str, after: Option<u64>) -> Vec<StoredRunEvent> {
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = store
            .events_page(
                run,
                after,
                cursor.as_ref(),
                HistoryReadBudget {
                    max_rows: 2,
                    max_bytes: 4096,
                    snapshot: None,
                },
            )
            .unwrap();
        let next = page.rows.last().map(|row| row.cursor.clone());
        events.extend(page.rows.into_iter().map(|row| row.value));
        if !page.has_more {
            return events;
        }
        assert!(next.is_some(), "a continuation must advance");
        cursor = next;
        assert!(
            events.len() < 100,
            "finite synthetic history must terminate"
        );
    }
}

fn assert_reads(db: &Database) {
    let store = RunStore::new(db);
    for run in ["short", "long", "missing"] {
        let before = store.events_after(run, None).unwrap();
        let record = store.get(run).unwrap();
        let tail = before.last().map_or(0, |event| event.sequence);
        for after in [
            None,
            Some(0),
            Some(1),
            Some(tail / 2),
            Some(tail),
            Some(tail + 1),
            Some(i64::MAX as u64),
            Some(i64::MAX as u64 + 1),
            Some(u64::MAX),
        ] {
            let expected: Vec<_> = before
                .iter()
                .filter(|event| after.map_or(true, |after| event.sequence > after))
                .cloned()
                .collect();
            assert_eq!(
                store.events_after(run, after).unwrap(),
                expected,
                "direct read must not rewind: run={run}, after={after:?}"
            );
            assert_eq!(paged(&store, run, after), expected);
            assert_eq!(store.events_after(run, None).unwrap(), before);
            assert_eq!(store.get(run).unwrap(), record);
        }
    }
}

#[test]
fn h298_direct_and_paged_reads_agree_at_cursor_domain_boundaries() {
    let db = Database::open_in_memory().unwrap();
    populate(&db);
    assert_reads(&db);
}

#[test]
fn h298_file_reopen_keeps_future_cursors_empty_without_changing_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cursor.sqlite");
    {
        let db = Database::open(&path).unwrap();
        populate(&db);
    }
    // The writer connection is gone, not just an additional reader handle.
    let reopened = Database::open(&path).unwrap();
    assert_reads(&reopened);
}
