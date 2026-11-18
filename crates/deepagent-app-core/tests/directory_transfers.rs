#[path = "../../deepagent-ssh/tests/support/mod.rs"]
mod support;

use deepagent_app_core::file_transfer_service::{
    TransferCoordinator, TransferRequest, TransferState,
};
use deepagent_persistence::Database;
use deepagent_ssh::{FileConflictPolicy, SshService};
use std::{path::Path, sync::Arc};

fn request(id: &str, path: &Path, direction: &str, conflict: &str) -> TransferRequest {
    serde_json::from_value(serde_json::json!({
        "connectionId": id, "kind":"directory", "directoryConflict": conflict,
        "transfer": {"direction":direction, "localPath":path.to_str().unwrap(), "remotePath":"/tree", "conflict":"ask"}
    })).expect("the existing queue must accept a typed directory request")
}

async fn terminal(queue: &TransferCoordinator, id: &str) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), std::time::Duration::from_secs(1))
                .await
                .unwrap();
            if let Some(task) = snapshot
                .tasks
                .iter()
                .find(|t| t.id == id && t.state.terminal())
            {
                return serde_json::to_value(task).unwrap();
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .expect("directory queue should reach a terminal state")
}

async fn fixture() -> (
    support::SshServer,
    tempfile::TempDir,
    Arc<SshService>,
    String,
    Arc<Database>,
    TransferCoordinator,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let trust = dir.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = Arc::new(SshService::new(dir.path().to_owned()));
    let id = server.create(&ssh, Some(&trust)).await.id;
    let db = Arc::new(Database::open(dir.path().join("queue.sqlite")).unwrap());
    let queue = TransferCoordinator::new(db.clone(), ssh.clone()).unwrap();
    (server, dir, ssh, id, db, queue)
}

#[tokio::test]
async fn keep_both_decisions_and_actual_targets_survive_in_the_existing_history() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("data.txt"), b"new").unwrap();
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
        .insert("/tree/data.txt".into(), b"original".to_vec());
    let mut merge = request(&id, &input, "upload", "merge");
    merge.transfer.conflict = serde_json::from_str("\"keep_both\"").unwrap();
    let merged = queue.submit(merge).await.unwrap();
    let done = terminal(&queue, &merged.id).await;
    assert_eq!(done["directoryReceipt"]["destinationPath"], "/tree");
    assert_eq!(done["directoryReceipt"]["renamedFiles"], 1);
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data.txt"],
        b"original"
    );
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data (1).txt"],
        b"new"
    );
    let separate = queue
        .submit(request(&id, &input, "upload", "keep_both"))
        .await
        .unwrap();
    let done = terminal(&queue, &separate.id).await;
    assert_eq!(done["directoryReceipt"]["destinationPath"], "/tree (1)");
    assert_eq!(
        server.counts.files.read().unwrap()["/tree (1)/data.txt"],
        b"new"
    );
    drop(queue);
    let recovered = TransferCoordinator::new(db, ssh).unwrap();
    let tasks = recovered.snapshot().await.unwrap();
    let separate = tasks.iter().find(|task| task.id == separate.id).unwrap();
    let json = serde_json::to_value(separate).unwrap();
    assert_eq!(json["request"]["directoryConflict"], "keep_both");
    assert_eq!(json["directoryReceipt"]["destinationPath"], "/tree (1)");
}

#[tokio::test]
async fn directory_roundtrip_persists_aggregate_receipts_in_the_existing_queue() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir_all(input.join("空目录")).unwrap();
    std::fs::write(input.join("中文.bin"), b"binary\0\xff").unwrap();
    std::fs::write(input.join("zero"), []).unwrap();
    let upload = queue
        .submit(request(&id, &input, "upload", "ask"))
        .await
        .unwrap();
    assert_eq!(upload.total_files, None, "unknown until planning completes");
    let up = terminal(&queue, &upload.id).await;
    assert_eq!(up["state"], "completed");
    assert_eq!(up["bytes"], "8");
    assert_eq!(up["totalFiles"], 2);
    assert_eq!(up["completedFiles"], 2);
    assert_eq!(up["directoryReceipt"]["createdDirectories"], 2);
    let output = dir.path().join("output");
    let down = queue
        .submit(request(&id, &output, "download", "ask"))
        .await
        .unwrap();
    assert_eq!(terminal(&queue, &down.id).await["state"], "completed");
    assert_eq!(
        std::fs::read(output.join("中文.bin")).unwrap(),
        b"binary\0\xff"
    );
    assert!(output.join("空目录").is_dir());
    assert_eq!(server.auth_count(), 1);
    drop(queue);
    let recovered = TransferCoordinator::new(db, ssh).unwrap();
    let tasks = recovered.snapshot().await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|t| t.state == TransferState::Completed));
    let persisted = serde_json::to_value(&tasks[0]).unwrap();
    assert_eq!(persisted["request"]["kind"], "directory");
    assert_eq!(persisted["directoryReceipt"]["completedFiles"], 2);
    assert_eq!(persisted["speedBps"], serde_json::Value::Null);
}

#[tokio::test]
async fn restart_marks_a_durable_directory_attempt_interrupted_without_replaying_network_work() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("data"), b"fixture").unwrap();
    let task = queue
        .submit(request(&id, &input, "upload", "ask"))
        .await
        .unwrap();
    let mut durable = terminal(&queue, &task.id).await;
    drop(queue);
    durable["state"] = "running".into();
    durable["directoryReceipt"] = serde_json::Value::Null;
    durable["speedBps"] = "999".into();
    deepagent_persistence::document_store::DocumentStore::new(&db)
        .put(
            "remote_transfers_v1",
            "queue",
            &serde_json::json!({"version":1,"tasks":[durable]}).to_string(),
            None,
            deepagent_core::clock::Timestamp::EPOCH,
        )
        .unwrap();
    let before = server.counts.files.read().unwrap().clone();
    let recovered = TransferCoordinator::new(db, ssh).unwrap();
    let task = recovered.snapshot().await.unwrap().remove(0);
    assert_eq!(task.state, TransferState::Failed);
    assert_eq!(task.error.unwrap().code, "interrupted");
    assert_eq!(task.speed_bps, None);
    assert_eq!(
        serde_json::to_value(task.request).unwrap()["kind"],
        "directory"
    );
    assert_eq!(*server.counts.files.read().unwrap(), before);
}

#[tokio::test]
async fn failure_is_partial_and_retry_replans_with_independent_skip_policy() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a"), b"first").unwrap();
    std::fs::write(input.join("b"), b"second").unwrap();
    server
        .counts
        .reject_publish_paths
        .write()
        .unwrap()
        .insert("/tree/b".into());
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let failed = terminal(&queue, &task.id).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"]["relativePath"], "b");
    assert_eq!(failed["completedFiles"], 1);
    assert!(failed["directoryReceipt"].is_null());
    // Retry is a new plan, and never reuses a damaged stream or overwrites file a.
    std::fs::write(input.join("c"), b"new on retry").unwrap();
    server.counts.reject_publish_paths.write().unwrap().clear();
    let retried = queue
        .retry(&task.id, FileConflictPolicy::Skip)
        .await
        .unwrap();
    assert_eq!(retried.attempt, 2);
    assert_eq!(retried.total_files, None);
    assert_eq!(retried.completed_files, 0);
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed");
    assert_eq!(done["totalFiles"], 3);
    assert_eq!(done["completedFiles"], 2);
    assert_eq!(done["skippedFiles"], 1);
    assert_eq!(server.counts.files.read().unwrap()["/tree/a"], b"first");
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/c"],
        b"new on retry"
    );
}

#[tokio::test]
async fn directories_share_the_lane_and_cancellation_waits_for_real_staging_cleanup() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a"), b"first").unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.stage_open_gate.write().unwrap() = Some(gate.clone());
    let first_req = request(&id, &input, "upload", "merge");
    let first = queue.submit(first_req.clone()).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        server.counts.stage_created.notified(),
    )
    .await
    .unwrap();
    assert_eq!(queue.submit(first_req).await.unwrap().id, first.id);
    let other = dir.path().join("single");
    std::fs::write(&other, b"next").unwrap();
    let second = queue.submit(serde_json::from_value(serde_json::json!({
        "connectionId":id, "transfer":{"direction":"upload", "localPath":other.to_str().unwrap(), "remotePath":"/single"}
    })).unwrap()).await.unwrap();
    assert_eq!(
        queue.cancel(&first.id).await.unwrap().state,
        TransferState::Cancelling
    );
    assert_eq!(
        queue
            .snapshot()
            .await
            .unwrap()
            .iter()
            .find(|t| t.id == second.id)
            .unwrap()
            .state,
        TransferState::Queued
    );
    assert!(!server.counts.files.read().unwrap().contains_key("/single"));
    *server.counts.stage_open_gate.write().unwrap() = None;
    gate.notify_one();
    let cancelled = terminal(&queue, &first.id).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["completedFiles"], 0);
    assert_eq!(terminal(&queue, &second.id).await["state"], "completed");
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.ends_with(".part")));
}
