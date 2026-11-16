mod support;

use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

use deepagent_ssh::SshService;
use support::{random_key, SshServer};

// Deterministic TOCTOU fixture: the service has read trust before opening TCP;
// the server cannot send its key until the test releases the accept gate.
async fn replace_trust_after_connect_started(change_server: bool) {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("verified-known-hosts");
    server.write_trust(&path);
    let original = server.key.read().unwrap().clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.accept_gate.write().unwrap() = Some(gate.clone());
    let service = Arc::new(SshService::new(data.path().to_path_buf()));
    let connection = server.create(&service, Some(&path)).await;
    let connecting_service = service.clone();
    let id = connection.id.clone();
    let connecting = tokio::spawn(async move { connecting_service.connect(&id).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.counts.connections.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("client opened TCP after loading its trust snapshot");

    *server.key.write().unwrap() = random_key();
    server.write_trust(&path);
    if !change_server {
        *server.key.write().unwrap() = original;
    }
    gate.notify_one();

    let result = tokio::time::timeout(Duration::from_secs(5), connecting)
        .await
        .expect("handshake must settle")
        .unwrap();
    if result.is_ok() {
        service.disconnect(&connection.id).await.unwrap();
    }
    assert!(
        result.is_err(),
        "changed trust never publishes a connection"
    );
    assert_eq!(
        server.auth_count(),
        0,
        "reject before sending any password/key authentication, not after authenticating"
    );
    assert_eq!(server.counts.public_key_offers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_new_key_in_a_replaced_trust_file_cannot_receive_the_password() {
    replace_trust_after_connect_started(true).await;
}

#[tokio::test]
async fn changing_trust_during_handshake_prevents_auth_even_if_the_server_key_is_unchanged() {
    replace_trust_after_connect_started(false).await;
}

#[tokio::test]
async fn changed_key_diagnostic_reports_the_presented_public_fingerprint_without_secrets() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("known_hosts");
    server.write_trust(&path);
    *server.key.write().unwrap() = random_key();
    let fingerprint = server
        .key
        .read()
        .unwrap()
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&path)).await;
    let error = service
        .connect(&connection.id)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains(&fingerprint));
    assert!(error.contains("ssh-ed25519"));
    assert!(error.contains("ssh host key verification failed"));
    assert!(!error.contains("temporary-fixture-only"));
    assert!(!error.contains("PRIVATE KEY"));
    assert_eq!(server.auth_count(), 0);
    // Public-only loopback diagnostic for controlled UI acceptance; no real host,
    // user credentials or private key is exported.
    println!(
        "R002_UI_DIAGNOSTIC={}",
        serde_json::json!({
            "error": error, "fingerprint": fingerprint,
            "connection": connection,
        })
    );
}
