mod support;

use deepagent_ssh::{
    discover_default_ssh_identities_in, SshConnectionAttempt, SshDefaultIdentityState, SshService,
};
use std::sync::atomic::Ordering;

#[test]
fn discovery_checks_only_three_standard_names_without_reading_or_parsing_keys() {
    let home = tempfile::tempdir().unwrap();
    let ssh = home.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    for name in [
        "id_ecdsa",
        "id_rsa",
        "id_ed25519",
        "id_dsa",
        "id_ed25519.pub",
        "config",
        "custom-key",
    ] {
        std::fs::write(
            ssh.join(name),
            "not a key; discovery must only inspect metadata",
        )
        .unwrap();
    }
    let identities = discover_default_ssh_identities_in(home.path()).unwrap();
    assert_eq!(
        identities
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["id_ed25519", "id_rsa", "id_ecdsa"]
    );
    assert!(identities
        .iter()
        .all(|item| item.state == SshDefaultIdentityState::Available));
    assert_eq!(std::fs::read_dir(&ssh).unwrap().count(), 7);
    let dto = serde_json::to_string(&identities).unwrap();
    assert!(!dto.contains("not a key"));
    assert!(!dto.contains("private_key"));
}

#[test]
fn missing_files_and_unusable_candidates_are_reported_individually() {
    let home = tempfile::tempdir().unwrap();
    let missing = discover_default_ssh_identities_in(home.path()).unwrap();
    assert_eq!(missing.len(), 3);
    assert!(missing
        .iter()
        .all(|item| item.state == SshDefaultIdentityState::Missing));
    let ssh = home.path().join(".ssh");
    std::fs::create_dir_all(ssh.join("id_ed25519")).unwrap();
    std::fs::write(ssh.join("id_rsa"), "").unwrap();
    let oversized = std::fs::File::create(ssh.join("id_ecdsa")).unwrap();
    oversized.set_len(1024 * 1024 + 1).unwrap();
    let identities = discover_default_ssh_identities_in(home.path()).unwrap();
    assert_eq!(identities.len(), 3);
    assert!(identities
        .iter()
        .all(|item| item.state == SshDefaultIdentityState::Unavailable));
    assert!(identities[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("regular file"));
    assert!(identities[1].reason.as_deref().unwrap().contains("empty"));
    assert!(identities[2].reason.as_deref().unwrap().contains("1 MiB"));
}

#[test]
fn relative_home_is_rejected_instead_of_searching_the_working_directory() {
    assert!(discover_default_ssh_identities_in(std::path::Path::new("relative-home")).is_err());
}

#[tokio::test]
async fn selecting_a_discovered_key_uses_only_that_identity_and_existing_unlock_flow() {
    for encrypted in [false, true] {
        let server = support::SshServer::start("127.0.0.1").await;
        let home = tempfile::tempdir().unwrap();
        let ssh_dir = home.path().join(".ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        std::fs::write(
            ssh_dir.join("id_rsa"),
            "unselected malformed key must not be read",
        )
        .unwrap();
        let key = support::random_key();
        let fingerprint = key.public_key().fingerprint(russh::keys::HashAlg::Sha256);
        let key = if encrypted {
            key.encrypt(&mut rand::rng(), "discovery-unlock-only")
                .unwrap()
        } else {
            key
        };
        key.write_openssh_file(ssh_dir.join("id_ed25519"), Default::default())
            .unwrap();
        let identities = discover_default_ssh_identities_in(home.path()).unwrap();
        let selected = identities
            .iter()
            .find(|item| item.name == "id_ed25519")
            .unwrap();
        server.write_trust(&home.path().join("known_hosts"));
        *server.counts.authentication_factors.write().unwrap() = vec![russh::MethodKind::PublicKey];
        server
            .counts
            .factor_keys
            .write()
            .unwrap()
            .insert(0, fingerprint.clone());
        let service = SshService::new(home.path().to_owned());
        let connection = service
            .create_connection(
                serde_json::from_value(serde_json::json!({
                    "name":"discovered identity", "host":"127.0.0.1", "port":server.addr.port(),
                    "username":"fixture", "auth_type":"key_file", "key_path":selected.path,
                    "known_hosts_path":home.path().join("known_hosts"),
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let mut attempt = service.begin_connection(&connection.id).await.unwrap();
        if encrypted {
            let SshConnectionAttempt::AuthenticationRequired { challenge } = attempt else {
                panic!("expected local unlock")
            };
            assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
            attempt = service
                .respond_keyboard_interactive(
                    &connection.id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                    vec!["discovery-unlock-only".into()],
                )
                .await
                .unwrap();
        }
        assert!(matches!(attempt, SshConnectionAttempt::Connected));
        assert_eq!(
            *server.counts.received_keys.read().unwrap(),
            vec![fingerprint]
        );
        assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 1);
        assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
        service.disconnect(&connection.id).await.unwrap();
        let saved =
            std::fs::read_to_string(home.path().join("deepagent-ssh/connections.json")).unwrap();
        assert!(!saved.contains("discovery-unlock-only"));
    }
}
