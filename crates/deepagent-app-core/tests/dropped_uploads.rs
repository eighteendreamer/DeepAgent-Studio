#[path = "../../deepagent-ssh/tests/support/mod.rs"]
mod support;

use deepagent_app_core::file_transfer_service::{
    TransferCoordinator, TransferKind, TransferRequest, TransferState, TransferTask,
};
use deepagent_persistence::Database;
use deepagent_ssh::SshService;
use std::{path::Path, sync::Arc, time::Duration};

fn request(id: &str, local: &Path, remote: &str) -> TransferRequest {
    serde_json::from_value(serde_json::json!({
        "connectionId": id,
        "transfer": { "direction":"upload", "localPath":local, "remotePath":remote }
    }))
    .unwrap()
}

async fn fixture() -> (
    support::SshServer,
    tempfile::TempDir,
    String,
    Arc<Database>,
    TransferCoordinator,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let temp = tempfile::tempdir().unwrap();
    let trust = temp.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = Arc::new(SshService::new(temp.path().to_owned()));
    let id = server.create(&ssh, Some(&trust)).await.id;
    let db = Arc::new(Database::open(temp.path().join("queue.sqlite")).unwrap());
    let queue = TransferCoordinator::new(db.clone(), ssh).unwrap();
    (server, temp, id, db, queue)
}

async fn observe(queue: &TransferCoordinator, id: &str, pending: bool) -> TransferTask {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), Duration::from_secs(1))
                .await
                .unwrap();
            let task = snapshot.tasks.iter().find(|task| task.id == id).unwrap();
            if task.state.terminal() || (pending && task.pending_directory_conflict.is_some()) {
                return task.clone();
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .expect("drop must reach the expected queue state")
}

#[tokio::test]
async fn mixed_real_sources_use_exact_paths_and_existing_file_and_directory_executors() {
    let (server, temp, id, _db, queue) = fixture().await;
    let file = temp.path().join("same");
    let directory = temp.path().join("nested").join("same");
    std::fs::write(&file, b"standalone").unwrap();
    std::fs::create_dir_all(directory.join("empty")).unwrap();
    std::fs::write(directory.join("inside.txt"), b"directory content").unwrap();
    for (local, remote, kind) in [
        (&file, "/standalone", TransferKind::File),
        (&directory, "/folder", TransferKind::Directory),
    ] {
        let task = queue
            .submit_detected_upload(request(&id, local, remote))
            .await
            .unwrap();
        assert_eq!(task.request.kind, kind);
        let done = observe(&queue, &task.id, false).await;
        assert_eq!(done.state, TransferState::Completed, "{done:?}");
        assert_eq!(done.request.kind, kind);
        assert_eq!(
            done.directory_receipt.is_some(),
            kind == TransferKind::Directory
        );
    }
    assert_eq!(
        server.counts.files.read().unwrap()["/standalone"],
        b"standalone"
    );
    assert_eq!(
        server.counts.files.read().unwrap()["/folder/inside.txt"],
        b"directory content"
    );
    assert!(server
        .counts
        .directories
        .read()
        .unwrap()
        .contains("/folder/empty"));
}

#[tokio::test]
async fn detected_directory_keeps_ask_deduplicates_and_persists_only_resolved_kind() {
    let (server, temp, id, db, queue) = fixture().await;
    let directory = temp.path().join("directory.txt");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("new"), b"new").unwrap();
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/existing".into());
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/existing/old".into(), b"keep".to_vec());
    let input = request(&id, &directory, "/existing");
    let task = queue.submit_detected_upload(input.clone()).await.unwrap();
    let pending = observe(&queue, &task.id, true).await;
    assert_eq!(pending.state, TransferState::Running);
    assert!(pending.pending_directory_conflict.is_some());
    assert_eq!(
        queue.submit_detected_upload(input).await.unwrap().id,
        task.id
    );
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/existing/new"));
    queue.cancel(&task.id).await.unwrap();
    assert_eq!(
        observe(&queue, &task.id, false).await.state,
        TransferState::Cancelled
    );
    assert_eq!(
        server.counts.files.read().unwrap()["/existing/old"],
        b"keep"
    );
    drop(queue);
    let restored =
        TransferCoordinator::new(db, Arc::new(SshService::new(temp.path().to_owned()))).unwrap();
    let history = restored.snapshot().await.unwrap();
    assert_eq!(history[0].request.kind, TransferKind::Directory);
    let wire = serde_json::to_value(&history[0]).unwrap();
    assert_eq!(wire["request"]["kind"], "directory");
    assert!(wire["request"].get("detectLocalKind").is_none());
}

#[tokio::test]
async fn invalid_sources_and_download_detection_never_enter_queue() {
    let (_server, temp, id, _db, queue) = fixture().await;
    for local in [
        temp.path().join("missing"),
        "relative".into(),
        temp.path().ancestors().last().unwrap().to_owned(),
    ] {
        assert!(queue
            .submit_detected_upload(request(&id, &local, "/invalid"))
            .await
            .is_err());
    }
    let file = temp.path().join("file");
    std::fs::write(&file, b"local").unwrap();
    let mut download = request(&id, &file, "/invalid");
    download.transfer.direction = deepagent_ssh::FileTransferDirection::Download;
    assert!(queue.submit_detected_upload(download).await.is_err());
    assert!(queue.snapshot().await.unwrap().is_empty());
}

#[cfg(windows)]
#[tokio::test]
async fn directory_junction_is_rejected_without_following_external_source() {
    let (_server, temp, id, _db, queue) = fixture().await;
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("sentinel"), b"keep").unwrap();
    let link = temp.path().join("junction");
    assert!(std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(external.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(queue
        .submit_detected_upload(request(&id, &link, "/junction"))
        .await
        .is_err());
    assert!(queue.snapshot().await.unwrap().is_empty());
    assert_eq!(
        std::fs::read(external.path().join("sentinel")).unwrap(),
        b"keep"
    );
    std::fs::remove_dir(link).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_link_is_rejected_without_following_source() {
    let (_server, temp, id, _db, queue) = fixture().await;
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert!(queue
        .submit_detected_upload(request(&id, &link, "/link"))
        .await
        .is_err());
    assert!(queue.snapshot().await.unwrap().is_empty());
}
