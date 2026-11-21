use deepagent_app_core::{
    file_operations::WorkbenchOperation,
    file_transfer_service::{
        FileOperationRequest, TransferCoordinator, TransferExecutor, TransferState,
    },
};
use deepagent_persistence::Database;
use deepagent_runtime::CancellationTree;
use deepagent_ssh::{
    FileConflictPolicy, FileTransferError, FileTransferProgress, FileTransferReceipt,
    FileTransferRequest, ServerCopyMethod, ServerCopyRequest, ServerCopySpec,
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
struct Executor {
    active: Mutex<HashSet<String>>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl TransferExecutor for Executor {
    async fn execute(
        &self,
        _: &str,
        _: FileTransferRequest,
        _: CancellationTree,
        _: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        unreachable!()
    }
    async fn execute_workbench(
        &self,
        operation: WorkbenchOperation,
        _: CancellationTree,
        _: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> Result<serde_json::Value, FileTransferError> {
        let connections = operation.connections();
        {
            let mut active = self.active.lock().unwrap();
            for id in &connections {
                assert!(
                    active.insert(id.clone()),
                    "endpoint lane must be exclusive: {id}"
                );
            }
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(30)).await;
        {
            let mut active = self.active.lock().unwrap();
            for id in &connections {
                assert!(active.remove(id));
            }
        }
        Ok(serde_json::json!({ "bytes": "10", "files": 1 }))
    }
}
fn copy(source: &str, destination: &str) -> WorkbenchOperation {
    WorkbenchOperation::Copy {
        request: ServerCopyRequest {
            version: 1,
            source_generation: "1".into(),
            destination_generation: "1".into(),
            fingerprint: "a".repeat(64),
            spec: ServerCopySpec {
                source_connection_id: source.into(),
                destination_connection_id: destination.into(),
                source_path: "/source".into(),
                destination_path: "/dest".into(),
                conflict: FileConflictPolicy::Ask,
                method: ServerCopyMethod::Relay,
            },
        },
    }
}
#[tokio::test]
async fn shared_endpoints_are_serialized_without_ab_ba_deadlock_and_receipts_survive_reload() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let executor = Arc::new(Executor {
        active: Mutex::new(HashSet::new()),
        calls: AtomicUsize::new(0),
    });
    let queue = TransferCoordinator::new(db.clone(), executor.clone()).unwrap();
    for (source, dest) in [("a", "b"), ("b", "a"), ("c", "b"), ("d", "e")] {
        queue.submit_operation(copy(source, dest)).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let tasks = queue.snapshot().await.unwrap();
            if tasks.iter().all(|t| t.state.terminal()) {
                assert!(tasks.iter().all(|t| t.state == TransferState::Completed));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 4);
    drop(queue);
    let restored = TransferCoordinator::new(db, executor).unwrap();
    for task in restored.snapshot().await.unwrap() {
        assert!(matches!(task.request, FileOperationRequest::Workbench(_)));
        assert_eq!(task.operation_receipt.unwrap()["bytes"], "10");
    }
}
#[test]
fn legacy_transfer_wire_and_new_operations_remain_unambiguous() {
    let operation = FileOperationRequest::Workbench(copy("a", "b"));
    let json = serde_json::to_value(&operation).unwrap();
    assert_eq!(json["operation"], "copy");
    assert!(matches!(
        serde_json::from_value::<FileOperationRequest>(json).unwrap(),
        FileOperationRequest::Workbench(_)
    ));
    let legacy = serde_json::json!({ "connectionId":"a", "transfer":{"direction":"upload", "localPath":"D:\\file", "remotePath":"/file", "conflict":"ask"} });
    assert!(matches!(
        serde_json::from_value::<FileOperationRequest>(legacy).unwrap(),
        FileOperationRequest::Transfer(_)
    ));
}
