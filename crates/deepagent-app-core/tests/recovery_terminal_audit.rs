//! Opt-in audit: production startup recovery, actual file-backed history and
//! a transient terminal-update failure. No model, tool or user data is used.
use std::sync::Arc;

use deepagent_app_core::AppService;
use deepagent_core::clock::SystemClock;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_core::message::Message;
use deepagent_core::task::TaskState;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_session::Session;

#[test]
#[ignore = "audit: startup recovery must commit one replayable terminal across retry and reopen"]
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
