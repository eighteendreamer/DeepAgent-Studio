mod support;

use deepagent_ssh::{SshConnectionAttempt, SshHostKeyDecision, SshService, SshStatus};
use std::sync::atomic::Ordering;

async fn fixture() -> (support::SshServer, tempfile::TempDir, SshService, String) {
    let server = support::SshServer::start("127.0.0.1").await;
    server
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let trust = dir.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = SshService::new(dir.path().to_owned());
    let connection = server.create(&ssh, Some(&trust)).await;
    (server, dir, ssh, connection.id)
}

fn authentication_challenge(
    attempt: SshConnectionAttempt,
) -> deepagent_ssh::SshAuthenticationChallenge {
    match attempt {
        SshConnectionAttempt::AuthenticationRequired { challenge } => *challenge,
        other => panic!("expected an MFA challenge, received {other:?}"),
    }
}

#[tokio::test]
async fn server_initiated_mfa_preserves_prompt_metadata_and_completes_multiple_rounds() {
    let (server, _dir, ssh, id) = fixture().await;
    let first = authentication_challenge(ssh.begin_connection(&id).await.unwrap());

    assert_eq!(first.name, "Fixture verification");
    assert_eq!(first.instructions, "Complete both verification steps");
    assert_eq!(first.prompts.len(), 2);
    assert_eq!(first.prompts[0].prompt, "Verification code");
    assert!(!first.prompts[0].echo);
    assert_eq!(first.prompts[1].prompt, "Device label");
    assert!(first.prompts[1].echo);

    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["482901".into()],
        )
        .await
        .is_err());

    let second = authentication_challenge(
        ssh.respond_keyboard_interactive(
            &id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .unwrap(),
    );
    assert_eq!(second.attempt_id, first.attempt_id);
    assert_ne!(second.challenge_id, first.challenge_id);
    assert_eq!(second.instructions, "Enter the second factor");
    assert_eq!(second.prompts.len(), 1);
    assert!(!second.prompts[0].echo);

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
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        2
    );
}

#[tokio::test]
async fn completes_password_after_keyboard_interactive_partial_success() {
    let (server, _dir, ssh, id) = fixture().await;
    server
        .counts
        .require_password_after_interactive
        .store(true, Ordering::SeqCst);

    let first = authentication_challenge(ssh.begin_connection(&id).await.unwrap());
    let second = authentication_challenge(
        ssh.respond_keyboard_interactive(
            &id,
            &first.attempt_id,
            &first.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .unwrap(),
    );
    assert_eq!(first.attempt_id, second.attempt_id);
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
    // The none probe discovers the interactive factor before any password is offered.
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn wrong_response_count_keeps_current_challenge_and_cancellation_rejects_late_answer() {
    let (_server, _dir, ssh, id) = fixture().await;
    let challenge = authentication_challenge(ssh.begin_connection(&id).await.unwrap());

    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            vec!["482901".into()],
        )
        .await
        .is_err());

    ssh.cancel_connection_attempt(&id, &challenge.attempt_id, &challenge.challenge_id)
        .await
        .unwrap();
    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .is_err());
}

#[tokio::test]
async fn changed_trust_invalidates_pending_mfa_before_a_response_is_sent() {
    let (server, dir, ssh, id) = fixture().await;
    let challenge = authentication_challenge(ssh.begin_connection(&id).await.unwrap());

    *server.key.write().unwrap() = support::random_key();
    server.write_trust(&dir.path().join("known_hosts"));

    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .is_err());
    assert_eq!(
        server
            .counts
            .keyboard_interactive_response_rounds
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn expired_challenge_closes_authentication_and_updates_connection_status() {
    let (_server, dir, ssh, id) = fixture().await;
    let challenge = authentication_challenge(ssh.begin_connection(&id).await.unwrap());

    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    tokio::task::yield_now().await;

    let status = ssh.status(&id).await.unwrap();
    assert_eq!(status.status, SshStatus::Error);
    assert!(status
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("expired")));
    assert!(ssh
        .respond_keyboard_interactive(
            &id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            vec!["482901".into(), "terminal".into()],
        )
        .await
        .is_err());

    let reloaded = SshService::new(dir.path().to_path_buf());
    let persisted_status = reloaded.status(&id).await.unwrap();
    assert_eq!(persisted_status.status, SshStatus::Error);
    assert!(persisted_status
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("expired")));
}

#[tokio::test]
async fn host_key_confirmation_continues_into_mfa_on_the_same_attempt() {
    let server = support::SshServer::start("127.0.0.1").await;
    server
        .counts
        .require_keyboard_interactive
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let ssh = SshService::new(dir.path().to_owned());
    let connection = server.create(&ssh, None).await;
    let id = connection.id;

    let host_challenge = match ssh.begin_connection(&id).await.unwrap() {
        SshConnectionAttempt::ConfirmationRequired { challenge } => *challenge,
        other => panic!("expected a host-key challenge, received {other:?}"),
    };
    assert_eq!(server.auth_count(), 0);
    let auth_challenge = authentication_challenge(
        ssh.respond_host_key(
            &id,
            &host_challenge.attempt_id,
            &host_challenge.challenge_id,
            SshHostKeyDecision::Once,
        )
        .await
        .unwrap(),
    );
    assert_eq!(auth_challenge.attempt_id, host_challenge.attempt_id);
}
