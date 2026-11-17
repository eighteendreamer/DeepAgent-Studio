mod support;

use deepagent_ssh::SshService;
use deepagent_ssh::SshTerminalSessionBackend;
use deepagent_terminal::{TerminalInputHolder, TerminalOpenRequest, TerminalSessionBackend};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn remote_pty_handles_identify_distinct_channels_on_one_authenticated_connection() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let connection = server.create(&service, Some(&trust)).await;
        let base = service.connect(&connection.id).await.unwrap();
        let first = service.pty_spawn(&base, 100, 30).await.unwrap();
        let second = service.pty_spawn(&base, 80, 24).await.unwrap();
        assert_ne!(
            first.token, second.token,
            "a PTY handle must not just repeat its connection id"
        );
        assert_eq!(first.connection_id, second.connection_id);
        let first_bytes = b"first-channel-only";
        let second_bytes = b"second-channel-only";
        service.pty_write(&first, first_bytes).await.unwrap();
        service.pty_write(&second, second_bytes).await.unwrap();
        for (handle, expected) in [
            (&first, first_bytes.as_slice()),
            (&second, second_bytes.as_slice()),
        ] {
            let mut received = Vec::new();
            while !received.ends_with(expected) {
                received.extend(service.pty_read(handle).await.unwrap());
                tokio::task::yield_now().await;
            }
            assert_eq!(received, [b"fixture$ ".as_slice(), expected].concat());
        }
        assert_eq!(server.auth_count(), 1);
        service.disconnect(&connection.id).await.unwrap();
        let auth_count = server.auth_count();
        assert!(service.pty_read(&first).await.is_err());
        assert_eq!(
            server.auth_count(),
            auth_count,
            "a stale PTY read must not reconnect"
        );
    })
    .await
    .expect("two loopback PTYs must settle");
}

#[tokio::test]
async fn rejected_shell_is_reported_and_remote_exit_drains_bytes_before_eof() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let connection = server.create(&service, Some(&trust)).await;
        let base = service.connect(&connection.id).await.unwrap();
        server
            .counts
            .reject_shell
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            service.pty_spawn(&base, 80, 24).await.is_err(),
            "a rejected shell must not publish an apparently running PTY"
        );
        server
            .counts
            .reject_shell
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let backend = SshTerminalSessionBackend::for_service(service.clone(), None);
        let (session, lease) = backend
            .open(TerminalOpenRequest {
                target: deepagent_terminal::TerminalTarget::Ssh {
                    connection_id: connection.id.clone(),
                },
                run_id: "fixture-eof".into(),
                cwd: String::new(),
                cols: 80,
                rows: 24,
                initial_holder: TerminalInputHolder::User,
            })
            .await
            .unwrap();
        let status = backend.recovery_status(&session.session_id).await.unwrap();
        assert!(status.available && status.control_available);
        backend
            .write(&session, &lease, b"fixture-exit")
            .await
            .unwrap();
        let mut cursor = 0;
        let mut received = Vec::new();
        loop {
            let chunk = backend.read(&session, cursor).await.unwrap();
            cursor = chunk.cursor;
            received.extend(chunk.data);
            if chunk.eof {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(received, "fixture$ 尾部".as_bytes());
        assert!(
            !backend
                .recovery_status(&session.session_id)
                .await
                .unwrap()
                .available
        );
        backend.close(&session, &lease).await.unwrap();
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .expect("shell refusal/EOF must settle");
}

#[tokio::test]
async fn shared_terminal_close_and_takeover_are_scoped_to_one_pty_not_the_connection() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let connection = server.create(&service, Some(&trust)).await;
        let base = service.connect(&connection.id).await.unwrap();
        let backend = SshTerminalSessionBackend::new(service.clone(), &connection.id, &base.token);
        let open = |run: &str| TerminalOpenRequest {
            target: Default::default(),
            run_id: run.into(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        };
        let (first, first_lease) = backend.open(open("first")).await.unwrap();
        let (second, second_lease) = backend.open(open("second")).await.unwrap();
        assert_ne!(first.session_id, second.session_id);
        let taken = backend
            .takeover(&first, TerminalInputHolder::Runtime)
            .await
            .unwrap();
        assert!(backend.write(&first, &first_lease, b"stale").await.is_err());
        assert!(backend.close(&first, &first_lease).await.is_err());
        assert!(backend
            .write(&second, &taken, b"wrong-scope")
            .await
            .is_err());
        backend.close(&first, &taken).await.unwrap();
        while server
            .counts
            .closed_channels
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }
        assert!(backend.read(&first, 0).await.is_err());
        backend
            .write(&second, &second_lease, b"second survives")
            .await
            .unwrap();
        let mut cursor = 0;
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"second survives") {
            let chunk = backend.read(&second, cursor).await.unwrap();
            cursor = chunk.cursor;
            bytes.extend(chunk.data);
            tokio::task::yield_now().await;
        }
        assert_eq!(bytes, b"fixture$ second survives");
        assert_eq!(
            service.exec(&base, "still-live").await.unwrap().stdout,
            "ok\n"
        );
        assert!(service.read_file(&base, "/missing").await.is_err());
        assert_eq!(server.auth_count(), 1);
        assert!(
            service.pty_read(&base).await.is_err(),
            "legacy connection-only access cannot consume a shared PTY"
        );
        backend.close(&second, &second_lease).await.unwrap();
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .expect("shared PTY close must not disconnect its peers");
}

#[tokio::test]
async fn idle_pty_reads_never_rewrite_connection_configuration() {
    use deepagent_ssh::{SshConfigStore, SshConnectionConfig, SshResult};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Store {
        writes: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SshConfigStore for Store {
        async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
            Ok(vec![])
        }
        async fn save(&self, _: &[SshConnectionConfig]) -> SshResult<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let store = Arc::new(Store::default());
    let service = SshService::with_config_store(data.path().to_path_buf(), store.clone());
    let connection = server.create(&service, Some(&trust)).await;
    let base = service.connect(&connection.id).await.unwrap();
    let pty = service.pty_spawn(&base, 80, 24).await.unwrap();
    store.writes.store(0, Ordering::SeqCst);
    for _ in 0..16 {
        service.pty_read_with_cursor(&pty, 0).await.unwrap();
    }
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    service.disconnect(&connection.id).await.unwrap();
}
