//! End-to-end test: spawn the real `deepagent-cli server` stdio process and
//! drive it through the SDK over a live pipe pair.
//!
//! The exercised surface intentionally avoids model calls (no API key in CI):
//! initialize, thread lifecycle, tool/config/sandbox introspection, event
//! streaming, and the RPC-error path for an interrupt on an unknown turn.
//! Model-backed `turn/start` stays under real-machine verification.

use std::path::PathBuf;
use std::time::Duration;

use deepagent_harness_protocol::{
    HarnessEvent, ThreadReadRequest, ThreadStartRequest, TurnInterruptRequest,
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
async fn introspection_methods_round_trip_without_a_model() {
    let (_process, client) = spawn_server("introspect").await;

    let tools = client.tool_list().await.expect("tool/list");
    assert!(tools.result().is_object(), "tool/list returns an object");

    let config = client.config_read().await.expect("config/read");
    assert!(config.is_success(), "config/read succeeds");

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

    let read = client
        .thread_read(ThreadReadRequest {
            thread_id: thread_id.clone(),
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
