mod support;

use deepagent_ssh::{
    SshAuthenticationChallenge, SshAuthenticationKind, SshConnectionAttempt, SshService,
};
use russh::MethodKind;
use serde_json::json;
use std::sync::atomic::Ordering;

fn challenge(
    attempt: SshConnectionAttempt,
    kind: SshAuthenticationKind,
) -> SshAuthenticationChallenge {
    match attempt {
        SshConnectionAttempt::AuthenticationRequired { challenge } => {
            assert_eq!(challenge.kind, kind);
            *challenge
        }
        other => panic!("expected {kind:?}, received {other:?}"),
    }
}

async fn fixture(
    factors: &[MethodKind],
    encrypted_key: Option<bool>,
) -> (support::SshServer, tempfile::TempDir, SshService, String) {
    let server = support::SshServer::start("127.0.0.1").await;
    *server.counts.authentication_factors.write().unwrap() = factors.to_vec();
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let ssh = SshService::new(root.path().into());
    let mut request = json!({ "name":"factor order fixture", "host":"127.0.0.1", "port":server.addr.port(), "username":"fixture", "auth_type":"password", "password":"temporary-fixture-only", "known_hosts_path":root.path().join("known_hosts") });
    if let Some(encrypted) = encrypted_key {
        let key = support::random_key();
        let key = if encrypted {
            key.encrypt(&mut rand::rng(), "fixture-unlock-only")
                .unwrap()
        } else {
            key
        };
        key.write_openssh_file(root.path().join("identity"), Default::default())
            .unwrap();
        request["auth_type"] = json!("key_file");
        request["key_path"] = json!(root.path().join("identity"));
        request["password"] = json!(null);
    }
    let saved = ssh
        .create_connection(serde_json::from_value(request).unwrap())
        .await
        .unwrap();
    (server, root, ssh, saved.id)
}

async fn interactive(
    ssh: &SshService,
    id: &str,
    first: SshAuthenticationChallenge,
) -> SshConnectionAttempt {
    let second = challenge(
        ssh.respond_keyboard_interactive(
            id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    assert_eq!(second.attempt_id, first.attempt_id);
    ssh.respond_keyboard_interactive(
        id,
        &second.attempt_id,
        &second.challenge_id,
        vec!["731604".into()],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn password_interactive_password_respects_every_partial_success_on_the_same_connection() {
    let (server, _root, ssh, id) = fixture(
        &[
            MethodKind::Password,
            MethodKind::KeyboardInteractive,
            MethodKind::Password,
        ],
        None,
    )
    .await;
    let first = challenge(
        ssh.begin_connection(&id).await.unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    assert!(matches!(
        interactive(&ssh, &id, first).await,
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn interactive_precedes_public_key_without_offering_the_identity_early() {
    let (server, _root, ssh, id) = fixture(
        &[MethodKind::KeyboardInteractive, MethodKind::PublicKey],
        Some(false),
    )
    .await;
    let first = challenge(
        ssh.begin_connection(&id).await.unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert!(matches!(
        interactive(&ssh, &id, first).await,
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn encrypted_key_unlock_in_the_middle_continues_into_a_later_interactive_factor() {
    let (server, root, ssh, id) = fixture(
        &[
            MethodKind::KeyboardInteractive,
            MethodKind::PublicKey,
            MethodKind::KeyboardInteractive,
        ],
        Some(true),
    )
    .await;
    let first = challenge(
        ssh.begin_connection(&id).await.unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    let attempt_id = first.attempt_id.clone();
    let unlock = challenge(
        interactive(&ssh, &id, first).await,
        SshAuthenticationKind::PrivateKey,
    );
    assert_eq!(unlock.attempt_id, attempt_id);
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    let next = challenge(
        ssh.respond_keyboard_interactive(
            &id,
            &unlock.attempt_id,
            &unlock.challenge_id,
            vec!["fixture-unlock-only".into()],
        )
        .await
        .unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    assert_eq!(next.attempt_id, attempt_id);
    assert!(matches!(
        interactive(&ssh, &id, next).await,
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        4
    );
    assert!(
        !std::fs::read_to_string(root.path().join("deepagent-ssh/connections.json"))
            .unwrap()
            .contains("fixture-unlock-only")
    );
}

#[tokio::test]
async fn unsupported_remaining_methods_never_receive_a_configured_credential() {
    let (server, _root, ssh, id) = fixture(&[MethodKind::HostBased], None).await;
    assert!(ssh.begin_connection(&id).await.is_err());
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejected_interactive_answers_do_not_restart_the_method_or_try_an_unoffered_password() {
    let (server, _root, ssh, id) = fixture(
        &[MethodKind::KeyboardInteractive, MethodKind::Password],
        None,
    )
    .await;
    let first = challenge(
        ssh.begin_connection(&id).await.unwrap(),
        SshAuthenticationKind::KeyboardInteractive,
    );
    let error = ssh
        .respond_keyboard_interactive(
            &id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["wrong-fixture-answer".into(), "terminal".into()],
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("wrong-fixture-answer"));
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn excessive_partial_success_stages_stop_at_an_explicit_bound() {
    let (server, _root, ssh, id) = fixture(&[MethodKind::Password; 20], None).await;
    let error = ssh.begin_connection(&id).await.unwrap_err().to_string();
    assert!(error.contains("limit"), "{error}");
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 16);
}

#[tokio::test]
async fn unavailable_interactive_before_a_prompt_falls_back_to_an_offered_password_once() {
    let (server, _root, ssh, id) = fixture(&[], None).await;
    server
        .counts
        .unavailable_keyboard_interactive
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        ssh.begin_connection(&id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(
        server
            .counts
            .keyboard_interactive_starts
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejected_methods_do_not_loop_when_the_server_advertises_them_again() {
    let (server, _root, ssh, id) = fixture(&[], None).await;
    server
        .counts
        .unavailable_keyboard_interactive
        .store(true, Ordering::SeqCst);
    server.counts.reject_password.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), ssh.begin_connection(&id))
        .await
        .expect("authentication must stop after exhausting the offered configured methods");
    assert!(result.is_err());
    assert_eq!(
        server
            .counts
            .keyboard_interactive_starts
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn middle_unlock_respects_cancellation_trust_changes_and_the_original_expiry() {
    for boundary in ["cancel", "trust", "expiry"] {
        let (server, root, ssh, id) = fixture(
            &[MethodKind::KeyboardInteractive, MethodKind::PublicKey],
            Some(true),
        )
        .await;
        let first = challenge(
            ssh.begin_connection(&id).await.unwrap(),
            SshAuthenticationKind::KeyboardInteractive,
        );
        if boundary == "expiry" {
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(60)).await;
            tokio::time::resume();
        }
        let unlock = challenge(
            interactive(&ssh, &id, first).await,
            SshAuthenticationKind::PrivateKey,
        );
        match boundary {
            "cancel" => ssh
                .cancel_connection_attempt(&id, &unlock.attempt_id, &unlock.challenge_id)
                .await
                .unwrap(),
            "trust" => {
                *server.key.write().unwrap() = support::random_key();
                server.write_trust(&root.path().join("known_hosts"));
            }
            "expiry" => {
                tokio::time::pause();
                tokio::time::advance(std::time::Duration::from_secs(61)).await;
                tokio::task::yield_now().await;
            }
            _ => unreachable!(),
        }
        assert!(
            ssh.respond_keyboard_interactive(
                &id,
                &unlock.attempt_id,
                &unlock.challenge_id,
                vec!["fixture-unlock-only".into()],
            )
            .await
            .is_err(),
            "{boundary}"
        );
        assert_eq!(
            server.counts.public_key_offers.load(Ordering::SeqCst),
            0,
            "{boundary}"
        );
        assert_eq!(
            server.counts.passwords.load(Ordering::SeqCst),
            0,
            "{boundary}"
        );
        if boundary == "expiry" {
            tokio::time::resume();
        }
    }
}
