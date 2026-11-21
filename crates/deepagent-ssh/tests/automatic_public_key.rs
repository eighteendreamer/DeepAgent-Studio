use deepagent_ssh::{CreateSshConnectionRequest, SshService};
use serde_json::{json, Value};

fn request(auth_type: &str, fingerprint: Option<&str>) -> CreateSshConnectionRequest {
    serde_json::from_value(json!({
        "name":"automatic fixture", "host":"fixture.invalid", "port":22,
        "username":"fixture", "auth_type":auth_type, "agent_fingerprint":fingerprint,
    }))
    .unwrap()
}

#[tokio::test]
async fn automatic_public_key_is_a_distinct_persisted_strategy_without_stale_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let service = SshService::new(temp.path().to_owned());
    let created = service
        .create_connection(request("auto_public_key", None))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(created).unwrap()["auth_type"],
        "auto_public_key"
    );
    let restored = SshService::new(temp.path().to_owned());
    restored.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(restored.list_connections().await).unwrap()[0]["auth_type"],
        "auto_public_key"
    );
}

#[tokio::test]
async fn selected_agent_fingerprint_survives_persistence_and_an_omitted_update() {
    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let fingerprint = key
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let temp = tempfile::tempdir().unwrap();
    let service = SshService::new(temp.path().to_owned());
    let created = service
        .create_connection(request("agent", Some(&fingerprint)))
        .await
        .unwrap();
    let dto = serde_json::to_value(&created).unwrap();
    assert_eq!(dto["agent_fingerprint"], fingerprint);
    for (auth_type, selection, expected) in [
        ("agent", Value::Null, Some(fingerprint.as_str())),
        ("agent", json!(""), None),
    ] {
        let updated = service
            .update_connection(
                serde_json::from_value(json!({
                    "id":created.id, "name":"renamed", "host":"fixture.invalid", "port":22,
                    "username":"fixture", "auth_type":auth_type, "agent_fingerprint":selection,
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(updated).unwrap()["agent_fingerprint"].as_str(),
            expected
        );
    }
}

#[tokio::test]
async fn invalid_agent_fingerprint_is_rejected_before_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let service = SshService::new(temp.path().to_owned());
    for fingerprint in ["SHA256:not-a-digest", "MD5:ab:cd", "SHA256:injected\nfield"] {
        assert!(service
            .create_connection(request("agent", Some(fingerprint)))
            .await
            .is_err());
    }
    assert!(service.list_connections().await.is_empty());
}

#[tokio::test]
async fn target_and_jump_fingerprints_are_independent_and_provider_changes_clear_stale_selection() {
    let fingerprint = || {
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string()
    };
    let target = fingerprint();
    let jump = fingerprint();
    let temp = tempfile::tempdir().unwrap();
    let service = SshService::new(temp.path().to_owned());
    let created=service.create_connection(serde_json::from_value(json!({
        "name":"independent", "host":"target.invalid", "port":22, "username":"fixture", "auth_type":"agent", "agent_fingerprint":target,
        "jump":{"mode":"ssh", "host":"jump.invalid", "port":22, "username":"jump", "auth_type":"pageant", "agent_fingerprint":jump}
    })).unwrap()).await.unwrap();
    let dto = serde_json::to_value(&created).unwrap();
    assert_eq!(dto["agent_fingerprint"], target);
    assert_eq!(dto["jump"]["agent_fingerprint"], jump);
    let updated=service.update_connection(serde_json::from_value(json!({
        "id":created.id,"name":"changed provider", "host":"target.invalid", "port":22, "username":"fixture", "auth_type":"openssh_agent"
    })).unwrap()).await.unwrap();
    let dto = serde_json::to_value(updated).unwrap();
    assert!(dto["agent_fingerprint"].is_null());
    assert_eq!(dto["jump"]["agent_fingerprint"], jump);
}
