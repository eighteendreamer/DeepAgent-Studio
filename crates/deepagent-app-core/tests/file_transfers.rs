use deepagent_app_core::file_transfer_service::{
    TransferCoordinator, TransferExecutor, TransferRequest, TransferState,
};
use deepagent_persistence::Database;
use deepagent_runtime::CancellationTree;
use deepagent_ssh::{
    FileConflictPolicy, FileTransferDirection, FileTransferError, FileTransferProgress,
    FileTransferReceipt, FileTransferRequest,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Executor {
    started: AtomicUsize,
    release: tokio::sync::Notify,
    skipped: bool,
}
#[async_trait::async_trait]
impl TransferExecutor for Executor {
    async fn execute(
        &self,
        _id: &str,
        _request: FileTransferRequest,
        cancel: CancellationTree,
        progress: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        progress(FileTransferProgress {
            bytes: 3,
            total: Some(10),
        });
        tokio::select! {_ = self.release.notified()=>Ok(FileTransferReceipt{bytes:if self.skipped { "0" } else { "10" }.into(),sha256:"fixture".into(),skipped:self.skipped,publish_strategy:"created".into(),cleanup_warnings:vec![]}),_ = cancel.cancelled()=>Err(FileTransferError{code:"cancelled".into(),message:"cancelled after stream cleanup".into(),cleanup_warnings:vec![],relative_path:None})}
    }
}
fn request(id: &str, name: &str) -> TransferRequest {
    TransferRequest {
        connection_id: id.into(),
        kind: Default::default(),
        directory_conflict: Default::default(),
        transfer: FileTransferRequest {
            direction: FileTransferDirection::Upload,
            local_path: std::env::temp_dir().join(name).to_string_lossy().into(),
            remote_path: format!("/{name}"),
            conflict: FileConflictPolicy::Ask,
        },
    }
}

#[tokio::test]
async fn watch_closes_snapshot_subscribe_gap_and_coalesces_without_losing_terminal_state() {
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        executor.clone(),
    )
    .unwrap();
    let initial = service
        .watch(None, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(initial.revision.ends_with(":0"));
    assert!(initial.tasks.is_empty());
    let first = service.submit(request("a", "watch-gap")).await.unwrap();
    state(&service, &first.id, TransferState::Running).await;
    // Deliberately subscribe AFTER mutations; the old cursor must return immediately.
    let running = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        service.watch(Some(&initial.revision), std::time::Duration::from_secs(5)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(running.revision, initial.revision);
    assert_eq!(running.tasks[0].bytes, "3");
    let pending = service.watch(Some(&running.revision), std::time::Duration::from_secs(5));
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending)
            .await
            .is_err()
    );
    executor.release.notify_one();
    let finished = tokio::time::timeout(std::time::Duration::from_millis(250), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.tasks[0].state, TransferState::Completed);
    assert_eq!(finished.tasks[0].bytes, "10");
    service.clear_finished().await.unwrap();
    let cleared = service
        .watch(Some(&finished.revision), std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(cleared.tasks.is_empty());
    assert_ne!(cleared.revision, finished.revision);
}

#[tokio::test]
async fn watch_timeout_and_dropped_waiter_leave_queue_usable() {
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(Executor::default()),
    )
    .unwrap();
    let initial = service
        .watch(None, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(20),
        service.watch(Some(&initial.revision), std::time::Duration::from_secs(5)),
    )
    .await
    .is_err());
    let unchanged = service
        .watch(
            Some(&initial.revision),
            std::time::Duration::from_millis(10),
        )
        .await
        .unwrap();
    assert_eq!(unchanged.revision, initial.revision);
    let first = service.submit(request("a", "watch-drop")).await.unwrap();
    state(&service, &first.id, TransferState::Running).await;
    service.cancel(&first.id).await.unwrap();
    state(&service, &first.id, TransferState::Cancelled).await;
    let result = service
        .watch(Some(&initial.revision), std::time::Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(result.tasks[0].state, TransferState::Cancelled);
}

#[tokio::test]
async fn progress_snapshot_exposes_speed_and_single_file_counts() {
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        executor.clone(),
    )
    .unwrap();
    let task = service
        .submit(request("a", "progress-fields"))
        .await
        .unwrap();
    state(&service, &task.id, TransferState::Running).await;
    let running = service
        .snapshot()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == task.id)
        .unwrap();
    assert_eq!(running.current_file.as_deref(), Some("/progress-fields"));
    assert_eq!(running.total_files, Some(1));
    assert_eq!(running.completed_files, 0);
    assert!(
        running.speed_bps.is_some(),
        "running progress must expose a rate"
    );
    executor.release.notify_one();
    state(&service, &task.id, TransferState::Completed).await;
    let finished = service
        .snapshot()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == task.id)
        .unwrap();
    assert_eq!(finished.completed_files, 1);
    assert_eq!(
        finished.speed_bps, None,
        "terminal task cannot advertise a live rate"
    );
    let wire = serde_json::to_value(&finished).unwrap();
    assert_eq!(wire["completedFiles"], 1);
    assert_eq!(wire["totalFiles"], 1);
    assert!(wire.get("startedMs").is_none());
    assert!(wire.get("startedAt").is_none());
}

#[tokio::test]
async fn retry_resets_live_metrics_after_cancellation() {
    let executor = Arc::new(Executor {
        skipped: true,
        ..Default::default()
    });
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        executor.clone(),
    )
    .unwrap();
    let task = service.submit(request("a", "progress-skip")).await.unwrap();
    state(&service, &task.id, TransferState::Running).await;
    service.cancel(&task.id).await.unwrap();
    state(&service, &task.id, TransferState::Cancelled).await;
    let cancelled = service.snapshot().await.unwrap().remove(0);
    assert_eq!(cancelled.speed_bps, None);
    assert_eq!(cancelled.completed_files, 0);
    let retry = service
        .retry(&task.id, FileConflictPolicy::Skip)
        .await
        .unwrap();
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.bytes, "0");
    assert_eq!(retry.total, None);
    assert_eq!(retry.speed_bps, None);
    assert_eq!(retry.completed_files, 0);
    assert_eq!(retry.total_files, Some(1));
    state(&service, &task.id, TransferState::Running).await;
    executor.release.notify_one();
    state(&service, &task.id, TransferState::Completed).await;
    let skipped = service.snapshot().await.unwrap().remove(0);
    assert!(skipped.receipt.as_ref().unwrap().skipped);
    assert_eq!(skipped.bytes, "0");
    assert_eq!(skipped.completed_files, 0);
    assert_eq!(skipped.speed_bps, None);
}

#[tokio::test]
async fn skipped_file_is_not_counted_as_transferred() {
    let executor = Arc::new(Executor {
        skipped: true,
        ..Default::default()
    });
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        executor.clone(),
    )
    .unwrap();
    let task = service
        .submit(request("a", "progress-skipped-count"))
        .await
        .unwrap();
    state(&service, &task.id, TransferState::Running).await;
    executor.release.notify_one();
    state(&service, &task.id, TransferState::Completed).await;
    let skipped = service.snapshot().await.unwrap().remove(0);
    assert!(skipped.receipt.as_ref().unwrap().skipped);
    assert_eq!(
        skipped.completed_files, 0,
        "skipping does not transfer a file"
    );
}

#[tokio::test]
async fn recovered_task_clears_live_speed_and_legacy_retry_initializes_file_counts() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(db.clone(), executor).unwrap();
    let task = service
        .submit(request("a", "progress-recovery"))
        .await
        .unwrap();
    state(&service, &task.id, TransferState::Running).await;
    let mut saved = serde_json::to_value(service.snapshot().await.unwrap()).unwrap();
    saved[0]["speedBps"] = serde_json::json!("9007199254740993");
    drop(service);
    let store = deepagent_persistence::document_store::DocumentStore::new(&db);
    store
        .put(
            "remote_transfers_v1",
            "queue",
            &serde_json::json!({
                "version": 1, "tasks": saved,
            })
            .to_string(),
            None,
            deepagent_core::clock::Timestamp::EPOCH,
        )
        .unwrap();
    let restored = TransferCoordinator::new(db.clone(), Arc::new(Executor::default())).unwrap();
    let recovered = restored.snapshot().await.unwrap().remove(0);
    assert_eq!(recovered.state, TransferState::Failed);
    assert_eq!(
        recovered.speed_bps, None,
        "recovered interrupted history has no live rate"
    );
    drop(restored);
    let row = saved[0].as_object_mut().unwrap();
    row.insert("state".into(), serde_json::json!("failed"));
    for key in ["currentFile", "totalFiles", "completedFiles", "speedBps"] {
        row.remove(key);
    }
    store
        .put(
            "remote_transfers_v1",
            "queue",
            &serde_json::json!({
                "version": 1, "tasks": saved,
            })
            .to_string(),
            None,
            deepagent_core::clock::Timestamp::EPOCH,
        )
        .unwrap();
    let legacy = TransferCoordinator::new(db, Arc::new(Executor::default())).unwrap();
    let old = legacy.snapshot().await.unwrap().remove(0);
    assert_eq!(old.total_files, None);
    assert_eq!(old.current_file, None);
    let retry = legacy
        .retry(&task.id, FileConflictPolicy::Ask)
        .await
        .unwrap();
    assert_eq!(retry.total_files, Some(1));
    assert_eq!(retry.completed_files, 0);
    assert_eq!(retry.current_file.as_deref(), Some("/progress-recovery"));
    assert_eq!(retry.speed_bps, None);
}

#[tokio::test]
async fn watch_cursor_is_scoped_to_host_generation_even_when_both_queues_have_revision_zero() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let first = TransferCoordinator::new(db.clone(), Arc::new(Executor::default())).unwrap();
    let previous = first.watch(None, std::time::Duration::ZERO).await.unwrap();
    drop(first);
    let next = TransferCoordinator::new(db, Arc::new(Executor::default())).unwrap();
    let current = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        next.watch(Some(&previous.revision), std::time::Duration::from_secs(5)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(current.revision.ends_with(":0"));
    assert_ne!(current.revision, previous.revision);
}
async fn state(service: &TransferCoordinator, id: &str, expected: TransferState) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if service
                .snapshot()
                .await
                .unwrap()
                .iter()
                .any(|t| t.id == id && t.state == expected)
            {
                break;
            }
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn per_connection_queue_deduplicates_and_cancellation_waits_for_executor_cleanup() {
    let executor = Arc::new(Executor::default());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let service = TransferCoordinator::new(db, executor.clone()).unwrap();
    let first = service.submit(request("a", "first")).await.unwrap();
    state(&service, &first.id, TransferState::Running).await;
    assert_eq!(
        service.submit(request("a", "first")).await.unwrap().id,
        first.id
    );
    let second = service.submit(request("a", "second")).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
    service.cancel(&first.id).await.unwrap();
    state(&service, &first.id, TransferState::Cancelled).await;
    state(&service, &second.id, TransferState::Running).await;
    assert_eq!(executor.started.load(Ordering::SeqCst), 2);
    executor.release.notify_one();
    state(&service, &second.id, TransferState::Completed).await;
    let done = service.cancel(&second.id).await.unwrap();
    assert_eq!(done.state, TransferState::Completed);
}

#[tokio::test]
async fn retry_rotates_attempt_and_reopen_marks_unfinished_work_interrupted_without_reexecuting() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("db.sqlite")).unwrap());
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let first = service.submit(request("a", "retry")).await.unwrap();
    state(&service, &first.id, TransferState::Running).await;
    service.cancel(&first.id).await.unwrap();
    state(&service, &first.id, TransferState::Cancelled).await;
    let retried = service
        .retry(&first.id, FileConflictPolicy::Skip)
        .await
        .unwrap();
    assert_eq!(retried.attempt, 2);
    state(&service, &first.id, TransferState::Running).await;
    drop(service);
    tokio::task::yield_now().await;
    let reopened = TransferCoordinator::new(db, Arc::new(Executor::default())).unwrap();
    let snapshot = reopened.snapshot().await.unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(matches!(
        snapshot[0].state,
        TransferState::Failed | TransferState::Cancelled
    ));
    assert_eq!(snapshot[0].attempt, 2);
}

#[tokio::test]
async fn durable_admission_failure_starts_no_network_and_corrupt_history_is_not_overwritten() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    db.with_conn(|c|{c.execute_batch("CREATE TRIGGER reject_transfer BEFORE INSERT ON documents WHEN NEW.collection='remote_transfers_v1' BEGIN SELECT RAISE(FAIL,'fixture persistence failure'); END;").unwrap();Ok(())}).unwrap();
    assert!(service.submit(request("a", "blocked")).await.is_err());
    assert_eq!(executor.started.load(Ordering::SeqCst), 0);
    assert!(service.snapshot().await.unwrap().is_empty());
    db.with_conn(|c| {
        c.execute_batch("DROP TRIGGER reject_transfer").unwrap();
        Ok(())
    })
    .unwrap();
    deepagent_persistence::document_store::DocumentStore::new(&db)
        .put(
            "remote_transfers_v1",
            "queue",
            "{broken",
            None,
            deepagent_core::clock::Timestamp::EPOCH,
        )
        .unwrap();
    assert!(TransferCoordinator::new(db.clone(), executor).is_err());
    assert_eq!(
        deepagent_persistence::document_store::DocumentStore::new(&db)
            .get("remote_transfers_v1", "queue")
            .unwrap()
            .unwrap()
            .body,
        "{broken"
    );
}

#[derive(Default)]
struct CleanupExecutor {
    started: AtomicUsize,
    cleanup_started: tokio::sync::Notify,
    cleanup_release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl TransferExecutor for CleanupExecutor {
    async fn execute(
        &self,
        _: &str,
        _: FileTransferRequest,
        cancel: CancellationTree,
        _: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        cancel.cancelled().await;
        self.cleanup_started.notify_one();
        self.cleanup_release.notified().await;
        Err(FileTransferError::new(
            "cancelled",
            "fixture cleanup finished",
        ))
    }
}

#[tokio::test]
async fn cleanup_holds_connection_lane_and_cross_connection_work_can_continue() {
    let executor = Arc::new(CleanupExecutor::default());
    let service = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        executor.clone(),
    )
    .unwrap();
    let first = service.submit(request("a", "cleanup-first")).await.unwrap();
    state(&service, &first.id, TransferState::Running).await;
    let second = service.submit(request("a", "cleanup-next")).await.unwrap();
    service.cancel(&first.id).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        executor.cleanup_started.notified(),
    )
    .await
    .unwrap();
    let other = service.submit(request("b", "cleanup-other")).await.unwrap();
    state(&service, &other.id, TransferState::Running).await;
    let tasks = service.snapshot().await.unwrap();
    assert_eq!(
        tasks.iter().find(|t| t.id == first.id).unwrap().state,
        TransferState::Cancelling
    );
    assert_eq!(
        tasks.iter().find(|t| t.id == second.id).unwrap().state,
        TransferState::Queued
    );
    assert_eq!(executor.started.load(Ordering::SeqCst), 2);
    executor.cleanup_release.notify_one();
    state(&service, &first.id, TransferState::Cancelled).await;
    state(&service, &second.id, TransferState::Running).await;
    service.cancel(&second.id).await.unwrap();
    executor.cleanup_started.notified().await;
    executor.cleanup_release.notify_one();
    state(&service, &second.id, TransferState::Cancelled).await;
    service.cancel(&other.id).await.unwrap();
    executor.cleanup_started.notified().await;
    executor.cleanup_release.notify_one();
    state(&service, &other.id, TransferState::Cancelled).await;
}

#[path = "../../deepagent-ssh/tests/support/mod.rs"]
mod support;

#[tokio::test]
async fn rejected_directory_retry_policy_cannot_mutate_a_file_attempt() {
    let queue = TransferCoordinator::new(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(Executor::default()),
    )
    .unwrap();
    let task = queue.submit(request("a", "invalid-policy")).await.unwrap();
    state(&queue, &task.id, TransferState::Running).await;
    queue.cancel(&task.id).await.unwrap();
    state(&queue, &task.id, TransferState::Cancelled).await;
    let before = serde_json::to_value(queue.snapshot().await.unwrap()).unwrap();
    assert!(queue
        .retry_with_policies(
            &task.id,
            FileConflictPolicy::Replace,
            Some(deepagent_ssh::DirectoryConflictPolicy::Merge)
        )
        .await
        .is_err());
    assert_eq!(
        serde_json::to_value(queue.snapshot().await.unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn persisted_queue_executes_real_loopback_sftp_upload_and_download_without_reauth() {
    let server = support::SshServer::start("127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let trust = dir.path().join("known_hosts");
    server.write_trust(&trust);
    let ssh = Arc::new(deepagent_ssh::SshService::new(dir.path().to_owned()));
    let connection = server.create(&ssh, Some(&trust)).await;
    let db = Arc::new(Database::open(dir.path().join("queue.sqlite")).unwrap());
    let service = TransferCoordinator::new(db.clone(), ssh.clone()).unwrap();
    let input = dir.path().join("queue-input.bin");
    let output = dir.path().join("queue-output.bin");
    let bytes: Vec<u8> = (0..240_001).map(|i| (i % 251) as u8).collect();
    std::fs::write(&input, &bytes).unwrap();
    let upload = service
        .submit(TransferRequest {
            connection_id: connection.id.clone(),
            kind: Default::default(),
            directory_conflict: Default::default(),
            transfer: FileTransferRequest {
                direction: FileTransferDirection::Upload,
                local_path: input.to_string_lossy().into(),
                remote_path: "/queue.bin".into(),
                conflict: FileConflictPolicy::Ask,
            },
        })
        .await
        .unwrap();
    state(&service, &upload.id, TransferState::Completed).await;
    let download = service
        .submit(TransferRequest {
            connection_id: connection.id.clone(),
            kind: Default::default(),
            directory_conflict: Default::default(),
            transfer: FileTransferRequest {
                direction: FileTransferDirection::Download,
                local_path: output.to_string_lossy().into(),
                remote_path: "/queue.bin".into(),
                conflict: FileConflictPolicy::Ask,
            },
        })
        .await
        .unwrap();
    state(&service, &download.id, TransferState::Completed).await;
    assert_eq!(std::fs::read(output).unwrap(), bytes);
    assert_eq!(server.auth_count(), 1);
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 0);
    drop(service);
    let restored = TransferCoordinator::new(db, ssh.clone()).unwrap();
    let history = restored.snapshot().await.unwrap();
    assert_eq!(history.len(), 2);
    assert!(history
        .iter()
        .all(|task| task.state == TransferState::Completed));
    assert_eq!(
        history[0].receipt.as_ref().unwrap().sha256,
        history[1].receipt.as_ref().unwrap().sha256
    );
    ssh.disconnect(&connection.id).await.unwrap();
}

#[tokio::test]
async fn terminal_persistence_failure_is_visible_and_blocks_new_work_until_host_recovery() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(Executor::default());
    let service = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let task = service
        .submit(request("a", "terminal-save-failure"))
        .await
        .unwrap();
    state(&service, &task.id, TransferState::Running).await;
    db.with_conn(|c| {
        c.execute_batch("CREATE TRIGGER reject_terminal BEFORE UPDATE ON documents WHEN NEW.collection='remote_transfers_v1' BEGIN SELECT RAISE(FAIL,'fixture terminal persistence failure'); END;").unwrap();
        Ok(())
    }).unwrap();
    let previous = service
        .watch(None, std::time::Duration::ZERO)
        .await
        .unwrap();
    executor.release.notify_one();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        service.watch(Some(&previous.revision), std::time::Duration::from_secs(5)),
    )
    .await
    .expect("failed terminal persistence must wake the observer immediately")
    .unwrap_err();
    assert!(error.to_string().contains("reload"));
    assert!(service.submit(request("b", "not-admitted")).await.is_err());
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
    db.with_conn(|c| {
        c.execute_batch("DROP TRIGGER reject_terminal").unwrap();
        Ok(())
    })
    .unwrap();
    drop(service);
    let restored = TransferCoordinator::new(db, Arc::new(Executor::default())).unwrap();
    let tasks = restored.snapshot().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TransferState::Failed);
    assert_eq!(tasks[0].error.as_ref().unwrap().code, "interrupted");
}
