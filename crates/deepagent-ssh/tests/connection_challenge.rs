mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{
    SshConnectionAttempt, SshHostKeyChallenge, SshHostKeyDecision, SshHostKeyStore, SshService,
};
use std::sync::Arc;
use support::SshServer;

async fn challenge(service: &SshService, id: &str) -> SshHostKeyChallenge {
    match service.begin_connection(id).await.unwrap() {
        SshConnectionAttempt::ConfirmationRequired { challenge } => *challenge,
        SshConnectionAttempt::Connected => {
            panic!("unknown host must not connect before a decision")
        }
    }
}

#[tokio::test]
async fn unknown_host_has_no_auth_and_once_trust_lasts_only_for_the_live_connection() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, None).await;
    assert!(
        service.connect(&connection.id).await.is_err(),
        "passive Strict never enrolls"
    );
    let pending = challenge(&service, &connection.id).await;
    assert_eq!(pending.host, connection.host);
    assert_eq!(pending.port, connection.port);
    assert_eq!(pending.algorithm, "ssh-ed25519");
    assert!(pending.fingerprint.starts_with("SHA256:"));
    assert_eq!(server.auth_count(), 0);
    let handle = service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            &pending.challenge_id,
            SshHostKeyDecision::Once,
        )
        .await
        .unwrap();
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    assert_eq!(server.auth_count(), 1);
    assert!(service
        .list_host_keys(&connection.id)
        .await
        .unwrap()
        .is_empty());
    assert!(
        service
            .respond_host_key(
                &connection.id,
                &pending.attempt_id,
                &pending.challenge_id,
                SshHostKeyDecision::Once
            )
            .await
            .is_err(),
        "response is single use"
    );
    service.disconnect(&connection.id).await.unwrap();
    assert!(
        service.connect(&connection.id).await.is_err(),
        "Once never authorizes reconnection"
    );
}

#[tokio::test]
async fn confirmed_save_survives_restart_but_changed_keys_are_not_confirmable() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("app.sqlite");
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&path).unwrap(),
    )));
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, None).await;
    let pending = challenge(&service, &connection.id).await;
    service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            &pending.challenge_id,
            SshHostKeyDecision::Save,
        )
        .await
        .unwrap();
    service.disconnect(&connection.id).await.unwrap();
    drop(service);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&path).unwrap(),
    )));
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    assert!(matches!(
        service.begin_connection(&connection.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    service.disconnect(&connection.id).await.unwrap();
    *server.key.write().unwrap() = support::random_key();
    assert!(
        service.begin_connection(&connection.id).await.is_err(),
        "changed is not unknown"
    );
    assert_eq!(server.auth_count(), 2);
}

#[tokio::test]
async fn cancellation_superseding_attempt_and_expiry_never_offer_credentials() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let old = challenge(&service, &connection.id).await;
    let current = challenge(&service, &connection.id).await;
    assert!(service
        .respond_host_key(
            &connection.id,
            &old.attempt_id,
            &old.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    service
        .cancel_host_key(&connection.id, &current.attempt_id, &current.challenge_id)
        .unwrap();
    assert!(service
        .respond_host_key(
            &connection.id,
            &current.attempt_id,
            &current.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    let expired = challenge(&service, &connection.id).await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    assert!(service
        .respond_host_key(
            &connection.id,
            &expired.attempt_id,
            &expired.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    tokio::time::resume();
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn server_rotation_after_the_prompt_rejects_before_authentication() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let pending = challenge(&service, &connection.id).await;
    *server.key.write().unwrap() = support::random_key();
    assert!(service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            &pending.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn changed_or_revoked_known_keys_never_create_an_unknown_host_challenge() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let public = server.key.read().unwrap().public_key().clone();
    let identity =
        deepagent_ssh::SshHostIdentity::direct(&server.addr.ip().to_string(), server.addr.port())
            .unwrap();
    store.trust_verified(&identity, &public, "fixture").unwrap();
    store
        .revoke(
            &identity,
            &public.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
        )
        .unwrap();
    let service = SshService::new(data.path().to_path_buf()).with_host_key_store(store);
    let connection = server.create(&service, Some(&path)).await;
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 0);
    assert_eq!(
        server
            .counts
            .public_key_offers
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn config_updates_and_file_changes_invalidate_an_existing_prompt() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let pending = challenge(&service, &connection.id).await;
    // Even an update back to the same identity invalidates the old attempt.
    service.update_connection(serde_json::from_value(serde_json::json!({
        "id":connection.id,"name":connection.name,"host":connection.host,"port":connection.port,
        "username":connection.username,"auth_type":"password","password":"temporary-fixture-only"
    })).unwrap()).await.unwrap();
    assert!(service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            &pending.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    let pending = challenge(&service, &connection.id).await;
    server.write_trust(std::path::Path::new(
        connection.known_hosts_path.as_deref().unwrap(),
    ));
    assert!(service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            &pending.challenge_id,
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn mismatched_response_ids_do_not_consume_the_current_challenge() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let pending = challenge(&service, &connection.id).await;
    assert!(!pending.can_save);
    assert!(service
        .cancel_host_key("other-profile", &pending.attempt_id, &pending.challenge_id)
        .is_err());
    assert!(service
        .respond_host_key(
            &connection.id,
            &pending.attempt_id,
            "wrong-challenge",
            SshHostKeyDecision::Once
        )
        .await
        .is_err());
    service
        .cancel_host_key(&connection.id, &pending.attempt_id, &pending.challenge_id)
        .unwrap();
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn a_failed_new_attempt_still_invalidates_the_previous_prompt() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let pending = challenge(&service, &connection.id).await;
    let path = std::path::Path::new(connection.known_hosts_path.as_deref().unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, [0xff, 0xfe]).unwrap();
    assert!(service.begin_connection(&connection.id).await.is_err());
    std::fs::remove_file(path).unwrap(); // Restore the exact original empty trust source.
    assert!(
        service
            .respond_host_key(
                &connection.id,
                &pending.attempt_id,
                &pending.challenge_id,
                SshHostKeyDecision::Once
            )
            .await
            .is_err(),
        "a failed newer attempt must not revive the previous decision"
    );
    assert_eq!(server.auth_count(), 0);
}
