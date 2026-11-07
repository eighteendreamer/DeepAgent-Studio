//! H229: real host processes, only synthetic TempDir state, no model request.
use deepagent_core::{clock::Timestamp, event::EventPayload, id::SessionId, message::Message};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::{event_store::EventStore, Database};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn h95_real_cli_startup_keeps_each_runs_reason_and_reopen_is_idempotent() {
    use deepagent_core::{clock::SystemClock, task::TaskState};
    use deepagent_session::Session;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    let path = root.path().join("data/state.sqlite");
    let id = {
        let db = Database::open(&path).unwrap();
        let mut session = Session::create(&db, &SystemClock, None).unwrap();
        let id = session.id();
        for (run, tool, now) in [("older-tool", true, 200), ("later-prompt", false, 100)] {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(run),
                })
                .unwrap();
            let task = session.create_task(run).unwrap();
            session.transition_task(task, TaskState::Running).unwrap();
            RunStore::new(&db)
                .create(run, &id.to_string(), Some(&task.to_string()), now)
                .unwrap();
            RunStore::new(&db)
                .transition(run, "running_turn", now + 1)
                .unwrap();
            if tool {
                session
                    .append(EventPayload::ToolCallRequested {
                        call: deepagent_core::message::ToolCall {
                            id: "synthetic-call".into(),
                            name: "read_file".into(),
                            arguments: json!({}),
                        },
                    })
                    .unwrap();
            }
        }
        id
    };
    let mut previous = None;
    for _ in 0..2 {
        let mut command = Command::new(env!("CARGO_BIN_EXE_deepagent"));
        isolated(&mut command, root.path());
        // EOF still traverses the real startup recovery boundary, but never
        // admits a turn, invokes a provider, or executes the synthetic tool.
        let mut child = ChildGuard(
            command
                .args(["server", "--transport", "stdio"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let end = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "CLI startup failed: {status}");
                break;
            }
            assert!(std::time::Instant::now() < end, "CLI startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        let db = Database::open(&path).unwrap();
        let history = EventStore::new(&db).load_session(id).unwrap();
        let mut ledgers = Vec::new();
        for (run, reason) in [
            ("older-tool", "interrupted_turn"),
            ("later-prompt", "interrupted_prompt"),
        ] {
            let store = RunStore::new(&db);
            let record = store.get(run).unwrap().unwrap();
            assert_eq!(record.terminal_kind.as_deref(), Some("failed"));
            let events = store.events_after(run, None).unwrap();
            let markers: Vec<_> = events
                .iter()
                .filter(|e| e.event_type == "run_recovered_after_startup")
                .collect();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].data["interruption"], reason, "{run}");
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.event_type == "run_terminal")
                    .count(),
                1
            );
            assert_eq!(
                history
                    .iter()
                    .filter(|e| matches!(&e.payload,
                        EventPayload::RunTerminal { run_id: Some(found), .. } if found == run
                    ))
                    .count(),
                1
            );
            ledgers.push(events);
        }
        let snapshot = serde_json::to_value((ledgers, history)).unwrap();
        if let Some(previous) = &previous {
            assert_eq!(&snapshot, previous, "cold restart rewrote durable recovery");
        }
        previous = Some(snapshot);
    }
}

fn isolated(command: &mut Command, root: &Path) {
    let environment: Vec<_> = ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (key, value)))
        .collect();
    command.env_clear().envs(environment).current_dir(root);
    for (key, value) in [
        ("HOME", root.join("home")),
        ("USERPROFILE", root.join("home")),
        ("APPDATA", root.join("home/roaming")),
        ("LOCALAPPDATA", root.join("home/local")),
        ("TEMP", root.join("temp")),
        ("TMP", root.join("temp")),
        ("DEEPAGENT_DATA_ROOT", root.join("data")),
    ] {
        std::fs::create_dir_all(&value).unwrap();
        command.env(key, value);
    }
    command.env("DEEPAGENT_DB_PATH", root.join("data/state.sqlite"));
}

fn fixture(root: &Path) -> (SessionId, SessionId, std::path::PathBuf) {
    std::fs::create_dir_all(root.join("data")).unwrap();
    let db = Database::open(root.join("data/state.sqlite")).unwrap();
    let store = EventStore::new(&db);
    let ids: Vec<_> = ["project-a", "project-b"]
        .into_iter()
        .map(|name| {
            let project = root.join(name);
            std::fs::create_dir(&project).unwrap();
            let id = SessionId::new();
            store
                .create_session_full(
                    id,
                    Some("H229 synthetic host history"),
                    Default::default(),
                    Some(project.to_str().unwrap()),
                    Timestamp::from_millis(100),
                )
                .unwrap();
            store
                .append(
                    id,
                    EventPayload::MessageAppended {
                        message: Message::user("repairneedle synthetic decision"),
                    },
                    Timestamp::from_millis(101),
                )
                .unwrap();
            id
        })
        .collect();
    let bad_path = std::fs::read_dir(db.session_transcript_dir(ids[1]))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".v1.jsonl.zst"))
        .unwrap();
    (ids[0], ids[1], bad_path)
}

#[test]
fn h229_real_cli_starts_lists_and_reads_good_history_when_another_file_is_invalid() {
    for tail in [&[][..], &[0x28, 0xb5, 0x2f, 0xfd][..], &[0xff][..]] {
        let root = tempfile::tempdir().unwrap();
        let (good, bad, bad_path) = fixture(root.path());
        let original_run_events = if tail == [0xff] {
            let db = Database::open(root.path().join("data/state.sqlite")).unwrap();
            RunStore::new(&db)
                .create("h229-unreadable-run", &bad.to_string(), None, 102)
                .unwrap();
            Some(
                serde_json::to_value(
                    RunStore::new(&db)
                        .events_after("h229-unreadable-run", None)
                        .unwrap(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let original = std::fs::read(&bad_path).unwrap();
        let mut damaged = original.clone();
        damaged.extend_from_slice(tail);
        std::fs::write(&bad_path, &damaged).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_deepagent"));
        isolated(&mut command, root.path());
        let mut child = ChildGuard(
            command
                .args(["server", "--transport", "stdio"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut input = child.0.stdin.take().unwrap();
        let output = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut request = |id, method, params| {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
            )
            .unwrap();
            input.flush().unwrap();
            loop {
                let line = rx
                    .recv_timeout(Duration::from_secs(20))
                    .expect("host RPC timed out");
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value["id"] == id {
                    assert!(value.get("error").is_none(), "host RPC failed: {value}");
                    break value;
                }
            }
        };
        request(
            1,
            "initialize",
            json!({"clientName":"H229", "clientVersion":"1", "protocolVersion":1}),
        );
        let list = request(2, "thread/list", json!({}));
        assert_eq!(list["result"]["threads"].as_array().unwrap().len(), 2);
        let read = request(3, "thread/read", json!({"threadId":good.to_string()}));
        assert!(read["result"].to_string().contains("repairneedle"));
        assert!(!read["result"].to_string().contains(&bad.to_string()));
        drop(input);
        let end = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < end,
                "host did not exit after EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        reader.join().unwrap();
        assert_eq!(
            std::fs::read(&bad_path).unwrap(),
            if tail == [0xff] { damaged } else { original }
        );
        let reopened = Database::open(root.path().join("data/state.sqlite")).unwrap();
        if tail == [0xff] {
            let run = RunStore::new(&reopened)
                .get("h229-unreadable-run")
                .unwrap()
                .unwrap();
            assert!(run.finished_at.is_none() && run.terminal_kind.is_none());
            assert_eq!(
                serde_json::to_value(
                    RunStore::new(&reopened)
                        .events_after(&run.id, None)
                        .unwrap()
                )
                .unwrap(),
                original_run_events.unwrap()
            );
        }
        assert_eq!(
            EventStore::new(&reopened).load_session(good).unwrap().len(),
            1
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires a locally built h229_startup_acceptance Wry example; no keychain or full desktop setup"]
fn h229_real_wry_setup_uses_the_production_database_boundary_without_a_credentials_store() {
    let exe = std::env::var_os("DEEPAGENT_H229_WRY_EXE").expect("build the Wry example first");
    let root = tempfile::tempdir().unwrap();
    let (good, bad, bad_path) = fixture(root.path());
    let original_run_events = {
        let db = Database::open(root.path().join("data/state.sqlite")).unwrap();
        RunStore::new(&db)
            .create("h229-unreadable-run", &bad.to_string(), None, 102)
            .unwrap();
        serde_json::to_value(
            RunStore::new(&db)
                .events_after("h229-unreadable-run", None)
                .unwrap(),
        )
        .unwrap()
    };
    let mut damaged = std::fs::read(&bad_path).unwrap();
    damaged.push(0xff);
    std::fs::write(&bad_path, &damaged).unwrap();
    let mut command = Command::new(exe);
    isolated(&mut command, root.path());
    let mut child = ChildGuard(
        command
            .args([
                root.path().join("data/state.sqlite").to_str().unwrap(),
                &good.to_string(),
                &bad.to_string(),
                root.path().join("project-a").to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let end = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "isolated Wry host failed with {status}");
            break;
        }
        assert!(std::time::Instant::now() < end, "Wry host timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&bad_path).unwrap(), damaged);
    let db = Database::open(root.path().join("data/state.sqlite")).unwrap();
    let run = RunStore::new(&db)
        .get("h229-unreadable-run")
        .unwrap()
        .unwrap();
    assert!(run.finished_at.is_none() && run.terminal_kind.is_none());
    assert_eq!(
        serde_json::to_value(RunStore::new(&db).events_after(&run.id, None).unwrap()).unwrap(),
        original_run_events
    );
}
