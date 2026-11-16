mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{SshConnectionAttempt, SshHostKeyStore, SshService};
use serde_json::json;
use std::sync::Arc;
use support::SshServer;

fn request(server: &SshServer) -> serde_json::Value {
    json!({
        "name": "policy-loopback-only", "host": server.addr.ip().to_string(),
        "port": server.addr.port(), "username": "fixture", "auth_type": "password",
        "key_path": null, "password": "temporary-fixture-only",
    })
}

#[tokio::test]
async fn legacy_requests_default_to_strict_and_never_enroll_implicitly() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let connection = service
        .create_connection(serde_json::from_value(request(&server)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&connection).unwrap()["host_key_policy"],
        "strict"
    );
    assert!(service.connect(&connection.id).await.is_err());
    assert!(matches!(
        service.begin_connection(&connection.id).await.unwrap(),
        SshConnectionAttempt::ConfirmationRequired { .. }
    ));
    assert_eq!(server.auth_count(), 0);
    assert!(service
        .list_host_keys(&connection.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn explicit_accept_new_persists_and_strict_reuses_but_rejects_rotation() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("trust.sqlite");
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&path).unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let mut input = request(&server);
    input["host_key_policy"] = "accept_new".into();
    let connection = service
        .create_connection(serde_json::from_value(input.clone()).unwrap())
        .await
        .unwrap();
    let handle = service
        .connect(&connection.id)
        .await
        .expect("explicit TOFU enrolls an unknown key before authentication");
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    assert_eq!(server.auth_count(), 1);
    assert_eq!(
        service.list_host_keys(&connection.id).await.unwrap().len(),
        1
    );
    service.disconnect(&connection.id).await.unwrap();
    drop(service);

    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open(&path).unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    assert_eq!(
        serde_json::to_value(&service.list_connections().await[0]).unwrap()["host_key_policy"],
        "accept_new"
    );
    input["id"] = connection.id.clone().into();
    input["host_key_policy"] = "strict".into();
    service
        .update_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    service
        .connect(&connection.id)
        .await
        .expect("Strict reuses the persisted public key");
    service.disconnect(&connection.id).await.unwrap();
    *server.key.write().unwrap() = support::random_key();
    assert!(service.connect(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 2);
}

#[test]
fn invalid_or_insecure_policies_are_not_silently_treated_as_strict() {
    for policy in [json!("insecure"), json!("typo"), json!(null), json!(true)] {
        let input = json!({"name":"fixture", "host":"fixture.invalid", "port":22,
            "username":"fixture", "auth_type":"agent", "host_key_policy":policy});
        assert!(
            serde_json::from_value::<deepagent_ssh::CreateSshConnectionRequest>(input).is_err(),
            "invalid policy must be visible, not ignored"
        );
    }
}

#[tokio::test]
async fn accept_new_begin_connects_without_prompt_but_never_restores_revoked_trust() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let mut input = request(&server);
    input["host_key_policy"] = "accept_new".into();
    let connection = service
        .create_connection(serde_json::from_value(input.clone()).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        service.begin_connection(&connection.id).await.unwrap(),
        SshConnectionAttempt::Connected
    ));
    let records = service.list_host_keys(&connection.id).await.unwrap();
    assert_eq!(records[0].source, "accept_new");
    input["id"] = connection.id.clone().into();
    input.as_object_mut().unwrap().remove("host_key_policy");
    let updated = service
        .update_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(updated).unwrap()["host_key_policy"],
        "accept_new",
        "legacy update preserves explicit policy"
    );
    service
        .revoke_host_key(&connection.id, &records[0].fingerprint)
        .await
        .unwrap();
    assert!(service.connect(&connection.id).await.is_err());
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert_eq!(server.auth_count(), 1);
    assert!(service.list_host_keys(&connection.id).await.unwrap()[0].revoked);
}

#[tokio::test]
async fn accept_new_rejects_changed_file_keys_and_requires_a_store_for_unknown_hosts() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("known_hosts");
    server.write_trust(&file);
    *server.key.write().unwrap() = support::random_key();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let connection = server.create(&service, Some(&file)).await;
    let mut input = request(&server);
    input["id"] = connection.id.clone().into();
    input["host_key_policy"] = "accept_new".into();
    service
        .update_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    assert!(service.connect(&connection.id).await.is_err());
    assert!(service.begin_connection(&connection.id).await.is_err());
    assert!(service
        .list_host_keys(&connection.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(server.auth_count(), 0);

    let separate = tempfile::tempdir().unwrap();
    let service = SshService::new(separate.path().into());
    let mut input = request(&server);
    input["host_key_policy"] = "accept_new".into();
    let connection = service
        .create_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    let error = service
        .connect(&connection.id)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("application trust store"), "{error}");
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn trust_enrolled_by_another_writer_during_probe_never_sends_authentication() {
    use std::sync::atomic::Ordering;
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store.clone());
    let mut input = request(&server);
    input["host_key_policy"] = "accept_new".into();
    let connection = service
        .create_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.accept_gate.write().unwrap() = Some(gate.clone());
    let connect = service.connect(&connection.id);
    let change = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while server.counts.connections.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let identity =
            deepagent_ssh::SshHostIdentity::direct(&connection.host, connection.port).unwrap();
        store
            .trust_verified(
                &identity,
                support::random_key().public_key(),
                "concurrent-writer",
            )
            .unwrap();
        gate.notify_one();
    };
    let (connected, ()) = tokio::join!(connect, change);
    assert!(connected.is_err());
    assert_eq!(server.auth_count(), 0);
    let records = service.list_host_keys(&connection.id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, "concurrent-writer");
}

#[tokio::test]
async fn server_rotation_between_probe_and_authenticated_handshake_is_rejected() {
    use std::sync::atomic::Ordering;
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let mut input = request(&server);
    input["host_key_policy"] = "accept_new".into();
    let connection = service
        .create_connection(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.accept_gate.write().unwrap() = Some(gate.clone());
    let original = server
        .key
        .read()
        .unwrap()
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let connect = service.connect(&connection.id);
    let rotate = async {
        for expected in [1, 2] {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while server.counts.connections.load(Ordering::SeqCst) < expected {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if expected == 2 {
                *server.key.write().unwrap() = support::random_key();
            }
            gate.notify_one();
        }
    };
    let (connected, ()) = tokio::join!(connect, rotate);
    assert!(connected.is_err());
    assert_eq!(server.auth_count(), 0);
    // TOFU registration is durable even if the subsequent connection fails.
    assert_eq!(
        service.list_host_keys(&connection.id).await.unwrap()[0].fingerprint,
        original
    );
}

#[tokio::test]
async fn accept_new_never_ignores_invalid_or_unsupported_trust_files() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("known_hosts");
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    for bytes in [
        vec![0xff],
        b"@revoked fixture invalid\n".to_vec(),
        format!(
            "[{}]:{} invalid-key\n",
            server.addr.ip(),
            server.addr.port()
        )
        .into_bytes(),
    ] {
        std::fs::write(&file, bytes).unwrap();
        let mut input = request(&server);
        input["host_key_policy"] = "accept_new".into();
        input["known_hosts_path"] = file.to_str().unwrap().into();
        let connection = service
            .create_connection(serde_json::from_value(input).unwrap())
            .await
            .unwrap();
        assert!(service.connect(&connection.id).await.is_err());
        assert!(service
            .list_host_keys(&connection.id)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn unsupported_patterns_cannot_be_misclassified_as_unknown() {
    let server = SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("known_hosts");
    let public = support::random_key().public_key().to_openssh().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    for pattern in ["*", "127.0.0.?", "!excluded.invalid,127.0.0.1"] {
        std::fs::write(&file, format!("{pattern} {public}\n")).unwrap();
        let mut input = request(&server);
        input["host_key_policy"] = "accept_new".into();
        input["known_hosts_path"] = file.to_str().unwrap().into();
        let connection = service
            .create_connection(serde_json::from_value(input).unwrap())
            .await
            .unwrap();
        assert!(
            service.connect(&connection.id).await.is_err(),
            "unsupported host patterns must not become unknown"
        );
        assert!(service
            .list_host_keys(&connection.id)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(server.auth_count(), 0);
}
