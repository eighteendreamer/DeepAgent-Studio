//! H-32: real file databases and production admission/recovery gates. All
//! records are synthetic; no model, tool, network or user data is executed.
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deepagent_app_core::{host_lock::HostRole, AppService, RunCoordinator};
use deepagent_core::{clock::SystemClock, id::SessionId};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{
    ApprovalState, NewExecutionLease, NewRunAction, NewRunApproval, RunActionState, RunControlStore,
};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::{SubagentRunRecord, SubagentRunStore};
use deepagent_persistence::Database;
use deepagent_session::Session;

fn session(db: &Database) -> SessionId {
    Session::create(db, &SystemClock, Some("H-32 fixture"))
        .unwrap()
        .id()
}

fn controls(db: &Database, run: &str) {
    let store = RunControlStore::new(db);
    for (index, call) in ["tool", "approval"].into_iter().enumerate() {
        store
            .create_action(&NewRunAction {
                run_id: run,
                turn_id: "turn",
                call_id: call,
                sequence: index as u64,
                tool_name: "read_file",
                arguments_hash: "fixture-no-execution",
                risk: "low",
                parent_action_id: None,
                now: 1,
            })
            .unwrap();
        store
            .transition_action(run, call, RunActionState::Prepared, 2, None, None)
            .unwrap();
    }
    store
        .transition_action(run, "tool", RunActionState::Running, 3, None, None)
        .unwrap();
    store
        .request_approval(&NewRunApproval {
            approval_id: &format!("{run}-approval"),
            run_id: run,
            call_id: "approval",
            scope: "once",
            risk: "low",
            reason: None,
            policy_snapshot: None,
            expires_at: Some(10),
            now: 3,
        })
        .unwrap();
    SubagentRunStore::new(db)
        .create(&SubagentRunRecord {
            id: format!("{run}-child"),
            parent_run_id: run.into(),
            origin_parent_run_id: run.into(),
            state: "running".into(),
            agent_type: "explore".into(),
            transcript_path: None,
            worktree_path: None,
            summary: None,
            created_at: 3,
            updated_at: 3,
            finished_at: None,
            resume_count: 0,
        })
        .unwrap();
}

fn assert_controls(db: &Database, run: &str, recovered: bool) {
    let store = RunControlStore::new(db);
    assert_eq!(
        store.get_action(run, "tool").unwrap().unwrap().state,
        if recovered {
            RunActionState::Failed
        } else {
            RunActionState::Running
        }
    );
    assert_eq!(
        store.list_approvals(run).unwrap()[0].state,
        if recovered {
            ApprovalState::Expired
        } else {
            ApprovalState::Pending
        }
    );
    let child = SubagentRunStore::new(db)
        .get(&format!("{run}-child"))
        .unwrap()
        .unwrap();
    assert_eq!(child.state, if recovered { "cancelled" } else { "running" });
    assert_eq!(child.finished_at.is_some(), recovered);
}

#[test]
fn repeated_primary_open_and_shared_recovery_preserve_local_admission() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deepagent.db");
    let app = AppService::open(&path).unwrap();
    let db = app.shared_database();
    let id = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    coordinator.admit("live", &id.to_string()).unwrap();
    controls(&db, "live");
    let stale_session = session(&db);
    RunStore::new(&db)
        .create("stale", &stale_session.to_string(), None, 1)
        .unwrap();
    controls(&db, "stale");
    let reopened = AppService::open(&path).unwrap();
    assert_eq!(reopened.host_role(), &HostRole::Primary);
    let recovered = reopened.recover_unfinished_runs().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].run_id, "stale");
    assert_controls(&db, "live", false);
    assert_controls(&db, "stale", true);
    assert!(AppService::from_shared(db.clone())
        .recover_unfinished_runs()
        .unwrap()
        .is_empty());
    assert_controls(&db, "live", false);
    // Normal owner completion still persists, not just the labels.
    RunControlStore::new(&db)
        .transition_action("live", "tool", RunActionState::Completed, 20, None, None)
        .unwrap();
    SubagentRunStore::new(&db)
        .finish("live-child", "succeeded", Some("done"), 20)
        .unwrap();
    RunStore::new(&db)
        .finish("live", "succeeded", None, 20)
        .unwrap();
    drop(coordinator);
    assert!(reopened.recover_unfinished_runs().unwrap().is_empty());
    assert_eq!(
        RunStore::new(&db)
            .get("live")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
}

#[test]
fn recovery_scope_fences_admission_history_and_second_recovery_until_drop() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let id = session(&db);
    RunStore::new(&db)
        .create("old", &id.to_string(), None, 1)
        .unwrap();
    let events = EventStore::new(&db);
    let scope = events.try_begin_recovery(id).unwrap().unwrap();
    let coordinator = RunCoordinator::new(db.clone());
    assert!(coordinator
        .admit("new", &id.to_string())
        .unwrap_err()
        .to_string()
        .contains("dispatch.lock"));
    assert!(RunStore::new(&db).get("new").unwrap().is_none());
    assert!(events.begin_control(id).is_err());
    assert!(events.try_begin_recovery(id).unwrap().is_none());
    // Discovery may predate normal completion: the scoped query must be fresh.
    assert_eq!(
        RunStore::new(&db).recovery_sessions(50).unwrap(),
        [id.to_string()]
    );
    RunStore::new(&db)
        .finish("old", "succeeded", None, 20)
        .unwrap();
    assert!(scope.unfinished_runs().unwrap().is_empty());
    drop(scope);
    let history = events.begin_control(id).unwrap();
    assert!(events.try_begin_recovery(id).unwrap().is_none());
    drop(history);
    coordinator.admit("new", &id.to_string()).unwrap();
    assert!(events.try_begin_recovery(id).unwrap().is_none());
    drop(coordinator);
    assert!(events.try_begin_recovery(id).unwrap().is_some());
}

#[test]
fn terminal_parent_controls_recover_by_current_not_origin_parent() {
    let directory = tempfile::tempdir().unwrap();
    let app = AppService::open(directory.path().join("deepagent.db")).unwrap();
    let db = app.shared_database();
    let old_session = session(&db);
    RunStore::new(&db)
        .create("old", &old_session.to_string(), None, 1)
        .unwrap();
    controls(&db, "old");
    SubagentRunStore::new(&db)
        .finish("old-child", "succeeded", None, 4)
        .unwrap();
    RunStore::new(&db)
        .finish("old", "succeeded", None, 5)
        .unwrap();
    let current_session = session(&db);
    let coordinator = RunCoordinator::new(db.clone());
    coordinator
        .admit("current", &current_session.to_string())
        .unwrap();
    // The storage API allows changing parents; recovery must key off current
    // ownership even for a malformed cross-session lineage, never the origin.
    SubagentRunStore::new(&db)
        .resume("old-child", "current", 6)
        .unwrap();
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_eq!(
        RunControlStore::new(&db)
            .get_action("old", "tool")
            .unwrap()
            .unwrap()
            .state,
        RunActionState::Failed
    );
    assert_eq!(
        SubagentRunStore::new(&db)
            .get("old-child")
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    drop(coordinator);
    assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
    assert_eq!(
        SubagentRunStore::new(&db)
            .get("old-child")
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    // Parent already terminal, only the orphan child remains a candidate.
    SubagentRunStore::new(&db)
        .resume("old-child", "old", 7)
        .unwrap();
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_eq!(
        SubagentRunStore::new(&db)
            .get("old-child")
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    let failures = RunStore::new(&db)
        .events_after("old", None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "action.failed")
        .count();
    assert_eq!(failures, 1);
}

#[test]
fn recovery_failure_releases_gates_retries_without_replaying_and_preserves_valid_leases() {
    let directory = tempfile::tempdir().unwrap();
    let app = AppService::open(directory.path().join("deepagent.db")).unwrap();
    let db = app.shared_database();
    let id = session(&db);
    RunStore::new(&db)
        .create("stale", &id.to_string(), None, 1)
        .unwrap();
    controls(&db, "stale");
    let control = RunControlStore::new(&db);
    for (lease, resource, expires_at) in
        [("expired", "old-pty", 1), ("valid", "live-pty", i64::MAX)]
    {
        control
            .acquire_lease(&NewExecutionLease {
                lease_id: lease,
                resource_kind: "terminal_session",
                resource_id: resource,
                owner: "worker",
                fencing_token_hash: "synthetic",
                expires_at,
                now: 0,
            })
            .unwrap();
    }
    db.with_conn(|connection| {
        connection
            .execute_batch(
                "CREATE TRIGGER fail_recovery BEFORE INSERT ON run_events
            WHEN NEW.event_type='action.failed'
            BEGIN SELECT RAISE(ABORT, 'H-32 recovery fault'); END;",
            )
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    assert!(app
        .recover_unfinished_runs()
        .unwrap_err()
        .to_string()
        .contains("H-32 recovery fault"));
    assert_controls(&db, "stale", false);
    // Lock scope unwinds on an actual SQLite error, not only a happy return.
    drop(EventStore::new(&db).acquire_dispatch(id).unwrap());
    db.with_conn(|connection| {
        connection
            .execute_batch(
                "DROP TRIGGER fail_recovery;
                CREATE TRIGGER fail_child_recovery BEFORE UPDATE ON subagent_runs
                WHEN NEW.state='cancelled'
                BEGIN SELECT RAISE(ABORT, 'H-32 child recovery fault'); END;",
            )
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    // The action transaction committed but child recovery failed. A retry
    // must preserve that commit and not emit a second action.failed event.
    assert!(app
        .recover_unfinished_runs()
        .unwrap_err()
        .to_string()
        .contains("H-32 child recovery fault"));
    assert_eq!(
        control.get_action("stale", "tool").unwrap().unwrap().state,
        RunActionState::Failed
    );
    assert_eq!(
        SubagentRunStore::new(&db)
            .get("stale-child")
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    assert!(RunStore::new(&db)
        .get("stale")
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    drop(EventStore::new(&db).acquire_dispatch(id).unwrap());
    db.with_conn(|connection| {
        connection
            .execute_batch("DROP TRIGGER fail_child_recovery")
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
    assert_controls(&db, "stale", true);
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert!(control
        .active_lease("terminal_session", "old-pty", 50)
        .unwrap()
        .is_none());
    assert!(control
        .active_lease("terminal_session", "live-pty", 50)
        .unwrap()
        .is_some());
    assert_eq!(
        RunStore::new(&db)
            .events_after("stale", None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "action.failed")
            .count(),
        1
    );
}

#[test]
fn real_lock_open_error_is_not_treated_as_a_busy_or_abandoned_session() {
    let directory = tempfile::tempdir().unwrap();
    let app = AppService::open(directory.path().join("deepagent.db")).unwrap();
    let db = app.shared_database();
    let id = session(&db);
    RunStore::new(&db)
        .create("old", &id.to_string(), None, 1)
        .unwrap();
    controls(&db, "old");
    let lock = db
        .data_root()
        .join("files/locks/sessions")
        .join(format!("{id}.dispatch.lock"));
    std::fs::create_dir(&lock).unwrap();
    assert!(app.recover_unfinished_runs().is_err());
    assert_controls(&db, "old", false);
    assert!(RunStore::new(&db)
        .get("old")
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    std::fs::remove_dir(&lock).unwrap();
    assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
    assert_controls(&db, "old", true);
}

#[test]
fn concurrent_admission_and_recovery_have_exactly_one_owner() {
    use std::sync::{mpsc, Barrier};
    let db = Arc::new(Database::open_in_memory().unwrap());
    for index in 0..24 {
        let id = session(&db);
        let start = Arc::new(Barrier::new(3));
        let (release_recovery, wait_recovery) = mpsc::channel();
        let (release_admission, wait_admission) = mpsc::channel();
        let (send, receive) = mpsc::channel();
        std::thread::scope(|threads| {
            let db_recovery = db.clone();
            let recovery_start = start.clone();
            let recovery_send = send.clone();
            threads.spawn(move || {
                recovery_start.wait();
                let scope = EventStore::new(&db_recovery)
                    .try_begin_recovery(id)
                    .unwrap();
                recovery_send.send(scope.is_some()).unwrap();
                let _ = wait_recovery.recv_timeout(Duration::from_secs(10));
                drop(scope);
            });
            let db_admission = db.clone();
            let admission_start = start.clone();
            threads.spawn(move || {
                let coordinator = RunCoordinator::new(db_admission);
                admission_start.wait();
                let admitted = coordinator
                    .admit(&format!("race-{index}"), &id.to_string())
                    .is_ok();
                send.send(admitted).unwrap();
                let _ = wait_admission.recv_timeout(Duration::from_secs(10));
                drop(coordinator);
            });
            start.wait();
            let first = receive.recv_timeout(Duration::from_secs(10));
            let second = receive.recv_timeout(Duration::from_secs(10));
            let _ = release_recovery.send(());
            let _ = release_admission.send(());
            assert_ne!(
                first.unwrap(),
                second.unwrap(),
                "exactly one held owner in race {index}"
            );
        });
        assert!(EventStore::new(&db)
            .try_begin_recovery(id)
            .unwrap()
            .is_some());
    }
}

struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_ready(child: &mut Child, marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fixture exited before ready"
        );
        assert!(Instant::now() < deadline, "fixture readiness timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn primary_preserves_live_standby_then_recovers_after_real_process_kill() {
    const CHILD_ROOT: &str = "DEEPAGENT_H32_RECOVERY_CHILD";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = Path::new(&root);
        let app = AppService::open(root.join("deepagent.db")).unwrap();
        assert!(matches!(app.host_role(), HostRole::Standby { .. }));
        let db = app.shared_database();
        let id = session(&db);
        let coordinator = RunCoordinator::new(db.clone());
        coordinator.admit("standby-live", &id.to_string()).unwrap();
        controls(&db, "standby-live");
        std::fs::write(root.join("ready"), id.to_string()).unwrap();
        // Killed by the parent; keep production admission alive until exit.
        loop {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deepagent.db");
    let app = AppService::open(&path).unwrap();
    assert_eq!(app.host_role(), &HostRole::Primary);
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "primary_preserves_live_standby_then_recovers_after_real_process_kill",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(&mut child.0, &directory.path().join("ready"));
    let reopened = AppService::open(&path).unwrap();
    assert!(reopened.recover_unfinished_runs().unwrap().is_empty());
    assert_controls(app.database(), "standby-live", false);
    assert!(RunStore::new(app.database())
        .get("standby-live")
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let recovered = reopened.recover_unfinished_runs().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].run_id, "standby-live");
    assert_controls(app.database(), "standby-live", true);
    assert!(reopened.recover_unfinished_runs().unwrap().is_empty());
}
