mod support;

use deepagent_ssh::{
    SshAuthenticationKind, SshConfigStore, SshConnectionAttempt, SshConnectionConfig, SshResult,
    SshService,
};
use russh::MethodKind;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct SecretStore(Mutex<Vec<SshConnectionConfig>>);

#[async_trait::async_trait]
impl SshConfigStore for SecretStore {
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

fn request(server: &support::SshServer, root: &std::path::Path, factors: Value) -> Value {
    json!({"name":"explicit chain fixture", "host":"127.0.0.1", "port":server.addr.port(),
        "username":"fixture", "auth_type":"chain", "authentication_factors":factors,
        "known_hosts_path":root.join("known_hosts")})
}

fn password(value: &str) -> Value {
    json!({"auth_type":"password", "password":value})
}

fn private_key(
    server: &support::SshServer,
    stage: usize,
    passphrase: Option<&str>,
    save: bool,
) -> Value {
    let key = support::random_key();
    server
        .counts
        .factor_keys
        .write()
        .unwrap()
        .insert(stage, key.public_key().fingerprint(Default::default()));
    let key = match passphrase {
        Some(secret) => key.encrypt(&mut rand::rng(), secret).unwrap(),
        None => key,
    };
    let mut factor = json!({"auth_type":"key_content", "key_content":key.to_openssh(Default::default()).unwrap().to_string()});
    if save {
        factor["passphrase"] = json!(passphrase);
    }
    factor
}

async fn create(ssh: &SshService, wire: Value) -> deepagent_ssh::SshConnectionDto {
    let request =
        serde_json::from_value(wire).expect("explicit authentication chains must deserialize");
    ssh.create_connection(request).await.unwrap()
}

#[tokio::test]
async fn password_and_key_follow_both_server_orders_on_one_connection() {
    for password_first in [true, false] {
        let server = support::SshServer::start("127.0.0.1").await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let methods = if password_first {
            vec![MethodKind::Password, MethodKind::PublicKey]
        } else {
            vec![MethodKind::PublicKey, MethodKind::Password]
        };
        *server.counts.authentication_factors.write().unwrap() = methods;
        server
            .counts
            .factor_passwords
            .write()
            .unwrap()
            .insert(usize::from(!password_first), "chain-password-only".into());
        let key = private_key(
            &server,
            usize::from(password_first),
            Some("chain-key-unlock-only"),
            true,
        );
        let store = Arc::new(SecretStore::default());
        let ssh = SshService::with_config_store(root.path().into(), store);
        let saved = create(
            &ssh,
            request(
                &server,
                root.path(),
                json!([password("chain-password-only"), key]),
            ),
        )
        .await;
        assert!(matches!(
            ssh.begin_connection(&saved.id).await.unwrap(),
            SshConnectionAttempt::Connected
        ));
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
        assert_eq!(
            *server.counts.received_passwords.read().unwrap(),
            ["chain-password-only"]
        );
        assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn same_method_candidates_are_distinct_and_partial_success_never_reuses_an_accepted_password()
{
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![MethodKind::Password; 2];
    *server.counts.factor_passwords.write().unwrap() = [
        (0, "chain-first-only".into()),
        (1, "chain-second-only".into()),
    ]
    .into();
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!([
                password("chain-rejected-only"),
                password("chain-first-only"),
                password("chain-second-only")
            ]),
        ),
    )
    .await;
    assert!(matches!(
        ssh.begin_connection(&saved.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(
        *server.counts.received_passwords.read().unwrap(),
        [
            "chain-rejected-only",
            "chain-first-only",
            "chain-rejected-only",
            "chain-second-only"
        ]
    );
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn distinct_public_keys_are_signed_for_the_correct_stage() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![MethodKind::PublicKey; 2];
    let rejected = private_key(&server, 9, None, false);
    let first = private_key(&server, 0, None, false);
    let second = private_key(&server, 1, None, false);
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(&server, root.path(), json!([rejected, first, second])),
    )
    .await;
    assert!(matches!(
        ssh.begin_connection(&saved.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    let expected = server.counts.factor_keys.read().unwrap();
    assert_eq!(
        *server.counts.received_keys.read().unwrap(),
        [
            expected[&9].clone(),
            expected[&0].clone(),
            expected[&9].clone(),
            expected[&1].clone()
        ]
    );
}

#[tokio::test]
async fn a_chain_does_not_invent_a_second_factor_by_reusing_its_only_password() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![MethodKind::Password; 2];
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(&server, root.path(), json!([password("chain-only-one")])),
    )
    .await;
    assert!(ssh.begin_connection(&saved.id).await.is_err());
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 1);
}

async fn locked_second_key() -> (
    support::SshServer,
    tempfile::TempDir,
    SshService,
    Arc<SecretStore>,
    String,
    deepagent_ssh::SshAuthenticationChallenge,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![
        MethodKind::PublicKey,
        MethodKind::PublicKey,
        MethodKind::KeyboardInteractive,
    ];
    let first = private_key(&server, 0, Some("chain-first-key-only"), true);
    let second = private_key(&server, 1, Some("chain-second-key-only"), false);
    let store = Arc::new(SecretStore::default());
    let ssh = SshService::with_config_store(root.path().into(), store.clone());
    let saved = create(&ssh, request(&server, root.path(), json!([first, second]))).await;
    let unlock = match ssh.begin_connection(&saved.id).await.unwrap() {
        SshConnectionAttempt::AuthenticationRequired { challenge } => *challenge,
        other => panic!("second key must need local unlock: {other:?}"),
    };
    assert_eq!(unlock.kind, SshAuthenticationKind::PrivateKey);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    (server, root, ssh, store, saved.id, unlock)
}

#[tokio::test]
async fn middle_unlock_retains_the_correct_identity_and_continues_into_mfa_without_saving_the_answer(
) {
    let (server, _root, ssh, store, id, unlock) = locked_second_key().await;
    let mut round = unlock.clone();
    let mut connected = false;
    for responses in [
        vec!["chain-second-key-only"],
        vec!["482901", "terminal"],
        vec!["731604"],
    ] {
        match ssh
            .respond_keyboard_interactive(
                &id,
                &round.attempt_id,
                &round.challenge_id,
                responses.into_iter().map(String::from).collect(),
            )
            .await
            .unwrap()
        {
            SshConnectionAttempt::AuthenticationRequired { challenge } => {
                assert_eq!(challenge.kind, SshAuthenticationKind::KeyboardInteractive);
                assert_eq!(challenge.attempt_id, unlock.attempt_id);
                round = *challenge;
            }
            SshConnectionAttempt::Connected => {
                connected = true;
                assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 2);
                assert_eq!(
                    server
                        .counts
                        .keyboard_interactive_response_rounds
                        .load(Ordering::SeqCst),
                    2
                );
            }
            other => panic!("unexpected chain progress: {other:?}"),
        }
    }
    assert!(connected, "the final factor must complete the connection");
    assert!(!serde_json::to_string(&*store.0.lock().unwrap())
        .unwrap()
        .contains("chain-second-key-only"));
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn middle_chain_unlock_keeps_cancellation_trust_and_original_expiry_fences() {
    for action in ["cancel", "trust", "expiry"] {
        let (server, root, ssh, _store, id, unlock) = locked_second_key().await;
        match action {
            "cancel" => ssh
                .cancel_connection_attempt(&id, &unlock.attempt_id, &unlock.challenge_id)
                .await
                .unwrap(),
            "trust" => std::fs::write(root.path().join("known_hosts"), "").unwrap(),
            "expiry" => {
                tokio::time::pause();
                tokio::time::advance(std::time::Duration::from_secs(121)).await;
                tokio::task::yield_now().await;
            }
            _ => unreachable!(),
        }
        assert!(ssh
            .respond_keyboard_interactive(
                &id,
                &unlock.attempt_id,
                &unlock.challenge_id,
                vec!["chain-second-key-only".into()]
            )
            .await
            .is_err());
        assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
        if action == "expiry" {
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn opaque_factor_edits_preserve_only_this_connections_compatible_secrets_and_never_export_them(
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(SecretStore::default());
    let ssh = SshService::with_config_store(root.path().into(), store.clone());
    let key = private_key(&server, 0, Some("chain-private-secret"), true);
    let created = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!([password("chain-password-secret"), key]),
        ),
    )
    .await;
    let dto = serde_json::to_value(&created).unwrap();
    assert_eq!(dto["authentication_factors"][0]["has_password"], true);
    assert_eq!(dto["authentication_factors"][1]["has_key_content"], true);
    for factor in dto["authentication_factors"].as_array().unwrap() {
        assert!(factor["id"].as_str().is_some_and(|id| !id.is_empty()));
        for secret in ["password", "passphrase", "key_content"] {
            assert!(factor.get(secret).is_none());
        }
    }
    let config_debug = format!("{:?}", store.0.lock().unwrap());
    assert!(!config_debug.contains("chain-password-secret"));
    let mut update = request(
        &server,
        root.path(),
        json!([
            {"id":dto["authentication_factors"][0]["id"],"auth_type":"password"},
            {"id":dto["authentication_factors"][1]["id"],"auth_type":"key_content"}
        ]),
    );
    update["id"] = json!(created.id);
    ssh.update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    let before = serde_json::to_value(&store.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        before["authentication_factors"][0]["password"],
        "chain-password-secret"
    );
    assert_eq!(
        before["authentication_factors"][1]["passphrase"],
        "chain-private-secret"
    );
    let other = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!([password("other-chain-secret")]),
        ),
    )
    .await;
    let other = serde_json::to_value(other).unwrap();
    for bad_id in [
        json!("unknown-factor"),
        other["authentication_factors"][0]["id"].clone(),
    ] {
        update["authentication_factors"][0]["id"] = bad_id;
        assert!(ssh
            .update_connection(serde_json::from_value(update.clone()).unwrap())
            .await
            .is_err());
    }
    update["authentication_factors"] = json!([{"id":dto["authentication_factors"][0]["id"],"auth_type":"password"},{"id":dto["authentication_factors"][0]["id"],"auth_type":"password"}]);
    assert!(ssh
        .update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .is_err());
    update["authentication_factors"] = json!([{"id":dto["authentication_factors"][1]["id"],"auth_type":"key_content","key_content":"replacement-body-only"}]);
    ssh.update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    let changed = serde_json::to_value(
        &store
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == created.id)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        changed["authentication_factors"][0]["passphrase"],
        Value::Null
    );
    update["auth_type"] = json!("agent");
    ssh.update_connection(serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    let changed = serde_json::to_value(
        &store
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == created.id)
            .unwrap(),
    )
    .unwrap();
    assert!(changed["authentication_factors"]
        .as_array()
        .map_or(true, |f| f.is_empty()));
}

#[tokio::test]
async fn chain_structure_is_bounded_and_new_factors_cannot_claim_saved_ids() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    for factors in [
        json!([]),
        json!(vec![password("bound-only"); 7]),
        json!([{"auth_type":"chain","authentication_factors":[password("nested-only")]}]),
        json!([{"id":"borrowed-id","auth_type":"password","password":"new-only"}]),
    ] {
        let decoded = serde_json::from_value(request(&server, root.path(), factors));
        assert!(decoded.is_err() || ssh.create_connection(decoded.unwrap()).await.is_err());
    }
    assert!(ssh.list_connections().await.is_empty());
}

#[tokio::test]
async fn plaintext_store_refuses_chain_secrets_on_create_update_and_load_before_publication() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let ssh = SshService::new(root.path().into());
    let chain = request(
        &server,
        root.path(),
        json!([password("plaintext-chain-canary")]),
    );
    let decoded =
        serde_json::from_value(chain.clone()).expect("chain request must reach storage validation");
    assert!(ssh
        .create_connection(decoded)
        .await
        .unwrap_err()
        .to_string()
        .contains("secure"));
    assert!(ssh.list_connections().await.is_empty());
    assert!(!root.path().join("deepagent-ssh/connections.json").exists());
    let mut legacy = chain.clone();
    legacy["auth_type"] = json!("agent");
    legacy["authentication_factors"] = json!([]);
    let old = create(&ssh, legacy).await;
    let path = root.path().join("deepagent-ssh/connections.json");
    let before = std::fs::read(&path).unwrap();
    let mut update = chain.clone();
    update["id"] = json!(old.id);
    assert!(ssh
        .update_connection(serde_json::from_value(update).unwrap())
        .await
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut injected = chain;
    injected["id"] = json!("injected-chain");
    injected["authentication_factors"][0]["id"] = json!("stored-factor");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"injected-chain":injected})).unwrap(),
    )
    .unwrap();
    let fresh = SshService::new(root.path().into());
    assert!(fresh.load().await.is_err());
    assert!(fresh.list_connections().await.is_empty());
}

#[tokio::test]
async fn withdrawn_methods_without_partial_success_do_not_mark_a_credential_accepted() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![
        MethodKind::Password,
        MethodKind::PublicKey,
        MethodKind::Password,
    ];
    *server.counts.factor_passwords.write().unwrap() = [
        (0, "not-accepted-at-this-stage".into()),
        (2, "chain-retry-only".into()),
    ]
    .into();
    server
        .counts
        .advance_rejected_password
        .store(true, Ordering::SeqCst);
    let key = private_key(&server, 1, None, false);
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!([password("chain-retry-only"), key]),
        ),
    )
    .await;
    assert!(matches!(
        ssh.begin_connection(&saved.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(
        *server.counts.received_passwords.read().unwrap(),
        ["chain-retry-only", "chain-retry-only"]
    );
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejected_chain_candidates_stop_after_one_pass_without_progress() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    // Keep Password offered after each rejection; Auth::reject() withdraws it.
    *server.counts.authentication_factors.write().unwrap() = vec![MethodKind::Password];
    server
        .counts
        .factor_passwords
        .write()
        .unwrap()
        .insert(0, "not-in-the-configured-chain".into());
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!((0..6)
                .map(|index| password(&format!("rejected-{index}")))
                .collect::<Vec<_>>()),
        ),
    )
    .await;
    assert!(ssh.begin_connection(&saved.id).await.is_err());
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 6);
    assert_eq!(
        *server.counts.received_passwords.read().unwrap(),
        (0..6)
            .map(|index| format!("rejected-{index}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn public_connection_test_uses_the_saved_chain_instead_of_metadata_as_credentials() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() =
        vec![MethodKind::Password, MethodKind::PublicKey];
    server
        .counts
        .factor_passwords
        .write()
        .unwrap()
        .insert(0, "connection-test-password-only".into());
    let key = private_key(&server, 1, Some("connection-test-unlock-only"), true);
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(
        &ssh,
        request(
            &server,
            root.path(),
            json!([password("connection-test-password-only"), key]),
        ),
    )
    .await;
    let result = ssh.test_connection(&saved).await.unwrap();
    assert!(
        result.ok,
        "saved chain must authenticate: {:?}",
        result.error
    );
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_second_local_unlock_does_not_restart_the_connection_deadline() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    *server.counts.authentication_factors.write().unwrap() = vec![MethodKind::PublicKey; 2];
    let first = private_key(&server, 0, Some("first-attempt-only"), false);
    let second = private_key(&server, 1, Some("second-attempt-only"), false);
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = create(&ssh, request(&server, root.path(), json!([first, second]))).await;
    let SshConnectionAttempt::AuthenticationRequired { challenge: first } =
        ssh.begin_connection(&saved.id).await.unwrap()
    else {
        panic!("first unlock required")
    };
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    tokio::time::resume();
    let SshConnectionAttempt::AuthenticationRequired { challenge: second } = ssh
        .respond_keyboard_interactive(
            &saved.id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["first-attempt-only".into()],
        )
        .await
        .unwrap()
    else {
        panic!("second unlock required")
    };
    assert_eq!(first.attempt_id, second.attempt_id);
    assert_eq!(second.kind, SshAuthenticationKind::PrivateKey);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    assert!(ssh
        .respond_keyboard_interactive(
            &saved.id,
            &second.attempt_id,
            &second.challenge_id,
            vec!["second-attempt-only".into()]
        )
        .await
        .is_err());
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    tokio::time::resume();
}
