mod support;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use deepagent_ssh::{
    CreateSshConnectionRequest, SshAuthenticationChallenge, SshConfigStore, SshConnectionAttempt,
    SshConnectionConfig, SshHostKeyDecision, SshResult, SshService, SshStatus,
    UpdateSshConnectionRequest,
};
use serde_json::json;

#[derive(Default)]
struct MemoryStore(Mutex<Vec<SshConnectionConfig>>);

#[async_trait::async_trait]
impl SshConfigStore for MemoryStore {
    fn supports_secret_storage(&self) -> bool {
        true
    }
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()> {
        *self.0.lock().unwrap() = configs.to_vec();
        Ok(())
    }
}

fn challenge(attempt: SshConnectionAttempt) -> SshAuthenticationChallenge {
    match attempt {
        SshConnectionAttempt::AuthenticationRequired { challenge } => *challenge,
        other => panic!("expected a private-key unlock challenge, received {other:?}"),
    }
}

async fn locked_file() -> (support::SshServer, tempfile::TempDir, SshService, String) {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    support::random_key()
        .encrypt(&mut rand::rng(), "fixture-unlock-only")
        .unwrap()
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    let ssh = SshService::new(root.path().to_owned());
    let connection = ssh
        .create_connection(serde_json::from_value(json!({
            "name":"locked fixture", "host":"127.0.0.1", "port":server.addr.port(),
            "username":"fixture", "auth_type":"key_file", "key_path":root.path().join("identity"),
            "known_hosts_path":root.path().join("known_hosts"),
        })).unwrap())
        .await
        .unwrap();
    (server, root, ssh, connection.id)
}

#[tokio::test]
async fn unsaved_passphrase_unlocks_only_this_connection_attempt_and_never_reaches_the_server() {
    let (server, root, ssh, id) = locked_file().await;
    let unlock = challenge(ssh.begin_connection(&id).await.unwrap());
    assert_eq!(
        serde_json::to_value(&unlock).unwrap()["kind"],
        "private_key"
    );
    assert_eq!(unlock.prompts.len(), 1);
    assert!(!unlock.prompts[0].echo);
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert!(matches!(
        ssh.respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-unlock-only".into()],
        )
        .await
        .unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    let persisted =
        std::fs::read_to_string(root.path().join("deepagent-ssh/connections.json")).unwrap();
    assert!(!persisted.contains("fixture-unlock-only"));
    ssh.disconnect(&id).await.unwrap();
    let next = challenge(ssh.begin_connection(&id).await.unwrap());
    assert_ne!(next.attempt_id, unlock.attempt_id);
    ssh.cancel_connection_attempt(&id, &next.attempt_id, &next.challenge_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelled_unlock_rejects_late_answers_without_offering_an_identity() {
    let (server, _root, ssh, id) = locked_file().await;
    let unlock = challenge(ssh.begin_connection(&id).await.unwrap());
    ssh.cancel_connection_attempt(&id, &unlock.attempt_id, &unlock.challenge_id)
        .await
        .unwrap();
    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-unlock-only".into()],
        )
        .await
        .is_err());
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert_eq!(
        ssh.status(&id).await.unwrap().status,
        SshStatus::Disconnected
    );
}

#[tokio::test]
async fn incorrect_unlock_closes_the_attempt_without_exposing_the_passphrase() {
    let (server, _root, ssh, id) = locked_file().await;
    let unlock = challenge(ssh.begin_connection(&id).await.unwrap());
    let error = ssh
        .respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-wrong-only".into()],
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("fixture-wrong-only"));
    assert!(!error.contains("fixture-unlock-only"));
    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-unlock-only".into()],
        )
        .await
        .is_err());
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn trust_change_and_expiry_each_fence_pending_local_unlocks() {
    for expire in [false, true] {
        let (server, root, ssh, id) = locked_file().await;
        let unlock = challenge(ssh.begin_connection(&id).await.unwrap());
        if expire {
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(121)).await;
            tokio::task::yield_now().await;
        } else {
            *server.key.write().unwrap() = support::random_key();
            server.write_trust(&root.path().join("known_hosts"));
        }
        assert!(ssh
            .respond_keyboard_interactive(
                &id,
                &unlock.attempt_id,
                &unlock.challenge_id,
                vec!["fixture-unlock-only".into()],
            )
            .await
            .is_err());
        assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
        assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn host_confirmation_precedes_local_unlock_and_preserves_the_attempt_identity() {
    let (server, root, ssh, id) = locked_file().await;
    std::fs::remove_file(root.path().join("known_hosts")).unwrap();
    let host = match ssh.begin_connection(&id).await.unwrap() {
        SshConnectionAttempt::ConfirmationRequired { challenge } => *challenge,
        other => panic!("expected host-key confirmation, received {other:?}"),
    };
    let unlock = challenge(
        ssh.respond_host_key(
            &id,
            &host.attempt_id,
            &host.challenge_id,
            SshHostKeyDecision::Once,
        )
        .await
        .unwrap(),
    );
    assert_eq!(unlock.attempt_id, host.attempt_id);
    assert_eq!(server.auth_count(), 0);
    ssh.cancel_connection_attempt(&id, &unlock.attempt_id, &unlock.challenge_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn superseding_or_editing_a_pending_unlock_invalidates_its_response() {
    for edit in [false, true] {
        let (server, _root, ssh, id) = locked_file().await;
        let first = challenge(ssh.begin_connection(&id).await.unwrap());
        if edit {
            let mut update = serde_json::to_value(&ssh.list_connections().await[0]).unwrap();
            update["name"] = json!("updated identity generation");
            ssh.update_connection(serde_json::from_value(update).unwrap())
                .await
                .unwrap();
        } else {
            let next = challenge(ssh.begin_connection(&id).await.unwrap());
            assert_ne!(next.attempt_id, first.attempt_id);
            ssh.cancel_connection_attempt(&id, &next.attempt_id, &next.challenge_id)
                .await
                .unwrap();
        }
        assert!(ssh
            .respond_keyboard_interactive(
                &id,
                &first.attempt_id,
                &first.challenge_id,
                vec!["fixture-unlock-only".into()]
            )
            .await
            .is_err());
        assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn local_unlock_continues_into_server_mfa_without_reusing_the_passphrase_as_a_response() {
    let (server, _root, ssh, id) = locked_file().await;
    server
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    let unlock = challenge(ssh.begin_connection(&id).await.unwrap());
    let first = challenge(
        ssh.respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-unlock-only".into()],
        )
        .await
        .unwrap(),
    );
    assert_eq!(first.attempt_id, unlock.attempt_id);
    assert_ne!(first.challenge_id, unlock.challenge_id);
    assert_eq!(
        serde_json::to_value(&first).unwrap()["kind"],
        "keyboard_interactive"
    );
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        0
    );
    let second = challenge(
        ssh.respond_keyboard_interactive(
            &id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .unwrap(),
    );
    assert!(matches!(
        ssh.respond_keyboard_interactive(
            &id,
            &second.attempt_id,
            &second.challenge_id,
            vec!["731604".into()],
        )
        .await
        .unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        2
    );
}

fn content_request(
    server: &support::SshServer,
    root: &std::path::Path,
    content: &str,
) -> CreateSshConnectionRequest {
    serde_json::from_value(json!({
        "name":"content fixture", "host":"127.0.0.1", "port":server.addr.port(),
        "username":"fixture", "auth_type":"key_content", "key_content":content,
        "known_hosts_path":root.join("known_hosts"),
    }))
    .expect("private-key content must be a supported explicit identity source")
}

#[tokio::test]
async fn private_key_content_authenticates_without_a_local_private_key_file_or_secret_dto() {
    for encrypted in [false, true] {
        let server = support::SshServer::start("127.0.0.1").await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let key = support::random_key();
        let key = if encrypted {
            key.encrypt(&mut rand::rng(), "fixture-unlock-only")
                .unwrap()
        } else {
            key
        };
        let body = key.to_openssh(Default::default()).unwrap();
        let store = Arc::new(MemoryStore::default());
        let ssh = SshService::with_config_store(root.path().to_owned(), store.clone());
        let connection = ssh
            .create_connection(content_request(&server, root.path(), &body))
            .await
            .unwrap();
        let wire = serde_json::to_string(&connection).unwrap();
        assert!(!wire.contains("PRIVATE KEY"));
        assert!(serde_json::to_value(&connection)
            .unwrap()
            .get("key_content")
            .is_none());
        assert!(connection.key_path.is_none());
        let attempt = ssh.begin_connection(&connection.id).await.unwrap();
        let attempt = if encrypted {
            let unlock = challenge(attempt);
            ssh.respond_keyboard_interactive(
                &connection.id,
                &unlock.attempt_id,
                &unlock.challenge_id,
                vec!["fixture-unlock-only".into()],
            )
            .await
            .unwrap()
        } else {
            attempt
        };
        assert!(matches!(attempt, SshConnectionAttempt::Connected));
        assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
        assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
        assert!(!format!("{:?}", store.0.lock().unwrap()).contains("PRIVATE KEY"));
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "no plaintext identity file may be materialized"
        );
    }
}

#[tokio::test]
async fn plaintext_store_rejects_private_key_content_before_inserting_or_writing_any_config() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let body = support::random_key()
        .to_openssh(Default::default())
        .unwrap();
    let ssh = SshService::new(root.path().to_owned());
    let error = ssh
        .create_connection(content_request(&server, root.path(), &body))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("secure") || error.contains("encrypted"));
    assert!(!error.contains("PRIVATE KEY"));
    assert!(ssh.list_connections().await.is_empty());
    assert!(!root.path().join("deepagent-ssh/connections.json").exists());
}

#[tokio::test]
async fn plaintext_store_rejects_loaded_private_key_content_before_publishing_any_connection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("deepagent-ssh/connections.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut config = SshConnectionConfig::new(
        "injected plaintext identity",
        "fixture.invalid",
        22,
        "fixture",
        deepagent_ssh::SshAuthType::KeyContent,
    );
    config.key_content = Some("fixture-private-content-must-not-be-rewritten".into());
    let bytes = serde_json::to_vec(&json!({ config.id.clone(): config })).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let ssh = SshService::new(root.path().to_owned());

    let error = ssh.load().await.unwrap_err().to_string();
    assert!(error.contains("secure") || error.contains("encrypted"));
    assert!(!error.contains("fixture-private-content"));
    assert!(ssh.list_connections().await.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn content_edit_preserves_an_omitted_secret_and_switching_to_agent_erases_it() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let body = support::random_key()
        .to_openssh(Default::default())
        .unwrap();
    let store = Arc::new(MemoryStore::default());
    let ssh = SshService::with_config_store(root.path().to_owned(), store.clone());
    let created = ssh
        .create_connection(content_request(&server, root.path(), &body))
        .await
        .unwrap();
    let mut update = serde_json::to_value(&created).unwrap();
    update["name"] = json!("renamed without reading back the private key");
    ssh.update_connection(
        serde_json::from_value::<UpdateSshConnectionRequest>(update.clone()).unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        ssh.begin_connection(&created.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    update["auth_type"] = json!("agent");
    ssh.update_connection(serde_json::from_value::<UpdateSshConnectionRequest>(update).unwrap())
        .await
        .unwrap();
    assert!(serde_json::to_value(&store.0.lock().unwrap()[0]).unwrap()["key_content"].is_null());
}
