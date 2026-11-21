mod support;
use deepagent_ssh::{
    FileConflictPolicy, ServerCopyMethod, ServerCopyRequest, ServerCopySpec, SshService,
};
use russh_sftp::protocol::FileAttributes;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
async fn fixtures() -> (
    support::SshServer,
    support::SshServer,
    tempfile::TempDir,
    SshService,
    String,
    String,
) {
    let a = support::SshServer::start("127.0.0.1").await;
    let b = support::SshServer::start("127.0.0.1").await;
    let data = tempfile::tempdir().unwrap();
    let ta = data.path().join("a");
    let tb = data.path().join("b");
    a.write_trust(&ta);
    b.write_trust(&tb);
    let ssh = SshService::new(data.path().into());
    let ai = a.create(&ssh, Some(&ta)).await.id;
    let bi = b.create(&ssh, Some(&tb)).await.id;
    (a, b, data, ssh, ai, bi)
}
fn spec(a: &str, b: &str, source: &str, dest: &str) -> ServerCopySpec {
    ServerCopySpec {
        source_connection_id: a.into(),
        destination_connection_id: b.into(),
        source_path: source.into(),
        destination_path: dest.into(),
        conflict: FileConflictPolicy::Ask,
        method: ServerCopyMethod::Relay,
    }
}
#[tokio::test]
async fn relay_tree_preserves_empty_directories_and_binary_bytes() {
    let (a, b, _d, ssh, ai, bi) = fixtures().await;
    a.counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree", "/tree/empty", "/tree/sub"].map(String::from));
    let bytes = (0..200_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    a.counts
        .files
        .write()
        .unwrap()
        .insert("/tree/sub/中文 file\\name".into(), bytes.clone());
    let plan = ssh
        .preview_server_copy(spec(&ai, &bi, "/tree", "/copy"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(plan.total_files, 1);
    assert_eq!(plan.total_bytes, "200000");
    assert!(b.counts.files.read().unwrap().is_empty());
    let receipt = ssh
        .execute_server_copy(
            ServerCopyRequest::from(&plan),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(receipt.bytes, "200000");
    assert_eq!(
        b.counts.files.read().unwrap()["/copy/sub/中文 file\\name"],
        bytes
    );
    assert!(b.counts.directories.read().unwrap().contains("/copy/empty"));
    assert!(!b
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .any(|p| p.ends_with(".part")));
}
#[tokio::test]
async fn copy_conflicts_require_fresh_preview_and_keep_both_preserves_original() {
    let (a, b, _d, ssh, ai, bi) = fixtures().await;
    a.counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), b"source".to_vec());
    b.counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), b"original".to_vec());
    let mut input = spec(&ai, &bi, "/file", "/file");
    input.conflict = FileConflictPolicy::KeepBoth;
    let plan = ssh
        .preview_server_copy(input.clone(), CancellationToken::new())
        .await
        .unwrap();
    b.counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), b"updated content".to_vec());
    assert!(ssh
        .execute_server_copy(
            ServerCopyRequest::from(&plan),
            CancellationToken::new(),
            Arc::new(|_| {})
        )
        .await
        .is_err());
    let plan = ssh
        .preview_server_copy(input, CancellationToken::new())
        .await
        .unwrap();
    ssh.execute_server_copy(
        ServerCopyRequest::from(&plan),
        CancellationToken::new(),
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let files = b.counts.files.read().unwrap();
    assert_eq!(files["/file"], b"updated content");
    assert!(files
        .iter()
        .any(|(p, bytes)| p != "/file" && bytes == b"source"));
}
#[tokio::test]
async fn relay_rejects_links_self_descendants_and_cancellation_before_mutation() {
    let (a, b, _d, ssh, ai, bi) = fixtures().await;
    a.counts.file_attributes.write().unwrap().insert(
        "/link".into(),
        FileAttributes {
            permissions: Some(0o120777),
            ..FileAttributes::empty()
        },
    );
    assert!(ssh
        .preview_server_copy(spec(&ai, &bi, "/link", "/copy"), CancellationToken::new())
        .await
        .is_err());
    assert!(ssh
        .preview_server_copy(
            spec(&ai, &ai, "/tree", "/tree/sub"),
            CancellationToken::new()
        )
        .await
        .is_err());
    a.counts
        .files
        .write()
        .unwrap()
        .insert("/file".into(), vec![9]);
    let plan = ssh
        .preview_server_copy(spec(&ai, &bi, "/file", "/copy"), CancellationToken::new())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(ssh
        .execute_server_copy(ServerCopyRequest::from(&plan), cancel, Arc::new(|_| {}))
        .await
        .is_err());
    assert!(b.counts.files.read().unwrap().is_empty());
}
