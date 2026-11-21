mod support;

use std::sync::atomic::Ordering;

use deepagent_ssh::{
    CreateSshConnectionRequest, SshService, SshStatus, UpdateSshConnectionRequest,
};
use support::{random_key, SshServer};

#[tokio::test]
async fn h125_rejects_unknown_before_auth() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let result = service.connect(&connection.id).await;
    if result.is_ok() {
        service.disconnect(&connection.id).await.unwrap();
    }
    assert!(result.is_err(), "unknown key must be rejected");
    assert_eq!(
        server.auth_count(),
        0,
        "no credentials may reach an untrusted server"
    );
    assert!(
        !data.path().join("deepagent-ssh/known_hosts").exists(),
        "no auto-enrollment"
    );
}

#[tokio::test]
async fn h125_matching_key_reuse_test_status_and_restart() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    server.write_trust(&data.path().join("deepagent-ssh/known_hosts"));
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let handle = service.connect(&connection.id).await.unwrap();
    let reused = service.connect(&connection.id).await.unwrap();
    assert_eq!(handle.connection_id, reused.connection_id);
    assert_eq!(
        server.auth_count(),
        1,
        "verified live client should be reused"
    );
    assert_eq!(
        service
            .exec(&handle, "fixture-command")
            .await
            .unwrap()
            .stdout,
        "ok\n"
    );
    assert!(service.test_connection(&connection).await.unwrap().ok);
    service.disconnect(&connection.id).await.unwrap();
    drop(service);
    let restored = SshService::new(data.path().to_path_buf());
    restored.load().await.unwrap();
    let listed = restored.list_connections().await;
    assert_eq!(listed.len(), 1);
    restored.connect(&connection.id).await.unwrap();
    restored.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 3);
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn h125_changed_key_rejected_by_connect_test_and_status_without_auth() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("verified known hosts");
    server.write_trust(&path);
    *server.key.write().unwrap() = random_key();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&path)).await;
    let error = service
        .connect(&connection.id)
        .await
        .expect_err("changed key rejected");
    assert!(error.to_string().contains("host key"), "{error}");
    let test = service.test_connection(&connection).await.unwrap();
    assert!(!test.ok);
    // A freshly restored config has no live client; background refresh must not bypass verification.
    let restored = SshService::new(data.path().to_path_buf());
    restored.load().await.unwrap();
    // Fresh config's checked timestamp is reset in the temporary fixture only.
    let store_path = data.path().join("deepagent-ssh/connections.json");
    let mut configs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&store_path).unwrap()).unwrap();
    configs[&connection.id]["cached_checked_at_ms"] = serde_json::Value::Null;
    std::fs::write(&store_path, serde_json::to_vec(&configs).unwrap()).unwrap();
    restored.load().await.unwrap();
    assert_eq!(restored.refresh_due_statuses().await.unwrap(), 1);
    assert_eq!(
        restored.status(&connection.id).await.unwrap().status,
        SshStatus::Error
    );
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn h125_revocation_blocks_reuse_and_explicit_rotation_restores_connection() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("trusted");
    server.write_trust(&path);
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&path)).await;
    service.connect(&connection.id).await.unwrap();
    std::fs::write(&path, "# trust revoked\n").unwrap();
    assert!(
        service.connect(&connection.id).await.is_err(),
        "removed trust cannot reuse a live client"
    );
    assert_eq!(server.auth_count(), 1);
    *server.key.write().unwrap() = random_key();
    server.write_trust(&path); // Simulates an administrator's independently verified replacement.
    service.connect(&connection.id).await.unwrap();
    service.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 2);
}

#[tokio::test]
async fn h125_update_trust_source_invalidates_live_connection_and_persists() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("trusted");
    let revoked = data.path().join("revoked");
    server.write_trust(&path);
    std::fs::write(&revoked, "# not trusted\n").unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&path)).await;
    service.connect(&connection.id).await.unwrap();
    let request: UpdateSshConnectionRequest = serde_json::from_value(serde_json::json!({
        "id": connection.id, "name": "updated", "host": connection.host, "port": connection.port,
        "username": connection.username, "auth_type": "password", "key_path": null,
        "password": "temporary-fixture-only", "known_hosts_path": revoked,
    }))
    .unwrap();
    service.update_connection(request).await.unwrap();
    assert!(service.connect(&connection.id).await.is_err());
    drop(service);
    let restored = SshService::new(data.path().to_path_buf());
    assert!(restored.connect(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn h125_ipv6_host_port_identity() {
    let server = SshServer::start("::1").await;
    let data = tempfile::tempdir().unwrap();
    server.write_trust(&data.path().join("deepagent-ssh/known_hosts"));
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    service.connect(&connection.id).await.unwrap();
    service.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn h125_key_file_auth_verifies_host_before_offering_identity() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let identity = data.path().join("temporary-fixture-identity");
    let private = random_key()
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    std::fs::write(&identity, private.as_bytes()).unwrap();
    let path = data.path().join("verified");
    let service = SshService::new(data.path().to_path_buf());
    let request: CreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
        "name": "key-file-fixture", "host": "127.0.0.1", "port": server.addr.port(),
        "username": "fixture", "auth_type": "key_file", "key_path": identity,
        "password": null, "known_hosts_path": path,
    }))
    .unwrap();
    let connection = service.create_connection(request).await.unwrap();
    assert!(
        service.connect(&connection.id).await.is_err(),
        "missing trust rejected"
    );
    let server_key = server.key.read().unwrap().clone();
    *server.key.write().unwrap() = random_key();
    server.write_trust(&path);
    *server.key.write().unwrap() = server_key;
    assert!(
        service.connect(&connection.id).await.is_err(),
        "mismatched trust rejected"
    );
    assert_eq!(server.auth_count(), 0);
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    server.write_trust(&path);
    let handle = service.connect(&connection.id).await.unwrap();
    assert_eq!(
        service
            .exec(&handle, "fixture-command")
            .await
            .unwrap()
            .exit_code,
        0
    );
    service.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h125_legacy_and_update_omission_never_restore_no_check() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let custom = data.path().join("verified-custom");
    server.write_trust(&custom);
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&custom)).await;
    let mut update = serde_json::json!({
        "id": connection.id, "name": "updated", "host": connection.host, "port": connection.port,
        "username": connection.username, "auth_type": "password", "key_path": null,
        "password": "temporary-fixture-only",
    });
    let kept = service
        .update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&kept).unwrap()["known_hosts_path"],
        custom.to_str().unwrap()
    );
    update["known_hosts_path"] = serde_json::json!("relative/known_hosts");
    assert!(service
        .update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .is_err());
    update["known_hosts_path"] = serde_json::json!("");
    let reset = service
        .update_connection(serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    let default = data.path().join("deepagent-ssh/known_hosts");
    assert_eq!(
        std::path::Path::new(reset.known_hosts_path.as_deref().unwrap()),
        default
    );
    assert!(
        service.connect(&connection.id).await.is_err(),
        "no system default or custom fallback"
    );
    // Recreate the pre-H125 persisted shape, using only this temporary fixture store.
    let file = data.path().join("deepagent-ssh/connections.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    stored[&connection.id]
        .as_object_mut()
        .unwrap()
        .remove("known_hosts_path");
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    let legacy = SshService::new(data.path().to_path_buf());
    assert!(legacy.connect(&connection.id).await.is_err());
    server.write_trust(&default);
    legacy.connect(&connection.id).await.unwrap();
    legacy.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn h125_nonempty_key_change_invalidates_reuse_and_status() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("deepagent-ssh/known_hosts");
    server.write_trust(&path);
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    service.connect(&connection.id).await.unwrap();
    // Change only trust material, not the server: a live authenticated client must be discarded.
    let key = server.key.read().unwrap().clone();
    *server.key.write().unwrap() = random_key();
    server.write_trust(&path);
    *server.key.write().unwrap() = key;
    assert!(service.connect(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 1);
    server.write_trust(&path);
    service.connect(&connection.id).await.unwrap();
    // Force the existing background entry point due without sleeping or touching user data.
    let file = data.path().join("deepagent-ssh/connections.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    stored[&connection.id]["cached_checked_at_ms"] = serde_json::Value::Null;
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    service.load().await.unwrap();
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
    assert_eq!(
        service.status(&connection.id).await.unwrap().status,
        SshStatus::Connected
    );
    assert_eq!(
        server.auth_count(),
        2,
        "health check reuses the verified client"
    );
    std::fs::write(&path, "# revoked\n").unwrap();
    stored[&connection.id]["cached_checked_at_ms"] = serde_json::Value::Null;
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    service.load().await.unwrap();
    assert_eq!(service.refresh_due_statuses().await.unwrap(), 1);
    assert_eq!(
        service.status(&connection.id).await.unwrap().status,
        SshStatus::Error
    );
    assert_eq!(
        server.auth_count(),
        2,
        "revocation blocks health command and reauthentication"
    );
}

#[tokio::test]
async fn h125_update_cancels_inflight_verified_handshake_then_enforces_revocation() {
    use std::sync::Arc;
    use std::time::Duration;

    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trusted = data.path().join("trusted");
    let revoked = data.path().join("revoked");
    server.write_trust(&trusted);
    std::fs::write(&revoked, "# revoked\n").unwrap();
    let service = Arc::new(SshService::new(data.path().to_path_buf()));
    let connection = server.create(&service, Some(&trusted)).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.auth_gate.write().unwrap() = Some(gate.clone());
    let connecting_service = service.clone();
    let id = connection.id.clone();
    let connecting = tokio::spawn(async move { connecting_service.connect(&id).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.auth_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let request: UpdateSshConnectionRequest = serde_json::from_value(serde_json::json!({
        "id": connection.id, "name": "updated", "host": connection.host, "port": connection.port,
        "username": connection.username, "auth_type": "password", "key_path": null,
        "password": "temporary-fixture-only", "known_hosts_path": revoked,
    }))
    .unwrap();
    let updating_service = service.clone();
    let mut updating =
        tokio::spawn(async move { updating_service.update_connection(request).await });
    let updated = tokio::time::timeout(Duration::from_secs(2), &mut updating).await;
    assert!(
        updated.is_ok(),
        "identity updates must cancel a blocked handshake promptly"
    );
    updated.unwrap().unwrap().unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), connecting)
        .await
        .expect("retired handshake cannot wait for the old server reply")
        .unwrap()
        .is_err());
    gate.notify_one();
    assert_eq!(
        service.status(&connection.id).await.unwrap().status,
        SshStatus::Disconnected
    );
    assert!(service.connect(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 1);
}
