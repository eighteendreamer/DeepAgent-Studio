#[path = "../../deepagent-ssh/tests/support/mod.rs"]
mod support;

use deepagent_app_core::file_transfer_service::{
    DirectoryConflictResponse, FileConflictResponse, TransferCoordinator, TransferRequest,
    TransferState, TransferTask,
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

#[tokio::test]
async fn real_directory_cleanup_diagnostics_cannot_overflow_the_admitted_task() {
    const FILES: usize = 150;
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = dir.path().join("diagnostic-input");
    // A virtual SFTP filesystem may accept this root within the public 16 KiB
    // path contract. Keep local fixtures flat and small; only remote facts are long.
    let remote = format!("/{}", "r".repeat(8 * 1024));
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert(remote.clone());
    std::fs::create_dir(&input).unwrap();
    for index in 0..FILES {
        let name = format!("file-{index:04}");
        std::fs::write(input.join(&name), b"new").unwrap();
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert(format!("{remote}/{name}"), b"old".to_vec());
    }
    server
        .counts
        .reject_file_backup_cleanup
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut input = request(&id, &input, "upload", "merge");
    input.transfer.remote_path = remote;
    input.transfer.conflict = FileConflictPolicy::Replace;
    let task = queue.submit(input).await.unwrap();
    let saved = tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), std::time::Duration::from_secs(1))
                .await
                .expect("real cleanup diagnostics must not destroy terminal persistence");
            if let Some(task) = snapshot
                .tasks
                .iter()
                .find(|t| t.id == task.id && t.state.terminal())
            {
                break serde_json::to_value(task).unwrap();
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .expect("bounded diagnostic handling must finish without timing out");
    assert_eq!(
        saved["state"], "failed",
        "unresolved cleanup stops later publication"
    );
    assert_eq!(saved["error"]["code"], "cleanup_required");
    let result = &saved["error"];
    let warnings = result["cleanupWarnings"].as_array().unwrap();
    assert!(!warnings.is_empty());
    assert!(
        saved.to_string().len() <= 1024 * 1024,
        "task persistence stays bounded"
    );
    {
        let files = server.counts.files.read().unwrap();
        let backups: Vec<_> = files
            .keys()
            .filter(|path| path.ends_with(".backup"))
            .collect();
        assert_eq!(
            warnings.len(),
            backups.len(),
            "every retained original needs its complete diagnostic"
        );
        for backup in backups {
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.as_str().unwrap().contains(backup.as_str())),
                "cleanup paths cannot be silently dropped or shortened"
            );
        }
        let published = files
            .iter()
            .filter(|(path, bytes)| !path.ends_with(".backup") && bytes.as_slice() == b"new")
            .count();
        let untouched = files
            .iter()
            .filter(|(path, bytes)| !path.ends_with(".backup") && bytes.as_slice() == b"old")
            .count();
        assert_eq!(published, warnings.len());
        assert_eq!(published + untouched, FILES);
        assert_eq!(
            saved["completedFiles"].as_u64().unwrap() as usize,
            published
        );
        assert_eq!(
            published, 1,
            "stop at the first unresolved backup before another file starts"
        );
        assert_eq!(untouched, FILES - 1);
        assert!(!files.keys().any(|path| path.ends_with(".part")));
    }
    drop(queue);
    let before = server
        .counts
        .file_opens
        .load(std::sync::atomic::Ordering::SeqCst);
    let reopened = TransferCoordinator::new(db, ssh).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.snapshot().await.unwrap().remove(0)).unwrap(),
        saved
    );
    assert_eq!(
        server
            .counts
            .file_opens
            .load(std::sync::atomic::Ordering::SeqCst),
        before,
        "reopening history must not replay completed or stopped publication"
    );
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

async fn waiting(queue: &TransferCoordinator, id: &str) -> TransferTask {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), std::time::Duration::from_secs(1))
                .await
                .unwrap();
            let task = snapshot.tasks.iter().find(|task| task.id == id).unwrap();
            assert!(!task.state.terminal(), "conflict should wait, got {task:?}");
            if task.pending_file_conflict.is_some() {
                return task.clone();
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .expect("a pending file conflict must become observable")
}

async fn root_waiting(queue: &TransferCoordinator, id: &str) -> TransferTask {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), std::time::Duration::from_secs(1))
                .await
                .unwrap();
            let task = snapshot.tasks.iter().find(|task| task.id == id).unwrap();
            let value = serde_json::to_value(task).unwrap();
            assert!(
                !task.state.terminal(),
                "directory root must wait, got {value}"
            );
            if !value["pendingDirectoryConflict"].is_null() {
                return task.clone();
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .expect("a pending directory-root conflict must become observable")
}

#[tokio::test]
async fn directory_roots_wait_in_the_current_attempt_without_publishing_and_cancel_cleanly() {
    for direction in ["upload", "download"] {
        let (server, dir, _ssh, id, _db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a"]);
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/original".into(), b"keep".to_vec());
        let task = queue
            .submit(request(&id, &input, direction, "ask"))
            .await
            .unwrap();
        let pending = root_waiting(&queue, &task.id).await;
        let json = serde_json::to_value(&pending).unwrap();
        assert_eq!(pending.state, TransferState::Running);
        assert_eq!(pending.attempt, 1);
        assert_eq!(pending.completed_files, 0);
        assert!(pending.pending_file_conflict.is_none());
        assert_eq!(
            json["pendingDirectoryConflict"]["conflict"]["direction"],
            direction
        );
        assert!(!server.counts.files.read().unwrap().contains_key("/tree/a"));
        assert!(!input.join("original").exists());
        queue.cancel(&task.id).await.unwrap();
        let done = terminal(&queue, &task.id).await;
        assert_eq!(done["state"], "cancelled");
        assert!(done["pendingDirectoryConflict"].is_null());
        assert_eq!(
            server.counts.files.read().unwrap()["/tree/original"],
            b"keep"
        );
        assert_eq!(std::fs::read(input.join("a")).unwrap(), b"new-a");
    }
}

fn response(task: &TransferTask, policy: &str, remaining: bool) -> FileConflictResponse {
    let pending = task.pending_file_conflict.as_ref().unwrap();
    serde_json::from_value(serde_json::json!({
        "version":pending.version,"id":pending.id,"attempt":pending.attempt,
        "decision":{"policy":policy,"applyToRemainingFiles":remaining}
    }))
    .unwrap()
}

fn root_response(task: &TransferTask, policy: &str) -> DirectoryConflictResponse {
    let pending = task.pending_directory_conflict.as_ref().unwrap();
    serde_json::from_value(serde_json::json!({
        "version":pending.version,"id":pending.id,"attempt":pending.attempt,"decision":policy,
    }))
    .unwrap()
}

#[tokio::test]
async fn directory_root_choices_continue_both_directions_and_preserve_the_existing_root() {
    for direction in ["upload", "download"] {
        for policy in ["skip", "merge", "keep_both"] {
            let (server, dir, ssh, id, db, queue) = fixture().await;
            let input = conflicting_input(&server, dir.path(), &["a"]);
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert("/tree/original".into(), b"keep".to_vec());
            let task = queue
                .submit(request(&id, &input, direction, "ask"))
                .await
                .unwrap();
            let pending = root_waiting(&queue, &task.id).await;
            let answer = root_response(&pending, policy);
            queue
                .resolve_directory_conflict(&task.id, answer.clone())
                .await
                .unwrap();
            let done = terminal(&queue, &task.id).await;
            assert_eq!(done["state"], "completed", "{direction}/{policy}: {done}");
            assert_eq!(done["attempt"], 1);
            assert!(done["pendingDirectoryConflict"].is_null());
            assert_eq!(done["lastDirectoryConflict"]["decision"], policy);
            assert_eq!(done["completedFiles"], u32::from(policy != "skip"));
            assert_eq!(done["skippedFiles"], u32::from(policy == "skip"));
            let files = server.counts.files.read().unwrap().clone();
            assert_eq!(files["/tree/original"], b"keep");
            assert_eq!(std::fs::read(input.join("a")).unwrap(), b"new-a");
            if direction == "upload" {
                if policy == "merge" {
                    assert_eq!(files["/tree/a"], b"new-a");
                } else {
                    assert!(!files.contains_key("/tree/a"));
                }
                if policy == "keep_both" {
                    assert_eq!(files["/tree (1)/a"], b"new-a");
                    assert_eq!(done["directoryReceipt"]["destinationPath"], "/tree (1)");
                }
            } else {
                assert_eq!(input.join("original").exists(), policy == "merge");
                if policy == "keep_both" {
                    let destination = input.with_file_name("input (1)");
                    assert_eq!(
                        std::fs::read(destination.join("original")).unwrap(),
                        b"keep"
                    );
                    assert_eq!(
                        Path::new(
                            done["directoryReceipt"]["destinationPath"]
                                .as_str()
                                .unwrap()
                        ),
                        std::fs::canonicalize(destination).unwrap()
                    );
                }
            }
            assert!(queue
                .resolve_directory_conflict(&task.id, answer)
                .await
                .is_err());
            drop(queue);
            let reopened = TransferCoordinator::new(db, ssh).unwrap();
            let durable = reopened.snapshot().await.unwrap().remove(0);
            assert_eq!(
                durable.last_directory_conflict.unwrap().decision,
                serde_json::from_value(serde_json::json!(policy)).unwrap()
            );
        }
    }
}

#[tokio::test]
async fn directory_merge_advances_to_file_decisions_without_replanning_or_replaying() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b"]);
    let task = queue
        .submit(request(&id, &input, "upload", "ask"))
        .await
        .unwrap();
    let root = root_waiting(&queue, &task.id).await;
    let root_answer = root_response(&root, "merge");
    let file_shaped = FileConflictResponse {
        version: root_answer.version,
        id: root_answer.id.clone(),
        attempt: 1,
        decision: deepagent_ssh::FileConflictDecision {
            policy: FileConflictPolicy::Replace,
            apply_to_remaining_files: true,
        },
    };
    assert!(queue
        .resolve_file_conflict(&task.id, file_shaped)
        .await
        .is_err());
    for invalid in [
        DirectoryConflictResponse {
            version: 2,
            ..root_answer.clone()
        },
        DirectoryConflictResponse {
            attempt: 2,
            ..root_answer.clone()
        },
        DirectoryConflictResponse {
            id: "other-root".into(),
            ..root_answer.clone()
        },
        root_response(&root, "ask"),
    ] {
        assert!(queue
            .resolve_directory_conflict(&task.id, invalid)
            .await
            .is_err());
    }
    assert!(queue
        .resolve_directory_conflict("another-task", root_answer.clone())
        .await
        .is_err());
    queue
        .resolve_directory_conflict(&task.id, root_answer.clone())
        .await
        .unwrap();
    let file = waiting(&queue, &task.id).await;
    assert_eq!(file.attempt, 1);
    assert_eq!(file.completed_files, 1);
    assert!(file.pending_directory_conflict.is_none());
    assert!(queue
        .resolve_directory_conflict(&task.id, root_answer.clone())
        .await
        .is_err());
    let file_nonce = file.pending_file_conflict.as_ref().unwrap().id.clone();
    assert!(queue
        .resolve_directory_conflict(
            &task.id,
            DirectoryConflictResponse {
                id: file_nonce,
                ..root_answer
            }
        )
        .await
        .is_err());
    assert_eq!(server.counts.files.read().unwrap()["/tree/a"], b"new-a");
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/a".into(), b"published-and-edited".to_vec());
    queue
        .resolve_file_conflict(&task.id, response(&file, "skip", false))
        .await
        .unwrap();
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed");
    assert_eq!(done["attempt"], 1);
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/a"],
        b"published-and-edited"
    );
    assert_eq!(server.counts.files.read().unwrap()["/tree/b"], b"old-b");
}

#[tokio::test]
async fn whole_directory_replace_resumes_the_original_attempt_and_persists_its_receipt() {
    for direction in ["upload", "download"] {
        let (server, dir, ssh, id, db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a"]);
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/remote-only".into(), b"old".to_vec());
        let task = queue
            .submit(request(&id, &input, direction, "ask"))
            .await
            .unwrap();
        let pending = root_waiting(&queue, &task.id).await;
        let answer = root_response(&pending, "replace");
        queue
            .resolve_directory_conflict(&task.id, answer.clone())
            .await
            .unwrap();
        let done = terminal(&queue, &task.id).await;
        assert_eq!(done["state"], "completed", "{done}");
        assert_eq!(done["attempt"], 1);
        assert_eq!(
            done["directoryReceipt"]["publishStrategy"],
            "directory_backup_replace"
        );
        assert_eq!(done["lastDirectoryConflict"]["pending"]["id"], answer.id);
        assert_eq!(done["lastDirectoryConflict"]["decision"], "replace");
        assert!(done["pendingDirectoryConflict"].is_null());
        assert!(done["pendingFileConflict"].is_null());
        if direction == "upload" {
            assert!(!server
                .counts
                .files
                .read()
                .unwrap()
                .contains_key("/tree/remote-only"));
            assert_eq!(server.counts.files.read().unwrap()["/tree/a"], b"new-a");
        } else {
            assert!(!input.join("a").exists());
            assert_eq!(std::fs::read(input.join("remote-only")).unwrap(), b"old");
        }
        assert!(queue
            .resolve_directory_conflict(&task.id, answer)
            .await
            .is_err());
        drop(queue);
        let reopened = TransferCoordinator::new(db, ssh).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.snapshot().await.unwrap().remove(0)).unwrap(),
            done
        );
    }
}

#[tokio::test]
async fn cancellation_during_directory_publication_records_the_actual_success() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a"]);
    let release = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_rename_gate.write().unwrap() = Some(release.clone());
    let task = queue
        .submit(request(&id, &input, "upload", "replace"))
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        server.counts.directory_rename_started.notified(),
    )
    .await
    .unwrap();
    assert_eq!(
        queue.cancel(&task.id).await.unwrap().state,
        TransferState::Cancelling
    );
    release.notify_one();
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(
        done["directoryReceipt"]["publishStrategy"],
        "directory_backup_replace"
    );
    assert_eq!(server.counts.files.read().unwrap()["/tree/a"], b"new-a");
}

#[tokio::test]
async fn failed_directory_recovery_is_persisted_with_the_real_backup_path() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a"]);
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/original".into(), b"keep".to_vec());
    server
        .counts
        .reject_directory_publish
        .store(true, std::sync::atomic::Ordering::SeqCst);
    server
        .counts
        .reject_directory_restore
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let task = queue
        .submit(request(&id, &input, "upload", "replace"))
        .await
        .unwrap();
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert!(done["directoryReceipt"].is_null());
    assert_eq!(done["error"]["code"], "publish");
    assert_eq!(done["error"]["destinationPath"], "/tree");
    let backup = server.counts.directory_renames.read().unwrap()[0].1.clone();
    assert!(done["error"]["cleanupWarnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains(&backup)));
    assert_eq!(
        server.counts.files.read().unwrap()[&format!("{backup}/original")],
        b"keep"
    );
    drop(queue);
    let reopened = TransferCoordinator::new(db, ssh).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.snapshot().await.unwrap().remove(0)).unwrap(),
        done
    );
}

#[tokio::test]
async fn directory_root_consent_is_durable_and_a_failed_save_does_not_release_work() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a"]);
    let task = queue
        .submit(request(&id, &input, "upload", "ask"))
        .await
        .unwrap();
    let pending = root_waiting(&queue, &task.id).await;
    let answer = root_response(&pending, "merge");
    db.with_conn(|conn| {conn.execute_batch("CREATE TRIGGER reject_root_decision BEFORE UPDATE ON documents WHEN NEW.collection='remote_transfers_v1' BEGIN SELECT RAISE(FAIL,'synthetic root persistence failure'); END;").unwrap();Ok(())}).unwrap();
    assert!(queue
        .resolve_directory_conflict(&task.id, answer.clone())
        .await
        .is_err());
    let still = queue.snapshot().await.unwrap().remove(0);
    assert_eq!(still.pending_directory_conflict.unwrap().id, answer.id);
    assert!(still.last_directory_conflict.is_none());
    assert!(!server.counts.files.read().unwrap().contains_key("/tree/a"));
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_root_decision")
            .unwrap();
        Ok(())
    })
    .unwrap();
    let stored = deepagent_persistence::document_store::DocumentStore::new(&db)
        .get("remote_transfers_v1", &format!("task:{}", task.id))
        .unwrap()
        .unwrap();
    assert!(stored.body.contains(&answer.id));
    drop(queue);
    let reopened = TransferCoordinator::new(db, ssh).unwrap();
    let restored = reopened.snapshot().await.unwrap().remove(0);
    assert_eq!(restored.state, TransferState::Failed);
    assert!(restored.pending_directory_conflict.is_none());
    assert!(reopened
        .resolve_directory_conflict(&task.id, answer)
        .await
        .is_err());
    assert!(!server.counts.files.read().unwrap().contains_key("/tree/a"));
}

#[tokio::test]
async fn directory_root_wait_revalidates_source_and_target_and_keeps_retry_nonce_scoped() {
    for change_source in [true, false] {
        let (server, dir, _ssh, id, _db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a"]);
        let task = queue
            .submit(request(&id, &input, "upload", "ask"))
            .await
            .unwrap();
        let pending = root_waiting(&queue, &task.id).await;
        let answer = root_response(&pending, "merge");
        if change_source {
            std::fs::write(input.join("a"), b"changed source size").unwrap();
        } else {
            server
                .counts
                .canonical_paths
                .write()
                .unwrap()
                .insert("/tree".into(), "/outside".into());
        }
        queue
            .resolve_directory_conflict(&task.id, answer)
            .await
            .unwrap();
        let done = terminal(&queue, &task.id).await;
        assert_eq!(done["state"], "failed");
        assert_eq!(
            done["error"]["code"],
            if change_source {
                "integrity"
            } else {
                "invalid"
            }
        );
        assert!(!server.counts.files.read().unwrap().contains_key("/tree/a"));
    }
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a"]);
    let task = queue
        .submit(request(&id, &input, "upload", "ask"))
        .await
        .unwrap();
    let pending = root_waiting(&queue, &task.id).await;
    let old = root_response(&pending, "merge");
    let mut second = request(&id, &input, "upload", "ask");
    second.transfer.remote_path = "/second".into();
    let second = queue.submit(second).await.unwrap();
    assert_eq!(second.state, TransferState::Queued);
    queue.cancel(&task.id).await.unwrap();
    assert_eq!(terminal(&queue, &task.id).await["state"], "cancelled");
    assert!(queue
        .resolve_directory_conflict(&task.id, old.clone())
        .await
        .is_err());
    assert_eq!(terminal(&queue, &second.id).await["state"], "completed");
    queue
        .retry(&task.id, FileConflictPolicy::Ask)
        .await
        .unwrap();
    let retry = root_waiting(&queue, &task.id).await;
    assert_eq!(retry.attempt, 2);
    assert_ne!(
        retry.pending_directory_conflict.as_ref().unwrap().id,
        old.id
    );
    assert!(retry.last_directory_conflict.is_none());
    assert!(queue
        .resolve_directory_conflict(&task.id, old)
        .await
        .is_err());
    queue.cancel(&task.id).await.unwrap();
    terminal(&queue, &task.id).await;
}

#[tokio::test]
async fn root_wait_rejects_a_changed_download_target_and_reports_a_failed_keep_both_target() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a"]);
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/original".into(), b"keep".to_vec());
    let task = queue
        .submit(request(&id, &input, "download", "ask"))
        .await
        .unwrap();
    let pending = root_waiting(&queue, &task.id).await;
    std::fs::remove_file(input.join("a")).unwrap();
    std::fs::remove_dir(&input).unwrap();
    std::fs::write(&input, b"replacement file").unwrap();
    queue
        .resolve_directory_conflict(&task.id, root_response(&pending, "merge"))
        .await
        .unwrap();
    let failed = terminal(&queue, &task.id).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"]["code"], "invalid");
    assert_eq!(std::fs::read(&input).unwrap(), b"replacement file");
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/original"],
        b"keep"
    );

    for direction in ["upload", "download"] {
        let (server, dir, _ssh, id, _db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a"]);
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/source".into(), b"old source".to_vec());
        let task = queue
            .submit(request(&id, &input, direction, "ask"))
            .await
            .unwrap();
        let pending = root_waiting(&queue, &task.id).await;
        if direction == "upload" {
            std::fs::write(input.join("a"), b"changed source size").unwrap();
        } else {
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert("/tree/source".into(), b"changed source size".to_vec());
        }
        queue
            .resolve_directory_conflict(&task.id, root_response(&pending, "keep_both"))
            .await
            .unwrap();
        let failed = terminal(&queue, &task.id).await;
        assert_eq!(failed["state"], "failed", "{direction}: {failed}");
        assert_eq!(failed["error"]["code"], "integrity");
        assert_eq!(failed["completedFiles"], 0);
        let actual = failed["error"]["destinationPath"].as_str().unwrap();
        if direction == "upload" {
            assert_eq!(actual, "/tree (1)");
            assert!(!server
                .counts
                .files
                .read()
                .unwrap()
                .contains_key("/tree (1)/a"));
        } else {
            assert_eq!(
                Path::new(actual),
                std::fs::canonicalize(input.with_file_name("input (1)")).unwrap()
            );
            assert_eq!(std::fs::read(input.join("a")).unwrap(), b"new-a");
            assert!(!Path::new(actual).join("source").exists());
        }
    }
}

fn conflicting_input(
    server: &support::SshServer,
    dir: &Path,
    names: &[&str],
) -> std::path::PathBuf {
    let input = dir.join("input");
    std::fs::create_dir(&input).unwrap();
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    for name in names {
        std::fs::write(input.join(name), format!("new-{name}")).unwrap();
        if *name != "a" {
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert(format!("/tree/{name}"), format!("old-{name}").into_bytes());
        }
    }
    input
}

#[tokio::test]
async fn individual_skip_keep_both_and_replace_continue_without_replaying_published_files() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b", "c", "d"]);
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    for (name, policy) in [("b", "skip"), ("c", "keep_both"), ("d", "replace")] {
        let pending = waiting(&queue, &task.id).await;
        let conflict = pending.pending_file_conflict.as_ref().unwrap();
        assert_eq!(conflict.conflict.relative_path, name);
        assert_eq!(conflict.conflict.source_size, "5");
        assert_eq!(conflict.attempt, 1);
        assert_eq!(pending.speed_bps, None);
        if name == "b" {
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert("/tree/a".into(), b"changed-after-publication".to_vec());
        }
        queue
            .resolve_file_conflict(&task.id, response(&pending, policy, false))
            .await
            .unwrap();
    }
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed");
    assert_eq!(done["attempt"], 1);
    assert_eq!(done["completedFiles"], 3);
    assert_eq!(done["skippedFiles"], 1);
    assert_eq!(done["directoryReceipt"]["renamedFiles"], 1);
    let files = server.counts.files.read().unwrap().clone();
    assert_eq!(files["/tree/a"], b"changed-after-publication");
    assert!(!files.contains_key("/tree/a (1)"));
    assert_eq!(files["/tree/b"], b"old-b");
    assert_eq!(files["/tree/c"], b"old-c");
    assert_eq!(files["/tree/c (1)"], b"new-c");
    assert_eq!(files["/tree/d"], b"new-d");
    assert!(files.keys().all(|path| !path.contains(".deepagent-")));
    drop(queue);
    let reopened = TransferCoordinator::new(db, ssh).unwrap();
    let restored = reopened.snapshot().await.unwrap().remove(0);
    assert!(restored.pending_file_conflict.is_none());
    let audit = restored.last_file_conflict.unwrap();
    assert_eq!(audit.pending.conflict.relative_path, "d");
    assert_eq!(audit.decision.policy, FileConflictPolicy::Replace);
    assert!(!audit.decision.apply_to_remaining_files);
}

#[tokio::test]
async fn remaining_file_policy_only_applies_forward_in_this_attempt() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b", "c", "d"]);
    std::fs::write(input.join("e"), b"fresh").unwrap();
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let pending = waiting(&queue, &task.id).await;
    queue
        .resolve_file_conflict(&task.id, response(&pending, "keep_both", true))
        .await
        .unwrap();
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed");
    assert_eq!(done["attempt"], 1);
    assert_eq!(done["completedFiles"], 5);
    assert_eq!(done["directoryReceipt"]["renamedFiles"], 3);
    assert_eq!(done["request"]["transfer"]["conflict"], "ask");
    assert_eq!(
        done["lastFileConflict"]["pending"]["conflict"]["relativePath"],
        "b"
    );
    assert_eq!(
        done["lastFileConflict"]["decision"]["applyToRemainingFiles"],
        true
    );
    let files = server.counts.files.read().unwrap();
    for name in ["b", "c", "d"] {
        assert_eq!(
            files[&format!("/tree/{name}")],
            format!("old-{name}").as_bytes()
        );
        assert_eq!(
            files[&format!("/tree/{name} (1)")],
            format!("new-{name}").as_bytes()
        );
    }
    assert_eq!(files["/tree/e"], b"fresh");
    assert!(!files.contains_key("/tree/a (1)"));
}

#[tokio::test]
async fn skip_and_replace_can_each_apply_to_the_remaining_file_conflicts() {
    for policy in ["skip", "replace"] {
        let (server, dir, _ssh, id, _db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a", "b", "c", "d"]);
        let task = queue
            .submit(request(&id, &input, "upload", "merge"))
            .await
            .unwrap();
        let pending = waiting(&queue, &task.id).await;
        queue
            .resolve_file_conflict(&task.id, response(&pending, policy, true))
            .await
            .unwrap();
        let done = terminal(&queue, &task.id).await;
        assert_eq!(done["state"], "completed");
        assert_eq!(done["completedFiles"], if policy == "skip" { 1 } else { 4 });
        assert_eq!(done["skippedFiles"], if policy == "skip" { 3 } else { 0 });
        assert_eq!(done["attempt"], 1);
        let files = server.counts.files.read().unwrap();
        for name in ["b", "c", "d"] {
            assert_eq!(
                files[&format!("/tree/{name}")],
                format!("{}-{name}", if policy == "skip" { "old" } else { "new" }).as_bytes()
            );
        }
    }
}

#[tokio::test]
async fn pending_decision_must_be_saved_before_it_is_advertised_or_answered() {
    for (policy, field, published) in [
        ("merge", "pendingFileConflict", 1),
        ("ask", "pendingDirectoryConflict", 0),
    ] {
        let (server, dir, _ssh, id, db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a", "b"]);
        db.with_conn(|conn| {
            conn.execute_batch(&format!(
                "CREATE TRIGGER reject_pending BEFORE UPDATE ON documents \
                 WHEN NEW.collection='remote_transfers_v1' AND NEW.body LIKE '%{field}%' \
                 BEGIN SELECT RAISE(FAIL,'fixture pending persistence failure'); END;"
            ))
            .unwrap();
            Ok(())
        })
        .unwrap();
        let task = queue
            .submit(request(&id, &input, "upload", policy))
            .await
            .unwrap();
        let failed = terminal(&queue, &task.id).await;
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["error"]["code"], "persistence");
        assert!(failed["pendingFileConflict"].is_null());
        assert!(failed["lastFileConflict"].is_null());
        assert!(failed["pendingDirectoryConflict"].is_null());
        assert!(failed["lastDirectoryConflict"].is_null());
        assert_eq!(failed["completedFiles"], published);
        let files = server.counts.files.read().unwrap();
        assert_eq!(files.contains_key("/tree/a"), published == 1);
        assert_eq!(files["/tree/b"], b"old-b");
    }
}

#[tokio::test]
async fn stale_duplicate_cancelled_and_wrong_scope_responses_cannot_resume_a_file() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b", "c"]);
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let pending = waiting(&queue, &task.id).await;
    let first = response(&pending, "skip", false);
    for invalid in [
        FileConflictResponse {
            version: 2,
            ..first.clone()
        },
        FileConflictResponse {
            attempt: 2,
            ..first.clone()
        },
        FileConflictResponse {
            id: "other-nonce".into(),
            ..first.clone()
        },
        response(&pending, "ask", false),
    ] {
        assert!(queue
            .resolve_file_conflict(&task.id, invalid)
            .await
            .is_err());
    }
    assert!(queue
        .resolve_file_conflict("another-task", first.clone())
        .await
        .is_err());
    queue
        .resolve_file_conflict(&task.id, first.clone())
        .await
        .unwrap();
    let second = waiting(&queue, &task.id).await;
    assert_eq!(
        second
            .pending_file_conflict
            .as_ref()
            .unwrap()
            .conflict
            .relative_path,
        "c"
    );
    assert!(queue
        .resolve_file_conflict(&task.id, first.clone())
        .await
        .is_err());
    let second_response = response(&second, "replace", true);
    queue.cancel(&task.id).await.unwrap();
    assert!(queue
        .resolve_file_conflict(&task.id, second_response)
        .await
        .is_err());
    let cancelled = terminal(&queue, &task.id).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert!(cancelled["pendingFileConflict"].is_null());
    queue
        .retry_with_policies(&task.id, FileConflictPolicy::Ask, None)
        .await
        .unwrap();
    let retried = waiting(&queue, &task.id).await;
    assert_eq!(retried.attempt, 2);
    assert!(retried.last_file_conflict.is_none());
    assert_eq!(
        retried
            .pending_file_conflict
            .as_ref()
            .unwrap()
            .conflict
            .relative_path,
        "a"
    );
    assert!(queue.resolve_file_conflict(&task.id, first).await.is_err());
    queue.cancel(&task.id).await.unwrap();
    terminal(&queue, &task.id).await;
    assert_eq!(server.counts.files.read().unwrap()["/tree/c"], b"old-c");
}

#[tokio::test]
async fn decision_persistence_failure_keeps_the_file_waiting_and_preserves_its_target() {
    let (server, dir, _ssh, id, db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b"]);
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let pending = waiting(&queue, &task.id).await;
    let answer = response(&pending, "replace", false);
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_decision BEFORE UPDATE ON documents WHEN NEW.collection='remote_transfers_v1' BEGIN SELECT RAISE(FAIL,'fixture decision persistence failure'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert!(queue
        .resolve_file_conflict(&task.id, answer.clone())
        .await
        .is_err());
    let still_waiting = queue.snapshot().await.unwrap().remove(0);
    assert_eq!(still_waiting.pending_file_conflict.unwrap().id, answer.id);
    assert!(still_waiting.last_file_conflict.is_none());
    assert_eq!(server.counts.files.read().unwrap()["/tree/b"], b"old-b");
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_decision").unwrap();
        Ok(())
    })
    .unwrap();
    queue.resolve_file_conflict(&task.id, answer).await.unwrap();
    assert_eq!(terminal(&queue, &task.id).await["state"], "completed");
    assert_eq!(server.counts.files.read().unwrap()["/tree/b"], b"new-b");
}

#[tokio::test]
async fn restart_invalidates_a_durable_pending_file_decision_without_network_replay() {
    let (server, dir, ssh, id, db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b"]);
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let pending = waiting(&queue, &task.id).await;
    let old = response(&pending, "replace", true);
    let durable = deepagent_persistence::document_store::DocumentStore::new(&db)
        .get("remote_transfers_v1", &format!("task:{}", task.id))
        .unwrap()
        .unwrap();
    assert!(durable.body.contains(&old.id));
    let before = server.counts.files.read().unwrap().clone();
    drop(queue);
    let reopened = TransferCoordinator::new(db, ssh).unwrap();
    let restored = reopened.snapshot().await.unwrap().remove(0);
    assert_eq!(restored.state, TransferState::Failed);
    assert_eq!(restored.error.unwrap().code, "interrupted");
    assert!(restored.pending_file_conflict.is_none());
    assert!(reopened.resolve_file_conflict(&task.id, old).await.is_err());
    assert_eq!(*server.counts.files.read().unwrap(), before);
}

#[tokio::test]
async fn source_and_target_are_revalidated_after_waiting_for_a_file_decision() {
    for change_source in [true, false] {
        let (server, dir, _ssh, id, _db, queue) = fixture().await;
        let input = conflicting_input(&server, dir.path(), &["a", "b"]);
        let task = queue
            .submit(request(&id, &input, "upload", "merge"))
            .await
            .unwrap();
        let pending = waiting(&queue, &task.id).await;
        if change_source {
            std::fs::write(input.join("b"), b"different length after asking").unwrap();
        } else {
            server.counts.files.write().unwrap().remove("/tree/b");
            server
                .counts
                .directories
                .write()
                .unwrap()
                .insert("/tree/b".into());
        }
        queue
            .resolve_file_conflict(&task.id, response(&pending, "replace", false))
            .await
            .unwrap();
        let failed = terminal(&queue, &task.id).await;
        assert_eq!(failed["state"], "failed");
        assert_eq!(
            failed["error"]["code"],
            if change_source {
                "integrity"
            } else {
                "invalid"
            }
        );
        assert_eq!(failed["error"]["relativePath"], "b");
        assert!(failed["pendingFileConflict"].is_null());
        let files = server.counts.files.read().unwrap();
        assert_eq!(files["/tree/a"], b"new-a");
        if change_source {
            assert_eq!(files["/tree/b"], b"old-b");
        } else {
            assert!(!files.contains_key("/tree/b"));
            assert!(server
                .counts
                .directories
                .read()
                .unwrap()
                .contains("/tree/b"));
        }
        assert!(files.keys().all(|path| !path.contains(".deepagent-")));
    }
}

#[tokio::test]
async fn download_conflicts_resume_only_the_current_file_and_preserve_originals() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server.counts.files.write().unwrap().extend([
        ("/tree/a".into(), b"remote-a".to_vec()),
        ("/tree/b".into(), b"remote-b".to_vec()),
    ]);
    let output = dir.path().join("output");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("b"), b"local-original").unwrap();
    let task = queue
        .submit(request(&id, &output, "download", "merge"))
        .await
        .unwrap();
    let pending = waiting(&queue, &task.id).await;
    assert_eq!(pending.completed_files, 1);
    assert_eq!(
        pending
            .pending_file_conflict
            .as_ref()
            .unwrap()
            .conflict
            .destination_path,
        output.canonicalize().unwrap().join("b").to_str().unwrap()
    );
    queue
        .resolve_file_conflict(&task.id, response(&pending, "keep_both", false))
        .await
        .unwrap();
    let done = terminal(&queue, &task.id).await;
    assert_eq!(done["state"], "completed");
    assert_eq!(done["attempt"], 1);
    assert_eq!(std::fs::read(output.join("a")).unwrap(), b"remote-a");
    assert!(!output.join("a (1)").exists());
    assert_eq!(std::fs::read(output.join("b")).unwrap(), b"local-original");
    assert_eq!(std::fs::read(output.join("b (1)")).unwrap(), b"remote-b");
}

#[tokio::test]
async fn cancellation_of_a_file_decision_releases_the_existing_connection_lane() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = conflicting_input(&server, dir.path(), &["a", "b"]);
    let first = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    waiting(&queue, &first.id).await;
    let mut next = request(&id, &input, "upload", "merge");
    next.transfer.remote_path = "/second".into();
    let second = queue.submit(next).await.unwrap();
    assert_eq!(second.state, TransferState::Queued);
    queue.cancel(&first.id).await.unwrap();
    assert_eq!(terminal(&queue, &first.id).await["state"], "cancelled");
    assert_eq!(terminal(&queue, &second.id).await["state"], "completed");
    assert_eq!(server.counts.files.read().unwrap()["/tree/b"], b"old-b");
    assert_eq!(server.counts.files.read().unwrap()["/second/b"], b"new-b");
}

#[tokio::test]
async fn directory_file_conflicts_wait_in_the_current_attempt_without_replaying_published_files() {
    let (server, dir, _ssh, id, _db, queue) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a.txt"), b"first").unwrap();
    std::fs::write(input.join("b.txt"), b"second").unwrap();
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
        .insert("/tree/b.txt".into(), b"original".to_vec());
    let task = queue
        .submit(request(&id, &input, "upload", "merge"))
        .await
        .unwrap();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut cursor = None;
        loop {
            let snapshot = queue
                .watch(cursor.as_deref(), std::time::Duration::from_secs(1))
                .await
                .unwrap();
            let current = snapshot
                .tasks
                .iter()
                .find(|entry| entry.id == task.id)
                .unwrap();
            let value = serde_json::to_value(current).unwrap();
            if current.state.terminal() || !value["pendingFileConflict"].is_null() {
                break value;
            }
            cursor = Some(snapshot.revision);
        }
    })
    .await
    .unwrap();
    assert_eq!(
        snapshot["state"], "running",
        "a file conflict must wait for its own decision in the same attempt"
    );
    assert_eq!(snapshot["attempt"], 1);
    assert_eq!(snapshot["completedFiles"], 1);
    assert_eq!(
        snapshot["pendingFileConflict"]["conflict"]["relativePath"],
        "b.txt"
    );
    assert_eq!(server.counts.files.read().unwrap()["/tree/a.txt"], b"first");
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/b.txt"],
        b"original"
    );
    queue.cancel(&task.id).await.unwrap();
    assert_eq!(terminal(&queue, &task.id).await["state"], "cancelled");
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
    let (server, dir, ssh, id, _db, queue) = fixture().await;
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
    let db = Arc::new(Database::open_in_memory().unwrap());
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
