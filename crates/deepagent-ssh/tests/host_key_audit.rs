//! Opt-in loopback probe for the production SSH server-key verification path.

use std::sync::Arc;

use async_ssh2_tokio::{AuthMethod, Client, ServerCheckMethod};
use deepagent_ssh::{CreateSshConnectionRequest, SshAuthType, SshService};
use russh::keys::{self, ssh_key::rand_core::OsRng, Algorithm, PrivateKey};
use russh::server::{self, Auth};
use tokio::net::TcpListener;

struct AcceptPassword;

impl server::Handler for AcceptPassword {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "audit probe: production SSH connection must reject an untrusted server key"]
async fn audit_ssh_service_accepts_an_untrusted_server_key() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("loopback address");
    let host_key =
        PrivateKey::random(&mut OsRng, Algorithm::Ed25519).expect("generate temporary server key");
    let untrusted_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .expect("generate unrelated known-host key");
    let host_public_key = host_key.public_key().clone();
    let untrusted_public_key = untrusted_key.public_key().clone();

    let mut server_config = server::Config::default();
    server_config.inactivity_timeout = None;
    server_config.keys.push(host_key);
    let server_config = Arc::new(server_config);
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.expect("accept loopback client");
            if let Ok(session) =
                server::run_stream(server_config.clone(), stream, AcceptPassword).await
            {
                let _ = session.await;
            }
        }
    });

    let app_data = tempfile::tempdir().expect("create temporary app data");
    let service = SshService::new(app_data.path().to_path_buf());
    let connection = service
        .create_connection(CreateSshConnectionRequest {
            name: "untrusted-loopback-audit".into(),
            host: addr.ip().to_string(),
            port: addr.port(),
            username: "audit".into(),
            auth_type: SshAuthType::Password,
            key_path: None,
            password: Some("audit-only".into()),
        })
        .await
        .expect("create test connection");

    let connected = service.connect(&connection.id).await;
    eprintln!(
        "production_connect_accepts_untrusted_key={} host=127.0.0.1 port={}",
        connected.is_ok(),
        addr.port()
    );
    if connected.is_ok() {
        service
            .disconnect(&connection.id)
            .await
            .expect("disconnect production client");
    }

    let known_hosts = app_data.path().join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!(
            "[127.0.0.1]:{} {}\n",
            addr.port(),
            host_public_key.to_openssh().expect("encode trusted key")
        ),
    )
    .expect("write matching known-host key");

    let trusted = Client::connect(
        ("127.0.0.1", addr.port()),
        "audit",
        AuthMethod::with_password("audit-only"),
        ServerCheckMethod::KnownHostsFile(known_hosts.to_string_lossy().into_owned()),
    )
    .await
    .expect("known-host verification should accept the matching key");
    trusted
        .disconnect()
        .await
        .expect("disconnect trusted client");

    std::fs::write(
        &known_hosts,
        format!(
            "[127.0.0.1]:{} {}\n",
            addr.port(),
            untrusted_public_key.to_openssh().expect("encode decoy key")
        ),
    )
    .expect("replace with unrelated known-host key");

    let checked = Client::connect(
        ("127.0.0.1", addr.port()),
        "audit",
        AuthMethod::with_password("audit-only"),
        ServerCheckMethod::KnownHostsFile(known_hosts.to_string_lossy().into_owned()),
    )
    .await;
    let known_hosts_rejects_actual_key =
        keys::check_known_hosts_path("127.0.0.1", addr.port(), &host_public_key, &known_hosts)
            .is_err();
    eprintln!(
        "known_hosts_rejects_mismatched_key={} production_accepted_untrusted_key={}",
        checked.is_err(),
        connected.is_ok()
    );
    assert!(
        checked.is_err(),
        "known-host verification must reject the decoy key"
    );
    assert!(
        known_hosts_rejects_actual_key,
        "decoy known_hosts key must mismatch"
    );

    tokio::time::timeout(std::time::Duration::from_secs(3), server)
        .await
        .expect("loopback server exits after the three verification attempts")
        .expect("loopback server task completes");

    assert!(
        !connected.is_ok(),
        "production SSH service accepted a server key absent from the configured known_hosts file"
    );
}
