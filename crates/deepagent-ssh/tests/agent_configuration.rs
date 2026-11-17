use deepagent_ssh::{CreateSshConnectionRequest, SshService, UpdateSshConnectionRequest};

#[tokio::test]
async fn agent_provider_variants_roundtrip_without_private_credentials() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    for auth in ["agent", "openssh_agent", "pageant"] {
        let request: CreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "name": auth, "host": "agent.invalid", "port": 2222,
            "username": "fixture", "auth_type": auth,
        }))
        .unwrap();
        let saved = service.create_connection(request).await.unwrap();
        let dto = serde_json::to_value(&saved).unwrap();
        assert_eq!(dto["auth_type"], auth);
        assert!(dto["key_path"].is_null());
        assert!(dto.get("password").is_none());
    }
    let reopened = SshService::new(data.path().to_path_buf());
    reopened.load().await.unwrap();
    assert_eq!(reopened.list_connections().await.len(), 3);
}

#[test]
fn unknown_agent_provider_is_not_silently_changed_to_auto() {
    assert!(
        serde_json::from_value::<CreateSshConnectionRequest>(serde_json::json!({
            "name": "fixture", "host": "agent.invalid", "port": 2222,
            "username": "fixture", "auth_type": "misspelled_provider",
        }))
        .is_err()
    );
}

#[tokio::test]
async fn agent_configuration_does_not_retain_stale_password_or_key_path() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().to_path_buf());
    let mut payload = serde_json::json!({
        "name": "fixture", "host": "agent.invalid", "port": 2222,
        "username": "fixture", "auth_type": "pageant",
        "key_path": "stale-key-never-read", "password": "stale-secret-never-used",
    });
    let saved = service
        .create_connection(
            serde_json::from_value::<CreateSshConnectionRequest>(payload.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert!(saved.key_path.is_none());
    payload["id"] = saved.id.into();
    let updated = service
        .update_connection(serde_json::from_value::<UpdateSshConnectionRequest>(payload).unwrap())
        .await
        .unwrap();
    assert!(updated.key_path.is_none());
    let stored =
        std::fs::read_to_string(data.path().join("deepagent-ssh/connections.json")).unwrap();
    assert!(!stored.contains("stale-key-never-read"));
    assert!(!stored.contains("stale-secret-never-used"));
}
