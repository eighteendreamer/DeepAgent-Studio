mod support;
use deepagent_ssh::{ConditionalUploadRequest, SshService};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio_util::sync::CancellationToken;

async fn fixture() -> (
    support::SshServer,
    tempfile::TempDir,
    Arc<SshService>,
    String,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let trust = dir.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = Arc::new(SshService::new(dir.path().into()));
    let connection = server.create(&ssh, Some(&trust)).await;
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/edit".into(), b"initial".to_vec());
    (server, dir, ssh, connection.id)
}

#[tokio::test]
async fn concurrent_remote_change_and_delete_pause_before_publication() {
    for delete in [false, true] {
        let (server, dir, ssh, id) = fixture().await;
        let (generation, hash) = ssh
            .file_content_version(&id, "/edit", CancellationToken::new())
            .await
            .unwrap();
        let local = dir.path().join("snapshot");
        std::fs::write(&local, b"local update").unwrap();
        let counts = server.counts.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let changed_clone = changed.clone();
        let result = ssh
            .conditional_upload(
                ConditionalUploadRequest {
                    connection_id: id.clone(),
                    connection_generation: generation,
                    local_path: local.to_string_lossy().into(),
                    remote_path: "/edit".into(),
                    expected_sha256: hash,
                },
                CancellationToken::new(),
                Arc::new(move |p| {
                    if p.bytes > 0 && !changed_clone.swap(true, Ordering::SeqCst) {
                        let mut files = counts.files.write().unwrap();
                        if delete {
                            files.remove("/edit");
                        } else {
                            files.insert("/edit".into(), b"remote update".to_vec());
                        }
                    }
                }),
            )
            .await;
        assert!(changed.load(Ordering::SeqCst));
        assert_eq!(result.unwrap_err().code, "conflict");
        let files = server.counts.files.read().unwrap();
        if delete {
            assert!(!files.contains_key("/edit"));
        } else {
            assert_eq!(files["/edit"], b"remote update");
        }
        assert!(!files.keys().any(|p| p.ends_with(".part")));
    }
}
#[tokio::test]
async fn matching_version_uploads_and_save_as_never_clobbers() {
    let (server, dir, ssh, id) = fixture().await;
    let (generation, hash) = ssh
        .file_content_version(&id, "/edit", CancellationToken::new())
        .await
        .unwrap();
    let local = dir.path().join("snapshot");
    std::fs::write(&local, b"local update").unwrap();
    let mut request = ConditionalUploadRequest {
        connection_id: id.clone(),
        connection_generation: generation,
        local_path: local.to_string_lossy().into(),
        remote_path: "/edit".into(),
        expected_sha256: hash,
    };
    let result = ssh
        .conditional_upload(request.clone(), CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(
        server.counts.files.read().unwrap()["/edit"],
        b"local update"
    );
    assert_eq!(result.bytes, "12");
    request.expected_sha256 = None;
    assert_eq!(
        ssh.conditional_upload(request.clone(), CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    request.remote_path = "/saved-as".into();
    ssh.conditional_upload(request, CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(
        server.counts.files.read().unwrap()["/saved-as"],
        b"local update"
    );
}
