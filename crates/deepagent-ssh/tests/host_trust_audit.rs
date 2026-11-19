mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{
    SshConnectionAttempt, SshHostIdentity, SshHostKeyDecision, SshHostKeyStore, SshService,
};
use russh::keys::{HashAlg, PublicKey};
use std::sync::Arc;

fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

#[tokio::test]
async fn file_import_and_accept_new_are_audited_as_distinct_trust_changes() {
    let server = support::SshServer::start("127.0.0.1").await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("known_hosts");
    server.write_trust(&path);
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(temp.path().into()).with_host_key_store(store);
    let c = server.create(&service, Some(&path)).await;
    let inspected = service
        .inspect_host_trust(&c.host, c.port, Some(path.to_str().unwrap()))
        .await
        .unwrap();
    service.import_host_trust(&c.id, inspected).await.unwrap();
    assert_eq!(
        serde_json::to_value(service.host_trust_audit(&c.id).await.unwrap()).unwrap()["records"][0]
            ["action"],
        "trust_verified"
    );
    assert_eq!(server.auth_count(), 0);
    let other = support::SshServer::start("127.0.0.1").await;
    let connection = service.create_connection(serde_json::from_value(serde_json::json!({
        "name":"TOFU fixture","host":"127.0.0.1","port":other.addr.port(),"username":"fixture","auth_type":"password","password":"temporary-fixture-only","host_key_policy":"accept_new"
    })).unwrap()).await.unwrap();
    service.connect(&connection.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(service.host_trust_audit(&connection.id).await.unwrap()).unwrap()
            ["records"][0]["action"],
        "accept_new"
    );
    service.disconnect(&connection.id).await.unwrap();
}

#[test]
fn verified_enrollment_and_revocation_are_durable_idempotent_public_audit_events() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.sqlite");
    let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
    let key = support::random_key().public_key().clone();
    let store = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    store
        .trust_verified(&identity, &key, "independently verified fixture")
        .unwrap();
    store.trust_verified(&identity, &key, "duplicate").unwrap();
    store.revoke(&identity, &fingerprint(&key)).unwrap();
    store.revoke(&identity, &fingerprint(&key)).unwrap();
    let events = store.audit_history(&identity).unwrap();
    let json = serde_json::to_value(&events).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(json[0]["action"], "trust_verified");
    assert_eq!(json[1]["action"], "revoke");
    assert_eq!(
        json[0]["trusted_fingerprints"],
        serde_json::json!([fingerprint(&key)])
    );
    assert_eq!(
        json[1]["revoked_fingerprints"],
        serde_json::json!([fingerprint(&key)])
    );
    assert_eq!(json[0]["revision"], 1);
    assert_eq!(json[1]["revision"], 2);
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("independently verified fixture"),
        "generic audit does not contain local source paths/labels"
    );
    let reopened = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    assert_eq!(reopened.audit_history(&identity).unwrap(), events);
    assert!(reopened
        .audit_history(&SshHostIdentity::direct("other.invalid", 22).unwrap())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn service_audit_covers_confirmed_enrollment_rotation_and_revocation_without_read_side_effects(
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(data.path().into()).with_host_key_store(store);
    let c = server.create(&service, None).await;
    let SshConnectionAttempt::ConfirmationRequired { challenge } =
        service.begin_connection(&c.id).await.unwrap()
    else {
        panic!("must verify unknown host")
    };
    service
        .respond_host_key(
            &c.id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            SshHostKeyDecision::Save,
        )
        .await
        .unwrap();
    service.disconnect(&c.id).await.unwrap();
    *server.key.write().unwrap() = support::random_key();
    let p = service.begin_host_key_rotation(&c.id).await.unwrap();
    let receipt = service
        .confirm_host_key_rotation(&c.id, &p.challenge.attempt_id, &p.challenge.challenge_id)
        .await
        .unwrap();
    let stored_rotation = service
        .host_trust_audit(&c.id)
        .await
        .unwrap()
        .records
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        stored_rotation.at_ms, receipt.audit.at_ms,
        "receipt uses the one committed event clock"
    );
    assert_eq!(stored_rotation.revision, receipt.audit.revision);
    service
        .revoke_host_key(&c.id, &receipt.audit.fingerprint)
        .await
        .unwrap();
    let view = service.host_trust_audit(&c.id).await.unwrap();
    assert_eq!(view.connection_id, c.id);
    assert_eq!(view.host, c.host);
    assert_eq!(view.port, c.port);
    assert_eq!(view.revision, 3);
    assert_eq!(view.records.len(), 3);
    assert_eq!(
        serde_json::to_value(&view.records)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["trust_verified", "explicit_host_key_rotation", "revoke"]
    );
    let wire = serde_json::to_string(&view).unwrap();
    assert!(!wire.contains("password"));
    assert!(!wire.contains("public_key"));
    assert_eq!(server.auth_count(), 1);
    for _ in 0..4 {
        assert_eq!(service.host_trust_audit(&c.id).await.unwrap(), view);
    }
    assert_eq!(server.auth_count(), 1, "audit listing never opens SSH");
}

#[tokio::test]
async fn whole_file_import_audit_stays_bound_to_each_endpoint_and_is_not_a_login_event() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("known_hosts");
    let key = support::random_key().public_key().clone();
    std::fs::write(
        &path,
        format!("one.invalid,two.invalid {}\n", key.to_openssh().unwrap()),
    )
    .unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(temp.path().into()).with_host_key_store(store.clone());
    let c = service.create_connection(serde_json::from_value(serde_json::json!({"name":"fixture","host":"one.invalid","port":22,"username":"fixture","auth_type":"agent","known_hosts_path":path})).unwrap()).await.unwrap();
    let preview = service.preview_known_hosts_import(&c.id).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.import_known_hosts(&c.id, preview),
    )
    .await
    .expect("whole-file import must finish connection cleanup without reentering its writer lock")
    .unwrap();
    for host in ["one.invalid", "two.invalid"] {
        let events = store
            .audit_history(&SshHostIdentity::direct(host, 22).unwrap())
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            serde_json::to_value(&events).unwrap()[0]["action"],
            "known_hosts_import"
        );
    }
    assert!(store
        .audit_history(&SshHostIdentity::direct("one.invalid", 2222).unwrap())
        .unwrap()
        .is_empty());
}
