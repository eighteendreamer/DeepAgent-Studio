mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{SshHostIdentity, SshHostKeyDecision, SshHostKeyStore, SshService};
use russh::keys::HashAlg;
use std::sync::Arc;
use support::SshServer;

#[tokio::test]
async fn explicit_rotation_retires_file_and_app_keys_atomically_without_authenticating() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let original_file = std::fs::read(&path).unwrap();
    let old = server.key.read().unwrap().public_key().clone();
    let db_path = data.path().join("app.sqlite");
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&db_path).unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store.clone());
    let connection = server.create(&service, Some(&path)).await;
    let identity = SshHostIdentity::direct(&connection.host, connection.port).unwrap();
    store.trust_verified(&identity, &old, "fixture").unwrap();
    let alias = server.create(&service, Some(&path)).await;
    service.connect(&alias.id).await.unwrap();
    let auth_before = server.auth_count();
    *server.key.write().unwrap() = support::random_key();
    let new = server.key.read().unwrap().public_key().clone();
    assert!(service.begin_connection(&connection.id).await.is_err());
    let preview = service
        .begin_host_key_rotation(&connection.id)
        .await
        .unwrap();
    assert_eq!(
        preview.previous_keys.len(),
        1,
        "file/app duplicates are one identity"
    );
    assert_eq!(preview.challenge.kind, "key_rotation");
    assert_eq!(
        preview.challenge.fingerprint,
        new.fingerprint(HashAlg::Sha256).to_string()
    );
    assert_eq!(server.auth_count(), auth_before);
    let receipt = service
        .confirm_host_key_rotation(
            &connection.id,
            &preview.challenge.attempt_id,
            &preview.challenge.challenge_id,
        )
        .await
        .unwrap();
    assert_eq!(
        server.auth_count(),
        auth_before,
        "confirmation probes but never authenticates"
    );
    assert_eq!(
        receipt.audit.retired_fingerprints,
        vec![old.fingerprint(HashAlg::Sha256).to_string()]
    );
    assert!(receipt.disconnect_failed_ids.is_empty());
    assert_eq!(
        service
            .list_connections()
            .await
            .into_iter()
            .find(|c| c.id == alias.id)
            .unwrap()
            .status,
        deepagent_ssh::SshStatus::Disconnected
    );
    assert_eq!(std::fs::read(&path).unwrap(), original_file);
    service.connect(&connection.id).await.unwrap();
    service.disconnect(&connection.id).await.unwrap();
    *server.key.write().unwrap() = support::random_key();
    let second = service
        .begin_host_key_rotation(&connection.id)
        .await
        .unwrap();
    service
        .confirm_host_key_rotation(
            &connection.id,
            &second.challenge.attempt_id,
            &second.challenge.challenge_id,
        )
        .await
        .unwrap();
    let reopened = SshHostKeyStore::new(Arc::new(Database::open(&db_path).unwrap()));
    assert_eq!(reopened.rotation_history(&identity).unwrap().len(), 2);
    assert_eq!(
        reopened
            .list(&identity)
            .unwrap()
            .iter()
            .filter(|k| k.revoked)
            .count(),
        2
    );
    let retired = service.list_host_keys(&connection.id).await.unwrap();
    assert_eq!(retired.iter().filter(|k| !k.revoked).count(), 1);
}

#[tokio::test]
async fn rotation_from_revoked_only_state_preserves_the_tombstone() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store.clone());
    let c = server.create(&service, None).await;
    let identity = SshHostIdentity::direct(&c.host, c.port).unwrap();
    let old = server.key.read().unwrap().public_key().clone();
    store.trust_verified(&identity, &old, "fixture").unwrap();
    store
        .revoke(&identity, &old.fingerprint(HashAlg::Sha256).to_string())
        .unwrap();
    *server.key.write().unwrap() = support::random_key();
    assert!(service.begin_connection(&c.id).await.is_err());
    let preview = service.begin_host_key_rotation(&c.id).await.unwrap();
    assert!(preview.previous_keys.is_empty());
    service
        .confirm_host_key_rotation(
            &c.id,
            &preview.challenge.attempt_id,
            &preview.challenge.challenge_id,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .list(&identity)
            .unwrap()
            .iter()
            .filter(|k| k.revoked)
            .count(),
        1
    );
    service.connect(&c.id).await.unwrap();
    service.disconnect(&c.id).await.unwrap();
}

#[tokio::test]
async fn editing_configuration_invalidates_rotation_even_when_endpoint_is_unchanged() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("known_hosts");
    server.write_trust(&file);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let c = server.create(&service, Some(&file)).await;
    *server.key.write().unwrap() = support::random_key();
    let pending = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    service
        .update_connection(
            serde_json::from_value(serde_json::json!({
                "id":c.id,"name":"edited","host":c.host,"port":c.port,"username":c.username,
                "auth_type":"password","password":"temporary-fixture-only","known_hosts_path":file
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(service
        .confirm_host_key_rotation(&c.id, &pending.attempt_id, &pending.challenge_id)
        .await
        .is_err());
    assert!(service.list_host_keys(&c.id).await.unwrap().is_empty());
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn rotation_requires_existing_identity_rejects_unchanged_and_never_revives_revoked() {
    let server = SshServer::start("127.0.0.1").await;
    let old_private = server.key.read().unwrap().clone();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store.clone());
    let c = server.create(&service, None).await;
    assert!(
        service.begin_host_key_rotation(&c.id).await.is_err(),
        "unknown uses enrollment, not rotation"
    );
    let identity = SshHostIdentity::direct(&c.host, c.port).unwrap();
    store
        .trust_verified(&identity, old_private.public_key(), "fixture")
        .unwrap();
    assert!(
        service.begin_host_key_rotation(&c.id).await.is_err(),
        "unchanged is not a rotation"
    );
    *server.key.write().unwrap() = support::random_key();
    let preview = service.begin_host_key_rotation(&c.id).await.unwrap();
    assert!(
        service
            .respond_host_key(
                &c.id,
                &preview.challenge.attempt_id,
                &preview.challenge.challenge_id,
                SshHostKeyDecision::Save
            )
            .await
            .is_err(),
        "ordinary confirmation must not process rotation"
    );
    let preview = service.begin_host_key_rotation(&c.id).await.unwrap();
    service
        .confirm_host_key_rotation(
            &c.id,
            &preview.challenge.attempt_id,
            &preview.challenge.challenge_id,
        )
        .await
        .unwrap();
    *server.key.write().unwrap() = old_private;
    assert!(service.connect(&c.id).await.is_err());
    assert!(service.begin_host_key_rotation(&c.id).await.is_err());
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn rotation_is_single_use_scoped_expiring_and_reprobes_before_commit() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let c = server.create(&service, Some(&path)).await;
    let alias = server.create(&service, Some(&path)).await;
    *server.key.write().unwrap() = support::random_key();
    let old = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    let current = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    assert!(service
        .confirm_host_key_rotation(&c.id, &old.attempt_id, &old.challenge_id)
        .await
        .is_err());
    assert!(service
        .confirm_host_key_rotation(&alias.id, &current.attempt_id, &current.challenge_id)
        .await
        .is_err());
    *server.key.write().unwrap() = support::random_key();
    assert!(service
        .confirm_host_key_rotation(&c.id, &current.attempt_id, &current.challenge_id)
        .await
        .is_err());
    assert!(service
        .confirm_host_key_rotation(&c.id, &current.attempt_id, &current.challenge_id)
        .await
        .is_err());
    let pending = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    std::fs::write(&path, "# changed source\n").unwrap();
    assert!(service
        .confirm_host_key_rotation(&c.id, &pending.attempt_id, &pending.challenge_id)
        .await
        .is_err());
    server.write_trust(&path);
    *server.key.write().unwrap() = support::random_key();
    let pending = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    assert!(service
        .confirm_host_key_rotation(&c.id, &pending.attempt_id, &pending.challenge_id)
        .await
        .is_err());
    tokio::time::resume();
    let pending = service
        .begin_host_key_rotation(&c.id)
        .await
        .unwrap()
        .challenge;
    service
        .cancel_host_key(&c.id, &pending.attempt_id, &pending.challenge_id)
        .unwrap();
    assert!(service
        .confirm_host_key_rotation(&c.id, &pending.attempt_id, &pending.challenge_id)
        .await
        .is_err());
    assert!(service.list_host_keys(&c.id).await.unwrap().is_empty());
    assert_eq!(server.auth_count(), 0);
}
