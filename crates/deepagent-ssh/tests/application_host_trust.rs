mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{SshHostIdentity, SshHostKeyStore, SshService};
use russh::keys::HashAlg;
use std::sync::Arc;
use support::SshServer;

#[tokio::test]
async fn persisted_application_trust_authorizes_native_connect_and_survives_restart() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let db_path = data.path().join("app.sqlite");
    let identity =
        SshHostIdentity::direct(&server.addr.ip().to_string(), server.addr.port()).unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&db_path).unwrap(),
    )));
    store
        .trust_verified(
            &identity,
            server.key.read().unwrap().public_key(),
            "independently verified fixture",
        )
        .unwrap();
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, None).await;
    let handle = service
        .connect(&connection.id)
        .await
        .expect("SQLite trust must authorize without a file");
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    service.disconnect(&connection.id).await.unwrap();
    drop(service);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&db_path).unwrap(),
    )));
    let reopened = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    reopened.connect(&connection.id).await.unwrap();
    reopened.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 2);
}

#[tokio::test]
async fn revoked_application_key_overrides_a_matching_known_hosts_file_and_live_client() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let identity =
        SshHostIdentity::direct(&server.addr.ip().to_string(), server.addr.port()).unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let public = server.key.read().unwrap().public_key().clone();
    store.trust_verified(&identity, &public, "fixture").unwrap();
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store.clone());
    let connection = server.create(&service, Some(&path)).await;
    let handle = service.connect(&connection.id).await.unwrap();
    store
        .revoke(&identity, &public.fingerprint(HashAlg::Sha256).to_string())
        .unwrap();
    assert!(service.exec(&handle, "must not execute").await.is_err());
    assert!(!service.test_connection(&connection).await.unwrap().ok);
    assert_eq!(
        server.auth_count(),
        1,
        "revocation must not trigger another credential offer"
    );
    assert_eq!(
        server
            .counts
            .commands
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn import_uses_the_reviewed_snapshot_and_revocation_blocks_reimport() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, Some(&path)).await;
    let preview = service
        .inspect_host_trust(
            &connection.host,
            connection.port,
            connection.known_hosts_path.as_deref(),
        )
        .await
        .unwrap();
    let records = service
        .import_host_trust(&connection.id, preview.clone())
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let handle = service.connect(&connection.id).await.unwrap();
    service
        .revoke_host_key(&connection.id, &records[0].fingerprint)
        .await
        .unwrap();
    assert!(service.exec(&handle, "must not execute").await.is_err());
    assert!(service
        .import_host_trust(&connection.id, preview)
        .await
        .is_err());
    assert!(service.list_host_keys(&connection.id).await.unwrap()[0].revoked);
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn changing_a_file_after_preview_never_imports_the_replacement_key() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, Some(&path)).await;
    let preview = service
        .inspect_host_trust(
            &connection.host,
            connection.port,
            connection.known_hosts_path.as_deref(),
        )
        .await
        .unwrap();
    *server.key.write().unwrap() = support::random_key();
    server.write_trust(&path);
    assert!(service
        .import_host_trust(&connection.id, preview)
        .await
        .is_err());
    assert!(service
        .list_host_keys(&connection.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn application_revocation_during_handshake_stops_before_credentials() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let identity =
        SshHostIdentity::direct(&server.addr.ip().to_string(), server.addr.port()).unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let public = server.key.read().unwrap().public_key().clone();
    store.trust_verified(&identity, &public, "fixture").unwrap();
    let service =
        Arc::new(SshService::new(data.path().to_path_buf()).with_host_key_store(store.clone()));
    let connection = server.create(&service, None).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.accept_gate.write().unwrap() = Some(gate.clone());
    let connecting_service = service.clone();
    let id = connection.id.clone();
    let connecting = tokio::spawn(async move { connecting_service.connect(&id).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while server
            .counts
            .connections
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    store
        .revoke(&identity, &public.fingerprint(HashAlg::Sha256).to_string())
        .unwrap();
    gate.notify_one();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(server.auth_count(), 0);
}
