mod support;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_ssh::{
    CreateSshConnectionRequest, SshConnectionAttempt, SshConnectionDto, SshService,
    UpdateSshConnectionRequest,
};
use russh::keys::ssh_key::certificate::{Builder, CertType};
use russh::keys::HashAlg;
use serde_json::json;

fn request(server: &support::SshServer, root: &Path) -> CreateSshConnectionRequest {
    serde_json::from_value(json!({
        "name": "loopback-cert", "host": server.addr.ip().to_string(),
        "port": server.addr.port(), "username": "fixture", "auth_type": "key_file",
        "key_path": root.join("identity"),
        "certificate_path": root.join("identity-cert.pub"),
        "known_hosts_path": root.join("known_hosts")
    }))
    .unwrap()
}

#[tokio::test]
async fn authenticates_with_the_configured_user_certificate() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let identity = support::random_key();
    identity
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    let ca = support::random_key();
    *server.counts.trusted_user_ca.write().unwrap() =
        Some(ca.public_key().fingerprint(HashAlg::Sha256));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut builder = Builder::new_with_random_nonce(
        &mut rand::rng(),
        identity.public_key().clone(),
        now - 60,
        now + 600,
    )
    .unwrap();
    builder.cert_type(CertType::User).unwrap();
    builder.valid_principal("fixture").unwrap();
    let cert = builder.sign(&ca).unwrap();
    std::fs::write(
        root.path().join("identity-cert.pub"),
        cert.to_openssh().unwrap(),
    )
    .unwrap();

    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    assert!(matches!(
        service.begin_connection(&connection.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    assert_eq!(server.counts.user_certificates.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_certificate_does_not_fall_back_to_the_raw_identity() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    support::random_key()
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    std::fs::write(root.path().join("identity-cert.pub"), "not a certificate").unwrap();
    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.user_certificates.load(Ordering::SeqCst), 0);
}

fn write_certificate(
    root: &Path,
    key: &russh::keys::PrivateKey,
    kind: CertType,
) -> russh::keys::ssh_key::Fingerprint {
    let ca = support::random_key();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut builder = Builder::new_with_random_nonce(
        &mut rand::rng(),
        key.public_key().clone(),
        now - 60,
        now + 600,
    )
    .unwrap();
    builder.cert_type(kind).unwrap();
    builder.valid_principal("fixture").unwrap();
    std::fs::write(
        root.join("identity-cert.pub"),
        builder.sign(&ca).unwrap().to_openssh().unwrap(),
    )
    .unwrap();
    ca.public_key().fingerprint(HashAlg::Sha256)
}

#[tokio::test]
async fn encrypted_private_key_authenticates_with_and_without_a_user_certificate() {
    for attach_certificate in [false, true] {
        let server = support::SshServer::start("127.0.0.1").await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let identity = support::random_key();
        identity
            .encrypt(&mut rand::rng(), "fixture-unlock-only")
            .unwrap()
            .write_openssh_file(root.path().join("identity"), Default::default())
            .unwrap();
        let mut request = request(&server, root.path());
        request.password = Some("fixture-unlock-only".into());
        if attach_certificate {
            *server.counts.trusted_user_ca.write().unwrap() =
                Some(write_certificate(root.path(), &identity, CertType::User));
        } else {
            request.certificate_path = None;
        }
        let service = SshService::new(root.path().to_owned());
        let connection = service.create_connection(request).await.unwrap();
        let wire = serde_json::to_string(&connection).unwrap();
        assert!(!wire.contains("fixture-unlock-only"));
        assert!(matches!(
            service.begin_connection(&connection.id).await.unwrap(),
            SshConnectionAttempt::Connected
        ));
        assert_eq!(
            server.counts.user_certificates.load(Ordering::SeqCst),
            usize::from(attach_certificate)
        );
        assert_eq!(
            server.counts.public_keys.load(Ordering::SeqCst),
            usize::from(!attach_certificate)
        );
    }
}

#[tokio::test]
async fn incorrect_private_key_passphrase_is_not_exposed_or_sent_to_the_server() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    support::random_key()
        .encrypt(&mut rand::rng(), "fixture-correct-only")
        .unwrap()
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    let service = SshService::new(root.path().to_owned());
    let mut request = request(&server, root.path());
    request.certificate_path = None;
    request.password = Some("fixture-incorrect-only".into());
    let connection = service.create_connection(request).await.unwrap();
    let error = service
        .begin_connection(&connection.id)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("fixture-correct-only"));
    assert!(!error.contains("fixture-incorrect-only"));
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_certificate_does_not_offer_the_raw_identity() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    support::random_key()
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mismatched_identity_or_host_certificate_is_rejected_before_any_identity_offer() {
    for kind in [CertType::User, CertType::Host] {
        let server = support::SshServer::start("127.0.0.1").await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let identity = support::random_key();
        identity
            .write_openssh_file(root.path().join("identity"), Default::default())
            .unwrap();
        let different = support::random_key();
        write_certificate(
            root.path(),
            if kind == CertType::User {
                &different
            } else {
                &identity
            },
            kind,
        );
        let service = SshService::new(root.path().to_owned());
        let connection = service
            .create_connection(request(&server, root.path()))
            .await
            .unwrap();
        assert!(service.begin_connection(&connection.id).await.is_err());
        assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
        assert_eq!(server.counts.user_certificates.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn rejected_user_certificate_never_retries_with_the_bare_private_key() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let identity = support::random_key();
    identity
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    write_certificate(root.path(), &identity, CertType::User);
    *server.counts.trusted_user_ca.write().unwrap() = Some(
        support::random_key()
            .public_key()
            .fingerprint(HashAlg::Sha256),
    );
    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert_eq!(server.counts.user_certificates.load(Ordering::SeqCst), 1);
    assert_eq!(server.counts.public_keys.load(Ordering::SeqCst), 0);
    assert_eq!(
        server
            .counts
            .keyboard_interactive_starts
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn certificate_path_survives_reopen_and_legacy_edit_until_explicitly_removed() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    let expected = root
        .path()
        .join("identity-cert.pub")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        serde_json::to_value(&connection).unwrap()["certificate_path"],
        expected
    );
    let reopened = SshService::new(root.path().to_owned());
    reopened.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.list_connections().await[0]).unwrap()["certificate_path"],
        expected
    );
    let mut payload = serde_json::to_value(&connection).unwrap();
    payload.as_object_mut().unwrap().remove("certificate_path");
    let legacy = reopened
        .update_connection(
            serde_json::from_value::<UpdateSshConnectionRequest>(payload.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["certificate_path"],
        expected
    );
    payload["certificate_path"] = json!("");
    let cleared = reopened
        .update_connection(serde_json::from_value::<UpdateSshConnectionRequest>(payload).unwrap())
        .await
        .unwrap();
    assert!(serde_json::to_value(cleared).unwrap()["certificate_path"].is_null());
    let legacy: SshConnectionDto = serde_json::from_value(json!({
        "id":"legacy", "name":"legacy", "host":"fixture.invalid", "port":22,
        "username":"fixture", "auth_type":"key_file", "key_path":"identity",
        "status":"disconnected", "last_error":null, "latency_ms":null,
    }))
    .unwrap();
    assert!(serde_json::to_value(legacy).unwrap()["certificate_path"].is_null());
}

#[tokio::test]
async fn user_certificate_does_not_authorize_an_unknown_server() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let identity = support::random_key();
    identity
        .write_openssh_file(root.path().join("identity"), Default::default())
        .unwrap();
    write_certificate(root.path(), &identity, CertType::User);
    let service = SshService::new(root.path().to_owned());
    let connection = service
        .create_connection(request(&server, root.path()))
        .await
        .unwrap();
    assert!(matches!(
        service.begin_connection(&connection.id).await.unwrap(),
        SshConnectionAttempt::ConfirmationRequired { .. }
    ));
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
    assert_eq!(server.counts.user_certificates.load(Ordering::SeqCst), 0);
}
