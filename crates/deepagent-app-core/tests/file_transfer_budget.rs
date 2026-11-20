//! Admission must account for results which arrive after remote side effects.
use deepagent_app_core::file_transfer_service::{
    TransferCoordinator, TransferExecutor, TransferRequest, TransferState,
};
use deepagent_core::clock::Timestamp;
use deepagent_persistence::{document_store::DocumentStore, Database};
use deepagent_runtime::CancellationTree;
use deepagent_ssh::{
    FileTransferError, FileTransferProgress, FileTransferReceipt, FileTransferRequest,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

const COLLECTION: &str = "remote_transfers_v1";
const DOCUMENT_BYTES: usize = 1024 * 1024;

struct ResultExecutor {
    started: AtomicUsize,
    release: tokio::sync::Notify,
    failure: bool,
    warning_repeats: usize,
}

impl ResultExecutor {
    fn new(failure: bool) -> Self {
        Self {
            started: AtomicUsize::new(0),
            release: tokio::sync::Notify::new(),
            failure,
            warning_repeats: 4096,
        }
    }
}

#[async_trait::async_trait]
impl TransferExecutor for ResultExecutor {
    async fn execute(
        &self,
        _: &str,
        _: FileTransferRequest,
        _: CancellationTree,
        _: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        let warnings = vec![format!(
            "backup cleanup required: /{}",
            "残留".repeat(self.warning_repeats)
        )];
        if self.failure {
            let mut error = FileTransferError::new("io", "publication failed after backup");
            error.cleanup_warnings = warnings;
            Err(error)
        } else {
            Ok(FileTransferReceipt {
                bytes: "9007199254740993".into(),
                sha256: "0123456789abcdef".repeat(4),
                skipped: false,
                publish_strategy: "backup_replace".into(),
                cleanup_warnings: warnings,
                destination_path: Some("/result".into()),
            })
        }
    }
}

fn request() -> TransferRequest {
    serde_json::from_value(serde_json::json!({
        "connectionId": "budget-fixture",
        "transfer": {
            "direction": "upload",
            "localPath": std::env::temp_dir().join("budget-input").to_string_lossy(),
            "remotePath": "/result",
            "conflict": "ask"
        }
    }))
    .unwrap()
}

/// Preserve a readable legacy history just below the existing whole-document cap.
fn install_history(db: &Database) -> String {
    let mut document = serde_json::json!({
        "version": 1,
        "tasks": [{
            "id": "existing-history", "request": request(), "attempt": 1,
            "state": "failed", "bytes": "7", "total": null, "revision": 3,
            "updatedMs": 0, "receipt": null,
            "error": {"code": "io", "message": "", "cleanupWarnings": []}
        }]
    });
    let empty = serde_json::to_string(&document).unwrap().len();
    document["tasks"][0]["error"]["message"] =
        serde_json::Value::String("h".repeat(DOCUMENT_BYTES - 8 * 1024 - empty));
    let body = serde_json::to_string(&document).unwrap();
    assert_eq!(body.len(), DOCUMENT_BYTES - 8 * 1024);
    DocumentStore::new(db)
        .put(COLLECTION, "queue", &body, None, Timestamp::EPOCH)
        .unwrap();
    body
}

async fn observe_terminal(queue: &TransferCoordinator, id: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut after = None;
        loop {
            let snapshot = queue
                .watch(after.as_deref(), Duration::from_secs(1))
                .await
                .expect("admitted remote work must retain an observable durable result");
            if let Some(task) = snapshot.tasks.iter().find(|task| task.id == id) {
                if task.state.terminal() {
                    return serde_json::to_value(task).unwrap();
                }
            }
            after = Some(snapshot.revision);
        }
    })
    .await
    .expect("terminal result was not published")
}

async fn check_result_budget(failure: bool) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let previous = install_history(&db);
    let executor = Arc::new(ResultExecutor::new(failure));
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let existing = serde_json::to_value(queue.snapshot().await.unwrap()).unwrap();
    let task = match queue.submit(request()).await {
        Ok(task) => task,
        Err(error) => {
            assert!(error.to_string().contains("limit") || error.to_string().contains("capacity"));
            assert_eq!(
                executor.started.load(Ordering::SeqCst),
                0,
                "capacity rejection must precede execution"
            );
            assert_eq!(
                serde_json::to_value(queue.snapshot().await.unwrap()).unwrap(),
                existing
            );
            assert_eq!(
                DocumentStore::new(&db)
                    .get(COLLECTION, "queue")
                    .unwrap()
                    .unwrap()
                    .body,
                previous,
                "rejected work cannot rewrite or discard existing history"
            );
            return;
        }
    };
    // Notify retains a permit if the worker has not reached the executor yet.
    executor.release.notify_one();
    let terminal = observe_terminal(&queue, &task.id).await;
    assert_eq!(
        terminal["state"],
        if failure { "failed" } else { "completed" }
    );
    let result = &terminal[if failure { "error" } else { "receipt" }];
    assert_eq!(
        result["cleanupWarnings"][0].as_str().unwrap(),
        format!("backup cleanup required: /{}", "残留".repeat(4096))
    );
    if !failure {
        assert_eq!(result["bytes"], "9007199254740993");
    }
    drop(queue);
    let restored = TransferCoordinator::new(db, executor.clone()).unwrap();
    let saved = serde_json::to_value(restored.snapshot().await.unwrap()).unwrap();
    assert_eq!(
        saved[0], existing[0],
        "capacity handling must retain previous history"
    );
    assert_eq!(
        saved[1], terminal,
        "the complete terminal result must survive host restart"
    );
    assert_eq!(
        executor.started.load(Ordering::SeqCst),
        1,
        "recovery must not replay work"
    );
}

#[tokio::test]
async fn successful_remote_publication_cannot_overflow_an_admitted_history() {
    check_result_budget(false).await;
}

#[tokio::test]
async fn cleanup_failure_cannot_overflow_an_admitted_history() {
    check_result_budget(true).await;
}

#[tokio::test]
async fn oversized_admission_keeps_existing_history_and_never_starts_executor() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let previous = install_history(&db);
    let executor = Arc::new(ResultExecutor::new(false));
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let mut oversized = request();
    // This is an endpoint violation independent of how history is partitioned.
    oversized.transfer.remote_path = format!("/{}", "x".repeat(16 * 1024));
    assert!(queue.submit(oversized).await.is_err());
    assert_eq!(executor.started.load(Ordering::SeqCst), 0);
    assert_eq!(queue.snapshot().await.unwrap().len(), 1);
    assert_eq!(
        queue.snapshot().await.unwrap()[0].state,
        TransferState::Failed
    );
    assert_eq!(
        DocumentStore::new(&db)
            .get(COLLECTION, "queue")
            .unwrap()
            .unwrap()
            .body,
        previous
    );
}

fn saved_bodies(db: &Database) -> Vec<(String, String)> {
    let mut rows: Vec<_> = DocumentStore::new(db)
        .list(COLLECTION)
        .unwrap()
        .into_iter()
        .map(|row| (row.id, row.body))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn migration_and_history_clear_are_atomic_and_reclaim_task_rows() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let previous = install_history(&db);
    let executor = Arc::new(ResultExecutor::new(false));
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_task_insert BEFORE INSERT ON documents WHEN NEW.collection='remote_transfers_v1' AND NEW.id!='queue' BEGIN SELECT RAISE(FAIL,'fixture task insert failure'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert!(queue.submit(request()).await.is_err());
    assert_eq!(saved_bodies(&db), vec![("queue".into(), previous)]);
    assert_eq!(executor.started.load(Ordering::SeqCst), 0);
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_task_insert")
            .unwrap();
        Ok(())
    })
    .unwrap();
    let task = queue.submit(request()).await.unwrap();
    executor.release.notify_one();
    observe_terminal(&queue, &task.id).await;
    let rows = saved_bodies(&db);
    let manifest: serde_json::Value =
        serde_json::from_str(&rows.iter().find(|(id, _)| id == "queue").unwrap().1).unwrap();
    assert_eq!(manifest["version"], 2);
    assert_eq!(rows.len(), 3, "index and two independently bounded tasks");
    let before = serde_json::to_value(queue.snapshot().await.unwrap()).unwrap();
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_task_delete BEFORE DELETE ON documents WHEN OLD.collection='remote_transfers_v1' BEGIN SELECT RAISE(FAIL,'fixture task delete failure'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert!(queue.clear_finished().await.is_err());
    assert_eq!(
        saved_bodies(&db),
        rows,
        "index and deletes must roll back together"
    );
    assert_eq!(
        serde_json::to_value(queue.snapshot().await.unwrap()).unwrap(),
        before
    );
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_task_delete")
            .unwrap();
        Ok(())
    })
    .unwrap();
    queue.clear_finished().await.unwrap();
    assert_eq!(
        saved_bodies(&db).len(),
        1,
        "cleared payload rows cannot accumulate"
    );
    assert!(queue.snapshot().await.unwrap().is_empty());
    let reopened = TransferCoordinator::new(db, executor.clone()).unwrap();
    assert!(reopened.snapshot().await.unwrap().is_empty());
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn admission_and_retry_reserve_each_active_tasks_entire_result_capacity() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    install_history(&db);
    let executor = Arc::new(ResultExecutor::new(false));
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let mut tasks = Vec::new();
    for index in 0..15 {
        let mut input = request();
        input.transfer.remote_path = format!("/reserved-{index}");
        tasks.push(queue.submit(input).await.unwrap());
    }
    let mut overflow = request();
    overflow.transfer.remote_path = "/reservation-overflow".into();
    let error = queue.submit(overflow.clone()).await.unwrap_err();
    assert!(error.to_string().contains("capacity"));
    // A duplicate neither consumes another reservation nor starts another worker.
    assert_eq!(
        queue.submit(tasks[0].request.clone()).await.unwrap().id,
        tasks[0].id
    );
    let cancelled = tasks.last().unwrap();
    queue.cancel(&cancelled.id).await.unwrap();
    queue.submit(overflow).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while executor.started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = saved_bodies(&db);
    let cancelled_before = serde_json::to_value(
        queue
            .snapshot()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.id == cancelled.id)
            .unwrap(),
    )
    .unwrap();
    let error = queue
        .retry(&cancelled.id, deepagent_ssh::FileConflictPolicy::Ask)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("capacity"));
    assert_eq!(saved_bodies(&db), before);
    let cancelled_after = serde_json::to_value(
        queue
            .snapshot()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.id == cancelled.id)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cancelled_after, cancelled_before);
    assert!(
        executor.started.load(Ordering::SeqCst) <= 1,
        "same-connection lane remains serial"
    );
}

#[tokio::test]
async fn stale_coordinator_cannot_resurrect_cleared_history() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    install_history(&db);
    let executor = Arc::new(ResultExecutor::new(false));
    let first = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let task = first.submit(request()).await.unwrap();
    executor.release.notify_one();
    observe_terminal(&first, &task.id).await;
    let stale = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    first.clear_finished().await.unwrap();
    let cleared = saved_bodies(&db);
    assert!(stale.clear_finished().await.is_err());
    assert_eq!(saved_bodies(&db), cleared);
    assert!(stale.submit(request()).await.is_err());
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_task_references_fail_closed_without_rewriting_history() {
    for defect in [
        "missing",
        "duplicate",
        "wrong_id",
        "wrong_revision",
        "wrong_bytes",
        "oversized",
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let executor = Arc::new(ResultExecutor::new(false));
        let mut task = serde_json::json!({
            "id": "history", "request": request(), "attempt": 1,
            "state": "failed", "bytes": "7", "total": null, "revision": 3,
            "updatedMs": 0, "receipt": null, "error": null
        });
        if defect == "wrong_id" {
            task["id"] = "other".into();
        }
        if defect == "wrong_revision" {
            task["revision"] = 4.into();
        }
        if defect == "oversized" {
            task["bytes"] = "x".repeat(DOCUMENT_BYTES).into();
        }
        let body = task.to_string();
        let reference = serde_json::json!({"id":"history", "revision":3,
            "bytes":if defect == "wrong_bytes" { 1 } else { body.len() }});
        let refs = if defect == "duplicate" {
            vec![reference.clone(), reference]
        } else {
            vec![reference]
        };
        let manifest = serde_json::json!({"version":2, "revision":1, "tasks":refs}).to_string();
        let store = DocumentStore::new(&db);
        store
            .put(COLLECTION, "queue", &manifest, None, Timestamp::EPOCH)
            .unwrap();
        if defect != "missing" {
            store
                .put(COLLECTION, "task:history", &body, None, Timestamp::EPOCH)
                .unwrap();
        }
        let before = saved_bodies(&db);
        assert!(
            TransferCoordinator::new(db.clone(), executor.clone()).is_err(),
            "{defect}"
        );
        assert_eq!(saved_bodies(&db), before, "{defect}");
        assert_eq!(executor.started.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn index_revision_fences_an_empty_queue_aba_cycle() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(ResultExecutor::new(false));
    let first = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    first.clear_finished().await.unwrap();
    let stale = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let original_index = saved_bodies(&db);
    let task = first.submit(request()).await.unwrap();
    executor.release.notify_one();
    observe_terminal(&first, &task.id).await;
    first.clear_finished().await.unwrap();
    let current_index = saved_bodies(&db);
    assert_ne!(
        current_index, original_index,
        "empty history still advances durable ownership"
    );
    assert!(stale.clear_finished().await.is_err());
    assert_eq!(saved_bodies(&db), current_index);
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn clear_cannot_delete_a_task_row_changed_outside_its_observed_index() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(ResultExecutor::new(false));
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    let task = queue.submit(request()).await.unwrap();
    executor.release.notify_one();
    let mut terminal = observe_terminal(&queue, &task.id).await;
    terminal["revision"] = (terminal["revision"].as_u64().unwrap() + 1).into();
    DocumentStore::new(&db)
        .put(
            COLLECTION,
            &format!("task:{}", task.id),
            &terminal.to_string(),
            None,
            Timestamp::EPOCH,
        )
        .unwrap();
    let before = saved_bodies(&db);
    assert!(queue.clear_finished().await.is_err());
    assert_eq!(
        saved_bodies(&db),
        before,
        "index and conditional deletes must all roll back"
    );
    drop(queue);
    assert!(TransferCoordinator::new(db, executor.clone()).is_err());
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn oversized_executor_result_requires_reconciliation_instead_of_silent_truncation() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(ResultExecutor {
        warning_repeats: DOCUMENT_BYTES / "残留".len() + 1,
        ..ResultExecutor::new(false)
    });
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    queue.submit(request()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while executor.started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = saved_bodies(&db);
    let active = queue.watch(None, Duration::ZERO).await.unwrap();
    executor.release.notify_one();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        queue.watch(Some(&active.revision), Duration::from_secs(5)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("reload"));
    assert_eq!(
        saved_bodies(&db),
        before,
        "no shortened or fake completed receipt"
    );
    assert!(queue.submit(request()).await.is_err());
    drop(queue);
    let restored = TransferCoordinator::new(db, executor.clone()).unwrap();
    let task = restored.snapshot().await.unwrap().remove(0);
    assert_eq!(task.state, TransferState::Failed);
    assert_eq!(task.error.as_ref().unwrap().code, "interrupted");
    assert!(task.receipt.is_none());
    assert_eq!(
        executor.started.load(Ordering::SeqCst),
        1,
        "host restart cannot replay publication"
    );
}
