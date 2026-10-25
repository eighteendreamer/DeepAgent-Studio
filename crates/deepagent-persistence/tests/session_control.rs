//! Public history-control contract, including independent host-process handles.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use deepagent_core::clock::Timestamp;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;

fn seed(db: &Database) -> SessionId {
    let id = SessionId::new();
    let store = EventStore::new(db);
    store
        .create_session(id, None, Timestamp::from_millis(1))
        .unwrap();
    for _ in 0..3 {
        store
            .append(
                id,
                EventPayload::SessionEnded { reason: None },
                Timestamp::from_millis(2),
            )
            .unwrap();
    }
    id
}

fn assert_busy<T>(result: deepagent_core::error::Result<T>) {
    match result {
        Err(error) => assert!(error.to_string().contains("is busy"), "{error}"),
        Ok(_) => panic!("conflicting control access was accepted"),
    }
}

#[test]
fn dispatch_owner_refuses_competitors_without_blocking_append_or_other_sessions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("dispatch.db");
    let db = Database::open(&path).unwrap();
    let second = Database::open(&path).unwrap();
    let id = seed(&db);
    let other = seed(&db);
    let store = EventStore::new(&db);
    let second_store = EventStore::new(&second);
    let owner = store.acquire_dispatch(id).unwrap();
    assert_busy(second_store.acquire_dispatch(id));
    let other_owner = second_store.acquire_dispatch(other).unwrap();
    store
        .append(
            id,
            EventPayload::SessionEnded { reason: None },
            Timestamp::from_millis(3),
        )
        .unwrap();
    assert_eq!(store.load_session(id).unwrap().len(), 4);
    drop(owner);
    let owner = second_store.acquire_dispatch(id).unwrap();
    assert_busy(store.acquire_dispatch(id));
    drop(owner);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owner = store.acquire_dispatch(id).unwrap();
        panic!("fixture dispatch owner unwinds");
    }));
    assert!(caught.is_err());
    drop(second_store.acquire_dispatch(id).unwrap());
    drop(other_owner);
}

#[test]
fn activity_allows_append_but_refuses_control_until_every_owner_exits() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db);
    let other = seed(&db);
    let store = EventStore::new(&db);
    let first = store.acquire_activity(id).unwrap();
    let second = store.acquire_activity(id).unwrap();
    assert_busy(store.begin_control(id));
    assert_busy(store.truncate_after(id, 0));
    store
        .append(
            id,
            EventPayload::SessionEnded { reason: None },
            Timestamp::from_millis(3),
        )
        .unwrap();
    assert_eq!(store.load_session(id).unwrap().len(), 4);
    store
        .begin_control(other)
        .unwrap()
        .truncate_after(0)
        .unwrap();
    drop(first);
    assert_busy(store.begin_control(id));
    drop(second);
    assert_eq!(store.truncate_after(id, 0).unwrap(), 3);
    let activity = store.acquire_activity(id).unwrap();
    assert_eq!(store.load_session(id).unwrap().len(), 1);
    drop(activity);
}

#[test]
fn exclusive_scope_fences_admission_and_raw_writes_across_database_handles() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("control.db");
    let db = Database::open(&path).unwrap();
    let id = seed(&db);
    let second = Database::open(&path).unwrap();
    let store = EventStore::new(&second);
    let before = serde_json::to_value(store.load_session(id).unwrap()).unwrap();
    let control = EventStore::new(&db).begin_control(id).unwrap();
    assert_busy(store.acquire_activity(id));
    assert_busy(store.begin_control(id));
    assert_busy(store.truncate_after(id, 0));
    assert_busy(store.append(
        id,
        EventPayload::SessionEnded { reason: None },
        Timestamp::from_millis(4),
    ));
    assert_eq!(
        serde_json::to_value(store.load_session(id).unwrap()).unwrap(),
        before
    );
    assert_eq!(control.truncate_after(0).unwrap(), 2);
    drop(control);
    let event = store
        .append(
            id,
            EventPayload::SessionEnded { reason: None },
            Timestamp::from_millis(5),
        )
        .unwrap();
    assert_eq!(event.sequence, 1);
    drop(second);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        EventStore::new(&reopened).load_session(id).unwrap().len(),
        2
    );
    drop(EventStore::new(&reopened).begin_control(id).unwrap());
}

#[test]
fn failed_control_and_unwinding_release_the_gate() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db);
    let store = EventStore::new(&db);
    let control = store.begin_control(id).unwrap();
    // A real unpublished-generation filesystem fault must not leak ownership.
    let blocked = db
        .session_transcript_dir(id)
        .join("session.g000002.v1.jsonl.zst");
    std::fs::create_dir(&blocked).unwrap();
    assert!(control.truncate_after(0).is_err());
    drop(control);
    std::fs::remove_dir(&blocked).unwrap();
    assert_eq!(store.load_session(id).unwrap().len(), 3);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _activity = store.acquire_activity(id).unwrap();
        panic!("fixture owner unwinds");
    }));
    assert!(caught.is_err());
    assert_eq!(store.truncate_after(id, 0).unwrap(), 2);
}

struct ChildOwner(Child);

impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn control_process_child() {
    let Some(root) = std::env::var_os("DEEPAGENT_CONTROL_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let id: SessionId = std::env::var("DEEPAGENT_CONTROL_TEST_SESSION")
        .unwrap()
        .parse()
        .unwrap();
    let db = Database::open(root.join("control.db")).unwrap();
    let store = EventStore::new(&db);
    let exclusive = std::env::var_os("DEEPAGENT_CONTROL_TEST_EXCLUSIVE").is_some();
    let _activity = (!exclusive).then(|| store.acquire_activity(id).unwrap());
    let _control = exclusive.then(|| store.begin_control(id).unwrap());
    std::fs::write(root.join("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("release").exists() {
        assert!(Instant::now() < deadline, "parent did not release fixture");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn cross_process(exclusive: bool, terminate: bool) {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path().join("control.db")).unwrap();
    let id = seed(&db);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "control_process_child", "--nocapture"])
        .env("DEEPAGENT_CONTROL_TEST_ROOT", root.path())
        .env("DEEPAGENT_CONTROL_TEST_SESSION", id.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if exclusive {
        command.env("DEEPAGENT_CONTROL_TEST_EXCLUSIVE", "1");
    } else {
        command.env_remove("DEEPAGENT_CONTROL_TEST_EXCLUSIVE");
    }
    let mut child = ChildOwner(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.path().join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before lock barrier"
        );
        assert!(
            Instant::now() < deadline,
            "child did not acquire control gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let store = EventStore::new(&db);
    assert_busy(store.begin_control(id));
    if exclusive {
        assert_busy(store.acquire_activity(id));
    } else {
        drop(store.acquire_activity(id).unwrap());
    }
    if terminate {
        child.0.kill().unwrap();
        child.0.wait().unwrap();
    } else {
        std::fs::write(root.path().join("release"), b"release").unwrap();
        assert!(child.0.wait().unwrap().success());
    }
    assert_eq!(store.truncate_after(id, 0).unwrap(), 2);
}

#[test]
fn another_process_activity_blocks_rewind_then_releases() {
    cross_process(false, false);
}

#[test]
fn another_process_control_blocks_admission_and_releases_after_exit() {
    cross_process(true, false);
}

#[test]
fn killed_fixture_owner_does_not_leave_a_stale_busy_flag() {
    cross_process(false, true);
}

#[test]
fn activity_and_control_race_has_exactly_one_winner() {
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let id = seed(&db);
    for _ in 0..16 {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let runner_db = db.clone();
        let runner_barrier = barrier.clone();
        let runner = std::thread::spawn(move || {
            runner_barrier.wait();
            let activity = EventStore::new(&runner_db).acquire_activity(id);
            runner_barrier.wait();
            activity.is_ok()
        });
        barrier.wait();
        let control = EventStore::new(&db).begin_control(id);
        barrier.wait();
        let running = runner.join().unwrap();
        assert_ne!(running, control.is_ok());
        drop(control);
        drop(EventStore::new(&db).begin_control(id).unwrap());
    }
}
