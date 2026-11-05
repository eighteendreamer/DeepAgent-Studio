//! End-to-end test: spawn the real `deepagent-cli server` stdio process and
//! drive it through the SDK over a live pipe pair.
//!
//! The exercised surface intentionally avoids model calls (no API key in CI):
//! initialize, thread lifecycle, tool/config/sandbox introspection, event
//! streaming, local pre-provider failure/reopen, and the RPC-error path for an
//! interrupt on an unknown turn. Configured-model success is not exercised.

use std::path::PathBuf;
use std::time::Duration;

use deepagent_harness_protocol::{
    ConfigReadRequest, HarnessEvent, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest,
    TurnInterruptRequest, TurnStartRequest,
};
use deepagent_harness_sdk::{HarnessProcess, SdkError};

fn cli_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = if cfg!(windows) {
        "deepagent.exe"
    } else {
        "deepagent"
    };
    // The CLI package's `[[bin]]` is named `deepagent`; it lands in the
    // workspace default target dir `target/debug` next to the repo root.
    manifest_dir
        .join("../..")
        .join("target")
        .join("debug")
        .join(exe)
}

fn temp_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deepagent-sdk-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

async fn spawn_server(tag: &str) -> (HarnessProcess, deepagent_harness_sdk::HarnessClient) {
    let bin = cli_binary();
    assert!(
        bin.exists(),
        "cli binary not built: {} (run `cargo build -p deepagent-cli` first)",
        bin.display()
    );
    let process = HarnessProcess::spawn(&bin, &temp_workspace(tag))
        .await
        .expect("spawn cli server");
    let client = process.client();
    client
        .initialize("deepagent-sdk-test", "0.1.0")
        .await
        .expect("initialize");
    (process, client)
}

#[tokio::test]
async fn initialize_then_thread_start_streams_thread_started_event() {
    let (_process, client) = spawn_server("thread-event").await;

    let mut events = client.subscribe();

    let response = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: Some("deepseek-official".into()),
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .expect("thread/start");
    let thread_id = response.result()["threadId"]
        .as_str()
        .expect("threadId in result")
        .to_string();

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timed out waiting for thread.started")
        .expect("event channel closed");
    let is_match = matches!(
        &event,
        HarnessEvent::ThreadStarted { thread_id: id, .. } if id == &thread_id
    );
    assert!(
        is_match,
        "expected thread.started for {thread_id}, got {event:?}"
    );
}

#[tokio::test]
async fn h214_owned_thread_start_and_reconnected_durable_read_use_the_real_cli() {
    let Some(bin) = isolated_cli(
        "h214_owned_thread_start_and_reconnected_durable_read_use_the_real_cli",
        "h214-thread",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h214-owned-reopen");
    let process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = process.client();
    client.initialize("h214-owned-e2e", "1").await.unwrap();
    let mut events = client
        .thread_start_streamed(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .unwrap();
    let thread = events.thread_id().to_owned();
    // Consumption deliberately begins only after the admission response exists.
    assert_eq!(events.response().result()["threadId"], thread);
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),events.recv()).await.unwrap().unwrap(),
        Some(HarnessEvent::ThreadStarted { thread_id, .. }) if thread_id==thread)
    );
    assert!(events.recv().await.unwrap().is_none());
    let read = || ThreadReadRequest {
        thread_id: thread.clone(),
        page_size: None,
        max_bytes: None,
        cursor: None,
        after_sequence: None,
        session_after_sequence: None,
        run_after_sequence: None,
        run_after_sequences: None,
    };
    let before = client.thread_read(read()).await.unwrap();
    assert_eq!(before.result()["threadId"], thread);
    assert!(process.wait().await.unwrap().success());

    let reopened = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let current = reopened.client();
    current
        .initialize("h214-owned-reconnected", "1")
        .await
        .unwrap();
    current
        .thread_resume(ThreadResumeRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    let after = current.thread_read(read()).await.unwrap();
    assert_eq!(
        after.result(),
        before.result(),
        "reconnect reads the existing durable thread; no replay or new turn is invented"
    );
    assert!(reopened.wait().await.unwrap().success());
}

async fn isolated_cli(test_name: &str, tag: &str) -> Option<PathBuf> {
    const CHILD: &str = "DEEPAGENT_H214_TEST_CHILD";
    const CLI: &str = "DEEPAGENT_H214_FIXTURE_CLI";
    if std::env::var_os(CHILD).is_none() {
        // Isolate the whole test process, not global env shared by parallel
        // Rust tests. A regular cargo test must never use a user's provider.
        let root = temp_workspace(tag);
        // Exercise the identical CLI bytes in an empty installation, not all
        // bundled plugin discovery/preparation and its unrelated shutdown work.
        let bin = root.join(cli_binary().file_name().unwrap());
        std::fs::copy(cli_binary(), &bin).unwrap();
        assert!(std::fs::read(cli_binary()).unwrap() == std::fs::read(&bin).unwrap());
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", test_name, "--nocapture"]);
        for (name, _) in std::env::vars_os() {
            let key = name.to_string_lossy().to_ascii_uppercase();
            if [
                "KEY",
                "TOKEN",
                "SECRET",
                "PASSWORD",
                "CREDENTIAL",
                "DEEPAGENT",
                "DEEPSEEK",
                "OPENAI",
                "ANTHROPIC",
            ]
            .iter()
            .any(|part| key.contains(part))
            {
                command.env_remove(name);
            }
        }
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "DEEPAGENT_DATA_ROOT",
        ] {
            let path = root.join(name.to_ascii_lowercase());
            std::fs::create_dir_all(&path).unwrap();
            command.env(name, path);
        }
        let log_path = root.join("isolated-test.log");
        let log = std::fs::File::create(&log_path).unwrap();
        let errors = log.try_clone().unwrap();
        let status = tokio::time::timeout(
            Duration::from_secs(30),
            command
                .env(CHILD, "1")
                .env(CLI, &bin)
                .kill_on_drop(true)
                .stdout(log)
                .stderr(errors)
                .status(),
        )
        .await;
        assert!(
            matches!(status, Ok(Ok(status)) if status.success()),
            "isolated CLI test {status:?}; log {}:\n{}",
            log_path.display(),
            std::fs::read_to_string(&log_path).unwrap()
        );
        return None;
    }
    Some(PathBuf::from(
        std::env::var_os(CLI).expect("isolated test CLI"),
    ))
}

#[tokio::test]
async fn h214_owned_real_cli_failure_is_unique_and_durable_after_reopen() {
    let Some(bin) = isolated_cli(
        "h214_owned_real_cli_failure_is_unique_and_durable_after_reopen",
        "h214-failure",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h214-owned-failure");
    let process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = process.client();
    client.initialize("h214-owned-failure", "1").await.unwrap();
    let thread = client
        .thread_start_streamed(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: Some("read_only".into()),
            sandbox_backend: Some("direct".into()),
        })
        .await
        .unwrap()
        .thread_id()
        .to_owned();
    // The isolated test child has an empty synthetic data root: model
    // selection fails before reading a credential or contacting a provider.
    let mut stream = client
        .turn_start_streamed(TurnStartRequest {
            expected_config_snapshot_id: None,
            thread_id: thread.clone(),
            input: "Synthetic local failure without provider settings".into(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .unwrap();
    let turn = stream.turn_id().unwrap().to_owned();
    let mut terminals = 0;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.recv())
        .await
        .unwrap()
        .unwrap()
    {
        match event {
            HarnessEvent::TurnFailed {
                turn_id, reason, ..
            } => {
                assert_eq!(turn_id.as_deref(), Some(turn.as_str()));
                assert!(reason.contains("not initialized"), "{reason}");
                terminals += 1;
            }
            HarnessEvent::TurnCompleted { .. } | HarnessEvent::TurnInterrupted { .. } => {
                panic!("synthetic settings failure must not become success/cancellation")
            }
            _ => {}
        }
    }
    assert_eq!(terminals, 1);
    let request = || ThreadReadRequest {
        thread_id: thread.clone(),
        page_size: None,
        max_bytes: None,
        cursor: None,
        after_sequence: None,
        session_after_sequence: None,
        run_after_sequence: None,
        run_after_sequences: None,
    };
    let before = client.thread_read(request()).await.unwrap();
    let runs = before.result()["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "no automatic redispatch");
    assert_eq!(runs[0]["runId"], turn);
    assert_eq!(runs[0]["terminalKind"], "failed");
    let records = runs[0]["events"].as_array().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|e| e["event_type"] == "run_terminal")
            .count(),
        1
    );
    assert!(runs[0]["actions"].as_array().unwrap().is_empty());
    assert!(runs[0]["approvals"].as_array().unwrap().is_empty());
    let mut page_request = request();
    page_request.page_size = Some(5);
    page_request.max_bytes = Some(1048576);
    let mut page = client.thread_read(page_request.clone()).await.unwrap();
    let first_continuation = page.result()["nextCursor"].as_str().unwrap().to_owned();
    let mut second_page = None;
    let mut session_rows = Vec::new();
    let mut run_rows = Vec::new();
    let mut graph_rows = Vec::new();
    for index in 0..100 {
        assert!(serde_json::to_vec(&page).unwrap().len() <= 1048576);
        let value = page.result();
        let events = value["events"].as_array().unwrap();
        session_rows.extend(events.iter().cloned());
        let mut count = events.len();
        for slice in value["runEvents"].as_array().unwrap() {
            assert_eq!(slice["runId"], turn);
            count += 1;
            for field in ["events", "actions", "approvals", "artifacts"] {
                count += slice[field].as_array().unwrap().len();
            }
            count += slice["graph"]["nodes"].as_array().unwrap().len();
            run_rows.extend(slice["events"].as_array().unwrap().iter().cloned());
            graph_rows.extend(slice["graph"]["nodes"].as_array().unwrap().iter().cloned());
        }
        assert!(count <= 5);
        if value["hasMore"] == false {
            break;
        }
        assert!(index < 99, "bounded history did not exhaust");
        page_request.cursor = Some(value["nextCursor"].as_str().unwrap().into());
        page = client.thread_read(page_request.clone()).await.unwrap();
        if index == 0 {
            second_page = Some(page.clone());
        }
    }
    assert_eq!(session_rows, *before.result()["events"].as_array().unwrap());
    assert_eq!(run_rows, *records);
    assert_eq!(graph_rows, *runs[0]["graph"]["nodes"].as_array().unwrap());
    let cursor = records.last().unwrap()["sequence"].as_u64().unwrap();
    assert!(process.wait().await.unwrap().success());
    let reopened = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let current = reopened.client();
    current
        .initialize("h214-owned-failure-reopen", "1")
        .await
        .unwrap();
    current
        .thread_resume(ThreadResumeRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    let after = current.thread_read(request()).await.unwrap();
    assert_eq!(after.result(), before.result());
    page_request.cursor = Some(first_continuation);
    let resumed_page = current.thread_read(page_request).await.unwrap();
    let expected_page = second_page.unwrap();
    for field in ["events", "runEvents", "hasMore"] {
        assert_eq!(resumed_page.result()[field], expected_page.result()[field]);
    }
    let incremental = current
        .thread_read(ThreadReadRequest {
            run_after_sequences: Some([(turn, cursor)].into()),
            ..request()
        })
        .await
        .unwrap();
    assert!(incremental.result()["runEvents"][0]["events"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(reopened.wait().await.unwrap().success());
}

#[tokio::test]
async fn introspection_methods_round_trip_without_a_model() {
    let (_process, client) = spawn_server("introspect").await;

    let tools = client.tool_list().await.expect("tool/list");
    assert!(tools.result().is_object(), "tool/list returns an object");

    let config = client.config_read().await.expect("config/read");
    assert!(config.is_success(), "config/read succeeds");
    assert_eq!(config.result()["configSnapshotVersion"], 1);
    assert_eq!(config.result()["resolutionStatus"], "uninitialized");
    assert_eq!(config.result()["credentialChecked"], false);
    assert_eq!(config.result()["provider"], "deepseek-official");

    let sandbox = client.sandbox_status().await.expect("sandbox/status");
    assert!(
        sandbox.result().get("available").is_some(),
        "sandbox.status has availability"
    );

    let thread = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .expect("thread/start");
    let thread_id = thread.result()["threadId"]
        .as_str()
        .expect("threadId")
        .to_string();

    let scoped = client
        .config_read_scoped(ConfigReadRequest {
            thread_id: Some(thread_id.clone()),
            cwd: None,
        })
        .await
        .expect("scoped config/read");
    assert!(scoped.is_success());
    assert_eq!(scoped.result()["scope"]["threadId"], thread_id);
    assert_eq!(scoped.result()["configSnapshotVersion"], 1);

    let read = client
        .thread_read(ThreadReadRequest {
            thread_id: thread_id.clone(),
            page_size: None,
            max_bytes: None,
            cursor: None,
            after_sequence: None,
            session_after_sequence: None,
            run_after_sequence: None,
            run_after_sequences: None,
        })
        .await
        .expect("thread/read");
    assert!(
        read.result().get("threadId").is_some(),
        "thread/read echoes thread"
    );

    // Acknowledge the events observed so far; server tracks a monotonic cursor.
    let ack = client.event_ack(1).await.expect("event/ack");
    assert!(ack.is_success(), "event/ack succeeds");
    assert!(
        matches!(
            client.event_ack(u64::MAX).await,
            Err(SdkError::Rpc { code: -32602, .. })
        ),
        "unemitted connection sequence must be rejected through the real SDK"
    );
}

#[tokio::test]
async fn interrupt_unknown_turn_surfaces_rpc_error() {
    let (_process, client) = spawn_server("interrupt").await;

    let thread = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .expect("thread/start");
    let thread_id = thread.result()["threadId"]
        .as_str()
        .expect("threadId")
        .to_string();

    let error = client
        .turn_interrupt(TurnInterruptRequest {
            thread_id,
            turn_id: "turn_does_not_exist".into(),
        })
        .await
        .expect_err("unknown turn must fail");
    assert!(
        matches!(error, SdkError::Rpc { code, .. } if code == -32004),
        "expected ERR_INVALID_TURN for unknown turn, got {error:?}"
    );
}

#[tokio::test]
async fn request_before_initialize_is_rejected() {
    let bin = cli_binary();
    let process = HarnessProcess::spawn(&bin, &temp_workspace("no-init"))
        .await
        .expect("spawn cli server");
    let client = process.client();

    let error = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .expect_err("request before initialize must be rejected");
    assert!(
        matches!(error, SdkError::NotInitialized),
        "expected NotInitialized, got {error:?}"
    );
}

#[tokio::test]
async fn process_exits_when_stdin_closes() {
    let bin = cli_binary();
    let process = HarnessProcess::spawn(&bin, &temp_workspace("shutdown"))
        .await
        .expect("spawn cli server");
    let client = process.client();
    client
        .initialize("shutdown-test", "0.1.0")
        .await
        .expect("initialize");
    drop(client);

    let status = tokio::time::timeout(Duration::from_secs(10), process.wait())
        .await
        .expect("server should exit after stdin EOF")
        .expect("wait on child");
    assert!(status.success(), "server exits cleanly, got {status}");
}

#[test]
fn cli_binary_path_resolves_for_platform() {
    let bin = cli_binary();
    assert!(
        bin.file_name().is_some(),
        "resolved cli binary has a file name"
    );
    if cfg!(windows) {
        assert!(bin.ends_with("deepagent.exe"));
    } else {
        assert!(bin.ends_with("deepagent"));
    }
}
