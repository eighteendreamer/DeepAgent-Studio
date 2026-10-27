//! H-143: the actual callback used by Desktop/CLI must only see committed completion.
use super::*;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const RUN: &str = "terminal-service";
const RESTART_ROOT: &str = "DEEPAGENT_H143_RESTART_ROOT";
const RESTART_REJECT: &str = "DEEPAGENT_H143_RESTART_REJECT";

fn restart_child_environment(command: &mut tokio::process::Command, root: &Path) {
    command.env_clear().kill_on_drop(true);
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "PATHEXT",
        "COMSPEC",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("TEMP", root)
        .env("TMP", root)
        .env("TMPDIR", root)
        .env("LANG", "C.UTF-8");
}

async fn observe(mode: &str, rejected: Option<&str>) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("terminal.sqlite");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let expected = {
        let db = Arc::new(Database::open(&path).unwrap());
        if let Some(event) = rejected {
            db.with_conn(|conn| {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER reject_terminal BEFORE INSERT ON run_events
                    WHEN NEW.run_id='{RUN}' AND NEW.event_type='{event}'
                    BEGIN SELECT RAISE(ABORT, 'formal terminal rejection'); END;"
                ))
                .unwrap();
                Ok(())
            })
            .unwrap();
        }
        let model = Arc::new(ReplayTransport::new([response_text(
            "formal committed answer",
        )]));
        let chat = ChatService::new(
            db.clone(),
            event_persistence::settings(db.clone()).await,
            model.clone(),
            &workspace,
        );
        let live = Arc::new(Mutex::new(Vec::new()));
        let capture = live.clone();
        let callback_db = db.clone();
        let on_event = move |event: RuntimeEvent| {
            if event.is_terminal() {
                let store = RunStore::new(&callback_db);
                let run = store.get(RUN).unwrap().unwrap();
                assert!(
                    run.finished_at.is_some(),
                    "formal callback arrived before commit"
                );
                let events = store.events_after(RUN, None).unwrap();
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| event.event_type == "run_terminal")
                        .count(),
                    1
                );
                if matches!(&event, RuntimeEvent::RunCompleted { .. }) {
                    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
                    assert_eq!(
                        events
                            .iter()
                            .filter(|event| event.event_type == "run_completed")
                            .count(),
                        1
                    );
                }
            }
            capture.lock().unwrap().push(event);
        };
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            if mode == "workflow" {
                let session = chat.create_session(None).unwrap();
                let workflow = serde_json::from_value(serde_json::json!({"definition":{
                "version":1,"nodes":[{"id":"start","kind":"start"},
                    {"id":"write","kind":"tool","config":{"toolName":"write_file",
                        "toolArguments":{"path":"effect.txt","content":"committed tool effect"}}},
                    {"id":"end","kind":"end"}],
                "edges":[{"id":"a","source":"start","target":"write"},
                    {"id":"b","source":"write","target":"end"}]}}))
                .unwrap();
                chat.run_workflow(workflow, RUN.into(), session, on_event, |_| {})
                    .await
            } else {
                chat.run_in_session(
                    "Report the result.",
                    None,
                    None,
                    None,
                    vec![],
                    (mode == "preflight").then(|| "local preflight result".into()),
                    false,
                    Some(RUN),
                    on_event,
                    |_| {},
                )
                .await
            }
        })
        .await
        .expect("formal terminal finalization must settle");
        let events = RunStore::new(&db).events_after(RUN, None).unwrap();
        let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
        let completed = live
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::RunCompleted { .. }))
            .count();
        if rejected.is_some() {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("formal terminal rejection"));
            assert_eq!(completed, 0);
            assert!(!events
                .iter()
                .any(|event| event.event_type == "run_completed"));
            if rejected == Some("run_terminal") {
                assert!(run.finished_at.is_none());
                assert!(!events
                    .iter()
                    .any(|event| event.event_type == "run_terminal"));
            } else {
                assert_eq!(
                    run.terminal_kind.as_deref(),
                    Some("failed"),
                    "outer failure must not retry success"
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| event.event_type == "run_terminal")
                        .count(),
                    1
                );
            }
        } else {
            result.unwrap();
            assert_eq!(completed, 1);
            assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == "run_completed")
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
        }
        if mode == "workflow" {
            assert_eq!(
                std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
                "committed tool effect"
            );
        }
        if mode == "preflight" || mode == "workflow" {
            assert_eq!(
                model.turns.lock().unwrap().len(),
                1,
                "this path must not invoke the model"
            );
        }
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event.sequence, sequence as u64);
        }
        events
    };
    let db = Database::open(path).unwrap();
    let store = RunStore::new(&db);
    let events = store.events_after(RUN, None).unwrap();
    assert_eq!(
        events, expected,
        "reopening must preserve the committed outcome pair"
    );
    let cursor = events[events.len() / 2].sequence;
    assert_eq!(
        store.events_after(RUN, Some(cursor)).unwrap(),
        events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn completion_callbacks_observe_committed_chat_preflight_and_workflow_outcomes() {
    for mode in ["chat", "preflight", "workflow"] {
        observe(mode, None).await;
    }
}

#[tokio::test]
async fn rejected_canonical_terminal_never_leaks_live_or_replayed_completion() {
    for mode in ["chat", "preflight", "workflow"] {
        observe(mode, Some("run_terminal")).await;
    }
}

#[tokio::test]
async fn rejected_completion_row_rolls_back_success_and_preserves_failure_propagation() {
    for mode in ["chat", "preflight", "workflow"] {
        observe(mode, Some("run_completed")).await;
    }
}

#[tokio::test]
async fn terminal_commit_survives_real_process_restart_and_startup_recovery() {
    for rejected in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("home")).unwrap();
        std::fs::create_dir(root.path().join("workspace")).unwrap();
        let database_path = root.path().join("state.sqlite");
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "terminal_persistence::terminal_persistence_process_child",
            "--ignored",
            "--test-threads=1",
        ]);
        restart_child_environment(&mut child, root.path());
        child
            .env(RESTART_ROOT, root.path())
            .env(RESTART_REJECT, rejected.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        child.creation_flags(0x08000000);
        let output = tokio::time::timeout(Duration::from_secs(20), child.output())
            .await
            .expect("the isolated terminal child must exit")
            .unwrap();
        assert!(
            output.status.success(),
            "terminal child failed: status={:?}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let database = Arc::new(Database::open(&database_path).unwrap());
        let store = RunStore::new(&database);
        let before_recovery = store.get(RUN).unwrap().unwrap();
        let before_events = store.events_after(RUN, None).unwrap();
        assert_eq!(
            before_events
                .iter()
                .filter(|event| event.event_type == "run_completed")
                .count(),
            usize::from(!rejected),
            "a rejected terminal transaction must not leave a completion row"
        );
        if rejected {
            assert!(before_recovery.finished_at.is_none());
            assert!(!before_events
                .iter()
                .any(|event| event.event_type == "run_terminal"));

            database
                .with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER reject_h143_terminal;")
                        .unwrap();
                    Ok(())
                })
                .unwrap();
            let service = deepagent_app_core::AppService::from_shared(database.clone());
            let recovered = service.recover_unfinished_runs().unwrap();
            assert_eq!(recovered.len(), 1);
            let after = RunStore::new(&database).get(RUN).unwrap().unwrap();
            assert_eq!(after.terminal_kind.as_deref(), Some("failed"));
            let after_events = RunStore::new(&database).events_after(RUN, None).unwrap();
            assert!(!after_events
                .iter()
                .any(|event| event.event_type == "run_completed"));
            assert_eq!(
                after_events
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
            assert!(service.recover_unfinished_runs().unwrap().is_empty());
        } else {
            assert_eq!(before_recovery.terminal_kind.as_deref(), Some("succeeded"));
            assert!(before_recovery.finished_at.is_some());
            assert_eq!(
                before_events
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
            assert!(before_events
                .iter()
                .any(|event| event.event_type == "run_completed"));
            let service = deepagent_app_core::AppService::from_shared(database);
            assert!(service.recover_unfinished_runs().unwrap().is_empty());
        }
    }
}

#[tokio::test]
#[ignore = "helper: only launched by the real process restart test"]
async fn terminal_persistence_process_child() {
    let root_value = std::env::var(RESTART_ROOT).expect("isolated restart root");
    let root = Path::new(&root_value);
    let rejected = std::env::var(RESTART_REJECT).unwrap() == "true";
    let database = Arc::new(Database::open(root.join("state.sqlite")).unwrap());
    if rejected {
        database
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_h143_terminal BEFORE INSERT ON run_events
                     WHEN NEW.run_id='terminal-service' AND NEW.event_type='run_terminal'
                     BEGIN SELECT RAISE(ABORT, 'process restart terminal rejection'); END;",
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
    }
    let model = Arc::new(ReplayTransport::new([response_text(
        "process restart answer",
    )]));
    let settings = event_persistence::settings(database.clone()).await;
    let chat = ChatService::new(database, settings, model, root.join("workspace"));
    let result = chat
        .run_in_session(
            "Complete the process restart fixture.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some(RUN),
            |_| {},
            |_| {},
        )
        .await;
    if rejected {
        assert!(
            result.is_err(),
            "rejected terminal must reach the child caller"
        );
    } else {
        result.unwrap();
    }
}
