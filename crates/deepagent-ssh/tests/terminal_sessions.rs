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
        let status = backend.recovery_status(&session).await.unwrap();
        assert!(status.available && status.control_available);
        let mut wrong_run = session.clone();
        wrong_run.run_id = "another-run".into();
        assert!(backend.recovery_status(&wrong_run).await.is_err());
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
        assert!(!backend.recovery_status(&session).await.unwrap().available);
        backend.close(&session, &lease).await.unwrap();
        let closed = backend
            .recovery_status(&session)
            .await
            .expect("a closed SSH terminal must remain queryable as unavailable");
        assert!(!closed.available && !closed.control_available);
        assert!(backend.write(&session, &lease, b"closed").await.is_err());
        assert!(backend.renew(&session, &lease).await.is_err());
        assert!(backend
            .takeover(&session, TerminalInputHolder::Runtime)
            .await
            .is_err());
        assert!(backend
            .release(&session, &lease, TerminalInputHolder::Runtime)
            .await
            .is_err());
        let mut missing_generation = session.clone();
        missing_generation.connection_generation = None;
        assert!(backend.recovery_status(&missing_generation).await.is_err());
        let mut malformed = session.clone();
        malformed.session_id = "ssh:missing-token".into();
        assert!(backend.recovery_status(&malformed).await.is_err());
        let mut wrong_backend = session.clone();
        wrong_backend.backend = "direct".into();
        assert!(backend.recovery_status(&wrong_backend).await.is_err());
        let recreated = SshTerminalSessionBackend::for_service(service.clone(), None);
        let absent = recreated.recovery_status(&session).await.unwrap();
        assert!(!absent.available && !absent.control_available);
        assert_eq!(server.auth_count(), 1);
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .expect("shell refusal/EOF must settle");
}

#[tokio::test]
async fn ssh_terminal_session_returns_its_transport_generation() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let service = Arc::new(SshService::new(data.path().to_path_buf()));
    let connection = server.create(&service, Some(&trust)).await;
    let expected_generation = service
        .connect(&connection.id)
        .await
        .unwrap()
        .connection_generation
        .unwrap();
    let backend = SshTerminalSessionBackend::for_service(service.clone(), None);
    let (session, lease) = backend
        .open(TerminalOpenRequest {
            target: deepagent_terminal::TerminalTarget::Ssh {
                connection_id: connection.id.clone(),
            },
            run_id: "terminal-generation".into(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        })
        .await
        .unwrap();

    let wire = serde_json::to_value(&session).unwrap();
    assert_eq!(
        wire.get("connectionGeneration")
            .and_then(serde_json::Value::as_u64),
        Some(expected_generation),
        "the shared terminal session must carry the transport generation to Tauri callers"
    );

    backend.close(&session, &lease).await.unwrap();
    service.disconnect(&connection.id).await.unwrap();
}

#[tokio::test]
async fn stale_transport_generation_cannot_operate_a_replacement_terminal_pty() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let connection = server.create(&service, Some(&trust)).await;
        let backend = SshTerminalSessionBackend::for_service(service.clone(), None);
        let request = || TerminalOpenRequest {
            target: deepagent_terminal::TerminalTarget::Ssh {
                connection_id: connection.id.clone(),
            },
            run_id: "terminal-generation-replacement".into(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        };

        let (old_session, old_lease) = backend.open(request()).await.unwrap();
        let old_generation = old_session.connection_generation.unwrap();
        service.disconnect(&connection.id).await.unwrap();
        let current_transport = service.connect(&connection.id).await.unwrap();
        assert_ne!(
            current_transport.connection_generation,
            Some(old_generation)
        );
        let (current_session, current_lease) = backend.open(request()).await.unwrap();
        assert_eq!(
            current_session.connection_generation,
            current_transport.connection_generation
        );

        let mut stale_session = current_session.clone();
        stale_session.connection_generation = Some(old_generation);
        assert!(
            !backend
                .recovery_status(&old_session)
                .await
                .unwrap()
                .available
        );
        assert!(backend.renew(&stale_session, &current_lease).await.is_err());
        assert!(backend
            .takeover(&stale_session, TerminalInputHolder::Runtime)
            .await
            .is_err());
        assert!(backend
            .release(&stale_session, &current_lease, TerminalInputHolder::Runtime)
            .await
            .is_err());
        assert!(backend.subscribe_output(&stale_session).await.is_err());
        assert!(backend
            .signal(
                &stale_session,
                &current_lease,
                deepagent_terminal::TerminalSignal::Interrupt,
            )
            .await
            .is_err());
        let stale_status = backend.recovery_status(&stale_session).await.unwrap();
        assert!(!stale_status.available && !stale_status.control_available);
        assert!(backend.read(&stale_session, 0).await.is_err());
        assert!(backend
            .write(
                &stale_session,
                &current_lease,
                b"must-not-reach-current-pty"
            )
            .await
            .is_err());
        assert!(backend
            .resize(&stale_session, &current_lease, 140, 60)
            .await
            .is_err());
        assert_eq!(
            *server.counts.pty_size.read().unwrap(),
            Some((80, 24)),
            "a stale resize must not change the replacement PTY"
        );
        assert!(backend.close(&stale_session, &current_lease).await.is_err());

        let mut missing_generation = current_session.clone();
        missing_generation.connection_generation = None;
        assert!(backend.recovery_status(&missing_generation).await.is_err());
        assert!(backend.read(&missing_generation, 0).await.is_err());
        assert!(backend
            .write(&missing_generation, &current_lease, b"missing-generation")
            .await
            .is_err());
        assert!(backend
            .close(&missing_generation, &current_lease)
            .await
            .is_err());

        assert!(
            backend
                .recovery_status(&current_session)
                .await
                .unwrap()
                .available
        );
        let prompt = backend.read(&current_session, 0).await.unwrap();
        assert_eq!(prompt.data, b"fixture$ ");
        backend
            .write(&current_session, &current_lease, b"current-generation-only")
            .await
            .unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let chunk = backend.read(&current_session, prompt.cursor).await.unwrap();
                if !chunk.data.is_empty() {
                    break chunk.data;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("replacement PTY output should remain readable");
        assert_eq!(echoed, b"current-generation-only");

        backend
            .renew(&current_session, &current_lease)
            .await
            .unwrap();
        let runtime_lease = backend
            .release(
                &current_session,
                &current_lease,
                TerminalInputHolder::Runtime,
            )
            .await
            .unwrap();
        assert!(backend
            .renew(&current_session, &current_lease)
            .await
            .is_err());
        backend
            .renew(&current_session, &runtime_lease)
            .await
            .unwrap();
        let user_lease = backend
            .takeover(&current_session, TerminalInputHolder::User)
            .await
            .unwrap();
        backend.close(&current_session, &user_lease).await.unwrap();
        assert_eq!(
            server.auth_count(),
            2,
            "stale operations must not reconnect"
        );
        let _ = backend.close(&old_session, &old_lease).await;
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .expect("terminal generation replacement must settle");
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

#[tokio::test]
async fn output_notifications_are_per_pty_non_destructive_and_include_final_eof() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let connection = server.create(&service, Some(&trust)).await;
        let backend = SshTerminalSessionBackend::for_service(service.clone(), None);
        let request = || TerminalOpenRequest {
            target: deepagent_terminal::TerminalTarget::Ssh {
                connection_id: connection.id.clone(),
            },
            run_id: "fixture-notification".into(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        };
        let (first, first_lease) = backend.open(request()).await.unwrap();
        let (second, second_lease) = backend.open(request()).await.unwrap();
        let mut notice = backend.subscribe_output(&first).await.unwrap();
        let initial = notice.wait_after(0, Duration::from_secs(1)).await;
        assert!(initial.revision > 0);
        let prompt = backend.read(&first, 0).await.unwrap();
        assert_eq!(prompt.data, b"fixture$ ");
        backend
            .write(&second, &second_lease, b"second-only")
            .await
            .unwrap();
        let quiet = notice
            .wait_after(initial.revision, Duration::from_millis(30))
            .await;
        assert_eq!(
            quiet.revision, initial.revision,
            "other PTY cannot wake this one"
        );
        backend
            .write(&first, &first_lease, b"first-only")
            .await
            .unwrap();
        let next = notice
            .wait_after(quiet.revision, Duration::from_secs(1))
            .await;
        assert_ne!(next.revision, quiet.revision);
        let echoed = backend.read(&first, prompt.cursor).await.unwrap();
        assert_eq!(echoed.data, b"first-only");
        assert_eq!(
            backend.read(&first, prompt.cursor).await.unwrap().data,
            echoed.data
        );
        backend
            .write(&first, &first_lease, b"fixture-exit")
            .await
            .unwrap();
        let mut revision = next.revision;
        let mut cursor = echoed.cursor;
        let mut tail = Vec::new();
        loop {
            let changed = notice.wait_after(revision, Duration::from_secs(1)).await;
            revision = changed.revision;
            let chunk = backend.read(&first, cursor).await.unwrap();
            cursor = chunk.cursor;
            tail.extend(chunk.data);
            if chunk.eof {
                break;
            }
        }
        assert_eq!(tail, "尾部".as_bytes());
        let mut forged = first.clone();
        forged.run_id = "another-run".into();
        assert!(backend.subscribe_output(&forged).await.is_err());
        backend.close(&first, &first_lease).await.unwrap();
        assert!(backend.subscribe_output(&first).await.is_err());
        backend.close(&second, &second_lease).await.unwrap();
        service.disconnect(&connection.id).await.unwrap();
        assert_eq!(server.auth_count(), 1);
    })
    .await
    .expect("loopback notification flow must settle");
}
