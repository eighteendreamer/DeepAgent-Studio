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
    ConfigReadRequest, HarnessEvent, InitializeResult, ThreadArchiveRequest, ThreadListRequest,
    ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, ThreadUnarchiveRequest,
    TurnInterruptRequest, TurnStartRequest,
};
use deepagent_harness_sdk::{HarnessProcess, SdkError, SequenceStatus};

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

async fn spawn_server(
    test_name: &str,
    tag: &str,
) -> Option<(HarnessProcess, deepagent_harness_sdk::HarnessClient)> {
    let bin = isolated_cli(test_name, tag).await?;
    let process = HarnessProcess::spawn(&bin, &temp_workspace(tag))
        .await
        .expect("spawn cli server");
    let client = process.client();
    let handshake = client
        .initialize("deepagent-sdk-test", "0.1.0")
        .await
        .expect("initialize");
    let identity: InitializeResult = serde_json::from_value(handshake.result().clone()).unwrap();
    assert_eq!(identity.server_name, "deepagent");
    assert_eq!(
        identity.protocol_version,
        deepagent_harness_protocol::PROTOCOL_VERSION
    );
    assert!(!identity.server_version.trim().is_empty());
    assert!(identity.capabilities.thread_lifecycle && identity.capabilities.streaming);
    Some((process, client))
}

#[tokio::test]
async fn h107_real_end_without_credentials_is_once_and_survives_process_reopen() {
    let Some(bin) = isolated_cli(
        "h107_real_end_without_credentials_is_once_and_survives_process_reopen",
        "h107-end",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h107-end-workspace");
    let mut process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let mut client = process.client();
    client.initialize("h107", "1").await.unwrap();
    let thread = client
        .thread_start(serde_json::from_value(serde_json::json!({})).unwrap())
        .await
        .unwrap()
        .result()["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let read = || {
        serde_json::from_value::<ThreadReadRequest>(serde_json::json!({"threadId":thread})).unwrap()
    };
    let mut turns = Vec::new();
    for _ in 0..2 {
        let mut stream = client
            .turn_start_streamed(
                serde_json::from_value(serde_json::json!({
                    "threadId":thread,"input":"/end"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let turn = stream.turn_id().unwrap().to_owned();
        let mut completed = 0;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.recv())
            .await
            .unwrap()
            .unwrap()
        {
            match event {
                HarnessEvent::TurnCompleted {
                    thread_id, turn_id, ..
                } => {
                    assert_eq!(thread_id.as_deref(), Some(thread.as_str()));
                    assert_eq!(turn_id.as_deref(), Some(turn.as_str()));
                    completed += 1;
                }
                HarnessEvent::TurnFailed { reason, .. } => panic!("explicit end failed: {reason}"),
                HarnessEvent::TurnInterrupted { .. } => {
                    panic!("explicit end unexpectedly cancelled")
                }
                _ => {}
            }
        }
        assert_eq!(completed, 1);
        turns.push(turn);
        let before = client.thread_read(read()).await.unwrap();
        assert!(!before.result()["hasMore"].as_bool().unwrap());
        assert_eq!(
            before.result()["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["payload"]["type"] == "session_ended")
                .count(),
            1
        );
        let runs = before.result()["runEvents"].as_array().unwrap();
        assert_eq!(runs.len(), turns.len(), "no automatic redispatch");
        for turn in &turns {
            let run = runs.iter().find(|run| run["runId"] == *turn).unwrap();
            assert_eq!(run["terminalKind"], "succeeded");
            for event_type in ["run_terminal", "run_completed"] {
                assert_eq!(
                    run["events"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|event| event["event_type"] == event_type)
                        .count(),
                    1,
                    "{turn}/{event_type}"
                );
            }
        }

        // Terminal delivery is not an idle/lease-release ACK. The existing
        // server deliberately stays busy through worker cleanup. A real,
        // graceful process exit is this cold-reopen test's synchronization
        // barrier: do not race another /end, retry busy, sleep, or unlock early.
        // Same-host repetition after cleanup is covered by the CLI dispatcher
        // and AppCore lifecycle tests, which can join that worker directly.
        drop(stream);
        drop(client);
        assert!(process.wait().await.unwrap().success());
        process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
        client = process.client();
        client.initialize("h107-reopen", "1").await.unwrap();
        client
            .thread_resume(ThreadResumeRequest {
                thread_id: thread.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            before.result(),
            client.thread_read(read()).await.unwrap().result()
        );
    }
    let after = client.thread_read(read()).await.unwrap();
    assert!(matches!(
        client
            .turn_start(
                serde_json::from_value(serde_json::json!({
                    "threadId":thread,"input":"must not silently reopen"
                }))
                .unwrap()
            )
            .await,
        Err(SdkError::Rpc { .. })
    ));
    assert_eq!(
        client.thread_read(read()).await.unwrap().result(),
        after.result()
    );
    drop(client);
    assert!(process.wait().await.unwrap().success());
}

#[tokio::test]
async fn h220_real_catalog_page_is_bounded_and_reopens_without_duplicates() {
    let Some(bin) = isolated_cli(
        "h220_real_catalog_page_is_bounded_and_reopens_without_duplicates",
        "h220-catalog",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h220-workspace");
    let host = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = host.client();
    client.initialize("h220", "1").await.unwrap();
    let mut expected = Vec::new();
    for _ in 0..42 {
        let result = client
            .thread_start(
                serde_json::from_value(serde_json::json!({"provider":"deepseek"})).unwrap(),
            )
            .await
            .unwrap();
        expected.push(result.result()["threadId"].as_str().unwrap().to_owned());
    }
    let first = client
        .thread_list_page(ThreadListRequest::default())
        .await
        .unwrap();
    assert_eq!(first.threads.len(), 25);
    assert!(first.has_more);
    assert!(host.wait().await.unwrap().success());
    let host = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = host.client();
    client.initialize("h220-reopen", "1").await.unwrap();
    let second = client
        .thread_list_page(ThreadListRequest {
            cursor: first.next_cursor,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!second.has_more);
    assert!(second.next_cursor.is_none());
    let mut actual: Vec<_> = first
        .threads
        .into_iter()
        .chain(second.threads)
        .map(|row| row.thread_id)
        .collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    let provider = client
        .thread_list_page(ThreadListRequest {
            provider: Some("deepseek".into()),
            limit: Some(100),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(provider.threads.len(), 42);
    assert!(host.wait().await.unwrap().success());
}

#[tokio::test]
async fn h216_real_cold_fork_is_ready_before_resume_and_keeps_durable_configuration() {
    let Some(bin) = isolated_cli(
        "h216_real_cold_fork_is_ready_before_resume_and_keeps_durable_configuration",
        "h216-fork",
    )
    .await
    else {
        return;
    };
    let first_workspace = temp_workspace("h216-host");
    let project = temp_workspace("h216-project");
    let first = HarnessProcess::spawn(&bin, &first_workspace).await.unwrap();
    let client = first.client();
    client.initialize("h216", "1").await.unwrap();
    let started = client
        .thread_start(
            serde_json::from_value(serde_json::json!({
                "cwd":project, "permissionProfile":"read_only",
                "sandboxBackend":"direct", "reasoningEffort":"none"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let source = started.result()["threadId"].as_str().unwrap().to_owned();
    let expected = started.result()["configuration"].clone();
    assert!(first.wait().await.unwrap().success());

    let reopened = HarnessProcess::spawn(&bin, &temp_workspace("h216-other-host"))
        .await
        .unwrap();
    let current = reopened.client();
    current.initialize("h216-cold", "1").await.unwrap();
    let forked = current
        .thread_fork(
            serde_json::from_value(serde_json::json!({
                "threadId":source, "atSequence":0
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let child = forked.result()["threadId"].as_str().unwrap().to_owned();
    assert_ne!(child, source);
    assert!(matches!(
        current
            .turn_start(
                serde_json::from_value(serde_json::json!({
                    "threadId":source,"input":"/help"
                }))
                .unwrap()
            )
            .await,
        Err(SdkError::Rpc { code: -32003, .. })
    ));
    // The actual first child execution has no preceding thread/resume.
    let mut events = current
        .turn_start_streamed(
            serde_json::from_value(serde_json::json!({
                "threadId":child,"input":"/help"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let mut completed = 0;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap()
    {
        if matches!(event, HarnessEvent::TurnCompleted { .. }) {
            completed += 1;
        }
    }
    assert_eq!(completed, 1);
    let resumed = current
        .thread_resume(ThreadResumeRequest {
            thread_id: child.clone(),
        })
        .await
        .unwrap();
    assert_eq!(resumed.result()["configuration"], expected);
    let page = current
        .thread_read(
            serde_json::from_value(serde_json::json!({
                "threadId":child,"maxBytes":1048576
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let runs = page.result()["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["terminalKind"], "succeeded");
    assert_eq!(
        runs[0]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "run_terminal")
            .count(),
        1
    );
    let source_page = current
        .thread_read(
            serde_json::from_value(serde_json::json!({
                "threadId":source
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(source_page.result()["runEvents"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        current.thread_list().await.unwrap().result()["threads"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(reopened.wait().await.unwrap().success());
}

#[tokio::test]
async fn h215_real_archive_unarchive_is_durable_and_restored_thread_requires_explicit_resume() {
    let Some(bin) = isolated_cli(
        "h215_real_archive_unarchive_is_durable_and_restored_thread_requires_explicit_resume",
        "h215-archive",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h215-archive-workspace");
    let process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = process.client();
    client.initialize("h215", "1").await.unwrap();
    let started = client
        .thread_start(serde_json::from_value(serde_json::json!({})).unwrap())
        .await
        .unwrap();
    let thread = started.result()["threadId"].as_str().unwrap().to_owned();
    let read = || {
        serde_json::from_value::<ThreadReadRequest>(serde_json::json!({"threadId":thread})).unwrap()
    };
    let turn = || {
        serde_json::from_value::<TurnStartRequest>(
            serde_json::json!({"threadId":thread,"input":"/help"}),
        )
        .unwrap()
    };
    let before = client.thread_read(read()).await.unwrap();
    client
        .thread_archive(ThreadArchiveRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    assert!(client.thread_list().await.unwrap().result()["threads"]
        .as_array()
        .unwrap()
        .is_empty());
    let archived = client
        .thread_list_with_options(ThreadListRequest {
            archived: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(archived.result()["threads"].as_array().unwrap().len(), 1);
    assert_eq!(archived.result()["threads"][0]["threadId"], thread);
    assert_eq!(archived.result()["threads"][0]["archived"], true);
    assert!(matches!(
        client.turn_start(turn()).await,
        Err(SdkError::Rpc { code: -32003, .. })
    ));
    assert_eq!(
        client.thread_read(read()).await.unwrap().result(),
        before.result()
    );
    assert!(process.wait().await.unwrap().success());

    let reopened = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let current = reopened.client();
    current.initialize("h215-reopen", "1").await.unwrap();
    assert!(matches!(
        current
            .thread_resume(ThreadResumeRequest {
                thread_id: thread.clone()
            })
            .await,
        Err(SdkError::Rpc { code: -32003, .. })
    ));
    let restored = current
        .thread_unarchive(ThreadUnarchiveRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        restored.result(),
        &serde_json::json!({"threadId":thread,"archived":false})
    );
    assert!(matches!(
        current.turn_start(turn()).await,
        Err(SdkError::Rpc { code: -32003, .. })
    ));
    current
        .thread_resume(ThreadResumeRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        current.thread_read(read()).await.unwrap().result(),
        before.result()
    );
    let mut events = current.turn_start_streamed(turn()).await.unwrap();
    let mut completed = 0;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap()
    {
        if matches!(event, HarnessEvent::TurnCompleted { .. }) {
            completed += 1;
        }
    }
    assert_eq!(completed, 1);
    let after = current.thread_read(read()).await.unwrap();
    let runs = after.result()["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["terminalKind"], "succeeded");
    assert_eq!(
        runs[0]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "run_terminal")
            .count(),
        1
    );
    assert!(runs[0]["actions"].as_array().unwrap().is_empty());
    assert!(runs[0]["approvals"].as_array().unwrap().is_empty());
    assert!(reopened.wait().await.unwrap().success());
}

#[tokio::test]
async fn initialize_then_thread_start_streams_thread_started_event() {
    let Some((process, client)) = spawn_server(
        "initialize_then_thread_start_streams_thread_started_event",
        "thread-event",
    )
    .await
    else {
        return;
    };

    let mut events = client.subscribe();

    let response = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: Some("deepseek-official".into()),
            model: None,
            reasoning_effort: None,
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
        &event.event,
        HarnessEvent::ThreadStarted { thread_id: id, .. } if id == &thread_id
    );
    assert!(
        is_match,
        "expected thread.started for {thread_id}, got {event:?}"
    );
    assert!(process.wait().await.unwrap().success());
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
            reasoning_effort: None,
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

#[tokio::test]
async fn h222_real_stdio_thread_reasoning_survives_process_restart() {
    let Some(bin) = isolated_cli(
        "h222_real_stdio_thread_reasoning_survives_process_restart",
        "h222-reasoning",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h222-reasoning-reopen");
    let process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = process.client();
    client.initialize("h222-start", "1").await.unwrap();
    let mut saved = Vec::new();
    for effort in ["none", "low", "medium", "high", "max"] {
        let response = client
            .thread_start(ThreadStartRequest {
                cwd: None,
                provider: None,
                model: None,
                reasoning_effort: Some(effort.into()),
                permission_profile: None,
                sandbox_backend: None,
            })
            .await
            .unwrap();
        let configuration = response.result()["configuration"].clone();
        assert_eq!(configuration["overrides"]["reasoning_effort"], effort);
        saved.push((
            response.result()["threadId"].as_str().unwrap().to_owned(),
            configuration,
        ));
    }
    drop(client);
    assert!(process.wait().await.unwrap().success());
    let reopened = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let current = reopened.client();
    current.initialize("h222-resume", "1").await.unwrap();
    for (thread_id, configuration) in saved {
        let response = current
            .thread_resume(ThreadResumeRequest { thread_id })
            .await
            .unwrap();
        assert_eq!(response.result()["configuration"], configuration);
    }
    drop(current);
    assert!(reopened.wait().await.unwrap().success());
}

async fn isolated_cli(test_name: &str, tag: &str) -> Option<PathBuf> {
    const CHILD: &str = "DEEPAGENT_H214_TEST_CHILD";
    const CLI: &str = "DEEPAGENT_H214_FIXTURE_CLI";
    if std::env::var_os(CHILD).is_none() {
        // Isolate the whole test process, not global env shared by parallel
        // Rust tests. A regular cargo test must never use a user's provider.
        let root = temp_workspace(tag);
        let source = cli_binary();
        assert!(
            source.exists(),
            "cli binary not built: {} (run `cargo build -p deepagent-cli` first)",
            source.display()
        );
        // Exercise the identical CLI bytes in an empty installation, not all
        // bundled plugin discovery/preparation and its unrelated shutdown work.
        let bin = root.join(source.file_name().unwrap());
        std::fs::copy(&source, &bin).unwrap();
        assert!(std::fs::read(&source).unwrap() == std::fs::read(&bin).unwrap());
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
            reasoning_effort: None,
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
    let Some((process, client)) = spawn_server(
        "introspection_methods_round_trip_without_a_model",
        "introspect",
    )
    .await
    else {
        return;
    };

    let tools = client.tool_list().await.expect("tool/list");
    assert!(tools.result().is_object(), "tool/list returns an object");

    let config = client.config_read().await.expect("config/read");
    assert!(config.is_success(), "config/read succeeds");
    assert_eq!(config.result()["configSnapshotVersion"], 1);
    assert_eq!(config.result()["resolutionStatus"], "uninitialized");
    assert_eq!(config.result()["credentialChecked"], false);
    assert_eq!(config.result()["provider"], "deepseek-official");
    assert!(config.result()["model"].is_null());
    let second_config = client.config_read().await.expect("second config/read");
    assert_eq!(second_config.result(), config.result());

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
            reasoning_effort: None,
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
    assert_eq!(scoped.result()["resolutionStatus"], "uninitialized");
    assert_eq!(scoped.result()["credentialChecked"], false);
    assert!(scoped.result()["model"].is_null());

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
    assert!(process.wait().await.unwrap().success());
}

#[tokio::test]
async fn interrupt_unknown_turn_surfaces_rpc_error() {
    let Some((process, client)) =
        spawn_server("interrupt_unknown_turn_surfaces_rpc_error", "interrupt").await
    else {
        return;
    };

    let thread = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            reasoning_effort: None,
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
    assert!(process.wait().await.unwrap().success());
}

#[tokio::test]
async fn request_before_initialize_is_rejected() {
    let Some(bin) = isolated_cli("request_before_initialize_is_rejected", "no-init").await else {
        return;
    };
    let process = HarnessProcess::spawn(&bin, &temp_workspace("no-init"))
        .await
        .expect("spawn cli server");
    let client = process.client();

    let error = client
        .thread_start(ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .expect_err("request before initialize must be rejected");
    assert!(
        matches!(error, SdkError::NotInitialized),
        "expected NotInitialized, got {error:?}"
    );
    assert!(process.wait().await.unwrap().success());
}

#[tokio::test]
async fn process_exits_when_stdin_closes() {
    let Some(bin) = isolated_cli("process_exits_when_stdin_closes", "shutdown").await else {
        return;
    };
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

#[tokio::test]
async fn h205_real_cli_ack_uses_delivered_wire_sequence_and_reopen_uses_durable_identity() {
    let Some(bin) = isolated_cli(
        "h205_real_cli_ack_uses_delivered_wire_sequence_and_reopen_uses_durable_identity",
        "h205-sequence",
    )
    .await
    else {
        return;
    };
    let workspace = temp_workspace("h205-sequence-workspace");
    let process = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let client = process.client();
    client.initialize("h205", "1").await.unwrap();
    let mut raw = client.subscribe();
    let request = || ThreadStartRequest {
        cwd: None,
        provider: None,
        model: None,
        reasoning_effort: None,
        permission_profile: None,
        sandbox_backend: None,
    };
    let mut records = Vec::new();
    for index in 0..2 {
        let mut owned = client.thread_start_streamed(request()).await.unwrap();
        let record = tokio::time::timeout(Duration::from_secs(5), owned.recv_notification())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            record,
            raw.recv().await.unwrap(),
            "raw and owned metadata use one bus"
        );
        assert_eq!(
            record.sequence_status,
            if index == 0 {
                SequenceStatus::First
            } else {
                SequenceStatus::Continuous
            }
        );
        assert_eq!(record.event_sequence, Some(index + 1));
        assert!(owned.recv_notification().await.unwrap().is_none());
        records.push((owned.thread_id().to_owned(), record));
    }
    let latest = records[1].1.event_sequence.unwrap();
    let ack = client.event_ack(latest).await.unwrap();
    assert_eq!(ack.result()["acknowledgedEventSequence"], latest);
    assert!(matches!(
        client.event_ack(records[0].1.event_sequence.unwrap()).await,
        Err(SdkError::Rpc { code: -32602, .. })
    ));
    assert!(matches!(
        client.event_ack(latest + 1).await,
        Err(SdkError::Rpc { code: -32602, .. })
    ));
    let thread = records[0].0.clone();
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
    assert!(process.wait().await.unwrap().success());
    let reopened = HarnessProcess::spawn(&bin, &workspace).await.unwrap();
    let current = reopened.client();
    current.initialize("h205-reopen", "1").await.unwrap();
    current
        .thread_resume(ThreadResumeRequest {
            thread_id: thread.clone(),
        })
        .await
        .unwrap();
    let after = current.thread_read(read()).await.unwrap();
    assert_eq!(
        before.result(),
        after.result(),
        "durable read is not indexed by live sequence"
    );
    let mut owned = current.thread_start_streamed(request()).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), owned.recv_notification())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // This is a fresh physical connection: the old ACK cursor is not carried.
    let sequence = first.event_sequence.unwrap();
    assert_eq!(sequence, 1); // thread/resume loads state without emitting a notification.
    assert_eq!(first.sequence_status, SequenceStatus::First);
    assert!(current.event_ack(sequence).await.is_ok());
    assert!(matches!(
        current.event_ack(sequence + 1).await,
        Err(SdkError::Rpc { code: -32602, .. })
    ));
    assert!(reopened.wait().await.unwrap().success());
}

#[tokio::test]
async fn h91_real_cli_parallel_local_turns_keep_raw_owned_and_ack_sequences_consistent() {
    let Some(bin) = isolated_cli(
        "h91_real_cli_parallel_local_turns_keep_raw_owned_and_ack_sequences_consistent",
        "h91-sequence",
    )
    .await
    else {
        return;
    };
    let process = HarnessProcess::spawn(&bin, &temp_workspace("h91-sequence-workspace"))
        .await
        .unwrap();
    let client = process.client();
    client.initialize("h91", "1").await.unwrap();
    let mut raw = client.subscribe();
    let mut threads = Vec::new();
    for _ in 0..8 {
        let response = client
            .thread_start(
                serde_json::from_value(serde_json::json!({
                    "permissionProfile":"read_only", "sandboxBackend":"direct"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        threads.push(response.result()["threadId"].as_str().unwrap().to_owned());
    }
    let mut workers = Vec::new();
    for thread in threads {
        let client = client.clone();
        workers.push(tokio::spawn(async move {
            // Local /help exercises the real run/terminal path but never
            // initializes a model, reads a credential or contacts a provider.
            let mut owned = client
                .turn_start_streamed(
                    serde_json::from_value(serde_json::json!({
                        "threadId":thread,"input":"/help"
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
            let turn = owned.turn_id().unwrap().to_owned();
            let mut terminals = 0;
            while let Some(record) =
                tokio::time::timeout(Duration::from_secs(5), owned.recv_notification())
                    .await
                    .unwrap()
                    .unwrap()
            {
                assert_eq!(record.sequence_status, SequenceStatus::Continuous);
                if matches!(record.event, HarnessEvent::TurnCompleted { .. }) {
                    terminals += 1;
                }
            }
            assert_eq!(terminals, 1);
            (thread, turn)
        }));
    }
    let mut turns = Vec::new();
    for worker in workers {
        turns.push(worker.await.unwrap());
    }
    let mut previous = 0;
    let mut terminals = 0;
    while terminals < turns.len() {
        let record = tokio::time::timeout(Duration::from_secs(5), raw.recv())
            .await
            .unwrap()
            .unwrap();
        let sequence = record.event_sequence.unwrap();
        assert_eq!(sequence, previous + 1);
        assert_eq!(
            record.sequence_status,
            if previous == 0 {
                SequenceStatus::First
            } else {
                SequenceStatus::Continuous
            }
        );
        let result = client.event_ack(sequence).await.unwrap();
        assert_eq!(result.result()["acknowledgedEventSequence"], sequence);
        previous = sequence;
        if matches!(record.event, HarnessEvent::TurnCompleted { .. }) {
            terminals += 1;
        }
    }
    for (thread, turn) in turns {
        let page = client
            .thread_read(serde_json::from_value(serde_json::json!({"threadId":thread})).unwrap())
            .await
            .unwrap();
        let runs = page.result()["runEvents"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["runId"], turn);
        assert_eq!(runs[0]["terminalKind"], "succeeded");
        assert_eq!(
            runs[0]["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["event_type"] == "run_terminal")
                .count(),
            1
        );
    }
    eprintln!("H-91 real CLI + SDK: eight parallel local turns, wire events={previous}, raw/owned continuous, each ACK accepted, one durable terminal per run");
    assert!(process.wait().await.unwrap().success());
}
