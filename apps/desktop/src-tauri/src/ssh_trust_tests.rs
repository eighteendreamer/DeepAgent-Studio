//! Temporary SQLite + in-memory wrapping key; never touches the OS keychain.
use super::*;

#[tokio::test]
async fn agent_providers_survive_encrypted_store_reopen_and_frontend_projection() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("fixture.db");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    for auth in [
        DtoSshAuthType::Agent,
        DtoSshAuthType::OpenSshAgent,
        DtoSshAuthType::Pageant,
    ] {
        let app = AppService::open(&file).unwrap();
        let secrets = Arc::new(SqliteSecretStore::new(
            app.shared_database(),
            wrapping.clone(),
        ));
        let store = Arc::new(SqliteSshConfigStore {
            secrets,
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().to_path_buf(), store);
        let saved = ssh
            .create_connection(DtoCreateSshConnectionRequest {
                name: to_dto_auth_type(auth).into(),
                host: "agent.invalid".into(),
                port: 2222,
                username: "fixture".into(),
                auth_type: auth,
                key_path: Some("must-not-retain-unused-key".into()),
                password: Some("must-not-retain-unused-secret".into()),
                known_hosts_path: None,
                host_key_policy: deepagent_ssh::SshHostKeyPolicy::Strict,
            })
            .await
            .unwrap();
        let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
        assert_eq!(dto["auth_type"], to_dto_auth_type(auth));
        assert!(dto["key_path"].is_null());
        assert!(dto.get("password").is_none());
    }
    let app = AppService::open(&file).unwrap();
    let secrets = Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping));
    let store = Arc::new(SqliteSshConfigStore {
        secrets,
        legacy_path: data.path().join("absent.json"),
    });
    let ssh = SshService::with_config_store(data.path().to_path_buf(), store);
    ssh.load().await.unwrap();
    let connections = ssh.list_connections().await;
    assert_eq!(connections.len(), 3);
    for connection in connections {
        assert_eq!(connection.name, to_dto_auth_type(connection.auth_type));
        assert!(connection.key_path.is_none());
    }
}

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
                host_key_policy: deepagent_ssh::SshHostKeyPolicy::AcceptNew,
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
        assert_eq!(frontend["host_key_policy"], "accept_new");
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
    assert_eq!(
        listed[0].host_key_policy,
        deepagent_ssh::SshHostKeyPolicy::AcceptNew
    );
}

#[test]
fn legacy_frontend_host_key_policy_defaults_to_strict() {
    let dto: deepagent_ssh::SshConnectionDto = serde_json::from_value(serde_json::json!({
        "id":"legacy", "name":"fixture", "host":"fixture.invalid", "port":22,
        "username":"fixture", "auth_type":"agent", "key_path":null,
        "status":"disconnected", "last_error":null, "latency_ms":null,
    }))
    .unwrap();
    let projected = serde_json::to_value(SshConnectionFrontendDto::from(dto)).unwrap();
    assert_eq!(projected["host_key_policy"], "strict");
}

#[tokio::test]
async fn whole_file_import_round_trips_its_reviewed_dto_and_reopens_application_trust() {
    // Generated public fixture only; its private counterpart is not retained.
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILFVCraC5ndXABfSOTg/2Bh+FzEW6t9pS09obPJduaMN";
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.sqlite");
    let file = temp.path().join("known_hosts");
    std::fs::write(&file, format!("other.invalid {key}\n*.invalid {key}\n")).unwrap();
    let app = AppService::open(&path).unwrap();
    let host_keys = Arc::new(deepagent_ssh::SshHostKeyStore::new(app.shared_database()));
    let ssh = SshService::new(temp.path().to_path_buf()).with_host_key_store(host_keys);
    let connection = ssh.create_connection(serde_json::from_value(serde_json::json!({
        "name":"fixture", "host":"selected.invalid", "port":22, "username":"fixture", "auth_type":"agent", "known_hosts_path":file,
    })).unwrap()).await.unwrap();
    let preview = ssh
        .preview_known_hosts_import(&connection.id)
        .await
        .unwrap();
    let wire = serde_json::to_value(&preview).unwrap();
    assert_eq!(wire["importable"], 1);
    assert_eq!(wire["skipped"], 1);
    assert!(wire.get("public_key").is_none());
    let decoded = serde_json::from_value(wire).unwrap();
    let result = ssh
        .import_known_hosts(&connection.id, decoded)
        .await
        .unwrap();
    let receipt = serde_json::to_value(result).unwrap();
    assert_eq!(receipt["imported"], 1);
    assert_eq!(receipt["disconnect_failed_ids"], serde_json::json!([]));
    drop(ssh);
    drop(app);
    let reopened = AppService::open(&path).unwrap();
    let store = deepagent_ssh::SshHostKeyStore::new(reopened.shared_database());
    assert_eq!(
        store
            .list(&deepagent_ssh::SshHostIdentity::direct("other.invalid", 22).unwrap())
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn host_trust_audit_projects_v2_history_without_exporting_source_or_authentication_material()
{
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILFVCraC5ndXABfSOTg/2Bh+FzEW6t9pS09obPJduaMN";
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.sqlite");
    let source = temp.path().join("known_hosts");
    std::fs::write(&source, format!("fixture.invalid {key}\n")).unwrap();
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let id;
    {
        let app = AppService::open(&path).unwrap();
        let configs = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: temp.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(temp.path().into(), configs).with_host_key_store(
            Arc::new(deepagent_ssh::SshHostKeyStore::new(app.shared_database())),
        );
        let c = ssh.create_connection(serde_json::from_value(serde_json::json!({
            "name":"fixture","host":"fixture.invalid","port":22,"username":"fixture","auth_type":"agent","known_hosts_path":source
        })).unwrap()).await.unwrap();
        let preview = ssh
            .inspect_host_trust(&c.host, c.port, Some(source.to_str().unwrap()))
            .await
            .unwrap();
        ssh.import_host_trust(&c.id, preview).await.unwrap();
        id = c.id;
    }
    let app = AppService::open(&path).unwrap();
    let configs = Arc::new(SqliteSshConfigStore {
        secrets: Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping)),
        legacy_path: temp.path().join("absent.json"),
    });
    let ssh = SshService::with_config_store(temp.path().into(), configs).with_host_key_store(
        Arc::new(deepagent_ssh::SshHostKeyStore::new(app.shared_database())),
    );
    let view = ssh.host_trust_audit(&id).await.unwrap();
    let wire = serde_json::to_value(&view).unwrap();
    assert_eq!(wire["revision"], 1);
    assert_eq!(wire["records"][0]["action"], "trust_verified");
    assert!(
        wire.get("last_used_at_ms").unwrap().is_null(),
        "import is not successful user authentication"
    );
    assert!(wire.get("activity_error").unwrap().is_null());
    let keys = serde_json::to_value(ssh.list_host_keys(&id).await.unwrap()).unwrap();
    assert!(keys[0].get("last_used_at_ms").unwrap().is_null());
    let text = serde_json::to_string(&view).unwrap();
    assert!(!text.contains("known_hosts"));
    assert!(!text.contains("public_key"));
    assert!(!text.contains("password"));
}
