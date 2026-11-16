use deepagent_persistence::Database;
use deepagent_ssh::{SshHostIdentity, SshHostKeyStore, SshService};
use russh::keys::{ssh_key::rand_core::OsRng, Algorithm, PrivateKey};
use std::sync::Arc;

fn public() -> russh::keys::PublicKey {
    PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .unwrap()
        .public_key()
        .clone()
}

async fn fixture(input: &str) -> (tempfile::TempDir, SshService, Arc<SshHostKeyStore>, String) {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("known_hosts");
    std::fs::write(&file, input).unwrap();
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(temp.path().into()).with_host_key_store(store.clone());
    let connection = service
        .create_connection(
            serde_json::from_value(serde_json::json!({
                "name":"import-fixture", "host":"selected.invalid", "port":22, "username":"fixture",
                "auth_type":"agent", "known_hosts_path":file,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    (temp, service, store, connection.id)
}

#[tokio::test]
async fn whole_file_import_reports_every_skip_and_never_overwrites_existing_trust() {
    let key = public();
    let revoked = public();
    let plain = key.to_openssh().unwrap();
    let revoked_text = revoked.to_openssh().unwrap();
    let input = format!("# fixture only\none.invalid {plain}\ntwo.invalid,three.invalid {plain}\n[::1]:2222 {plain}\none.invalid {plain}\n|1|aDEyNS10ZXN0LW9ubHktc2FsdA==|BaHjvW3QDT36o/gmQ/OjXFd7354= {plain}\n*.invalid {plain}\n!excluded.invalid {plain}\n@cert-authority ca.invalid {plain}\n@revoked revoked.invalid {revoked_text}\nordinary.invalid {revoked_text}\nbroken-entry\nexisting.invalid {plain}\n");
    let (_temp, service, store, id) = fixture(&input).await;
    let existing = SshHostIdentity::direct("existing.invalid", 22).unwrap();
    let existing_key = public();
    store
        .trust_verified(&existing, &existing_key, "previously-verified")
        .unwrap();
    let preview = service.preview_known_hosts_import(&id).await.unwrap();
    assert_eq!(preview.importable, 4);
    assert_eq!(preview.skipped, 9);
    assert_eq!(preview.rows.len(), 13);
    let statuses: Vec<_> = preview
        .rows
        .iter()
        .map(|row| {
            serde_json::to_value(row).unwrap()["status"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    for status in [
        "duplicate",
        "hashed_host",
        "unsupported_pattern",
        "cert_authority",
        "revoked_marker",
        "revoked_source_key",
        "malformed",
        "existing_trust",
    ] {
        assert!(
            statuses.iter().any(|value| value == status),
            "missing {status}"
        );
    }
    assert!(
        store
            .list(&SshHostIdentity::direct("one.invalid", 22).unwrap())
            .unwrap()
            .is_empty(),
        "preview is read-only"
    );
    let result = service
        .import_known_hosts(&id, preview.clone())
        .await
        .unwrap();
    assert_eq!(result.imported, 4);
    assert_eq!(result.skipped, 9);
    for (host, port) in [
        ("one.invalid", 22),
        ("two.invalid", 22),
        ("three.invalid", 22),
        ("::1", 2222),
    ] {
        assert_eq!(
            store
                .list(&SshHostIdentity::direct(host, port).unwrap())
                .unwrap()
                .len(),
            1
        );
    }
    let retained = store.list(&existing).unwrap();
    assert_eq!(
        retained[0].fingerprint,
        existing_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string()
    );
    assert!(
        service.import_known_hosts(&id, preview).await.is_err(),
        "stale repeated confirmation cannot reapply"
    );
    assert_eq!(
        std::fs::read_to_string(_temp.path().join("known_hosts")).unwrap(),
        input,
        "never edits the source file"
    );
}

#[tokio::test]
async fn changing_source_or_store_after_preview_refuses_the_entire_import() {
    let key = public().to_openssh().unwrap();
    let input = format!("one.invalid {key}\ntwo.invalid {key}\n");
    let (temp, service, store, id) = fixture(&input).await;
    let preview = service.preview_known_hosts_import(&id).await.unwrap();
    std::fs::write(
        temp.path().join("known_hosts"),
        format!("{input}# changed after review\n"),
    )
    .unwrap();
    assert!(service.import_known_hosts(&id, preview).await.is_err());
    let preview = service.preview_known_hosts_import(&id).await.unwrap();
    store
        .trust_verified(
            &SshHostIdentity::direct("one.invalid", 22).unwrap(),
            &public(),
            "concurrent",
        )
        .unwrap();
    assert!(service.import_known_hosts(&id, preview).await.is_err());
    assert!(store
        .list(&SshHostIdentity::direct("two.invalid", 22).unwrap())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn source_revocation_and_conflicting_keys_never_become_importable() {
    let key = public().to_openssh().unwrap();
    let other = public().to_openssh().unwrap();
    let input = format!("one.invalid {key}\n@revoked *.invalid {key}\nconflict.invalid {other}\nconflict.invalid {}\n",public().to_openssh().unwrap());
    let (_temp, service, store, id) = fixture(&input).await;
    let preview = service.preview_known_hosts_import(&id).await.unwrap();
    assert_eq!(preview.importable, 0);
    assert_eq!(preview.skipped, 4);
    let result = service.import_known_hosts(&id, preview).await.unwrap();
    assert_eq!(result.imported, 0);
    for host in ["one.invalid", "conflict.invalid"] {
        assert!(store
            .list(&SshHostIdentity::direct(host, 22).unwrap())
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn file_byte_and_expanded_row_limits_fail_without_partial_writes() {
    let (temp, service, store, id) = fixture(&"#".repeat(1024 * 1024 + 1)).await;
    assert!(service.preview_known_hosts_import(&id).await.is_err());
    let key = public().to_openssh().unwrap();
    let input = (0..4097)
        .map(|n| format!("host-{n}.invalid {key}\n"))
        .collect::<String>();
    std::fs::write(temp.path().join("known_hosts"), input).unwrap();
    assert!(service.preview_known_hosts_import(&id).await.is_err());
    assert!(store
        .list(&SshHostIdentity::direct("host-0.invalid", 22).unwrap())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn invalid_utf8_cannot_hide_a_source_revocation() {
    let key = public().to_openssh().unwrap();
    let (temp, service, store, id) = fixture(&format!("one.invalid {key}\n")).await;
    let mut bytes =
        format!("one.invalid {key}\n@revoked one.invalid {key} # comment ").into_bytes();
    bytes.push(0xff);
    std::fs::write(temp.path().join("known_hosts"), bytes).unwrap();
    assert!(
        service.preview_known_hosts_import(&id).await.is_err(),
        "do not skip a broken revocation and import the ordinary duplicate"
    );
    assert!(store
        .list(&SshHostIdentity::direct("one.invalid", 22).unwrap())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn existing_revocation_and_port_scopes_are_preserved_and_preview_cannot_be_edited() {
    let key = public();
    let text = key.to_openssh().unwrap();
    let input = format!("revoked.invalid {text}\n[revoked.invalid]:2222 {text}\n");
    let (_temp, service, store, id) = fixture(&input).await;
    let identity = SshHostIdentity::direct("revoked.invalid", 22).unwrap();
    store.trust_verified(&identity, &key, "fixture").unwrap();
    store
        .revoke(
            &identity,
            &key.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
        )
        .unwrap();
    let preview = service.preview_known_hosts_import(&id).await.unwrap();
    assert_eq!(preview.importable, 1);
    assert_eq!(preview.skipped, 1);
    let mut edited = preview.clone();
    edited.rows[1].port = Some(2200);
    assert!(service.import_known_hosts(&id, edited).await.is_err());
    assert!(store
        .list(&SshHostIdentity::direct("revoked.invalid", 2222).unwrap())
        .unwrap()
        .is_empty());
    let result = service.import_known_hosts(&id, preview).await.unwrap();
    assert_eq!(result.imported, 1);
    assert!(store.list(&identity).unwrap()[0].revoked);
}
