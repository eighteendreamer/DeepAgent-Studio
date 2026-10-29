//! Temporary SQLite + in-memory wrapping key; never touches the OS keychain.
use super::*;

#[tokio::test]
async fn h125_ssh_trust_survives_encrypted_store_reopen_and_frontend_projection() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("fixture.db");
    let trust = data
        .path()
        .join("verified-known-hosts")
        .to_str()
        .unwrap()
        .to_owned();
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let id;
    {
        let app = AppService::open(&file).unwrap();
        let secrets = Arc::new(SqliteSecretStore::new(
            app.shared_database(),
            wrapping.clone(),
        ));
        let store = Arc::new(SqliteSshConfigStore {
            secrets,
            legacy_path: data.path().join("absent-legacy.json"),
        });
        let ssh = SshService::with_config_store(data.path().to_path_buf(), store);
        let created = ssh
            .create_connection(DtoCreateSshConnectionRequest {
                name: "fixture".into(),
                host: "127.0.0.1".into(),
                port: 2222,
                username: "fixture".into(),
                auth_type: DtoSshAuthType::Password,
                key_path: None,
                password: Some("temporary-test-only".into()),
                known_hosts_path: Some(trust.clone()),
            })
            .await
            .unwrap();
        id = created.id.clone();
        let frontend = serde_json::to_value(SshConnectionFrontendDto::from(created)).unwrap();
        assert_eq!(frontend["known_hosts_path"], trust);
        assert!(frontend.get("password").is_none());
    }
    let app = AppService::open(&file).unwrap();
    let secrets = Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping));
    let store = Arc::new(SqliteSshConfigStore {
        secrets,
        legacy_path: data.path().join("absent-legacy.json"),
    });
    let ssh = SshService::with_config_store(data.path().to_path_buf(), store);
    ssh.load().await.unwrap();
    let listed = ssh.list_connections().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].known_hosts_path.as_deref(), Some(trust.as_str()));
}
