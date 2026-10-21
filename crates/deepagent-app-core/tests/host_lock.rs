//! Host election across two real processes, plus concurrent writers on one SQLite
//! file. These are the risks the "run the TUI alongside the desktop shell"
//! decision turns on, so they are exercised with separate OS processes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_app_core::host_lock::{self, HostRole};
use deepagent_app_core::AppService;
use deepagent_core::clock::SystemClock;
use deepagent_core::SessionMode;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{NewRunAction, RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::{SubagentRunRecord, SubagentRunStore};
use deepagent_persistence::Database;
use deepagent_session::Session;

const MODE: &str = "DEEPAGENT_HOST_LOCK_TEST";
const ROOT: &str = "DEEPAGENT_HOST_LOCK_TEST_ROOT";
const RUN_ID: &str = "DEEPAGENT_HOST_LOCK_TEST_RUN";
const TEST_NAME: &str = "second_host_becomes_standby_while_another_process_owns_the_data_root";
const CHILD_SESSIONS: usize = 40;

fn describe(role: &HostRole) -> String {
    match role {
        HostRole::Primary => "primary".to_string(),
        HostRole::Standby { holder } => format!("standby:{holder}"),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_millis() as i64
}

/// Children report through a file: a captured stdout gets interleaved with the
/// harness's own half-written lines, which makes parsing unreliable.
fn report_path(root: &Path, mode: &str) -> PathBuf {
    root.join(format!("child-report-{mode}.txt"))
}

fn report(mode: &str, root: &Path, payload: &str) {
    let path = report_path(root, mode);
    std::fs::write(&path, payload)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn start_child(mode: &str, root: &Path, run_id: &str) -> Child {
    let _ = std::fs::remove_file(report_path(root, mode));
    Command::new(std::env::current_exe().expect("test binary"))
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--test-threads=1")
        .env(MODE, mode)
        .env(ROOT, root)
        .env(RUN_ID, run_id)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child test process")
}

fn finish_child(child: &mut Child, mode: &str, root: &Path) -> String {
    let status = child.wait().expect("wait for child");
    assert!(status.success(), "child exited with {status:?}");
    let path = report_path(root, mode);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .trim()
        .to_string()
}

fn write_sessions(database: &Database, workspace: &Path, count: usize) -> usize {
    for index in 0..count {
        Session::create_in_project(
            database,
            &SystemClock,
            None,
            SessionMode::Normal,
            Some(
                &workspace
                    .join(format!("task-{index}"))
                    .display()
                    .to_string(),
            ),
        )
        .unwrap_or_else(|error| panic!("create session {index}: {error}"));
    }
    count
}

/// The owner string this process writes when it holds the root.
fn expected_holder() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "deepagent".to_string());
    format!("{exe}#{}", std::process::id())
}

fn create_running_run(database: &Database, workspace: &Path, run_id: &str) {
    let session = Session::create_in_project(
        database,
        &SystemClock,
        None,
        SessionMode::Normal,
        Some(&workspace.display().to_string()),
    )
    .expect("create session for the live run");
    let store = RunStore::new(database);
    let now = now_millis();
    store
        .create(run_id, &session.id().to_string(), None, now)
        .expect("create run");
    store
        .transition(run_id, "running", now)
        .expect("mark run running");
}

fn run_state(database: &Database, run_id: &str) -> String {
    RunStore::new(database)
        .get(run_id)
        .expect("read run")
        .map(|record| record.state)
        .unwrap_or_else(|| "missing".to_string())
}

/// Audit probe: run explicitly; the invariant currently fails in production.
/// The existing subprocess fixture opens the file database and performs the
/// normal standby recovery path while this process still owns the OS lock.
#[test]
#[ignore = "audit: opening a standby database mutates the primary's live control records"]
fn audit_standby_open_preserves_live_actions_and_subagents() {
    let directory = tempfile::tempdir().expect("isolated audit data root");
    let root = directory.path();
    assert_eq!(host_lock::claim(root).unwrap(), HostRole::Primary);
    let database = Database::open(root.join("deepagent.db")).unwrap();
    let run_id = "audit-live-parent";
    let call_id = "audit-live-tool";
    let child_id = "audit-live-child";
    create_running_run(&database, root, run_id);
    let now = now_millis();
    let controls = RunControlStore::new(&database);
    controls
        .create_action(&NewRunAction {
            run_id,
            turn_id: "audit-turn",
            call_id,
            sequence: 0,
            tool_name: "read_file",
            arguments_hash: "audit-fixture-no-execution",
            risk: "low",
            parent_action_id: None,
            now,
        })
        .unwrap();
    for state in [RunActionState::Prepared, RunActionState::Running] {
        controls
            .transition_action(run_id, call_id, state, now, None, None)
            .unwrap();
    }
    let children = SubagentRunStore::new(&database);
    children
        .create(&SubagentRunRecord {
            id: child_id.into(),
            parent_run_id: run_id.into(),
            origin_parent_run_id: run_id.into(),
            state: "running".into(),
            agent_type: "explore".into(),
            transcript_path: None,
            worktree_path: None,
            summary: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
            resume_count: 0,
        })
        .unwrap();

    let recovery = finish_child(&mut start_child("recover", root, run_id), "recover", root);
    assert_eq!(
        recovery,
        format!("standby:{} recovered:0 running", expected_holder()),
        "control: real standby election and run-level recovery protection work"
    );
    let action = controls.get_action(run_id, call_id).unwrap().unwrap();
    let child = children.get(child_id).unwrap().unwrap();
    // Also test the real owner's later success persistence, rather than only
    // observing labels. No tool, child agent, or external API is executed.
    let action_completion = controls.transition_action(
        run_id,
        call_id,
        RunActionState::Completed,
        now_millis(),
        None,
        None,
    );
    let child_completion = children.finish(child_id, "succeeded", Some("done"), now_millis());
    eprintln!(
        "run={} action={:?} reason={:?} child={} action_completion={:?} child_completion={:?}",
        run_state(&database, run_id),
        action.state,
        action.blocked_reason,
        child.state,
        action_completion,
        child_completion
    );
    host_lock::release(root);
    assert!(
        action.state == RunActionState::Running
            && child.state == "running"
            && child.finished_at.is_none()
            && action_completion.is_ok()
            && child_completion.is_ok(),
        "standby open must preserve live owner records and allow their normal completion"
    );
}

#[test]
fn second_host_becomes_standby_while_another_process_owns_the_data_root() {
    let mode = std::env::var(MODE).ok().unwrap_or_default();
    let root = match std::env::var(ROOT) {
        Ok(target) => PathBuf::from(target),
        Err(_) => std::env::temp_dir().join(format!("deepagent-host-lock-{}", std::process::id())),
    };
    let run_id = std::env::var(RUN_ID).unwrap_or_else(|_| "live-run".to_string());

    if mode == "claim" {
        let role = host_lock::claim(&root).expect("child claim");
        report("claim", &root, &describe(&role));
        return;
    }
    if mode == "recover" {
        // A standby host must leave the primary host's live run alone.
        let role = host_lock::claim(&root).expect("child claim");
        let database = Database::open(root.join("deepagent.db")).expect("child open database");
        let service = AppService::from_shared(Arc::new(database));
        let recovered = service
            .recover_unfinished_runs()
            .expect("standby recovery is a no-op, not an error");
        let state = {
            let db = service.database();
            run_state(db, &run_id)
        };
        report(
            "recover",
            &root,
            &format!("{} recovered:{} {state}", describe(&role), recovered.len()),
        );
        return;
    }
    if mode == "writers" {
        let role = host_lock::claim(&root).expect("child claim");
        let database = Database::open(root.join("deepagent.db")).expect("child open database");
        let written = write_sessions(&database, &root, CHILD_SESSIONS);
        report(
            "writers",
            &root,
            &format!("wrote:{written} {}", describe(&role)),
        );
        return;
    }

    std::fs::create_dir_all(&root).expect("create data root");

    // A lone process owns its data root, and a second process sees the holder.
    let role = host_lock::claim(&root).expect("parent claim");
    assert_eq!(role, HostRole::Primary, "the first process must be primary");
    let claimed = finish_child(&mut start_child("claim", &root, "unused"), "claim", &root);
    assert_eq!(
        claimed,
        format!("standby:{}", expected_holder()),
        "a second process must be standby and must name the live holder"
    );

    // The primary has a run in flight, exactly what a desktop task looks like.
    let database = Arc::new(Database::open(root.join("deepagent.db")).expect("open db"));
    create_running_run(&database, &root, &run_id);
    assert_eq!(run_state(&database, &run_id), "running");

    // A standby host reports standby, recovers nothing, and leaves the run running.
    let recovery = finish_child(
        &mut start_child("recover", &root, &run_id),
        "recover",
        &root,
    );
    assert_eq!(
        recovery,
        format!("standby:{} recovered:0 running", expected_holder()),
        "a standby host must not close out another host's live run"
    );

    // The primary itself still performs startup recovery.
    let service = AppService::from_shared(Arc::clone(&database));
    assert!(
        service.host_role().is_primary(),
        "the process holding the root must be primary"
    );
    let recovered = service.recover_unfinished_runs().expect("primary recovery");
    assert!(
        !recovered.is_empty(),
        "the primary host must still close out unfinished runs"
    );
    assert_ne!(
        run_state(&database, &run_id),
        "running",
        "primary recovery must finalize the run"
    );

    // Releasing hands primary to the next claimant.
    host_lock::release(&root);
    assert_eq!(
        finish_child(&mut start_child("claim", &root, "unused"), "claim", &root),
        "primary",
        "releasing the lock must hand primary to the next process"
    );

    // Two hosts writing the same database at the same time (WAL + busy_timeout):
    // the parent takes the root back, so the writer child is a standby host that
    // still reads and writes normally.
    assert!(
        host_lock::claim(&root).expect("reclaim").is_primary(),
        "parent must regain primary after the child process exits"
    );
    let mut writer = start_child("writers", &root, "unused");
    let parent_written = write_sessions(&database, &root, CHILD_SESSIONS);
    let writers = finish_child(&mut writer, "writers", &root);
    assert_eq!(
        writers,
        format!("wrote:{CHILD_SESSIONS} standby:{}", expected_holder()),
        "a standby host must still write normally"
    );
    assert_eq!(parent_written, CHILD_SESSIONS);
    let sessions = EventStore::new(&database)
        .list_sessions()
        .expect("list sessions");
    assert_eq!(
        sessions.len(),
        CHILD_SESSIONS * 2 + 1,
        "both hosts' sessions must be durable without SQLITE_BUSY"
    );

    host_lock::release(&root);
    let _ = std::fs::remove_dir_all(&root);
}
