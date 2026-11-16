mod support;

use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::Duration;

use deepagent_ssh::{RemotePushFileRequest, RemoteVerifyMode, SshError, SshService};
use support::SshServer;

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .expect("loopback transport must settle")
}

#[tokio::test]
async fn exec_drains_output_after_status_and_preserves_errors() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        let handle = service.connect(&connection.id).await.unwrap();

        let output = service.exec(&handle, "fixture-late-output").await.unwrap();
        assert_eq!(output.stdout, "ok\n尾部\n");
        assert_eq!(output.stderr, "late stderr");
        assert_eq!(output.exit_code, 0);
        assert!(matches!(
            service.exec(&handle, "fixture-nonzero").await,
            Err(SshError::CommandFailed { exit_code: 7, stderr }) if stderr == "fixture error"
        ));
        let missing = service.exec(&handle, "fixture-missing-status").await;
        assert!(missing
            .unwrap_err()
            .to_string()
            .contains("without an exit status"));
        assert_eq!(
            service.exec(&handle, "still-live").await.unwrap().stdout,
            "ok\n"
        );
        assert_eq!(
            server.auth_count(),
            1,
            "channels reuse one verified connection"
        );
        service.disconnect(&connection.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn rejected_exec_is_closed_without_disconnecting_other_channels() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        let handle = service.connect(&connection.id).await.unwrap();
        assert!(service.exec(&handle, "fixture-rejected").await.is_err());
        // The server deliberately does not close on rejection. A dropped local
        // channel alone does not send SSH_MSG_CHANNEL_CLOSE in russh 0.55.
        bounded(async {
            while server.counts.closed_channels.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert_eq!(
            service.exec(&handle, "still-live").await.unwrap().stdout,
            "ok\n"
        );
        service.disconnect(&connection.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn pty_streams_bytes_resizes_and_allows_parallel_exec() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        let handle = service.connect(&connection.id).await.unwrap();
        let pty = service.pty_spawn(&handle, 120, 32).await.unwrap();
        let mut output = Vec::new();
        while !output.ends_with(b"fixture$ ") {
            output.extend(service.pty_read(&pty).await.unwrap());
            tokio::task::yield_now().await;
        }
        assert_eq!(*server.counts.pty_size.read().unwrap(), Some((120, 32)));
        let input = "中文\0\u{1b}[31m\r\n".as_bytes();
        service.pty_write(&pty, input).await.unwrap();
        let mut echo = Vec::new();
        while echo.len() < input.len() {
            echo.extend(service.pty_read(&pty).await.unwrap());
            tokio::task::yield_now().await;
        }
        assert_eq!(
            echo, input,
            "transport must not decode or normalize PTY bytes"
        );
        service.pty_resize(&pty, 100, 24).await.unwrap();
        while *server.counts.pty_size.read().unwrap() != Some((100, 24)) {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            service.exec(&handle, "parallel-exec").await.unwrap().stdout,
            "ok\n"
        );
        service.disconnect(&connection.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn deployment_upload_preserves_binary_and_truncates_old_content() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        service.connect(&connection.id).await.unwrap();
        let local = data.path().join("binary-input");
        for payload in [
            (0..190_007)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
            b"short\0\xff".to_vec(),
            Vec::new(),
        ] {
            std::fs::write(&local, &payload).unwrap();
            let result = service
                .remote_push_file(
                    &connection.id,
                    RemotePushFileRequest {
                        local_path: local.to_str().unwrap().to_owned(),
                        remote_path: "/artifact".into(),
                        create_parent: false,
                        overwrite: true,
                        verify_mode: RemoteVerifyMode::Sha256,
                    },
                )
                .await
                .unwrap();
            assert!(result.ok && result.integrity_verified);
            assert_eq!(result.bytes, payload.len() as u64);
            assert_eq!(result.local_sha256, result.remote_sha256);
            assert_eq!(server.counts.files.read().unwrap()["/artifact"], payload);
        }
        assert_eq!(server.auth_count(), 1);
        service.disconnect(&connection.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn denied_sftp_upload_fails_without_poisoning_the_connection() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        let handle = service.connect(&connection.id).await.unwrap();
        let local = data.path().join("input");
        std::fs::write(&local, b"fixture").unwrap();
        let result = service
            .remote_push_file(
                &connection.id,
                RemotePushFileRequest {
                    local_path: local.to_str().unwrap().to_owned(),
                    remote_path: "/denied".into(),
                    create_parent: false,
                    overwrite: true,
                    verify_mode: RemoteVerifyMode::Size,
                },
            )
            .await;
        assert!(
            result.is_err(),
            "SFTP server permission failures must not look successful"
        );
        assert!(!server.counts.files.read().unwrap().contains_key("/denied"));
        assert_eq!(
            service.exec(&handle, "still-live").await.unwrap().stdout,
            "ok\n"
        );
        service.disconnect(&connection.id).await.unwrap();
    })
    .await;
}
