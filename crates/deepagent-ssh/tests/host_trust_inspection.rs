mod support;

use deepagent_ssh::SshService;
use russh::keys::HashAlg;
use support::{random_key, SshServer};

#[tokio::test]
async fn inspection_reports_matching_algorithms_and_sha256_without_connecting() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("verified known hosts");
    server.write_trust(&path);
    let public = server.key.read().unwrap().public_key().clone();
    let before = std::fs::read(&path).unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let inspected = service
        .inspect_host_trust("127.0.0.1", server.addr.port(), path.to_str())
        .await
        .unwrap();
    assert_eq!(inspected.host, "127.0.0.1");
    assert_eq!(inspected.port, server.addr.port());
    assert_eq!(std::path::Path::new(&inspected.known_hosts_path), path);
    assert_eq!(inspected.keys.len(), 1);
    assert_eq!(inspected.keys[0].algorithm, public.algorithm().as_str());
    assert_eq!(
        inspected.keys[0].fingerprint,
        public.fingerprint(HashAlg::Sha256).to_string()
    );
    assert_eq!(inspected.keys[0].line_number, 1);
    assert_eq!(std::fs::read(&path).unwrap(), before, "read only");
    assert_eq!(server.auth_count(), 0);
    assert_eq!(
        server
            .counts
            .connections
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "inspection must not contact even a loopback host"
    );
    assert!(service.list_connections().await.is_empty());
    // The same inspected source remains accepted by the real strict handshake.
    let connection = server.create(&service, Some(&path)).await;
    service.connect(&connection.id).await.unwrap();
    service.disconnect(&connection.id).await.unwrap();
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn inspection_uses_the_app_default_and_filters_other_hosts_and_ports() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("deepagent-ssh/known_hosts");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let first = random_key().public_key().clone();
    let second = random_key().public_key().clone();
    std::fs::write(
        &path,
        format!(
            "# verified fixture\nother.invalid {}\nfixture.invalid {}\nfixture.invalid {}\n[fixture.invalid]:2222 {}\n",
            second.to_openssh().unwrap(),
            first.to_openssh().unwrap(),
            second.to_openssh().unwrap(),
            second.to_openssh().unwrap(),
        ),
    )
    .unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let inspected = service
        .inspect_host_trust(" fixture.invalid ", 22, None)
        .await
        .unwrap();
    assert_eq!(inspected.host, "fixture.invalid");
    assert_eq!(inspected.keys.len(), 2);
    assert_eq!(inspected.keys[0].line_number, 3);
    assert_eq!(inspected.keys[1].line_number, 4);
    assert_eq!(
        inspected.keys[0].fingerprint,
        first.fingerprint(HashAlg::Sha256).to_string()
    );
    let other_port = service
        .inspect_host_trust("fixture.invalid", 2222, Some(""))
        .await
        .unwrap();
    assert_eq!(other_port.keys.len(), 1);
    assert_eq!(other_port.keys[0].line_number, 5);
    assert!(service
        .inspect_host_trust("fixture.invalid", 2223, None)
        .await
        .is_err());
}

#[tokio::test]
async fn inspection_keeps_the_strict_reader_marker_hash_and_invalid_file_rules() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("verified");
    let public = random_key().public_key().to_openssh().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    // Same independent HMAC-SHA1 fixture as the existing strict-reader regression.
    std::fs::write(
        &path,
        format!("|1|aDEyNS10ZXN0LW9ubHktc2FsdA==|BaHjvW3QDT36o/gmQ/OjXFd7354= {public}\n"),
    )
    .unwrap();
    assert_eq!(
        service
            .inspect_host_trust("127.0.0.1", 22, path.to_str())
            .await
            .unwrap()
            .keys
            .len(),
        1
    );
    for invalid in [
        format!("127.0.0.1 {public}\n@revoked 127.0.0.1 {public}\n"),
        format!("@cert-authority 127.0.0.1 {public}\n"),
        "127.0.0.1 ssh-ed25519 broken\n".to_owned(),
        "# empty trust\n".to_owned(),
    ] {
        std::fs::write(&path, invalid).unwrap();
        let error = service
            .inspect_host_trust("127.0.0.1", 22, path.to_str())
            .await
            .expect_err("inspection must not weaken the connection's strict reader");
        assert!(error.to_string().contains("host key verification failed"));
    }
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(service
        .inspect_host_trust("127.0.0.1", 22, path.to_str())
        .await
        .is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(service
        .inspect_host_trust("127.0.0.1", 22, path.to_str())
        .await
        .is_err());
}

#[tokio::test]
async fn inspection_rejects_invalid_endpoint_and_relative_sources_without_enrollment() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    for (host, port, source) in [
        ("", 22, None),
        ("fixture.invalid", 0, None),
        ("fixture.invalid", 22, Some("relative/known_hosts")),
    ] {
        assert!(service
            .inspect_host_trust(host, port, source)
            .await
            .is_err());
    }
    assert!(!data.path().join("deepagent-ssh/known_hosts").exists());
}
