mod support;

use deepagent_ssh::{SshError, SshService, SshStatus};
use std::{sync::atomic::Ordering, time::Duration};
use support::SshServer;

async fn advance(seconds: u64) {
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(seconds)).await;
    tokio::time::resume();
}

async fn fixture() -> (SshServer, tempfile::TempDir, SshService, String) {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let service = SshService::new(data.path().to_path_buf());
    let config = server.create(&service, Some(&trust)).await;
    (server, data, service, config.id)
}

async fn detect_disconnect(server: &SshServer, service: &SshService, id: &str) {
    server.disconnect_clients().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.status(id).await.unwrap().status == SshStatus::Connected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the server-side disconnect should reach the client");
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
    assert_eq!(service.status(id).await.unwrap().status, SshStatus::Error);
}

#[tokio::test]
async fn generation_bound_handle_revalidates_host_trust_before_exec() {
    let (server, data, service, id) = fixture().await;
    let handle = service.connect(&id).await.unwrap();
    std::fs::write(data.path().join("known_hosts"), "# revoked\n").unwrap();
    let result = service.exec(&handle, "must-not-run-after-revocation").await;
    assert!(
        result.is_err(),
        "a generation must not bypass host trust: {result:?}"
    );
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 0);
    assert_eq!(server.auth_count(), 1);
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn identity_reload_does_not_inherit_the_old_reconnect_backoff() {
    let (server, data, service, id) = fixture().await;
    service.connect(&id).await.unwrap();
    detect_disconnect(&server, &service, &id).await;
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);

    let path = data.path().join("deepagent-ssh/connections.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored[&id]["username"] = serde_json::json!("replacement-user");
    stored[&id]["cached_checked_at_ms"] = serde_json::Value::Null;
    std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
    service.load().await.unwrap();
    assert_eq!(
        service.refresh_due_statuses().await.unwrap(),
        1,
        "the retired identity's retry deadline must not delay a replacement identity"
    );
    assert_eq!(server.auth_count(), 2);
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn failed_recovery_increases_backoff_and_explicit_connect_resets_it() {
    tokio::time::timeout(Duration::from_secs(40), async {
        let (server, _data, service, id) = fixture().await;
        service.connect(&id).await.unwrap();
        detect_disconnect(&server, &service, &id).await;
        server.counts.reject_password.store(true, Ordering::SeqCst);
        advance(5).await;
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
        assert_eq!(server.auth_count(), 2);
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);
        advance(5).await;
        assert_eq!(
            service.refresh_due_statuses().await.unwrap(),
            0,
            "the second delay is ten seconds"
        );
        advance(5).await;
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
        assert_eq!(server.auth_count(), 3);

        server.counts.reject_password.store(false, Ordering::SeqCst);
        let current = service
            .connect(&id)
            .await
            .expect("explicit user connect can retry immediately");
        assert_eq!(server.auth_count(), 4);
        detect_disconnect(&server, &service, &id).await;
        advance(5).await;
        assert_eq!(
            service.refresh_due_statuses().await.unwrap(),
            1,
            "success resets the delay to five seconds"
        );
        assert_eq!(server.auth_count(), 5);
        assert!(service.exec(&current, "must-not-replay").await.is_err());
        service.disconnect(&id).await.unwrap();
    })
    .await
    .expect("backoff lifecycle must settle promptly");
}

#[tokio::test]
async fn explicit_disconnect_cancels_scheduled_recovery() {
    let (server, _data, service, id) = fixture().await;
    let stale = service.connect(&id).await.unwrap();
    detect_disconnect(&server, &service, &id).await;
    service.disconnect(&id).await.unwrap();
    advance(5).await;
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);
    assert_eq!(
        service.status(&id).await.unwrap().status,
        SshStatus::Disconnected
    );
    assert!(service.exec(&stale, "must-not-reconnect").await.is_err());
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn recovery_does_not_restore_old_ptys_or_allow_stale_file_mutations() {
    let (server, _data, service, id) = fixture().await;
    let stale = service.connect(&id).await.unwrap();
    let old_pty = service.pty_spawn(&stale, 80, 24).await.unwrap();
    detect_disconnect(&server, &service, &id).await;
    advance(5).await;
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
    let current = service.connect(&id).await.unwrap();
    let new_pty = service.pty_spawn(&current, 80, 24).await.unwrap();

    assert!(service
        .pty_write(&old_pty, b"must-not-replay\n")
        .await
        .is_err());
    assert!(service.pty_read(&old_pty).await.is_err());
    assert!(service.pty_resize(&old_pty, 120, 40).await.is_err());
    assert!(service.pty_close(&old_pty).await.is_err());
    assert!(service.create_file(&stale, "/stale-file").await.is_err());
    assert!(service.create_dir(&stale, "/stale-dir").await.is_err());
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/stale-file"));
    assert!(!server
        .counts
        .directories
        .read()
        .unwrap()
        .contains("/stale-dir"));

    service.pty_write(&new_pty, b"current-pty\n").await.unwrap();
    service.pty_close(&new_pty).await.unwrap();
    assert_eq!(
        server.auth_count(),
        2,
        "old handles must not create extra transports"
    );
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn disconnect_cancels_a_recovery_handshake_before_late_publication() {
    let (server, _data, service, id) = fixture().await;
    let service = std::sync::Arc::new(service);
    service.connect(&id).await.unwrap();
    detect_disconnect(&server, &service, &id).await;
    let gate = std::sync::Arc::new(tokio::sync::Notify::new());
    *server.counts.auth_gate.write().unwrap() = Some(gate.clone());
    advance(5).await;
    let recovering = {
        let service = service.clone();
        tokio::spawn(async move { service.refresh_due_statuses().await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.auth_count() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recovery must reach the real handshake");
    tokio::time::timeout(Duration::from_secs(2), service.disconnect(&id))
        .await
        .expect("disconnect must cancel pending recovery")
        .unwrap();
    *server.counts.auth_gate.write().unwrap() = None;
    gate.notify_one();
    recovering.await.unwrap().unwrap();
    assert_eq!(
        service.status(&id).await.unwrap().status,
        SshStatus::Disconnected
    );
    advance(5).await;
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);
    assert_eq!(server.auth_count(), 2);
}

#[tokio::test]
async fn stale_handle_cannot_execute_on_a_replacement_transport() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let config = server.create(&service, Some(&trust)).await;

        let stale = service.connect(&config.id).await.unwrap();
        service.disconnect(&config.id).await.unwrap();
        let current = service.connect(&config.id).await.unwrap();
        let commands_before_stale_use = server.counts.commands.load(Ordering::SeqCst);

        assert!(matches!(
            service.exec(&stale, "must-not-run-on-replacement").await,
            Err(SshError::ConnectionLost(_))
        ));
        assert_eq!(
            server.counts.commands.load(Ordering::SeqCst),
            commands_before_stale_use,
            "stale handles must not send commands through a newer transport"
        );
        assert_eq!(server.auth_count(), 2, "stale use must not reconnect");
        assert_eq!(
            service
                .exec(&current, "current-generation")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await
    .expect("generation fencing loopback case must settle promptly");
}

#[tokio::test]
async fn failed_healthcheck_rebuilds_after_backoff_without_replaying_old_handle() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let config = server.create(&service, Some(&trust)).await;

        let stale = service.connect(&config.id).await.unwrap();
        let first_generation = stale.connection_generation.unwrap();
        let commands_before_disconnect = server.counts.commands.load(Ordering::SeqCst);
        server.disconnect_clients().await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.status(&config.id).await.unwrap().status == SshStatus::Connected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the server-side disconnect should reach the client");

        assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
        assert_eq!(
            server.counts.commands.load(Ordering::SeqCst),
            commands_before_disconnect,
            "the closed transport must not receive another command"
        );
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Error
        );
        assert_eq!(server.auth_count(), 1, "the first failure starts backoff");
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);
        assert_eq!(
            server.auth_count(),
            1,
            "backoff suppresses an immediate retry"
        );

        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
        let current = service.connect(&config.id).await.unwrap();
        assert_ne!(current.connection_generation, Some(first_generation));
        assert_eq!(
            server.auth_count(),
            2,
            "health rebuild publishes one transport"
        );

        let commands_before_stale_use = server.counts.commands.load(Ordering::SeqCst);
        assert!(matches!(
            service
                .exec(&stale, "must-not-run-after-health-rebuild")
                .await,
            Err(SshError::ConnectionLost(_))
        ));
        assert_eq!(
            server.counts.commands.load(Ordering::SeqCst),
            commands_before_stale_use,
            "a stale handle cannot execute on the rebuilt transport"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await
    .expect("health reconnect loopback case must settle promptly");
}
