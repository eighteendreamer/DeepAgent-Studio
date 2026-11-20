//! H-64: current production automatic checks, not the standalone legacy runner.

use super::*;
use deepagent_app_core::ApprovalPolicy;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use std::time::Duration;

#[path = "../fixtures/verification_build.rs"]
mod fixture;
use fixture::BuildFixture;

async fn observe(post_edit: bool, cancel: bool, parent_exits: bool) {
    observe_fixture(BuildFixture::new().await, post_edit, cancel, parent_exits).await;
}

async fn observe_fixture(fixture: BuildFixture, post_edit: bool, cancel: bool, parent_exits: bool) {
    let db = Arc::new(Database::open(fixture.root.join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-placeholder")
        .await
        .unwrap();
    settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    let path = if post_edit {
        "src/added.rs"
    } else {
        "note.txt"
    };
    let model = Arc::new(ReplayTransport::new(
        std::iter::once(response_function_call(
            "fixture-write",
            "write_file",
            serde_json::json!({"path":path,"content":"// fixture edit\n"}),
        ))
        .chain((0..8).map(|_| response_text("Finished the fixture edit."))),
    ));
    // Default Direct backend, real write_file, normal production assembler.
    let chat = ChatService::new(db.clone(), settings, model, &fixture.root);
    let run_chat = chat.clone();
    let run_id = "verification-lifecycle";
    let mut run = tokio::spawn(async move {
        run_chat
            .run_in_session(
                "Create the fixture file and finish.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(run_id),
                |_| {},
                |_| {},
            )
            .await
    });
    let processes = match fixture.try_started().await {
        Ok(processes) => processes,
        Err(error) => {
            // Diagnose the first failure without leaving a detached chat task.
            let record = RunStore::new(&db).get(run_id).unwrap();
            let actions = RunControlStore::new(&db).list_actions(run_id).unwrap();
            let events: Vec<_> = chat
                .run_events(run_id, None)
                .unwrap()
                .into_iter()
                .filter(|event| event.event_type == "tool_started")
                .collect();
            fixture.release();
            let cancel = chat.request_cancel(run_id);
            let result = tokio::time::timeout(Duration::from_secs(4), &mut run).await;
            if result.is_err() {
                run.abort();
                let _ = run.await;
            }
            let settled_events: Vec<_> = chat
                .run_events(run_id, None)
                .unwrap()
                .into_iter()
                .filter(|event| {
                    matches!(
                        event.event_type.as_str(),
                        "tool_completed" | "verification" | "run_terminal"
                    )
                })
                .collect();
            panic!("{error}; post_edit={post_edit} cancel={cancel:?} parent_exits={parent_exits} record={record:?} actions={actions:?} started_events={events:?} settled_events={settled_events:?} run_result={result:?}");
        }
    };
    if parent_exits {
        fixture.return_parent(&processes).await;
    }
    assert!(RunStore::new(&db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .terminal_kind
        .is_none());
    let before_actions = RunControlStore::new(&db).list_actions(run_id).unwrap();
    let bash = before_actions
        .iter()
        .find(|a| a.tool_name == "bash")
        .unwrap();
    assert_eq!(bash.state, RunActionState::Running);
    assert!(fixture.root.join(path).is_file());

    let stopped = if cancel {
        assert!(chat.request_cancel(run_id).unwrap().accepted);
        tokio::time::timeout(Duration::from_secs(2), &mut run)
            .await
            .ok()
    } else {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!run.is_finished());
        None
    };
    let stopped_before_release = stopped.is_some();
    let processes_exited_before_release = processes.exited();
    let terminal_before_release = RunStore::new(&db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .terminal_kind;
    // Release/join first, including on a failed desired invariant.
    fixture.release();
    let result = match stopped {
        Some(result) => result,
        None => tokio::time::timeout(Duration::from_secs(4), run)
            .await
            .expect("released finite ChatService must settle"),
    }
    .expect("ChatService task must not panic");
    processes.settle().await;
    assert!(result.is_ok(), "{result:?}");
    let expected = if cancel { "cancelled" } else { "succeeded" };
    let terminal = RunStore::new(&db).get(run_id).unwrap().unwrap();
    assert_eq!(terminal.terminal_kind.as_deref(), Some(expected));
    let events = chat.run_events(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    let actions = RunControlStore::new(&db).list_actions(run_id).unwrap();
    assert!(actions.iter().all(|a| a.state.is_terminal()), "{actions:?}");
    let bash = actions.iter().find(|a| a.tool_name == "bash").unwrap();
    assert_eq!(
        bash.state,
        if cancel {
            RunActionState::Cancelled
        } else {
            RunActionState::Completed
        }
    );
    assert_eq!(
        actions.iter().filter(|a| a.tool_name == "bash").count(),
        if post_edit && !cancel { 2 } else { 1 },
        "cancellation must not start another automatic build",
    );
    fixture.assert_compiler_effects(cancel);
    eprintln!("verification-lifecycle post_edit={post_edit} cancel={cancel} parent_exits={parent_exits} stopped_before_release={stopped_before_release} processes_exited_before_release={processes_exited_before_release} terminal_before_release={terminal_before_release:?} wrote={} actions={}", fixture.wrote(), actions.len());
    if cancel {
        assert!(
            stopped_before_release,
            "cancel waited for the project script"
        );
        assert!(
            processes_exited_before_release,
            "run returned before descendant exit"
        );
        assert_eq!(terminal_before_release.as_deref(), Some(expected));
        assert!(
            !fixture.wrote(),
            "build descendant wrote after cancellation"
        );
    } else {
        assert!(!processes_exited_before_release);
        assert!(terminal_before_release.is_none());
        assert!(
            fixture.wrote(),
            "positive control must execute real build script"
        );
    }
}

#[tokio::test]
async fn completion_build_normal_and_active_cancellation() {
    observe(false, false, false).await;
    observe(false, true, false).await;
}

#[tokio::test]
async fn post_edit_build_normal_and_active_cancellation() {
    observe(true, false, false).await;
    observe(true, true, false).await;
}

#[tokio::test]
async fn build_parent_exit_does_not_detach_worker_from_either_automatic_path() {
    for post_edit in [false, true] {
        observe(post_edit, false, true).await;
        observe(post_edit, true, true).await;
    }
}
