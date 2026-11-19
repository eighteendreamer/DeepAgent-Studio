mod support;

use deepagent_ssh::{
    preview_ssh_config_in, resolve_ssh_config, SshConfigImportPreview, SshConfigOverrides,
};
use std::path::Path;

fn resolve(
    source: &str,
    home: &Path,
    alias: &str,
    overrides: &SshConfigOverrides,
) -> SshConfigImportPreview {
    resolve_ssh_config(source, home, "local-user", Some(alias), overrides).unwrap()
}

#[test]
fn explicit_identity_override_count_is_bounded_before_deduplication() {
    let home = tempfile::tempdir().unwrap();
    let overrides = SshConfigOverrides {
        identity_files: Some(vec!["~/.ssh/same".into(); 7]),
        ..Default::default()
    };
    assert!(resolve_ssh_config("Host app", home.path(), "local", Some("app"), &overrides).is_err());
}

#[test]
fn explicit_identity_override_path_is_bounded_like_source_lines() {
    let home = tempfile::tempdir().unwrap();
    let overrides = SshConfigOverrides {
        identity_files: Some(vec![format!("~/.ssh/{}", "x".repeat(8193))]),
        ..Default::default()
    };
    assert!(resolve_ssh_config("Host app", home.path(), "local", Some("app"), &overrides).is_err());
}

#[tokio::test]
async fn imported_file_identities_use_the_original_service_and_one_real_ssh_connection() {
    use deepagent_ssh::{SshConnectionAttempt, SshService};
    use std::sync::atomic::Ordering;
    for key_count in [1, 2] {
        let server = support::SshServer::start("127.0.0.1").await;
        *server.counts.auth_rejection_time.write().unwrap() = Some(std::time::Duration::ZERO);
        *server.counts.authentication_factors.write().unwrap() = vec![russh::MethodKind::PublicKey];
        let home = tempfile::tempdir().unwrap();
        let ssh_dir = home.path().join(".ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        let mut fingerprints = Vec::new();
        let mut source = format!(
            "Host app\n HostName 127.0.0.1\n User fixture\n Port {}\n",
            server.addr.port()
        );
        for index in 0..key_count {
            let key = support::random_key();
            fingerprints.push(key.public_key().fingerprint(Default::default()));
            key.write_openssh_file(ssh_dir.join(format!("identity{index}")), Default::default())
                .unwrap();
            source.push_str(&format!(" IdentityFile ~/.ssh/identity{index}\n"));
        }
        server
            .counts
            .factor_keys
            .write()
            .unwrap()
            .insert(0, fingerprints.last().unwrap().clone());
        std::fs::write(ssh_dir.join("config"), source).unwrap();
        let preview = preview_ssh_config_in(
            &ssh_dir.join("config"),
            home.path(),
            "local",
            Some("app"),
            &Default::default(),
        )
        .unwrap();
        let target = preview.connection.unwrap().target;
        server.write_trust(&home.path().join("known_hosts"));
        let service = SshService::new(home.path().into());
        let mut request = serde_json::json!({
            "name":"imported app", "host":target.host, "port":target.port, "username":target.username,
            "auth_type":if key_count == 1 { "key_file" } else { "chain" },
            "known_hosts_path":home.path().join("known_hosts"),
        });
        if key_count == 1 {
            request["key_path"] = serde_json::json!(target.identity_files[0]);
        } else {
            request["authentication_factors"] = serde_json::json!(target
                .identity_files
                .iter()
                .map(|path| serde_json::json!({"auth_type":"key_file", "key_path":path}))
                .collect::<Vec<_>>());
        }
        let saved = service
            .create_connection(serde_json::from_value(request).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            service.begin_connection(&saved.id).await.unwrap(),
            SshConnectionAttempt::Connected
        ));
        assert_eq!(*server.counts.received_keys.read().unwrap(), fingerprints);
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
        assert_eq!(server.counts.passwords.load(Ordering::SeqCst), 0);
        service.disconnect(&saved.id).await.unwrap();
    }
}

#[test]
fn first_matching_scalars_and_accumulated_identities_are_distinct() {
    let home = tempfile::tempdir().unwrap();
    let source = "Host app other !excluded\n HostName server.invalid\n User first\n Port 2201\n IdentityFile ~/.ssh/first\nHost *\n User fallback\n Port 22\n IdentityFile ~/.ssh/second\n";
    let preview = resolve(source, home.path(), "app", &SshConfigOverrides::default());
    assert_eq!(preview.aliases, ["app", "other"]);
    let connection = preview.connection.unwrap();
    assert_eq!(
        (
            connection.target.host.as_str(),
            connection.target.username.as_str(),
            connection.target.port
        ),
        ("server.invalid", "first", 2201)
    );
    assert_eq!(
        connection.target.identity_files,
        [
            home.path().join(".ssh/first").to_str().unwrap(),
            home.path().join(".ssh/second").to_str().unwrap()
        ]
    );
    assert!(!connection.target.default_identities);
}

#[test]
fn host_globs_are_case_insensitive_and_negation_vetoes_the_whole_block() {
    let home = tempfile::tempdir().unwrap();
    let source = "Host APP? !app2\n User special\nHost *\n User fallback\n";
    for (alias, expected) in [
        ("App1", "special"),
        ("app2", "fallback"),
        ("else", "fallback"),
    ] {
        assert_eq!(
            resolve(source, home.path(), alias, &SshConfigOverrides::default())
                .connection
                .unwrap()
                .target
                .username,
            expected
        );
    }
}

#[test]
fn quoted_paths_comments_equals_and_tokens_resolve_after_explicit_overrides() {
    let home = tempfile::tempdir().unwrap();
    let source = "hOsT app\n HostName=imported.invalid\n User imported\n Port=2201\n IdentityFile \"~/.ssh/%h %r %p %n %%\" # comment\n";
    let overrides = SshConfigOverrides {
        host: Some("override.invalid".into()),
        port: Some(22),
        username: Some("override".into()),
        ..Default::default()
    };
    let target = resolve(source, home.path(), "app", &overrides)
        .connection
        .unwrap()
        .target;
    assert_eq!(
        target.identity_files,
        [home
            .path()
            .join(".ssh/override.invalid override 22 app %")
            .to_str()
            .unwrap()]
    );
    assert_eq!(target.port, 22);
}

#[test]
fn single_jump_alias_resolves_its_own_identity_and_inline_overrides() {
    let home = tempfile::tempdir().unwrap();
    let source = "Host app\n HostName target.invalid\n User target-user\n ProxyJump override@bastion:2209\nHost bastion\n HostName jump.invalid\n User jump-user\n Port 2202\n IdentityFile ~/.ssh/jump\n";
    let connection = resolve(source, home.path(), "app", &Default::default())
        .connection
        .unwrap();
    let jump = connection.jump.unwrap();
    assert_eq!(
        (jump.host.as_str(), jump.username.as_str(), jump.port),
        ("jump.invalid", "override", 2209)
    );
    assert_eq!(
        jump.identity_files,
        [home.path().join(".ssh/jump").to_str().unwrap()]
    );
    assert_eq!(connection.target.username, "target-user");
}

#[test]
fn ipv6_jump_and_explicit_no_jump_are_supported() {
    let home = tempfile::tempdir().unwrap();
    let source = "Host app\n ProxyJump jump-user@[::1]:2222\n";
    let connection = resolve(source, home.path(), "app", &Default::default())
        .connection
        .unwrap();
    let jump = connection.jump.unwrap();
    assert_eq!(
        (jump.host.as_str(), jump.port, jump.username.as_str()),
        ("::1", 2222, "jump-user")
    );
    let overrides = SshConfigOverrides {
        proxy_jump: Some("none".into()),
        ..Default::default()
    };
    assert!(resolve(source, home.path(), "app", &overrides)
        .connection
        .unwrap()
        .jump
        .is_none());
}

#[test]
fn none_disables_default_file_discovery_and_explicit_files_replace_imported_ones() {
    let home = tempfile::tempdir().unwrap();
    let target = resolve(
        "Host app\n IdentityFile none\n",
        home.path(),
        "app",
        &Default::default(),
    )
    .connection
    .unwrap()
    .target;
    assert!(!target.default_identities);
    assert!(target.identity_files.is_empty());
    let overrides = SshConfigOverrides {
        identity_files: Some(vec!["~/.ssh/override".into()]),
        ..Default::default()
    };
    let target = resolve(
        "Host app\n IdentityFile ~/.ssh/imported\n",
        home.path(),
        "app",
        &overrides,
    )
    .connection
    .unwrap()
    .target;
    assert_eq!(
        target.identity_files,
        [home.path().join(".ssh/override").to_str().unwrap()]
    );
}

#[test]
fn unsupported_active_declarations_are_reported_with_line_and_without_command_contents() {
    let home = tempfile::tempdir().unwrap();
    for directive in [
        "ProxyCommand",
        "LocalCommand",
        "Include",
        "Match",
        "CertificateFile",
        "StrictHostKeyChecking",
    ] {
        let source = format!("Host app\n {directive} secret-command-value\n");
        let error = resolve_ssh_config(
            &source,
            home.path(),
            "local",
            Some("app"),
            &Default::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("line 2") && error.to_lowercase().contains(&directive.to_lowercase()),
            "{error}"
        );
        assert!(!error.contains("secret-command-value"));
    }
    // A nonmatching block must not poison an unrelated alias.
    assert!(resolve_ssh_config(
        "Host other\n ProxyCommand ignored\nHost app\n User app\n",
        home.path(),
        "local",
        Some("app"),
        &Default::default()
    )
    .is_ok());
}

#[test]
fn ambiguous_routes_invalid_tokens_and_unbounded_inputs_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    for source in [
        "Host app\n ProxyJump one,two\n",
        "Host app\n ProxyJump bastion\nHost bastion\n ProxyJump app\n",
        "Host app\n ProxyJump app\n",
        "Host app\n Port 0\n",
        "Host app\n Port 65536\n",
        "Host app\n IdentityFile relative-key\n",
        "Host app\n IdentityFile ~/.ssh/${SECRET}\n",
        "Host app\n IdentityFile ~/.ssh/%x\n",
        "Host app\n IdentityFile \"unterminated\n",
        "Host app\n User first second\n",
    ] {
        assert!(
            resolve_ssh_config(
                source,
                home.path(),
                "local",
                Some("app"),
                &Default::default()
            )
            .is_err(),
            "accepted {source:?}"
        );
    }
    let too_many = format!(
        "Host app\n{}",
        (0..7)
            .map(|i| format!("IdentityFile ~/.ssh/id{i}\n"))
            .collect::<String>()
    );
    assert!(resolve_ssh_config(
        &too_many,
        home.path(),
        "local",
        Some("app"),
        &Default::default()
    )
    .is_err());
    assert!(resolve_ssh_config(
        &"#".repeat(1024 * 1024 + 1),
        home.path(),
        "local",
        None,
        &Default::default()
    )
    .is_err());
}

#[test]
fn file_preview_is_bounded_and_does_not_read_or_create_identity_files() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config");
    std::fs::write(&config, "Host app\n IdentityFile ~/.ssh/missing\n").unwrap();
    let preview = preview_ssh_config_in(
        &config,
        home.path(),
        "local",
        Some("app"),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(preview.path, config.to_str().unwrap());
    assert_eq!(preview.aliases, ["app"]);
    assert!(!home.path().join(".ssh").exists());
    std::fs::write(&config, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(
        preview_ssh_config_in(&config, home.path(), "local", None, &Default::default()).is_err()
    );
    std::fs::write(&config, [0xff, 0xfe]).unwrap();
    assert!(
        preview_ssh_config_in(&config, home.path(), "local", None, &Default::default()).is_err()
    );
}
