//! Original loopback audit promoted to a default production regression.
mod support;

use async_ssh2_tokio::{AuthMethod, Client, ServerCheckMethod};
use deepagent_ssh::SshService;
use russh::keys;
use support::{random_key, SshServer};

#[tokio::test]
async fn audit_ssh_service_accepts_an_untrusted_server_key() {
    let server = SshServer::start("127.0.0.1").await;
    let app_data = tempfile::tempdir().expect("temporary app data");
    let service = SshService::new(app_data.path().to_path_buf());
    let connection = server.create(&service, None).await;
    let connected = service.connect(&connection.id).await;
    eprintln!(
        "production_connect_accepts_untrusted_key={}",
        connected.is_ok()
    );
    if connected.is_ok() {
        service.disconnect(&connection.id).await.unwrap();
    }

    let known_hosts = app_data.path().join("known_hosts");
    server.write_trust(&known_hosts);
    let trusted = Client::connect(
        ("127.0.0.1", server.addr.port()),
        "audit",
        AuthMethod::with_password("audit-only"),
        ServerCheckMethod::KnownHostsFile(known_hosts.to_string_lossy().into_owned()),
    )
    .await
    .expect("matching dependency control");
    trusted.disconnect().await.unwrap();
    let host_public_key = server.key.read().unwrap().public_key().clone();
    let decoy = random_key().public_key().to_openssh().unwrap();
    std::fs::write(
        &known_hosts,
        format!("[127.0.0.1]:{} {}\n", server.addr.port(), decoy),
    )
    .unwrap();
    let checked = Client::connect(
        ("127.0.0.1", server.addr.port()),
        "audit",
        AuthMethod::with_password("audit-only"),
        ServerCheckMethod::KnownHostsFile(known_hosts.to_string_lossy().into_owned()),
    )
    .await;
    eprintln!("known_hosts_rejects_mismatched_key={}", checked.is_err());
    assert!(checked.is_err(), "dependency mismatch control");
    assert!(keys::check_known_hosts_path(
        "127.0.0.1",
        server.addr.port(),
        &host_public_key,
        &known_hosts
    )
    .is_err());
    assert!(
        connected.is_err(),
        "production SSH service accepted a server key absent from the configured known_hosts file"
    );
    assert_eq!(
        server.auth_count(),
        1,
        "only the matching control may authenticate"
    );
}
