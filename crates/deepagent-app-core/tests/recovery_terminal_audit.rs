//! Production startup recovery, actual file-backed history and
//! a transient terminal-update failure. No model, tool or user data is used.
use std::sync::Arc;

use deepagent_app_core::AppService;
use deepagent_core::clock::SystemClock;
use deepagent_core::event::EventPayload;
use deepagent_core::id::{SessionId, TaskId};
use deepagent_core::message::Message;
use deepagent_core::task::TaskState;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_session::Session;

fn add_run(db: &Database, id: SessionId, run: &str, now: i64) -> TaskId {
    let mut session = Session::recover(db, &SystemClock, id).unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: Message::user(run),
        })
        .unwrap();
    let task = session.create_task(run).unwrap();
    session.transition_task(task, TaskState::Running).unwrap();
    RunStore::new(db)
        .create(run, &id.to_string(), Some(&task.to_string()), now)
        .unwrap();
    RunStore::new(db)
        .transition(run, "running_turn", now + 1)
        .unwrap();
    task
}

fn sql(db: &Database, sql: &str) {
    db.with_conn(|conn| {
        conn.execute_batch(sql).unwrap();
        Ok(())
    })
    .unwrap();
}

fn proposed(kind: deepagent_runtime::TerminalKind) -> deepagent_runtime::KernelTerminal {
    deepagent_runtime::KernelTerminal {
        kind,
        message: None,
        reason: None,
    }
}

fn assert_boundaries(db: &Database, id: SessionId, run: &str, kind: &str, markers: usize) {
    let store = RunStore::new(db);
    let record = store.get(run).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some(kind));
    let ledger = store.events_after(run, None).unwrap();
    for (index, event) in ledger.iter().enumerate() {
        assert_eq!(event.sequence, index as u64);
    }
    for (event_type, count) in [
        ("run_terminal", 1),
        ("session_terminal_projected", 1),
        ("run_recovered_after_startup", markers),
    ] {
        assert_eq!(
            ledger.iter().filter(|e| e.event_type == event_type).count(),
            count,
            "{run}: {event_type}"
        );
    }
    let events = EventStore::new(db).load_session(id).unwrap();
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64);
    }
    let terminals: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::RunTerminal {
                run_id: Some(found),
                kind,
                task_id,
                ..
            } if found == run => Some((kind, task_id)),
            _ => None,
        })
        .collect();
    assert_eq!(terminals.len(), 1, "{run}: session terminals");
    assert_eq!(terminals[0].0, kind);
    assert_eq!(terminals[0].1.map(|task| task.to_string()), record.task_id);
}

#[test]
fn forked_keyed_history_keeps_terminal_outcomes_without_cloning_source_runs() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    let first = add_run(&db, id, "source-first", 1);
    deepagent_runtime::kernel::persist_terminal(
        &db,
        "source-first",
        &proposed(deepagent_runtime::TerminalKind::Succeeded),
        &Default::default(),
    )
    .unwrap();
    let second = add_run(&db, id, "source-second", 3);
    deepagent_runtime::kernel::persist_terminal(
        &db,
        "source-second",
        &proposed(deepagent_runtime::TerminalKind::Cancelled),
        &Default::default(),
    )
    .unwrap();
    let app = AppService::from_shared(db.clone());
    let source = app.session_conversation(&id.to_string()).unwrap();
    let tail = EventStore::new(&db)
        .load_session(id)
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    let fork = app.fork_session(&id.to_string(), tail).unwrap();
    let child: SessionId = fork.new_session_id.parse().unwrap();
    assert!(RunStore::new(&db)
        .list_for_session(&fork.new_session_id)
        .unwrap()
        .is_empty());
    let messages = app.session_conversation(&fork.new_session_id).unwrap();
    let users: Vec<_> = messages
        .iter()
        .filter(|message| message.role == "user")
        .collect();
    assert_eq!(users.len(), 2);
    for (index, (task, kind)) in [(first, "succeeded"), (second, "cancelled")]
        .into_iter()
        .enumerate()
    {
        let turn = users[index].turn.as_ref().unwrap();
        assert_eq!(turn.id, format!("{child}:{task}"));
        assert_eq!(turn.terminal_kind.as_deref(), Some(kind));
    }
    assert_eq!(
        messages,
        app.session_conversation(&fork.new_session_id).unwrap()
    );
    add_run(&db, child, "child-only", 5);
    deepagent_runtime::kernel::persist_terminal(
        &db,
        "child-only",
        &proposed(deepagent_runtime::TerminalKind::Succeeded),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        RunStore::new(&db)
            .list_for_session(&fork.new_session_id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(source, app.session_conversation(&id.to_string()).unwrap());
}

#[test]
fn recycled_history_is_deferred_until_explicit_restore() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    add_run(&db, id, "recycled", 1);
    RunStore::new(&db)
        .finish_with_event(
            "recycled",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            3,
        )
        .unwrap();
    let archive = deepagent_app_core::ArchiveService::new(db.clone());
    archive.archive_session(&id.to_string()).unwrap();
    archive.delete_archived_session(&id.to_string()).unwrap();
    let app = AppService::from_shared(db.clone());
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert!(EventStore::new(&db).load_session(id).unwrap().is_empty());
    assert_eq!(
        RunStore::new(&db)
            .pending_terminal_projections(&id.to_string())
            .unwrap(),
        ["recycled"]
    );
    archive.unarchive_session(&id.to_string()).unwrap();
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(&db, id, "recycled", "succeeded", 0);
}

#[test]
fn projection_after_explicit_rewind_does_not_resurrect_removed_turns() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    add_run(&db, id, "removed", 1);
    RunStore::new(&db)
        .finish_with_event(
            "removed",
            "cancelled",
            None,
            &serde_json::json!({"kind":"cancelled"}),
            None,
            3,
        )
        .unwrap();
    EventStore::new(&db).truncate_after(id, 0).unwrap();
    add_run(&db, id, "kept", 4);
    deepagent_runtime::kernel::persist_terminal(
        &db,
        "kept",
        &proposed(deepagent_runtime::TerminalKind::Succeeded),
        &Default::default(),
    )
    .unwrap();
    let app = AppService::from_shared(db.clone());
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(&db, id, "removed", "cancelled", 0);
    assert_boundaries(&db, id, "kept", "succeeded", 0);
    let messages = app.session_conversation(&id.to_string()).unwrap();
    let users: Vec<_> = messages
        .iter()
        .filter(|message| message.role == "user")
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].content, "kept");
    assert_eq!(
        users[0].turn.as_ref().unwrap().terminal_kind.as_deref(),
        Some("succeeded")
    );
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
}

#[test]
fn startup_ack_failure_is_compensated_after_reopen_without_another_recovery_marker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.db");
    let id;
    {
        let db = Arc::new(Database::open(&path).unwrap());
        id = Session::create(&db, &SystemClock, None).unwrap().id();
        add_run(&db, id, "startup-ack", 1);
        sql(&db, "CREATE TRIGGER fail_ack BEFORE INSERT ON run_events WHEN NEW.event_type='session_terminal_projected' BEGIN SELECT RAISE(ABORT, 'startup ack fixture'); END;");
        let app = AppService::from_shared(db.clone());
        assert!(app
            .recover_unfinished_runs()
            .unwrap_err()
            .to_string()
            .contains("startup ack fixture"));
        assert!(RunStore::new(&db)
            .get("startup-ack")
            .unwrap()
            .unwrap()
            .finished_at
            .is_some());
        sql(&db, "DROP TRIGGER fail_ack");
    }
    let app = AppService::open(&path).unwrap();
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(app.database(), id, "startup-ack", "failed", 1);
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
}

#[test]
fn pending_projection_does_not_bypass_a_live_dispatch_owner() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    let coordinator = deepagent_app_core::RunCoordinator::new(db.clone());
    coordinator
        .admit("live-projection", &id.to_string())
        .unwrap();
    sql(&db, "CREATE TRIGGER fail_ack BEFORE INSERT ON run_events WHEN NEW.event_type='session_terminal_projected' BEGIN SELECT RAISE(ABORT, 'live ack fixture'); END;");
    assert!(deepagent_runtime::kernel::persist_terminal(
        &db,
        "live-projection",
        &proposed(deepagent_runtime::TerminalKind::Cancelled),
        &Default::default()
    )
    .is_err());
    sql(&db, "DROP TRIGGER fail_ack");
    let app = AppService::from_shared(db.clone());
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_eq!(
        RunStore::new(&db)
            .pending_terminal_projections(&id.to_string())
            .unwrap(),
        ["live-projection"]
    );
    drop(coordinator);
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(&db, id, "live-projection", "cancelled", 0);
}

#[test]
fn concurrent_projection_through_independent_connections_commits_one_file_event() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    add_run(&db, id, "projection-race", 1);
    RunStore::new(&db)
        .finish_with_event(
            "projection-race",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded", "reason":null}),
            None,
            3,
        )
        .unwrap();
    let second = Arc::new(Database::open(&path).unwrap());
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for connection in [db.clone(), second] {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            EventStore::new(&connection).project_run_terminal(
                "projection-race",
                deepagent_core::clock::Timestamp::from_millis(4),
            )
        }));
    }
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    assert_boundaries(&db, id, "projection-race", "succeeded", 0);
}

#[test]
fn recovery_event_failure_rolls_back_the_entire_terminal_boundary() {
    for rejected in ["run_recovered_after_startup", "run_terminal"] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = Session::create(&db, &SystemClock, None).unwrap().id();
        add_run(&db, id, "retry", 1);
        sql(&db, &format!("CREATE TRIGGER fail_event BEFORE INSERT ON run_events WHEN NEW.event_type='{rejected}' BEGIN SELECT RAISE(ABORT, 'terminal insert fixture'); END;"));
        let app = AppService::from_shared(db.clone());
        assert!(app
            .recover_unfinished_runs()
            .unwrap_err()
            .to_string()
            .contains("terminal insert fixture"));
        assert!(RunStore::new(&db)
            .get("retry")
            .unwrap()
            .unwrap()
            .finished_at
            .is_none());
        assert_eq!(
            RunStore::new(&db)
                .events_after("retry", None)
                .unwrap()
                .len(),
            1
        );
        assert!(!EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .iter()
            .any(|e| matches!(e.payload, EventPayload::RunTerminal { .. })));
        sql(&db, "DROP TRIGGER fail_event");
        assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
        assert!(app.recover_unfinished_runs().unwrap().is_empty());
        assert_boundaries(&db, id, "retry", "failed", 1);
    }
}

#[test]
fn terminal_commit_survives_unavailable_session_file_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.db");
    let id;
    {
        let db = Arc::new(Database::open(&path).unwrap());
        id = Session::create(&db, &SystemClock, None).unwrap().id();
        add_run(&db, id, "file-unavailable", 1);
        let generation = std::fs::read_dir(db.session_transcript_dir(id))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "zst"))
            .unwrap();
        let parked = generation.with_extension("parked");
        assert!(generation.starts_with(directory.path()) && parked.starts_with(directory.path()));
        std::fs::rename(&generation, &parked).unwrap();
        let result = deepagent_runtime::kernel::persist_terminal(
            &db,
            "file-unavailable",
            &proposed(deepagent_runtime::TerminalKind::Succeeded),
            &Default::default(),
        );
        // Restore the fixture before asserting; no production data is moved.
        std::fs::rename(&parked, &generation).unwrap();
        assert!(result.unwrap_err().to_string().contains("session file"));
        assert_eq!(
            RunStore::new(&db)
                .get("file-unavailable")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("succeeded")
        );
        assert_eq!(
            RunStore::new(&db)
                .pending_terminal_projections(&id.to_string())
                .unwrap(),
            ["file-unavailable"]
        );
    }
    let app = AppService::open(&path).unwrap();
    // Compensation is not a new failure recovery or another run execution.
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(app.database(), id, "file-unavailable", "succeeded", 0);
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
}

#[test]
fn file_commit_with_stale_cursor_and_failed_ack_is_deduplicated_on_retry_or_reopen() {
    for reopen in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovery.db");
        let id;
        {
            let db = Arc::new(Database::open(&path).unwrap());
            id = Session::create(&db, &SystemClock, None).unwrap().id();
            add_run(&db, id, "ack-retry", 1);
            let old_count = EventStore::new(&db).load_session(id).unwrap().len();
            sql(&db, "CREATE TRIGGER fail_cursor BEFORE UPDATE ON session_file_state BEGIN SELECT RAISE(ABORT, 'cursor fixture'); END;
                CREATE TRIGGER fail_ack BEFORE INSERT ON run_events WHEN NEW.event_type='session_terminal_projected' BEGIN SELECT RAISE(ABORT, 'ack fixture'); END;");
            let error = deepagent_runtime::kernel::persist_terminal(
                &db,
                "ack-retry",
                &proposed(deepagent_runtime::TerminalKind::Succeeded),
                &Default::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("ack fixture"));
            let last_sequence: u64 = db
                .with_conn(|conn| {
                    Ok(conn
                        .query_row(
                            "SELECT last_sequence FROM session_file_state WHERE session_id=?1",
                            [id.to_string()],
                            |row| row.get(0),
                        )
                        .unwrap())
                })
                .unwrap();
            assert_eq!(last_sequence, old_count as u64 - 1);
            assert_eq!(
                EventStore::new(&db).load_session(id).unwrap().len(),
                old_count + 1
            );
            sql(&db, "DROP TRIGGER fail_cursor; DROP TRIGGER fail_ack;");
            if !reopen {
                // A losing failed proposal must not overwrite the succeeded outcome.
                assert!(!deepagent_runtime::kernel::persist_terminal(
                    &db,
                    "ack-retry",
                    &proposed(deepagent_runtime::TerminalKind::Failed),
                    &Default::default()
                )
                .unwrap());
                assert_eq!(
                    EventStore::new(&db).load_session(id).unwrap().len(),
                    old_count + 1
                );
                assert_boundaries(&db, id, "ack-retry", "succeeded", 0);
            }
        }
        let app = AppService::open(&path).unwrap();
        assert!(app.recover_unfinished_runs().unwrap().is_empty());
        assert_boundaries(app.database(), id, "ack-retry", "succeeded", 0);
        assert!(RunStore::new(app.database())
            .pending_terminal_projections(&id.to_string())
            .unwrap()
            .is_empty());
    }
}

#[test]
fn delayed_recovery_terminals_keep_each_run_and_the_later_completed_turn_separate() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.db");
    let id;
    {
        let db = Arc::new(Database::open(&path).unwrap());
        id = Session::create(&db, &SystemClock, None).unwrap().id();
        add_run(&db, id, "older", 1);
        add_run(&db, id, "middle", 3);
        let task = add_run(&db, id, "latest", 5);
        let mut session = Session::recover(&db, &SystemClock, id).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("latest answer"),
            })
            .unwrap();
        session.transition_task(task, TaskState::Completed).unwrap();
        deepagent_runtime::kernel::persist_terminal(
            &db,
            "latest",
            &proposed(deepagent_runtime::TerminalKind::Succeeded),
            &Default::default(),
        )
        .unwrap();
        let app = AppService::from_shared(db.clone());
        assert_eq!(app.recover_unfinished_runs().unwrap().len(), 2);
        for run in ["older", "middle"] {
            assert_boundaries(&db, id, run, "failed", 1);
        }
        assert_boundaries(&db, id, "latest", "succeeded", 0);
    }
    let app = AppService::open(&path).unwrap();
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    let messages = app.session_conversation(&id.to_string()).unwrap();
    for message in messages.iter().filter(|message| message.role == "user") {
        let turn = message.turn.as_ref().unwrap();
        assert_eq!(turn.id, message.content);
        assert_eq!(
            turn.terminal_kind.as_deref(),
            Some(if turn.id == "latest" {
                "succeeded"
            } else {
                "failed"
            })
        );
        if turn.id == "latest" {
            assert_eq!(turn.final_answer.as_deref(), Some("latest answer"));
        }
    }
}

#[test]
fn legacy_finish_only_recovery_is_backfilled_without_rewriting_old_markers() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    add_run(&db, id, "legacy-recovered", 1);
    let store = RunStore::new(&db);
    for now in [3, 4] {
        store
            .append_event(
                "legacy-recovered",
                now,
                "finalizing",
                "completed",
                "run_recovered_after_startup",
                &serde_json::json!({"reason":"old interruption"}),
            )
            .unwrap();
    }
    store
        .finish("legacy-recovered", "failed", Some("old interruption"), 5)
        .unwrap();
    let before = store.get("legacy-recovered").unwrap().unwrap();
    let app = AppService::from_shared(db.clone());
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_boundaries(&db, id, "legacy-recovered", "failed", 2);
    assert_eq!(store.get("legacy-recovered").unwrap().unwrap(), before);
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
}

#[test]
fn legacy_unkeyed_terminal_is_reused_only_in_its_task_interval() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    add_run(&db, id, "legacy", 1);
    let mut session = Session::recover(&db, &SystemClock, id).unwrap();
    session
        .append(
            serde_json::from_value(serde_json::json!({"type":"run_terminal", "kind":"succeeded"}))
                .unwrap(),
        )
        .unwrap();
    add_run(&db, id, "later", 3);
    let store = RunStore::new(&db);
    store
        .finish_with_event(
            "legacy",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            5,
        )
        .unwrap();
    let before = EventStore::new(&db).load_session(id).unwrap().len();
    EventStore::new(&db)
        .project_run_terminal("legacy", deepagent_core::clock::Timestamp::from_millis(6))
        .unwrap();
    assert_eq!(EventStore::new(&db).load_session(id).unwrap().len(), before);
    assert!(store
        .pending_terminal_projections(&id.to_string())
        .unwrap()
        .is_empty());
    let app = AppService::from_shared(db.clone());
    assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
    let messages = app.session_conversation(&id.to_string()).unwrap();
    let turns: Vec<_> = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.turn.as_ref().unwrap())
        .collect();
    assert_eq!(turns[0].terminal_kind.as_deref(), Some("succeeded"));
    assert_eq!(turns[1].terminal_kind.as_deref(), Some("failed"));
}

#[test]
fn conflicting_or_malformed_task_identity_is_not_silently_acknowledged() {
    for malformed in [false, true] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = Session::create(&db, &SystemClock, None).unwrap().id();
        if malformed {
            RunStore::new(&db)
                .create("conflict", &id.to_string(), Some("invalid-task-id"), 1)
                .unwrap();
        } else {
            add_run(&db, id, "conflict", 1);
        }
        if !malformed {
            Session::recover(&db, &SystemClock, id)
                .unwrap()
                .append(EventPayload::RunTerminal {
                    run_id: Some("conflict".into()),
                    task_id: Some(TaskId::new()),
                    kind: "succeeded".into(),
                    reason: None,
                })
                .unwrap();
        }
        let before = EventStore::new(&db).load_session(id).unwrap().len();
        let error = deepagent_runtime::kernel::persist_terminal(
            &db,
            "conflict",
            &proposed(deepagent_runtime::TerminalKind::Succeeded),
            &Default::default(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains(if malformed { "invalid" } else { "conflicting" }));
        assert_eq!(EventStore::new(&db).load_session(id).unwrap().len(), before);
        assert_eq!(
            RunStore::new(&db)
                .pending_terminal_projections(&id.to_string())
                .unwrap(),
            ["conflict"]
        );
        let messages = AppService::from_shared(db.clone())
            .session_conversation(&id.to_string())
            .unwrap();
        assert!(messages.iter().all(|message| message
            .turn
            .as_ref()
            .map_or(true, |turn| turn.terminal_kind.is_none())));
    }
}

#[test]
fn audit_startup_recovery_preserves_one_replayable_terminal() {
    let mut observations = Vec::new();
    for inject_update_failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audit.sqlite");
        let session_id;
        {
            let db = Arc::new(Database::open(&path).unwrap());
            let task;
            {
                let mut session =
                    Session::create(&db, &SystemClock, Some("recovery audit")).unwrap();
                session_id = session.id().to_string();
                session
                    .append(EventPayload::MessageAppended {
                        message: Message::user("audit interrupted request"),
                    })
                    .unwrap();
                task = session.create_task("interrupted").unwrap();
                session.transition_task(task, TaskState::Running).unwrap();
            }
            RunStore::new(&db)
                .create("audit-run", &session_id, Some(&task.to_string()), 1)
                .unwrap();
            RunStore::new(&db)
                .transition("audit-run", "running_turn", 2)
                .unwrap();
            let service = AppService::from_shared(db.clone());
            assert!(service.host_role().is_primary());
            if inject_update_failure {
                db.with_conn(|conn| {
                    conn.execute_batch(
                        "CREATE TRIGGER audit_fail_finish BEFORE UPDATE OF finished_at ON runs
                         WHEN NEW.id='audit-run' AND NEW.finished_at IS NOT NULL
                         BEGIN SELECT RAISE(ABORT, 'audit transient finish failure'); END;",
                    )
                    .unwrap();
                    Ok(())
                })
                .unwrap();
                let error = service.recover_unfinished_runs().unwrap_err();
                assert!(error.to_string().contains("audit transient finish failure"));
                assert!(RunStore::new(&db)
                    .get("audit-run")
                    .unwrap()
                    .unwrap()
                    .finished_at
                    .is_none());
                db.with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER audit_fail_finish")
                        .unwrap();
                    Ok(())
                })
                .unwrap();
            }
            let recovered = service.recover_unfinished_runs().unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].terminal_kind, "failed");
            assert!(service.recover_unfinished_runs().unwrap().is_empty());
            assert_eq!(
                Session::recover(&db, &SystemClock, session_id.parse::<SessionId>().unwrap())
                    .unwrap()
                    .state()
                    .task(task)
                    .unwrap()
                    .state,
                TaskState::Failed
            );
        }
        // All old handles and the host lock have been dropped. Inspect the
        // production projection from a new service over the same actual files.
        let reopened = AppService::open(&path).unwrap();
        assert!(reopened.recover_unfinished_runs().unwrap().is_empty());
        let db = reopened.database();
        let run = RunStore::new(db).get("audit-run").unwrap().unwrap();
        assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
        let events = RunStore::new(db).events_after("audit-run", None).unwrap();
        let session_events = EventStore::new(db)
            .load_session(session_id.parse().unwrap())
            .unwrap();
        let conversation = reopened.session_conversation(&session_id).unwrap();
        let turn = conversation
            .iter()
            .find_map(|message| message.turn.as_ref())
            .expect("a recovered conversation turn");
        assert_eq!(turn.id, "audit-run");
        observations.push(serde_json::json!({
            "injected_finish_failure": inject_update_failure,
            "run_kind": run.terminal_kind,
            "recovery_markers": events.iter().filter(|e| e.event_type == "run_recovered_after_startup").count(),
            "run_terminals": events.iter().filter(|e| e.event_type == "run_terminal").count(),
            "session_terminals": session_events.iter().filter(|e| matches!(e.payload, EventPayload::RunTerminal { .. })).count(),
            "conversation_turn_kind": turn.terminal_kind,
        }));
    }
    eprintln!("{}", serde_json::to_string_pretty(&observations).unwrap());
    for observed in observations {
        assert_eq!(
            observed["recovery_markers"], 1,
            "recovery retry must not duplicate durable recovery boundaries"
        );
        assert_eq!(
            observed["run_terminals"], 1,
            "recovery must use the canonical terminal event"
        );
        assert_eq!(observed["session_terminals"], 1);
        assert_eq!(observed["conversation_turn_kind"], "failed");
    }
}

#[test]
#[ignore = "audit H-95: startup interruption classification must follow each run's task boundary"]
fn audit_recovery_classifies_each_run_from_its_own_task_turn() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let session_id;
    let old_task;
    let latest_task;
    {
        let mut session =
            Session::create(&db, &SystemClock, Some("overlapping recovery audit")).unwrap();
        session_id = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("older prompt whose tool call was interrupted"),
            })
            .unwrap();
        old_task = session.create_task("older in-progress run").unwrap();
        session
            .transition_task(old_task, TaskState::Running)
            .unwrap();
        session
            .append(EventPayload::ToolCallRequested {
                call: deepagent_core::message::ToolCall {
                    id: "old-call".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path":"audit.txt"}),
                },
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("newer prompt not yet answered"),
            })
            .unwrap();
        latest_task = session.create_task("newer in-progress run").unwrap();
        session
            .transition_task(latest_task, TaskState::Running)
            .unwrap();
    }

    let runs = RunStore::new(&db);
    runs.create(
        "audit-old-run",
        &session_id,
        Some(&old_task.to_string()),
        10,
    )
    .unwrap();
    runs.transition("audit-old-run", "running_turn", 11)
        .unwrap();
    runs.create(
        "audit-latest-run",
        &session_id,
        Some(&latest_task.to_string()),
        12,
    )
    .unwrap();
    runs.transition("audit-latest-run", "running_turn", 13)
        .unwrap();

    let service = AppService::from_shared(db);
    let projected_turn_ids: std::collections::HashSet<_> = service
        .session_conversation(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|message| message.turn.map(|turn| turn.id))
        .collect();
    assert!(projected_turn_ids.contains("audit-old-run"));
    assert!(projected_turn_ids.contains("audit-latest-run"));

    let recovered = service.recover_unfinished_runs().unwrap();
    let old = recovered
        .iter()
        .find(|run| run.run_id == "audit-old-run")
        .expect("older run recovery result");
    let latest = recovered
        .iter()
        .find(|run| run.run_id == "audit-latest-run")
        .expect("latest run recovery result");
    eprintln!(
        "run interruption audit: old={:?}, latest={:?}",
        old.interruption, latest.interruption
    );
    assert_eq!(
        old.interruption.as_deref(),
        Some("interrupted_turn"),
        "the older run ends in a pending tool call before the newer prompt"
    );
    assert_eq!(
        latest.interruption.as_deref(),
        Some("interrupted_prompt"),
        "the latest run ends immediately after its user prompt"
    );
}
