//! H-95: actual authority files/SQLite, existing recovery, no model or tool execution.
use super::*;
use deepagent_core::response_item::ResponseItem;

#[derive(Clone, Copy, Debug)]
enum Tail {
    Prompt,
    Tool,
    Reply,
    Quiet,
}

fn seed(db: &Database, id: SessionId, run: &str, tail: Tail, now: i64, native: bool) -> TaskId {
    let mut session = Session::recover(db, &SystemClock, id).unwrap();
    if !matches!(tail, Tail::Quiet) {
        if native {
            session
                .append_response_item(ResponseItem::InputMessage {
                    role: "user".into(),
                    content: run.into(),
                    attachments: Vec::new(),
                })
                .unwrap();
        } else {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(run),
                })
                .unwrap();
        }
    }
    let task = session.create_task(run).unwrap();
    session.transition_task(task, TaskState::Running).unwrap();
    RunStore::new(db)
        .create(run, &id.to_string(), Some(&task.to_string()), now)
        .unwrap();
    RunStore::new(db)
        .transition(run, "running_turn", now + 1)
        .unwrap();
    match tail {
        Tail::Tool if native => {
            session
                .append_response_item(ResponseItem::FunctionCall {
                    call_id: format!("{run}-call"),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                })
                .unwrap();
        }
        Tail::Tool => {
            session
                .append(EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: format!("{run}-call"),
                        name: "read_file".into(),
                        arguments: serde_json::json!({}),
                    },
                })
                .unwrap();
        }
        Tail::Reply => {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::assistant("settled reply"),
                })
                .unwrap();
        }
        _ => {}
    }
    task
}

fn assert_reason(app: &AppService, run: &str, expected: Option<&str>) {
    let events = RunStore::new(app.database())
        .events_after(run, None)
        .unwrap();
    let markers: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_recovered_after_startup")
        .collect();
    assert_eq!(markers.len(), 1);
    assert_eq!(
        markers[0].data["interruption"].as_str(),
        expected,
        "{run}: durable marker"
    );
}

#[test]
fn h95_each_run_keeps_its_own_reason_after_recovery_and_cold_reopen() {
    for reverse in [false, true] {
        for (older, latest, old_reason, latest_reason) in [
            (
                Tail::Tool,
                Tail::Prompt,
                Some("interrupted_turn"),
                Some("interrupted_prompt"),
            ),
            (
                Tail::Prompt,
                Tail::Tool,
                Some("interrupted_prompt"),
                Some("interrupted_turn"),
            ),
            (Tail::Reply, Tail::Prompt, None, Some("interrupted_prompt")),
            (Tail::Quiet, Tail::Prompt, None, Some("interrupted_prompt")),
            (Tail::Tool, Tail::Reply, Some("interrupted_turn"), None),
            (Tail::Quiet, Tail::Quiet, None, None),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("recovery.sqlite");
            let db = Arc::new(Database::open(&path).unwrap());
            let id = Session::create(&db, &SystemClock, None).unwrap().id();
            let first = if reverse { 200 } else { 100 };
            seed(&db, id, "older", older, first, false);
            seed(&db, id, "latest", latest, 300 - first, false);
            let app = AppService::from_shared(db.clone());
            let recovered = app.recover_unfinished_runs().unwrap();
            assert_eq!(recovered.len(), 2);
            for (run, expected) in [("older", old_reason), ("latest", latest_reason)] {
                assert_eq!(
                    recovered
                        .iter()
                        .find(|row| row.run_id == run)
                        .unwrap()
                        .interruption
                        .as_deref(),
                    expected,
                    "reverse={reverse}, older={older:?}, latest={latest:?}, run={run}"
                );
                assert_reason(&app, run, expected);
                assert_boundaries(&db, id, run, "failed", 1);
            }
            let ledgers: Vec<_> = ["older", "latest"]
                .into_iter()
                .map(|run| RunStore::new(&db).events_after(run, None).unwrap())
                .collect();
            let conversation = app.session_conversation(&id.to_string()).unwrap();
            drop(app);
            drop(db);
            let app = AppService::open(&path).unwrap();
            assert!(app.recover_unfinished_runs().unwrap().is_empty());
            assert_eq!(
                app.session_conversation(&id.to_string()).unwrap(),
                conversation
            );
            for (index, (run, expected)) in [("older", old_reason), ("latest", latest_reason)]
                .into_iter()
                .enumerate()
            {
                assert_reason(&app, run, expected);
                assert_eq!(
                    RunStore::new(app.database())
                        .events_after(run, None)
                        .unwrap(),
                    ledgers[index]
                );
            }
        }
    }
}

#[test]
fn h95_native_and_unbound_runs_do_not_borrow_another_tasks_prompt() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("native.sqlite")).unwrap());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    seed(&db, id, "native-tool", Tail::Tool, 100, true);
    seed(&db, id, "native-prompt", Tail::Prompt, 200, true);
    seed(&db, id, "no-prompt-task", Tail::Quiet, 300, false);
    RunStore::new(&db)
        .create("no-task", &id.to_string(), None, 400)
        .unwrap();
    Session::recover(&db, &SystemClock, id)
        .unwrap()
        .append_response_item(ResponseItem::Message {
            role: "user".into(),
            content: "unclaimed later input".into(),
        })
        .unwrap();
    let app = AppService::from_shared(db);
    let recovered = app.recover_unfinished_runs().unwrap();
    assert_eq!(recovered.len(), 4);
    for (run, expected) in [
        ("native-tool", Some("interrupted_turn")),
        ("native-prompt", Some("interrupted_prompt")),
        ("no-prompt-task", None),
        ("no-task", None),
    ] {
        assert_eq!(
            recovered
                .iter()
                .find(|row| row.run_id == run)
                .unwrap()
                .interruption
                .as_deref(),
            expected,
            "{run}"
        );
        assert_reason(&app, run, expected);
    }
}

#[test]
fn h95_shared_task_identity_is_unknown_instead_of_borrowed_by_either_run() {
    for duplicate_creation in [false, true] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = Session::create(&db, &SystemClock, None).unwrap().id();
        let task = seed(&db, id, "first-owner", Tail::Tool, 1, false);
        if duplicate_creation {
            Session::recover(&db, &SystemClock, id)
                .unwrap()
                .append(EventPayload::TaskCreated {
                    task_id: task,
                    goal: "ambiguous duplicate task creation".into(),
                })
                .unwrap();
        } else {
            RunStore::new(&db)
                .create("second-owner", &id.to_string(), Some(&task.to_string()), 3)
                .unwrap();
        }
        let app = AppService::from_shared(db);
        let recovered = app.recover_unfinished_runs().unwrap();
        assert_eq!(recovered.len(), if duplicate_creation { 1 } else { 2 });
        for record in recovered {
            assert_eq!(
                record.interruption, None,
                "ambiguous task owner: {}",
                record.run_id
            );
            assert_reason(&app, &record.run_id, None);
        }
    }
}

#[test]
fn h95_missing_or_invalid_bound_task_remains_an_explicit_recovery_error() {
    for missing in [TaskId::new().to_string(), "invalid-task".into()] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let id = Session::create(&db, &SystemClock, None).unwrap().id();
        RunStore::new(&db)
            .create("bad", &id.to_string(), Some(&missing), 1)
            .unwrap();
        let before = EventStore::new(&db).load_session(id).unwrap();
        let app = AppService::from_shared(db.clone());
        assert!(app.recover_unfinished_runs().is_err());
        assert!(RunStore::new(&db)
            .get("bad")
            .unwrap()
            .unwrap()
            .finished_at
            .is_none());
        assert_eq!(EventStore::new(&db).load_session(id).unwrap(), before);
        assert!(!RunStore::new(&db)
            .events_after("bad", None)
            .unwrap()
            .iter()
            .any(|e| e.event_type == "run_recovered_after_startup"));
    }
}

#[test]
fn h95_native_first_prompt_mirror_inside_a_task_is_not_a_new_input_boundary() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("native-first.sqlite")).unwrap());
    let mut session = Session::create(&db, &SystemClock, None).unwrap();
    let id = session.id();
    session
        .append_response_item(ResponseItem::Message {
            role: "user".into(),
            content: "native-first input".into(),
        })
        .unwrap();
    let task = session.create_task("native-first task").unwrap();
    session.transition_task(task, TaskState::Running).unwrap();
    // Supported native-first source projection: this is the existing Session
    // API for a UI message whose native item has already been persisted.
    session
        .append_without_response_projection(EventPayload::MessageAppended {
            message: Message::user("native-first input"),
        })
        .unwrap();
    session
        .append_response_item(ResponseItem::FunctionCall {
            call_id: "native-call".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        })
        .unwrap();
    RunStore::new(&db)
        .create(
            "native-first",
            &id.to_string(),
            Some(&task.to_string()),
            100,
        )
        .unwrap();
    drop(session);
    seed(&db, id, "later-input", Tail::Prompt, 200, true);
    let app = AppService::from_shared(db);
    let recovered = app.recover_unfinished_runs().unwrap();
    for (run, reason) in [
        ("native-first", "interrupted_turn"),
        ("later-input", "interrupted_prompt"),
    ] {
        assert_eq!(
            recovered
                .iter()
                .find(|r| r.run_id == run)
                .unwrap()
                .interruption
                .as_deref(),
            Some(reason),
            "{run}"
        );
        assert_reason(&app, run, Some(reason));
    }
}
