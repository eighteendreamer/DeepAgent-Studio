mod support;
use deepagent_ssh::{DirectoryWalkLimits, RemoteChmodRequest, SshService};
use russh_sftp::protocol::FileAttributes;
use std::sync::{atomic::Ordering, Arc};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn recursive_chmod_previews_skips_links_and_changes_children_before_parents() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = SshService::new(data.path().into());
    let id = server.create(&ssh, Some(&trust)).await.id;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree", "/tree/sub"].map(String::from));
    server.counts.files.write().unwrap().extend([
        ("/tree/sub/中文 file".into(), vec![7]),
        ("/tree/link".into(), vec![]),
    ]);
    server.counts.file_attributes.write().unwrap().insert(
        "/tree/link".into(),
        FileAttributes {
            permissions: Some(0o120777),
            ..FileAttributes::empty()
        },
    );
    let p = ssh
        .preview_chmod(
            &id,
            "/tree",
            0o600,
            true,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        (
            p.total_entries,
            p.total_files,
            p.total_directories,
            p.total_symlinks
        ),
        (4, 1, 2, 1)
    );
    assert!(server.counts.chmod_requests.read().unwrap().is_empty());
    let result = ssh
        .execute_chmod(
            RemoteChmodRequest::from(&p),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!((result.changed_entries, result.skipped_symlinks), (3, 1));
    let requests = server.counts.chmod_requests.read().unwrap();
    assert_eq!(requests.last().unwrap().0, "/tree");
    assert!(!requests.iter().any(|(p, _)| p.ends_with("link")));
    assert!(requests.iter().all(|(_, m)| m & 0o7777 == 0o600));
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn chmod_rejects_aliases_unknown_types_stale_facts_and_invalid_modes_before_mutation() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = SshService::new(data.path().into());
    let id = server.create(&ssh, Some(&trust)).await.id;
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), vec![1]);
    for (path, mode) in [
        ("/unknown", 0o644),
        ("/link", 0o644),
        ("/", 0o644),
        ("/file", 0o100644),
        ("/../file", 0o644),
    ] {
        assert!(
            ssh.preview_chmod(
                &id,
                path,
                mode,
                false,
                DirectoryWalkLimits::default(),
                CancellationToken::new()
            )
            .await
            .is_err(),
            "{path}"
        );
    }
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/alias", "/alias/sub"].map(String::from));
    server
        .counts
        .canonical_paths
        .write()
        .unwrap()
        .insert("/alias".into(), "/outside".into());
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/alias/sub/file".into(), vec![1]);
    assert!(ssh
        .preview_chmod(
            &id,
            "/alias/sub/file",
            0o644,
            false,
            DirectoryWalkLimits::default(),
            CancellationToken::new()
        )
        .await
        .is_err());
    let p = ssh
        .preview_chmod(
            &id,
            "/file",
            0o600,
            false,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), vec![1, 2]);
    assert!(ssh
        .execute_chmod(
            RemoteChmodRequest::from(&p),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {})
        )
        .await
        .is_err());
    assert!(server.counts.chmod_requests.read().unwrap().is_empty());
}

#[tokio::test]
async fn recursive_quota_and_cancel_are_fail_closed() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("trust");
    server.write_trust(&trust);
    let ssh = SshService::new(data.path().into());
    let id = server.create(&ssh, Some(&trust)).await.id;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server
        .counts
        .files
        .write()
        .unwrap()
        .extend([("/tree/a".into(), vec![]), ("/tree/b".into(), vec![])]);
    assert!(ssh
        .preview_chmod(
            &id,
            "/tree",
            0o700,
            true,
            DirectoryWalkLimits {
                max_entries: 2,
                max_depth: 4
            },
            CancellationToken::new()
        )
        .await
        .is_err());
    let token = CancellationToken::new();
    token.cancel();
    assert!(ssh
        .preview_chmod(
            &id,
            "/tree",
            0o700,
            true,
            DirectoryWalkLimits::default(),
            token
        )
        .await
        .is_err());
    assert!(server.counts.chmod_requests.read().unwrap().is_empty());
}
