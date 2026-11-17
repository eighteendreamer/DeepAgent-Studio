mod support;

use deepagent_ssh::{RemoteFileKind, SshService};

#[tokio::test]
async fn independent_lstat_stat_and_realpath_use_sftp_without_shell_commands() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/home/fixture/file.txt".into(), b"content".to_vec());
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&trust)).await;
    let handle = service.connect(&connection.id).await.unwrap();
    let link = service
        .file_metadata(&handle, "/link", false)
        .await
        .unwrap();
    assert_eq!(link.kind, RemoteFileKind::Symlink);
    assert!(!link.followed_symlinks);
    assert_eq!(link.size_bytes.as_deref(), Some("22"));
    assert_eq!((link.uid, link.gid), (Some(1001), Some(1002)));
    let target = service.file_metadata(&handle, "/link", true).await.unwrap();
    assert_eq!(target.kind, RemoteFileKind::File);
    assert!(target.followed_symlinks);
    assert_eq!(target.size_bytes.as_deref(), Some("7"));
    assert_eq!(target.permissions, Some(0o100640));
    assert_eq!(target.modified_ms, Some(1_700_000_000_000));
    let real = service.realpath(&handle, "./file.txt").await.unwrap();
    assert_eq!(real.canonical_path, "/home/fixture/file.txt");
    assert_eq!(real.path, "./file.txt");
    assert_eq!(real.connection_id, connection.id);
    assert_eq!(server.auth_count(), 1);
    assert_eq!(
        server
            .counts
            .commands
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    service.disconnect(&connection.id).await.unwrap();
}

#[tokio::test]
async fn metadata_preserves_large_sizes_missing_attributes_and_permission_errors() {
    let server = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let service = SshService::new(data.path().to_path_buf());
    let connection = server.create(&service, Some(&trust)).await;
    let handle = service.connect(&connection.id).await.unwrap();
    let huge = service
        .file_metadata(&handle, "/huge", false)
        .await
        .unwrap();
    let wire = serde_json::to_value(&huge).unwrap();
    assert_eq!(wire["size_bytes"], u64::MAX.to_string());
    let unknown = service
        .file_metadata(&handle, "/unknown", false)
        .await
        .unwrap();
    assert_eq!(unknown.kind, RemoteFileKind::Unknown);
    assert_eq!(
        (
            unknown.size_bytes,
            unknown.permissions,
            unknown.uid,
            unknown.gid
        ),
        (None, None, None, None)
    );
    assert!(service
        .file_metadata(&handle, "/denied", false)
        .await
        .is_err());
    assert!(service.realpath(&handle, "/missing").await.is_err());
    assert!(service
        .file_metadata(&handle, "bad\0path", false)
        .await
        .is_err());
    service.disconnect(&connection.id).await.unwrap();
}
