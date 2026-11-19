mod support;

use deepagent_ssh::{SshConfigStore, SshConnectionConfig, SshResult, SshService};
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
    async fn save(&self, value: &[SshConnectionConfig]) -> SshResult<()> {
        *self.0.lock().unwrap() = value.to_vec();
        Ok(())
    }
}

struct Fixture {
    target: support::SshServer,
    jump: support::SshServer,
    root: tempfile::TempDir,
    store: Arc<SecretStore>,
    ssh: SshService,
}

impl Fixture {
    async fn start() -> Self {
        let target = support::SshServer::start("127.0.0.1").await;
        let jump = support::SshServer::start("127.0.0.1").await;
        *jump.counts.forward_target.write().unwrap() = Some(target.addr);
        let root = tempfile::tempdir().unwrap();
        target.write_trust(&root.path().join("target_known_hosts"));
        jump.write_trust(&root.path().join("jump_known_hosts"));
        let store = Arc::new(SecretStore::default());
        let ssh = SshService::with_config_store(root.path().to_owned(), store.clone());
        Self {
            target,
            jump,
            root,
            store,
            ssh,
        }
    }

    fn request(&self) -> Value {
        json!({
            "name":"jump fixture", "host":"127.0.0.1", "port":self.target.addr.port(),
            "username":"target-fixture", "auth_type":"password", "password":"target-only-secret",
            "known_hosts_path":self.root.path().join("target_known_hosts"),
            "jump":{
                "mode":"ssh", "host":"127.0.0.1", "port":self.jump.addr.port(),
                "username":"jump-fixture", "auth_type":"password", "password":"jump-only-secret",
                "known_hosts_path":self.root.path().join("jump_known_hosts")
            }
        })
    }
}

#[tokio::test]
async fn jump_authentication_and_forwarding_are_independent_of_the_target() {
    let fixture = Fixture::start().await;
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let handle = fixture.ssh.connect(&dto.id).await.unwrap();
    fixture.ssh.exec(&handle, "echo ok").await.unwrap();
    assert_eq!(
        fixture
            .jump
            .counts
            .forward_requests
            .read()
            .unwrap()
            .as_slice(),
        &[("127.0.0.1".into(), u32::from(fixture.target.addr.port()))]
    );
    assert_eq!(
        fixture
            .jump
            .counts
            .received_passwords
            .read()
            .unwrap()
            .as_slice(),
        &["jump-only-secret"]
    );
    assert_eq!(
        fixture
            .target
            .counts
            .received_passwords
            .read()
            .unwrap()
            .as_slice(),
        &["target-only-secret"]
    );
    assert_eq!(fixture.jump.counts.commands.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.target.counts.commands.load(Ordering::SeqCst), 1);
    fixture.ssh.disconnect(&dto.id).await.unwrap();
}

#[tokio::test]
async fn jump_failure_never_falls_back_to_the_direct_target() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .reject_password
        .store(true, Ordering::SeqCst);
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let result = fixture.ssh.connect(&dto.id).await;
    assert!(
        result.is_err(),
        "a rejected jump login must not connect directly"
    );
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("jump"),
        "error must identify the jump stage: {error}"
    );
    assert!(!error.contains("jump-only-secret"));
    assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.target.auth_count(), 0);
}

#[tokio::test]
async fn unknown_jump_host_does_not_receive_credentials_or_contact_the_target() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap();
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    assert!(
        fixture.ssh.connect(&dto.id).await.is_err(),
        "unknown jump key must be rejected"
    );
    assert_eq!(fixture.jump.auth_count(), 0);
    assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn jump_configuration_persists_but_secrets_never_reach_dto_or_debug() {
    let fixture = Fixture::start().await;
    let request = serde_json::from_value(fixture.request()).unwrap();
    let debug = format!("{request:?}");
    let dto = fixture.ssh.create_connection(request).await.unwrap();
    let value = serde_json::to_value(&dto).unwrap();
    assert_eq!(value["jump"]["host"], "127.0.0.1");
    assert_eq!(value["jump"]["has_password"], true);
    assert!(!value.to_string().contains("jump-only-secret"));
    assert!(!debug.contains("jump-only-secret"));
    let saved = fixture.store.0.lock().unwrap()[0].clone();
    assert!(serde_json::to_string(&saved)
        .unwrap()
        .contains("jump-only-secret"));
    assert!(!format!("{saved:?}").contains("jump-only-secret"));
    let reopened =
        SshService::with_config_store(fixture.root.path().to_owned(), fixture.store.clone());
    reopened.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(reopened.list_connections().await).unwrap()[0]["jump"]["has_password"],
        true
    );
}

#[tokio::test]
async fn jump_secrets_require_an_encrypted_configuration_store() {
    let fixture = Fixture::start().await;
    let legacy = SshService::new(fixture.root.path().join("legacy"));
    let result = legacy
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await;
    assert!(
        result.is_err(),
        "jump credentials must not enter plaintext JSON persistence"
    );
}

#[tokio::test]
async fn forwarding_refusal_never_authenticates_the_target() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .reject_forward
        .store(true, Ordering::SeqCst);
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let error = fixture.ssh.connect(&dto.id).await.unwrap_err().to_string();
    assert!(error.contains("jump forwarding"), "{error}");
    assert_eq!(fixture.target.auth_count(), 0);
    assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn changing_jump_trust_discards_the_live_target_before_another_command() {
    let fixture = Fixture::start().await;
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let handle = fixture.ssh.connect(&dto.id).await.unwrap();
    std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap();
    assert!(fixture
        .ssh
        .exec(&handle, "echo should-not-run")
        .await
        .is_err());
    assert_eq!(fixture.target.counts.commands.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn jump_target_trust_and_import_previews_are_bound_to_the_jump_endpoint() {
    let fixture = Fixture::start().await;
    let trust = Arc::new(deepagent_ssh::SshHostKeyStore::new(Arc::new(
        deepagent_persistence::Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(fixture.root.path().into(), fixture.store.clone())
        .with_host_key_store(trust);
    let mut wire = fixture.request();
    wire["jump"] = json!({"mode":"direct"});
    let direct = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    let inspection = ssh.inspect_connection_host_trust(&direct.id).await.unwrap();
    assert_eq!(
        ssh.import_host_trust(&direct.id, inspection)
            .await
            .unwrap()
            .len(),
        1
    );
    wire = fixture.request();
    let jumped = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    assert!(
        ssh.list_host_keys(&jumped.id).await.unwrap().is_empty(),
        "jumped target must not inherit direct application trust"
    );
    let preview = ssh.preview_known_hosts_import(&jumped.id).await.unwrap();
    wire["id"] = json!(jumped.id);
    wire["jump"]["port"] = json!(fixture.jump.addr.port().saturating_sub(1));
    ssh.update_connection(serde_json::from_value(wire).unwrap())
        .await
        .unwrap();
    assert!(
        ssh.import_known_hosts(&jumped.id, preview).await.is_err(),
        "old route preview must be stale"
    );
}

#[tokio::test]
async fn unknown_jump_enrollment_has_its_own_role_and_does_not_probe_the_target() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap();
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let attempt = fixture.ssh.begin_connection(&dto.id).await.unwrap();
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge } = attempt else {
        panic!("expected jump host verification")
    };
    assert_eq!(challenge.role, "jump");
    assert_eq!(challenge.port, fixture.jump.addr.port());
    assert_eq!(challenge.connection_id, dto.id);
    assert_eq!(fixture.jump.auth_count(), 0);
    assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
    fixture
        .ssh
        .cancel_host_key(&dto.id, &challenge.attempt_id, &challenge.challenge_id)
        .unwrap();
}

#[tokio::test]
async fn both_hops_resume_independent_mfa_in_one_connection_attempt() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    fixture
        .target
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let mut attempt = fixture.ssh.begin_connection(&dto.id).await.unwrap();
    let mut attempt_id = String::new();
    for (hop, round, answers) in [
        ("jump", 0, vec!["482901", "terminal"]),
        ("jump", 1, vec!["731604"]),
        ("target", 0, vec!["482901", "terminal"]),
        ("target", 1, vec!["731604"]),
    ] {
        let deepagent_ssh::SshConnectionAttempt::AuthenticationRequired { challenge } = attempt
        else {
            panic!("expected {hop} MFA round {round}")
        };
        assert_eq!(serde_json::to_value(&challenge).unwrap()["role"], hop);
        assert_eq!(
            challenge.port,
            if hop == "jump" {
                fixture.jump.addr.port()
            } else {
                fixture.target.addr.port()
            }
        );
        assert_eq!(challenge.connection_id, dto.id);
        if attempt_id.is_empty() {
            attempt_id = challenge.attempt_id.clone()
        }
        assert_eq!(challenge.attempt_id, attempt_id);
        if hop == "jump" {
            assert_eq!(fixture.target.auth_count(), 0);
        }
        attempt = fixture
            .ssh
            .respond_keyboard_interactive(
                &dto.id,
                &challenge.attempt_id,
                &challenge.challenge_id,
                answers.into_iter().map(String::from).collect(),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        attempt,
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    fixture.ssh.disconnect(&dto.id).await.unwrap();
}

#[tokio::test]
async fn two_unknown_hops_accept_once_without_persisting_or_repeating_jump_authentication() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap();
    std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge: jump } =
        fixture.ssh.begin_connection(&dto.id).await.unwrap()
    else {
        panic!("expected jump host verification")
    };
    assert_eq!(jump.role, "jump");
    assert_eq!(fixture.jump.auth_count(), 0);
    assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
    let next = fixture
        .ssh
        .respond_host_key(
            &dto.id,
            &jump.attempt_id,
            &jump.challenge_id,
            deepagent_ssh::SshHostKeyDecision::Once,
        )
        .await;
    assert!(
        next.is_ok(),
        "accepting the jump must advance to target verification: {next:?}"
    );
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge: target } =
        next.unwrap()
    else {
        panic!("expected target host verification")
    };
    assert_eq!(target.role, "target");
    assert_eq!(target.attempt_id, jump.attempt_id);
    assert!(target.expires_at_ms <= jump.expires_at_ms);
    assert_eq!(fixture.target.auth_count(), 0);
    assert_eq!(
        fixture.jump.counts.received_passwords.read().unwrap().len(),
        1
    );
    let connected = fixture
        .ssh
        .respond_host_key(
            &dto.id,
            &target.attempt_id,
            &target.challenge_id,
            deepagent_ssh::SshHostKeyDecision::Once,
        )
        .await
        .unwrap();
    assert!(matches!(
        connected,
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    assert_eq!(
        fixture.jump.counts.received_passwords.read().unwrap().len(),
        1
    );
    assert_eq!(
        fixture
            .target
            .counts
            .received_passwords
            .read()
            .unwrap()
            .as_slice(),
        &["target-only-secret"]
    );
    fixture.ssh.disconnect(&dto.id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("jump_known_hosts")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("target_known_hosts")).unwrap(),
        ""
    );
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge: again } =
        fixture.ssh.begin_connection(&dto.id).await.unwrap()
    else {
        panic!("Once must not persist trust")
    };
    assert_eq!(again.role, "jump");
    fixture
        .ssh
        .cancel_host_key(&dto.id, &again.attempt_id, &again.challenge_id)
        .unwrap();
}

#[tokio::test]
async fn unknown_target_verification_continues_after_jump_mfa_without_restarting_it() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let first = fixture.ssh.begin_connection(&dto.id).await;
    assert!(
        first.is_ok(),
        "target probing must preserve jump MFA: {first:?}"
    );
    let mut attempt = first.unwrap();
    let mut attempt_id = String::new();
    for answers in [vec!["482901", "terminal"], vec!["731604"]] {
        let deepagent_ssh::SshConnectionAttempt::AuthenticationRequired { challenge } = attempt
        else {
            panic!("expected jump MFA")
        };
        assert_eq!(serde_json::to_value(&challenge).unwrap()["role"], "jump");
        if attempt_id.is_empty() {
            attempt_id = challenge.attempt_id.clone();
        }
        assert_eq!(challenge.attempt_id, attempt_id);
        assert_eq!(fixture.target.counts.connections.load(Ordering::SeqCst), 0);
        attempt = fixture
            .ssh
            .respond_keyboard_interactive(
                &dto.id,
                &challenge.attempt_id,
                &challenge.challenge_id,
                answers.into_iter().map(String::from).collect(),
            )
            .await
            .unwrap();
    }
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge } = attempt else {
        panic!("expected target host verification after jump MFA")
    };
    assert_eq!(challenge.role, "target");
    assert_eq!(challenge.attempt_id, attempt_id);
    assert_eq!(fixture.target.auth_count(), 0);
    let connected = fixture
        .ssh
        .respond_host_key(
            &dto.id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            deepagent_ssh::SshHostKeyDecision::Once,
        )
        .await
        .unwrap();
    assert!(matches!(
        connected,
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    assert_eq!(
        fixture
            .jump
            .counts
            .keyboard_interactive_starts
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .jump
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        2
    );
    fixture.ssh.disconnect(&dto.id).await.unwrap();
}

#[tokio::test]
async fn target_confirmation_cancellation_expiry_and_edits_release_the_authenticated_jump() {
    for action in ["cancel", "expire", "edit", "supersede"] {
        let fixture = Fixture::start().await;
        std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
        let dto = fixture
            .ssh
            .create_connection(serde_json::from_value(fixture.request()).unwrap())
            .await
            .unwrap();
        let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge } =
            fixture.ssh.begin_connection(&dto.id).await.unwrap()
        else {
            panic!("expected target confirmation")
        };
        match action {
            "cancel" => fixture
                .ssh
                .cancel_host_key(&dto.id, &challenge.attempt_id, &challenge.challenge_id)
                .unwrap(),
            "expire" => {
                tokio::time::pause();
                tokio::time::advance(std::time::Duration::from_secs(121)).await;
                tokio::task::yield_now().await;
                tokio::time::resume();
            }
            "edit" => {
                let mut update = fixture.request();
                update["id"] = json!(dto.id);
                update["name"] = json!("new generation");
                fixture
                    .ssh
                    .update_connection(serde_json::from_value(update).unwrap())
                    .await
                    .unwrap();
            }
            "supersede" => {
                let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge: next } =
                    fixture.ssh.begin_connection(&dto.id).await.unwrap()
                else {
                    panic!("expected replacement attempt")
                };
                fixture
                    .ssh
                    .cancel_host_key(&dto.id, &next.attempt_id, &next.challenge_id)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            fixture
                .ssh
                .respond_host_key(
                    &dto.id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                    deepagent_ssh::SshHostKeyDecision::Once
                )
                .await
                .is_err(),
            "{action}"
        );
        assert_eq!(fixture.target.auth_count(), 0, "{action}");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if fixture
                    .jump
                    .counts
                    .closed_connections
                    .load(Ordering::SeqCst)
                    == fixture.jump.counts.connections.load(Ordering::SeqCst)
                {
                    break;
                }
                fixture.jump.counts.connection_closed.notified().await;
            }
        })
        .await
        .expect("the dedicated jump must close promptly");
    }
}

#[tokio::test]
async fn target_confirmation_rechecks_both_hops_before_target_credentials() {
    for changed in ["jump_trust", "target_trust", "target_key"] {
        let fixture = Fixture::start().await;
        std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
        let dto = fixture
            .ssh
            .create_connection(serde_json::from_value(fixture.request()).unwrap())
            .await
            .unwrap();
        let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge } =
            fixture.ssh.begin_connection(&dto.id).await.unwrap()
        else {
            panic!("expected target confirmation")
        };
        match changed {
            "jump_trust" => {
                std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap()
            }
            "target_trust" => fixture
                .target
                .write_trust(&fixture.root.path().join("target_known_hosts")),
            "target_key" => *fixture.target.key.write().unwrap() = support::random_key(),
            _ => unreachable!(),
        }
        assert!(
            fixture
                .ssh
                .respond_host_key(
                    &dto.id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                    deepagent_ssh::SshHostKeyDecision::Once
                )
                .await
                .is_err(),
            "{changed}"
        );
        assert_eq!(fixture.target.auth_count(), 0, "{changed}");
    }
}

#[tokio::test]
async fn two_hop_save_persists_both_verified_keys_and_reconnects_without_prompts() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.root.path().join("jump_known_hosts"), "").unwrap();
    std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
    let store = Arc::new(deepagent_ssh::SshHostKeyStore::new(Arc::new(
        deepagent_persistence::Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(fixture.root.path().into(), fixture.store.clone())
        .with_host_key_store(store.clone());
    let dto = ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let mut outcome = ssh.begin_connection(&dto.id).await.unwrap();
    for role in ["jump", "target"] {
        let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge } = outcome
        else {
            panic!("expected {role} key")
        };
        assert_eq!(challenge.role, role);
        assert!(challenge.can_save);
        outcome = ssh
            .respond_host_key(
                &dto.id,
                &challenge.attempt_id,
                &challenge.challenge_id,
                deepagent_ssh::SshHostKeyDecision::Save,
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        outcome,
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    ssh.disconnect(&dto.id).await.unwrap();
    drop(ssh);
    let reopened = SshService::with_config_store(fixture.root.path().into(), fixture.store.clone())
        .with_host_key_store(store);
    assert!(matches!(
        reopened.begin_connection(&dto.id).await.unwrap(),
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    reopened.disconnect(&dto.id).await.unwrap();
}

#[tokio::test]
async fn crossing_jump_mfa_target_verification_and_target_mfa_keeps_the_original_deadline() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    fixture
        .target
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
    let dto = fixture
        .ssh
        .create_connection(serde_json::from_value(fixture.request()).unwrap())
        .await
        .unwrap();
    let mut outcome = fixture.ssh.begin_connection(&dto.id).await.unwrap();
    let mut deadline = i64::MAX;
    for answers in [vec!["482901", "terminal"], vec!["731604"]] {
        let deepagent_ssh::SshConnectionAttempt::AuthenticationRequired { challenge } = outcome
        else {
            panic!("expected jump MFA")
        };
        assert!(challenge.expires_at_ms <= deadline);
        deadline = challenge.expires_at_ms;
        outcome = fixture
            .ssh
            .respond_keyboard_interactive(
                &dto.id,
                &challenge.attempt_id,
                &challenge.challenge_id,
                answers.into_iter().map(String::from).collect(),
            )
            .await
            .unwrap();
    }
    let deepagent_ssh::SshConnectionAttempt::ConfirmationRequired { challenge: host } = outcome
    else {
        panic!("expected target verification")
    };
    assert!(host.expires_at_ms <= deadline);
    let deepagent_ssh::SshConnectionAttempt::AuthenticationRequired { challenge: auth } = fixture
        .ssh
        .respond_host_key(
            &dto.id,
            &host.attempt_id,
            &host.challenge_id,
            deepagent_ssh::SshHostKeyDecision::Once,
        )
        .await
        .unwrap()
    else {
        panic!("expected target MFA")
    };
    assert_eq!(auth.attempt_id, host.attempt_id);
    assert!(auth.expires_at_ms <= host.expires_at_ms);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    assert!(fixture
        .ssh
        .respond_keyboard_interactive(
            &dto.id,
            &auth.attempt_id,
            &auth.challenge_id,
            vec!["482901".into(), "terminal".into()]
        )
        .await
        .is_err());
    assert_eq!(
        fixture
            .target
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn target_accept_new_continues_after_jump_mfa_and_uses_the_routed_application_identity() {
    let fixture = Fixture::start().await;
    fixture
        .jump
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    std::fs::write(fixture.root.path().join("target_known_hosts"), "").unwrap();
    let store = Arc::new(deepagent_ssh::SshHostKeyStore::new(Arc::new(
        deepagent_persistence::Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(fixture.root.path().into(), fixture.store.clone())
        .with_host_key_store(store);
    let mut wire = fixture.request();
    wire["host_key_policy"] = json!("accept_new");
    let dto = ssh
        .create_connection(serde_json::from_value(wire).unwrap())
        .await
        .unwrap();
    let mut outcome = ssh.begin_connection(&dto.id).await.unwrap();
    for answers in [vec!["482901", "terminal"], vec!["731604"]] {
        let deepagent_ssh::SshConnectionAttempt::AuthenticationRequired { challenge } = outcome
        else {
            panic!("expected jump MFA")
        };
        outcome = ssh
            .respond_keyboard_interactive(
                &dto.id,
                &challenge.attempt_id,
                &challenge.challenge_id,
                answers.into_iter().map(String::from).collect(),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        outcome,
        deepagent_ssh::SshConnectionAttempt::Connected
    ));
    assert_eq!(ssh.list_host_keys(&dto.id).await.unwrap().len(), 1);
    let audit = serde_json::to_value(ssh.host_trust_audit(&dto.id).await.unwrap()).unwrap();
    assert_eq!(audit["route"], "jump");
    assert_eq!(audit["records"][0]["action"], "accept_new");
    ssh.disconnect(&dto.id).await.unwrap();
}
