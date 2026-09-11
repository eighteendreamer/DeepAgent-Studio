//! 协议级 PTY shell 通道的真实服务器端到端回归。
//!
//! 需要一台允许密码登录的 SSH 服务器。通过环境变量提供：
//! `DEEPAGENT_SSH_TEST_HOST`、`DEEPAGENT_SSH_TEST_PORT`（默认 22）、
//! `DEEPAGENT_SSH_TEST_USERNAME`（默认 root）、`DEEPAGENT_SSH_TEST_PASSWORD`。
//! 运行：`cargo test -p deepagent-ssh --test pty_shell -- --ignored --nocapture`。

use std::time::{Duration, Instant};

use deepagent_ssh::{CreateSshConnectionRequest, SshAuthType, SshService};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real SSH server via DEEPAGENT_SSH_TEST_* env vars"]
async fn pty_shell_streams_prompt_and_echoes_input() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("russh=debug,deepagent_ssh=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let host = match std::env::var("DEEPAGENT_SSH_TEST_HOST") {
        Ok(host) => host,
        Err(_) => {
            eprintln!("DEEPAGENT_SSH_TEST_HOST not set; skipping");
            return;
        }
    };
    let port: u16 = std::env::var("DEEPAGENT_SSH_TEST_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(22);
    let username = std::env::var("DEEPAGENT_SSH_TEST_USERNAME").unwrap_or_else(|_| "root".into());
    let password = std::env::var("DEEPAGENT_SSH_TEST_PASSWORD")
        .expect("DEEPAGENT_SSH_TEST_PASSWORD must be set when running this test");

    let app_data = tempfile::tempdir().expect("create temp app data dir");
    let service = SshService::new(app_data.path().to_path_buf());
    let connection = service
        .create_connection(CreateSshConnectionRequest {
            name: "pty-shell-it".into(),
            host: host.clone(),
            port,
            username: username.clone(),
            auth_type: SshAuthType::Password,
            key_path: None,
            password: Some(password),
        })
        .await
        .expect("create connection");
    let handle = service.connect(&connection.id).await.expect("connect");
    let handle = service
        .pty_spawn(&handle, 120, 32)
        .await
        .expect("pty spawn");

    let mut received: Vec<u8> = Vec::new();
    wait_for_bytes(&service, &handle, &mut received, 15, "initial shell output")
        .await
        .expect("initial shell output");
    println!("initial output: {:?}", String::from_utf8_lossy(&received));

    let marker = b"DEEPAGENT_PTY_OK";
    service
        .pty_write(&handle, b"echo DEEPAGENT_PTY_OK\r\n")
        .await
        .expect("pty write");
    wait_for_marker(&service, &handle, &mut received, marker, 15, "echo marker")
        .await
        .expect("echo marker after write");

    service
        .pty_resize(&handle, 100, 24)
        .await
        .expect("pty resize");
    let marker = b"DEEPAGENT_PTY_RESIZED";
    service
        .pty_write(&handle, b"echo DEEPAGENT_PTY_RESIZED\r\n")
        .await
        .expect("pty write after resize");
    wait_for_marker(
        &service,
        &handle,
        &mut received,
        marker,
        15,
        "post-resize marker",
    )
    .await
    .expect("echo marker after resize");

    let _ = service.disconnect(&connection.id).await;
}

async fn wait_for_bytes(
    service: &SshService,
    handle: &deepagent_ssh::SshServiceHandle,
    received: &mut Vec<u8>,
    timeout_secs: u64,
    label: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while received.is_empty() {
        match service.pty_read(handle).await {
            Ok(chunk) => received.extend_from_slice(&chunk),
            Err(error) => return Err(format!("{label}: pty_read failed: {error}")),
        }
        if Instant::now() >= deadline && received.is_empty() {
            return Err(format!("{label}: no output within {timeout_secs}s"));
        }
        if received.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

async fn wait_for_marker(
    service: &SshService,
    handle: &deepagent_ssh::SshServiceHandle,
    received: &mut Vec<u8>,
    marker: &[u8],
    timeout_secs: u64,
    label: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match service.pty_read(handle).await {
            Ok(chunk) => received.extend_from_slice(&chunk),
            Err(error) => return Err(format!("{label}: pty_read failed: {error}")),
        }
        if received
            .windows(marker.len())
            .any(|window| window == marker)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{label}: marker {:?} not seen within {timeout_secs}s; output so far: {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(received),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
