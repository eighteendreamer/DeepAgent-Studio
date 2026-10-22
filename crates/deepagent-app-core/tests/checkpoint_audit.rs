//! Offline audit of failed checkpoint restoration through the public app facade.
//! Every file, backup and database belongs to this temporary fixture.

use std::sync::Arc;

use deepagent_app_core::AppService;
use deepagent_core::clock::FixedClock;
use deepagent_core::event::EventPayload;
use deepagent_core::message::Message;
use deepagent_persistence::checkpoint_store::CheckpointStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_runtime::CheckpointManager;
use deepagent_session::Session;

#[derive(Debug, Clone, Copy)]
enum Operation {
    Rewind,
    Fork,
}

#[derive(Debug)]
struct Observation {
    failed: bool,
    current_file_exists: bool,
    missing_backup_target: Option<String>,
    healthy_target: Option<String>,
    history_unchanged: bool,
    checkpoint_unchanged: bool,
    session_count: usize,
}

fn observe(operation: Operation, remove_backup: bool) -> Observation {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let missing_backup_target = workspace.path().join("first.txt");
    let healthy_target = workspace.path().join("second.txt");
    std::fs::write(&missing_backup_target, "first-before").unwrap();
    std::fs::write(&healthy_target, "second-before").unwrap();

    let mut session = Session::create(&db, &clock, Some("checkpoint audit")).unwrap();
    let id = session.id();
    session
        .append(EventPayload::MessageAppended {
            message: Message::user("edit both temporary files"),
        })
        .unwrap();
    let sequence = EventStore::new(&db)
        .load_session(id)
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    assert!(sequence > 0);
    RunStore::new(&db)
        .create("audit-checkpoint-run", &id.to_string(), None, 1_000)
        .unwrap();
    let checkpoint = CheckpointManager::new(
        db.clone(),
        "audit-checkpoint-run",
        sequence as i64,
        workspace.path(),
        storage.path(),
    )
    .unwrap();
    // Restoration is reversed, so the healthy file is processed first.
    checkpoint.capture_before(&missing_backup_target).unwrap();
    checkpoint.capture_before(&healthy_target).unwrap();
    std::fs::write(&missing_backup_target, "first-after").unwrap();
    std::fs::write(&healthy_target, "second-after").unwrap();
    let checkpoint_id = checkpoint.commit().unwrap();
    let record = CheckpointStore::new(&db)
        .get(&checkpoint_id)
        .unwrap()
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
    if remove_backup {
        let backup = std::path::Path::new(manifest["entries"][0]["backup"].as_str().unwrap());
        assert!(backup.starts_with(storage.path()));
        assert!(backup.is_file());
        // Only this fixture's captured backup is removed, never a workspace file.
        std::fs::remove_file(backup).unwrap();
    }
    let events_before =
        serde_json::to_value(EventStore::new(&db).load_session(id).unwrap()).unwrap();
    drop(checkpoint);
    drop(session);

    let app = AppService::from_shared(db.clone());
    let result = match operation {
        Operation::Rewind => app.rewind_session(&id.to_string(), 0).map(|_| ()),
        Operation::Fork => app.fork_session(&id.to_string(), 0).map(|_| ()),
    };
    let events_after =
        serde_json::to_value(EventStore::new(&db).load_session(id).unwrap()).unwrap();
    Observation {
        failed: result.is_err(),
        current_file_exists: missing_backup_target.exists(),
        missing_backup_target: std::fs::read_to_string(&missing_backup_target).ok(),
        healthy_target: std::fs::read_to_string(&healthy_target).ok(),
        history_unchanged: events_before == events_after,
        checkpoint_unchanged: CheckpointStore::new(&db).get(&checkpoint_id).unwrap()
            == Some(record),
        session_count: app.list_sessions().unwrap().len(),
    }
}

#[test]
fn complete_backups_restore_both_files_through_rewind_and_fork() {
    for operation in [Operation::Rewind, Operation::Fork] {
        let observed = observe(operation, false);
        assert!(!observed.failed, "{operation:?}: {observed:?}");
        assert!(observed.current_file_exists);
        assert_eq!(
            observed.missing_backup_target.as_deref(),
            Some("first-before")
        );
        assert_eq!(observed.healthy_target.as_deref(), Some("second-before"));
        assert!(observed.checkpoint_unchanged);
        match operation {
            Operation::Rewind => {
                assert!(!observed.history_unchanged);
                assert_eq!(observed.session_count, 1);
            }
            Operation::Fork => {
                assert!(observed.history_unchanged);
                assert_eq!(observed.session_count, 2);
            }
        }
    }
}

#[test]
#[ignore = "audit: a missing backup must not destroy the current workspace file"]
fn audit_missing_backup_preserves_current_file_on_failed_rewind_and_fork() {
    let observations = [Operation::Rewind, Operation::Fork].map(|operation| {
        let observed = observe(operation, true);
        eprintln!("checkpoint audit: operation={operation:?}, {observed:?}");
        assert!(observed.failed);
        assert!(observed.history_unchanged);
        assert!(observed.checkpoint_unchanged);
        assert_eq!(observed.session_count, 1);
        (operation, observed)
    });
    for (operation, observed) in observations {
        assert!(
            observed.current_file_exists,
            "{operation:?}: failed restoration removed the current file"
        );
        assert_eq!(
            observed.missing_backup_target.as_deref(),
            Some("first-after"),
            "{operation:?}: restoration failed but destroyed the current file"
        );
    }
}

#[derive(Debug)]
struct CaptureRetryObservation {
    write_succeeded: bool,
    backup_count: usize,
    restored_count: usize,
    content_after_restore: String,
}

async fn observe_capture_retry(fail_first_capture: bool) -> CaptureRetryObservation {
    use deepagent_builtins::{file_tools::file_tools, fs_guard::WorkspaceRoot};
    use deepagent_runtime::tool_pipeline::ToolExecutionPipeline;
    use deepagent_tools::{PermissionSet, ToolInvocation, ToolRegistry};

    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let session = Session::create(&db, &clock, Some("capture retry audit")).unwrap();
    let session_id = session.id();
    RunStore::new(&db)
        .create("audit-capture-run", &session_id.to_string(), None, 1_000)
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let target = workspace.path().join("retry.txt");
    std::fs::write(&target, "before-retry").unwrap();
    let checkpoint = Arc::new(
        CheckpointManager::new(
            db.clone(),
            "audit-capture-run",
            0,
            workspace.path(),
            storage.path(),
        )
        .unwrap(),
    );
    let mut registry = ToolRegistry::new();
    for tool in file_tools(WorkspaceRoot::new(workspace.path())) {
        registry.register(tool).unwrap();
    }
    let pipeline = ToolExecutionPipeline::new(&registry, session_id, PermissionSet::developer())
        .with_auto_approve(true)
        .with_checkpoint(Some(checkpoint.clone()));
    // Use the real shared read cache required by the real overwrite tool.
    let read = pipeline
        .execute(ToolInvocation::new(
            "read_file",
            serde_json::json!({"path": target}),
        ))
        .await
        .unwrap();
    assert!(read.output.ok);
    let write = || {
        ToolInvocation::new(
            "write_file",
            serde_json::json!({"path": target, "content": "after-retry"}),
        )
    };
    if fail_first_capture {
        // The production destination for the first backup must be a file.
        // An empty directory there injects a real copy error without permissions
        // assumptions; removing only that empty fixture directory repairs it.
        let blocked_destination = storage.path().join(checkpoint.id()).join("files/0");
        assert!(blocked_destination.starts_with(storage.path()));
        std::fs::create_dir(&blocked_destination).unwrap();
        assert!(pipeline.execute(write()).await.is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "before-retry");
        assert!(CheckpointStore::new(&db)
            .get(checkpoint.id())
            .unwrap()
            .is_none());
        std::fs::remove_dir(&blocked_destination).unwrap();
    }
    let written = pipeline.execute(write()).await.unwrap();
    let id = checkpoint.commit().unwrap();
    let record = CheckpointStore::new(&db).get(&id).unwrap().unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
    let restored = CheckpointManager::restore(&db, &id).unwrap();
    CaptureRetryObservation {
        write_succeeded: written.output.ok,
        backup_count: manifest["entries"].as_array().unwrap().len(),
        restored_count: restored.len(),
        content_after_restore: std::fs::read_to_string(&target).unwrap(),
    }
}

#[tokio::test]
async fn successful_capture_before_real_write_remains_restorable() {
    let observed = observe_capture_retry(false).await;
    assert!(observed.write_succeeded);
    assert_eq!(observed.backup_count, 1);
    assert_eq!(observed.restored_count, 1);
    assert_eq!(observed.content_after_restore, "before-retry");
}

#[tokio::test]
#[ignore = "audit: failed capture must not poison deduplication of the next real write"]
async fn audit_capture_failure_does_not_skip_backup_on_write_retry() {
    let observed = observe_capture_retry(true).await;
    eprintln!("checkpoint capture retry audit: {observed:?}");
    assert!(observed.write_succeeded);
    assert_eq!(
        observed.backup_count, 1,
        "retry must capture the original file"
    );
    assert_eq!(observed.restored_count, 1);
    assert_eq!(observed.content_after_restore, "before-retry");
}

fn record_branch_edit(
    db: &Arc<Database>,
    session_id: deepagent_core::id::SessionId,
    target: &std::path::Path,
    storage: &std::path::Path,
    run_id: &str,
    content: &str,
) -> (String, u64) {
    let sequence = EventStore::new(db)
        .load_session(session_id)
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    RunStore::new(db)
        .create(run_id, &session_id.to_string(), None, 1_000)
        .unwrap();
    let checkpoint = CheckpointManager::new(
        db.clone(),
        run_id,
        sequence as i64,
        target.parent().unwrap(),
        storage,
    )
    .unwrap();
    checkpoint.capture_before(target).unwrap();
    std::fs::write(target, content).unwrap();
    let id = checkpoint.commit().unwrap();
    RunStore::new(db)
        .finish(run_id, "succeeded", None, 1_001)
        .unwrap();
    (id, sequence)
}

#[derive(Debug)]
struct BranchRewindObservation {
    content: String,
    restored_paths: usize,
    kept_sequence: u64,
    discarded_checkpoint_sequence: Option<u64>,
}

fn observe_rewind_after_new_branch(discard_old_branch: bool) -> BranchRewindObservation {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let target = workspace.path().join("branch.txt");
    std::fs::write(&target, "base").unwrap();
    let mut session = Session::create(&db, &clock, Some("branch checkpoint audit")).unwrap();
    let id = session.id();
    let app = AppService::from_shared(db.clone());
    let mut discarded_checkpoint_sequence = None;
    if discard_old_branch {
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("discarded branch first edit"),
            })
            .unwrap();
        record_branch_edit(&db, id, &target, storage.path(), "old-first", "old-first");
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("discarded branch first answer"),
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("discarded branch second edit"),
            })
            .unwrap();
        let (old_checkpoint, sequence) =
            record_branch_edit(&db, id, &target, storage.path(), "old-second", "old-second");
        discarded_checkpoint_sequence = Some(sequence);
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("discarded branch second answer"),
            })
            .unwrap();
        drop(session);
        app.rewind_session(&id.to_string(), 0).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "base");
        // Keeping old metadata for audit is not itself a defect. The later
        // active-branch restore must avoid selecting these retired captures.
        assert!(CheckpointStore::new(&db)
            .get(&old_checkpoint)
            .unwrap()
            .is_some());
        session = Session::recover(&db, &clock, id).unwrap();
    }
    session
        .append(EventPayload::MessageAppended {
            message: Message::user("current branch edit"),
        })
        .unwrap();
    record_branch_edit(
        &db,
        id,
        &target,
        storage.path(),
        "current",
        "current-branch",
    );
    session
        .append(EventPayload::MessageAppended {
            message: Message::assistant("current branch answer"),
        })
        .unwrap();
    let kept_sequence = EventStore::new(&db)
        .load_session(id)
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    if let Some(sequence) = discarded_checkpoint_sequence {
        assert!(sequence > kept_sequence);
    }
    // The discarded tail in the current branch never changes any file.
    for message in [
        Message::user("later current branch question without file edits"),
        Message::assistant("later current branch answer without file edits"),
    ] {
        session
            .append(EventPayload::MessageAppended { message })
            .unwrap();
    }
    drop(session);
    let result = app.rewind_session(&id.to_string(), kept_sequence).unwrap();
    let history = EventStore::new(&db).load_session(id).unwrap();
    assert_eq!(history.last().unwrap().sequence, kept_sequence);
    let rendered = serde_json::to_string(&history).unwrap();
    assert!(rendered.contains("current branch answer"));
    assert!(!rendered.contains("discarded branch"));
    assert!(!rendered.contains("later current branch"));
    BranchRewindObservation {
        content: std::fs::read_to_string(&target).unwrap(),
        restored_paths: result.restored_paths.len(),
        kept_sequence,
        discarded_checkpoint_sequence,
    }
}

#[test]
fn rewind_over_text_only_tail_preserves_current_branch_file() {
    let observed = observe_rewind_after_new_branch(false);
    assert_eq!(observed.content, "current-branch");
    assert_eq!(observed.restored_paths, 0);
    assert_eq!(observed.kept_sequence, 4);
    assert!(observed.discarded_checkpoint_sequence.is_none());
}

#[test]
#[ignore = "audit: old-branch checkpoints must not overwrite files on a later branch rewind"]
fn audit_rewind_does_not_restore_discarded_branch_checkpoints() {
    let observed = observe_rewind_after_new_branch(true);
    eprintln!("checkpoint branch audit: {observed:?}");
    assert_eq!(
        observed.content, "current-branch",
        "text-only rewind selected a file backup from the discarded branch"
    );
    assert_eq!(observed.restored_paths, 0);
}
