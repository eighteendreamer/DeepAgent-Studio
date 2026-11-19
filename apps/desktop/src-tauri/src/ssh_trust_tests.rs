//! Temporary SQLite + in-memory wrapping key; never touches the OS keychain.
use super::*;

#[tokio::test]
async fn jump_credentials_reopen_encrypted_and_frontend_only_receives_presence_metadata() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("jump.sqlite");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let id;
    {
        let app = AppService::open(&file).unwrap();
        let store = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().into(), store);
        let request: DtoCreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "name":"jump fixture", "host":"target.invalid", "port":22, "username":"target-user", "auth_type":"password", "password":"target-encrypted-canary",
            "jump":{"mode":"ssh", "host":"jump.invalid", "port":2222, "username":"jump-user", "auth_type":"chain",
                "authentication_factors":[{"auth_type":"password", "password":"jump-password-canary"}, {"auth_type":"key_content", "key_content":"jump-private-body-canary", "passphrase":"jump-unlock-canary"}]}
        })).unwrap();
        assert!(!format!("{request:?}").contains("canary"));
        let saved = ssh.create_connection(request).await.unwrap();
        id = saved.id.clone();
        let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
        assert_eq!(dto["jump"]["host"], "jump.invalid");
        assert_eq!(
            dto["jump"]["authentication_factors"][0]["has_password"],
            true
        );
        assert_eq!(
            dto["jump"]["authentication_factors"][1]["has_key_content"],
            true
        );
        assert_eq!(
            dto["jump"]["authentication_factors"][1]["has_passphrase"],
            true
        );
        assert!(!dto.to_string().contains("canary"));
    }
    let app = AppService::open(&file).unwrap();
    let store = Arc::new(SqliteSshConfigStore {
        secrets: Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping)),
        legacy_path: data.path().join("absent.json"),
    });
    let loaded = deepagent_ssh::SshConfigStore::load(store.as_ref())
        .await
        .unwrap();
    assert_eq!(
        loaded[0].password.as_deref(),
        Some("target-encrypted-canary")
    );
    let jump = loaded[0].jump.as_ref().unwrap();
    assert_eq!(
        jump.authentication_factors[0].password.as_deref(),
        Some("jump-password-canary")
    );
    assert_eq!(
        jump.authentication_factors[1].key_content.as_deref(),
        Some("jump-private-body-canary")
    );
    assert_eq!(
        jump.authentication_factors[1].passphrase.as_deref(),
        Some("jump-unlock-canary")
    );
    assert!(!format!("{:?}", loaded[0]).contains("canary"));
    let ssh = SshService::with_config_store(data.path().into(), store);
    ssh.load().await.unwrap();
    assert_eq!(ssh.list_connections().await[0].id, id);
    for path in [file.clone(), file.with_extension("sqlite-wal")] {
        if let Ok(bytes) = std::fs::read(path) {
            for marker in [
                b"target-encrypted-canary".as_slice(),
                b"jump-password-canary".as_slice(),
                b"jump-private-body-canary".as_slice(),
                b"jump-unlock-canary".as_slice(),
            ] {
                assert!(!bytes.windows(marker.len()).any(|part| part == marker));
            }
        }
    }
}

#[tokio::test]
async fn proxy_password_reopens_encrypted_and_never_reaches_frontend_or_debug_output() {
    let data = tempfile::tempdir().unwrap();
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    for mode in ["http_connect", "socks5"] {
        let file = data.path().join(format!("proxy-{mode}.sqlite"));
        let id;
        {
            let app = AppService::open(&file).unwrap();
            let store = Arc::new(SqliteSshConfigStore {
                secrets: Arc::new(SqliteSecretStore::new(
                    app.shared_database(),
                    wrapping.clone(),
                )),
                legacy_path: data.path().join("absent.json"),
            });
            let ssh = SshService::with_config_store(data.path().into(), store);
            let request: DtoCreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
                "name":"proxy fixture","host":"fixture.invalid","port":22,"username":"fixture","auth_type":"agent",
                "proxy":{"mode":mode,"host":"proxy.invalid","port":1080,"username":"proxy-user-canary","password":"proxy-encrypted-secret-canary"}
            })).unwrap();
            assert!(!format!("{request:?}").contains("canary"));
            let saved = ssh.create_connection(request).await.unwrap();
            id = saved.id.clone();
            let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
            assert_eq!(dto["proxy"]["mode"], mode);
            assert_eq!(dto["proxy"]["has_password"], true);
            assert!(dto["proxy"].get("password").is_none());
            assert!(!dto.to_string().contains("proxy-encrypted-secret-canary"));
        }
        let app = AppService::open(&file).unwrap();
        let store = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: data.path().join("absent.json"),
        });
        let loaded = deepagent_ssh::SshConfigStore::load(store.as_ref())
            .await
            .unwrap();
        assert_eq!(
            loaded[0].proxy.as_ref().unwrap().password.as_deref(),
            Some("proxy-encrypted-secret-canary")
        );
        assert!(!format!("{:?}", loaded[0]).contains("canary"));
        let ssh = SshService::with_config_store(data.path().into(), store);
        ssh.load().await.unwrap();
        let listed = ssh.list_connections().await;
        assert_eq!(listed[0].id, id);
        assert!(listed[0].proxy.as_ref().unwrap().has_password);
        for path in [file.clone(), file.with_extension("sqlite-wal")] {
            if let Ok(bytes) = std::fs::read(path) {
                let marker = b"proxy-encrypted-secret-canary";
                assert!(!bytes.windows(marker.len()).any(|part| part == marker));
            }
        }
    }
}

#[tokio::test]
async fn authentication_chain_secrets_reopen_encrypted_and_only_metadata_reaches_the_frontend() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("authentication-chain.sqlite");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let id;
    {
        let app = AppService::open(&file).unwrap();
        let store = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().into(), store);
        let request: DtoCreateSshConnectionRequest=serde_json::from_value(serde_json::json!({
            "name":"chain fixture","host":"fixture.invalid","port":22,"username":"fixture","auth_type":"chain",
            "authentication_factors":[
                {"auth_type":"password","password":"chain-encrypted-password-canary"},
                {"auth_type":"key_content","key_content":"chain-private-body-canary","passphrase":"chain-key-passphrase-canary"}
            ]
        })).unwrap();
        assert!(!format!("{request:?}").contains("canary"));
        let saved = ssh.create_connection(request).await.unwrap();
        id = saved.id.clone();
        let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
        assert_eq!(dto["auth_type"], "chain");
        assert_eq!(dto["authentication_factors"][0]["has_password"], true);
        assert_eq!(dto["authentication_factors"][1]["has_key_content"], true);
        assert_eq!(dto["authentication_factors"][1]["has_passphrase"], true);
        assert!(!dto.to_string().contains("canary"));
    }
    let app = AppService::open(&file).unwrap();
    let store = Arc::new(SqliteSshConfigStore {
        secrets: Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping)),
        legacy_path: data.path().join("absent.json"),
    });
    let loaded = deepagent_ssh::SshConfigStore::load(store.as_ref())
        .await
        .unwrap();
    assert_eq!(
        loaded[0].authentication_factors[0].password.as_deref(),
        Some("chain-encrypted-password-canary")
    );
    assert_eq!(
        loaded[0].authentication_factors[1].key_content.as_deref(),
        Some("chain-private-body-canary")
    );
    assert_eq!(
        loaded[0].authentication_factors[1].passphrase.as_deref(),
        Some("chain-key-passphrase-canary")
    );
    assert!(!format!("{:?}", loaded[0].authentication_factors).contains("canary"));
    let ssh = SshService::with_config_store(data.path().into(), store);
    ssh.load().await.unwrap();
    let listed = ssh.list_connections().await;
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].authentication_factors.len(), 2);
    for path in [file.clone(), file.with_extension("sqlite-wal")] {
        if let Ok(bytes) = std::fs::read(path) {
            for marker in [
                b"chain-encrypted-password-canary".as_slice(),
                b"chain-private-body-canary".as_slice(),
                b"chain-key-passphrase-canary".as_slice(),
            ] {
                assert!(!bytes.windows(marker.len()).any(|part| part == marker));
            }
        }
    }
}

#[tokio::test]
async fn private_key_body_is_encrypted_at_rest_and_never_projected_back_to_the_frontend() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("private-body.sqlite");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let body = "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture-private-body-storage-only\n-----END OPENSSH PRIVATE KEY-----";
    let id;
    {
        let app = AppService::open(&file).unwrap();
        let store = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().into(), store);
        let saved = ssh
            .create_connection(
                serde_json::from_value(serde_json::json!({
                    "name":"private body fixture", "host":"fixture.invalid", "port":22,
                    "username":"fixture", "auth_type":"key_content", "key_content":body,
                    "password":"fixture-stored-passphrase",
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        id = saved.id.clone();
        let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
        assert_eq!(dto["auth_type"], "key_content");
        assert_eq!(dto["has_key_content"], true);
        assert!(dto.get("key_content").is_none());
        assert!(!dto.to_string().contains("fixture-stored-passphrase"));
    }
    let app = AppService::open(&file).unwrap();
    let store = Arc::new(SqliteSshConfigStore {
        secrets: Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping)),
        legacy_path: data.path().join("absent.json"),
    });
    let loaded = deepagent_ssh::SshConfigStore::load(store.as_ref())
        .await
        .unwrap();
    assert_eq!(loaded[0].key_content.as_deref(), Some(body));
    assert_eq!(
        loaded[0].password.as_deref(),
        Some("fixture-stored-passphrase")
    );
    assert!(!format!("{:?}", loaded[0]).contains("PRIVATE KEY"));
    assert!(!format!("{:?}", loaded[0]).contains("fixture-stored-passphrase"));
    let ssh = SshService::with_config_store(data.path().into(), store);
    ssh.load().await.unwrap();
    let listed = ssh.list_connections().await;
    assert_eq!(listed[0].id, id);
    assert!(listed[0].has_key_content);
    let dto = serde_json::to_value(SshConnectionFrontendDto::from(listed[0].clone())).unwrap();
    assert!(dto.get("key_content").is_none());
    for path in [file.clone(), file.with_extension("sqlite-wal")] {
        if let Ok(bytes) = std::fs::read(path) {
            for marker in [
                b"fixture-private-body-storage-only".as_slice(),
                b"fixture-stored-passphrase".as_slice(),
            ] {
                assert!(!bytes.windows(marker.len()).any(|part| part == marker));
            }
        }
    }
}

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
                authentication_factors: None,
                proxy: None,
                jump: None,
                timing: Default::default(),
                name: to_dto_auth_type(auth).into(),
                host: "agent.invalid".into(),
                port: 2222,
                username: "fixture".into(),
                auth_type: auth,
                agent_fingerprint: None,
                key_content: None,
                key_path: Some("must-not-retain-unused-key".into()),
                certificate_path: Some("must-not-retain-unused-certificate".into()),
                password: Some("must-not-retain-unused-secret".into()),
                known_hosts_path: None,
                host_key_policy: deepagent_ssh::SshHostKeyPolicy::Strict,
            })
            .await
            .unwrap();
        let dto = serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap();
        assert_eq!(dto["auth_type"], to_dto_auth_type(auth));
        assert!(dto["key_path"].is_null());
        assert!(dto["certificate_path"].is_null());
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
        assert!(connection.certificate_path.is_none());
    }
}

#[tokio::test]
async fn user_certificate_survives_encrypted_store_reopen_without_exposing_the_passphrase() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("certificate.db");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let identity = data.path().join("identity").to_str().unwrap().to_owned();
    let certificate = data
        .path()
        .join("identity-cert.pub")
        .to_str()
        .unwrap()
        .to_owned();
    let id;
    {
        let app = AppService::open(&file).unwrap();
        let secrets = Arc::new(SqliteSecretStore::new(
            app.shared_database(),
            wrapping.clone(),
        ));
        let store = Arc::new(SqliteSshConfigStore {
            secrets,
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().to_owned(), store);
        let created = ssh
            .create_connection(DtoCreateSshConnectionRequest {
                authentication_factors: None,
                proxy: None,
                jump: None,
                timing: Default::default(),
                name: "user certificate".into(),
                host: "fixture.invalid".into(),
                port: 22,
                username: "fixture".into(),
                auth_type: DtoSshAuthType::KeyFile,
                agent_fingerprint: None,
                key_content: None,
                key_path: Some(identity.clone()),
                certificate_path: Some(certificate.clone()),
                password: Some("test-only-unlock-secret".into()),
                known_hosts_path: None,
                host_key_policy: deepagent_ssh::SshHostKeyPolicy::Strict,
            })
            .await
            .unwrap();
        id = created.id.clone();
        let frontend = serde_json::to_value(SshConnectionFrontendDto::from(created)).unwrap();
        assert_eq!(frontend["certificate_path"], certificate);
        assert!(frontend.get("password").is_none());
        assert!(frontend.get("passphrase").is_none());
        assert!(!frontend.to_string().contains("test-only-unlock-secret"));
    }
    let app = AppService::open(&file).unwrap();
    let secrets = Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping));
    let store = Arc::new(SqliteSshConfigStore {
        secrets,
        legacy_path: data.path().join("absent.json"),
    });
    let ssh = SshService::with_config_store(data.path().to_owned(), store.clone());
    ssh.load().await.unwrap();
    let listed = ssh.list_connections().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(
        listed[0].certificate_path.as_deref(),
        Some(certificate.as_str())
    );
    // Read through the real encrypted store, not a mock of its projection.
    let stored = deepagent_ssh::SshConfigStore::load(store.as_ref())
        .await
        .unwrap();
    assert_eq!(
        stored[0].password.as_deref(),
        Some("test-only-unlock-secret")
    );
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes
        .windows(b"test-only-unlock-secret".len())
        .any(|part| part == b"test-only-unlock-secret"));
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
                authentication_factors: None,
                proxy: None,
                jump: None,
                timing: Default::default(),
                host_key_policy: deepagent_ssh::SshHostKeyPolicy::AcceptNew,
                name: "fixture".into(),
                host: "127.0.0.1".into(),
                port: 2222,
                username: "fixture".into(),
                auth_type: DtoSshAuthType::Password,
                agent_fingerprint: None,
                key_content: None,
                key_path: None,
                certificate_path: None,
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
#[tokio::test]
async fn connection_timing_survives_encrypted_storage_and_frontend_projection() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("app.sqlite");
    let wrapping = Arc::new(deepagent_app_core::MemorySecretStore::new());
    let timing = serde_json::json!({"connect_timeout_secs":7,"inactivity_timeout_secs":90,
        "keepalive_interval_secs":15,"keepalive_max":4});
    let id;
    {
        let app = AppService::open(&path).unwrap();
        let store = Arc::new(SqliteSshConfigStore {
            secrets: Arc::new(SqliteSecretStore::new(
                app.shared_database(),
                wrapping.clone(),
            )),
            legacy_path: data.path().join("absent.json"),
        });
        let ssh = SshService::with_config_store(data.path().to_owned(), store);
        let saved = ssh
            .create_connection(
                serde_json::from_value(serde_json::json!({
                    "name":"timing fixture","host":"fixture.invalid","port":22,"username":"fixture",
                    "auth_type":"agent","timing":timing
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        id = saved.id.clone();
        assert_eq!(
            serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap()["timing"],
            timing
        );
    }
    let app = AppService::open(&path).unwrap();
    let store = Arc::new(SqliteSshConfigStore {
        secrets: Arc::new(SqliteSecretStore::new(app.shared_database(), wrapping)),
        legacy_path: data.path().join("absent.json"),
    });
    let ssh = SshService::with_config_store(data.path().to_owned(), store);
    ssh.load().await.unwrap();
    let saved = ssh
        .list_connections()
        .await
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(SshConnectionFrontendDto::from(saved)).unwrap()["timing"],
        timing
    );
}
