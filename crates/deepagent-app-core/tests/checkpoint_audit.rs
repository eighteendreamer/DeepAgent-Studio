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
        assert_eq!(observed.healthy_target.as_deref(), Some("second-after"));
    }
}

#[derive(Clone, Copy, Debug)]
enum RestoreFault {
    MissingOlderBackup,
    RewindPublication,
    ForkInsert,
    ForkCommit,
}

fn exercise_durable_failure_and_retry(operation: Operation, fault: RestoreFault) {
    use deepagent_core::error::CoreError;
    let data = tempfile::tempdir().unwrap();
    let db_path = data.path().join("audit.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let clock = FixedClock::new(1_000);
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    std::fs::write(&first, "first-before").unwrap();
    std::fs::write(&second, "second-before").unwrap();
    let mut session = Session::create(&db, &clock, Some("durable restore failure")).unwrap();
    let id = session.id();
    let mut checkpoints = Vec::new();
    for (index, target) in [&first, &second].into_iter().enumerate() {
        session
            .append(EventPayload::MessageAppended {
                message: Message::user(format!("edit-{index}")),
            })
            .unwrap();
        let sequence = EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .last()
            .unwrap()
            .sequence;
        let run_id = format!("restore-run-{index}");
        RunStore::new(&db)
            .create(&run_id, &id.to_string(), None, 1_000)
            .unwrap();
        let checkpoint = CheckpointManager::new(
            db.clone(),
            &run_id,
            sequence as i64,
            workspace.path(),
            storage.path(),
        )
        .unwrap();
        checkpoint.capture_before(target).unwrap();
        checkpoints.push(checkpoint.commit().unwrap());
        RunStore::new(&db)
            .finish(&run_id, "succeeded", None, 1_001)
            .unwrap();
    }
    session
        .append(EventPayload::MessageAppended {
            message: Message::assistant("edits finished"),
        })
        .unwrap();
    drop(session);
    std::fs::write(&first, "first-current").unwrap();
    std::fs::write(&second, "second-current").unwrap();
    let before = serde_json::to_value(EventStore::new(&db).load_session(id).unwrap()).unwrap();
    let mut deleted_backup = None;
    let mut blocked_generation = None;
    match fault {
        RestoreFault::MissingOlderBackup => {
            let record = CheckpointStore::new(&db)
                .get(&checkpoints[0])
                .unwrap()
                .unwrap();
            let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
            let path = std::path::PathBuf::from(manifest["entries"][0]["backup"].as_str().unwrap());
            assert!(path.starts_with(storage.path()));
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            deleted_backup = Some((path, bytes));
        }
        RestoreFault::RewindPublication => {
            // Current fixture has one generation. Obstruct only the next
            // generation's publication, not the readable source history.
            let path = db
                .session_transcript_dir(id)
                .join("session.g000002.v1.jsonl.zst");
            assert!(path.starts_with(data.path()));
            std::fs::create_dir(&path).unwrap();
            blocked_generation = Some(path);
        }
        RestoreFault::ForkInsert => {
            db.with_conn(|connection| { connection.execute_batch("CREATE TEMP TRIGGER reject_fork BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT, 'injected fork insert failure'); END;").map_err(|error| CoreError::Persistence(error.to_string()))?; Ok(()) }).unwrap();
        }
        RestoreFault::ForkCommit => {
            // A deferred FK fails at COMMIT, after the complete prefix was
            // published, exercising its unpublish path rather than INSERT.
            db.with_conn(|connection| { connection.execute_batch("CREATE TABLE audit_commit_guard (session_id TEXT REFERENCES sessions(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER reject_fork_commit AFTER INSERT ON sessions BEGIN INSERT INTO audit_commit_guard VALUES ('missing-audit-session'); END;").map_err(|error| CoreError::Persistence(error.to_string()))?; Ok(()) }).unwrap();
        }
    }
    let app = AppService::from_shared(db.clone());
    let result = match operation {
        Operation::Rewind => app.rewind_session(&id.to_string(), 0).map(|_| ()),
        Operation::Fork => app.fork_session(&id.to_string(), 0).map(|_| ()),
    };
    let error = result.expect_err("injected failure must not be reported as success");
    eprintln!("durable restore {operation:?}/{fault:?}: {error}");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first-current");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "second-current");
    if let Some(path) = blocked_generation {
        std::fs::remove_dir(path).unwrap();
    }
    assert_eq!(
        serde_json::to_value(EventStore::new(&db).load_session(id).unwrap()).unwrap(),
        before
    );
    assert_eq!(app.list_sessions().unwrap().len(), 1);
    if let Some((path, bytes)) = deleted_backup {
        std::fs::write(path, bytes).unwrap();
    }
    db.with_conn(|connection| { connection.execute_batch("DROP TRIGGER IF EXISTS reject_fork; DROP TRIGGER IF EXISTS reject_fork_commit; DROP TABLE IF EXISTS audit_commit_guard;").map_err(|error| CoreError::Persistence(error.to_string()))?; Ok(()) }).unwrap();
    drop(app);
    drop(db);
    let db = Arc::new(Database::open(&db_path).unwrap());
    let app = AppService::from_shared(db.clone());
    // Restart must not resurrect a header-only/partially-copied failed child.
    assert_eq!(app.list_sessions().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(EventStore::new(&db).load_session(id).unwrap()).unwrap(),
        before
    );
    for checkpoint in &checkpoints {
        assert!(CheckpointStore::new(&db).get(checkpoint).unwrap().is_some());
    }
    match operation {
        Operation::Rewind => {
            app.rewind_session(&id.to_string(), 0).unwrap();
            assert_eq!(app.list_sessions().unwrap().len(), 1);
        }
        Operation::Fork => {
            let fork = app.fork_session(&id.to_string(), 0).unwrap();
            assert_eq!(app.list_sessions().unwrap().len(), 2);
            assert_eq!(
                app.session_detail(&fork.new_session_id).unwrap().summary.id,
                fork.new_session_id
            );
        }
    }
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first-before");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "second-before");
    assert!(std::fs::read_dir(workspace.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".deepagent-restore-")));
}

#[test]
fn missing_backup_in_older_checkpoint_preserves_files_history_and_retry_after_reopen() {
    for operation in [Operation::Rewind, Operation::Fork] {
        exercise_durable_failure_and_retry(operation, RestoreFault::MissingOlderBackup);
    }
}

#[test]
fn unreadable_rewind_history_rejects_before_file_changes_and_allows_retry() {
    exercise_durable_failure_and_retry(Operation::Rewind, RestoreFault::RewindPublication);
}

#[test]
fn fork_sqlite_insert_failure_compensates_files_without_resurrecting_partial_child() {
    exercise_durable_failure_and_retry(Operation::Fork, RestoreFault::ForkInsert);
}

#[test]
fn fork_sqlite_commit_failure_unpublishes_child_and_compensates_files() {
    exercise_durable_failure_and_retry(Operation::Fork, RestoreFault::ForkCommit);
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
fn audit_rewind_does_not_restore_discarded_branch_checkpoints() {
    let observed = observe_rewind_after_new_branch(true);
    eprintln!("checkpoint branch audit: {observed:?}");
    assert_eq!(
        observed.content, "current-branch",
        "text-only rewind selected a file backup from the discarded branch"
    );
    assert_eq!(observed.restored_paths, 0);
}

/// File-backed fixture: all operations use the public production facade and
/// real capture/commit. Only legacy-format and storage-fault tests alter SQL.
struct BranchFixture {
    db: Arc<Database>,
    data: tempfile::TempDir,
    workspace: tempfile::TempDir,
    storage: tempfile::TempDir,
    session_id: deepagent_core::id::SessionId,
}

impl BranchFixture {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(data.path().join("branch.sqlite")).unwrap());
        let session_id = Session::create(&db, &FixedClock::new(1_000), Some("branch regression"))
            .unwrap()
            .id();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("branch.txt"), "base").unwrap();
        Self {
            db,
            data,
            workspace,
            storage: tempfile::tempdir().unwrap(),
            session_id,
        }
    }

    fn target(&self) -> std::path::PathBuf {
        self.workspace.path().join("branch.txt")
    }

    fn text(&self, content: &str) -> u64 {
        let clock = FixedClock::new(1_000);
        let mut session = Session::recover(&self.db, &clock, self.session_id).unwrap();
        for message in [Message::user(content), Message::assistant("answer")] {
            session
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        self.tail()
    }

    fn edit(&self, run_id: &str, content: &str) -> (String, u64, u64) {
        let clock = FixedClock::new(1_000);
        let mut session = Session::recover(&self.db, &clock, self.session_id).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user(run_id),
            })
            .unwrap();
        let (checkpoint, anchor_sequence) = record_branch_edit(
            &self.db,
            self.session_id,
            &self.target(),
            self.storage.path(),
            run_id,
            content,
        );
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("edit done"),
            })
            .unwrap();
        (checkpoint, anchor_sequence, self.tail())
    }

    fn tail(&self) -> u64 {
        EventStore::new(&self.db)
            .load_session(self.session_id)
            .unwrap()
            .last()
            .unwrap()
            .sequence
    }

    fn history(&self) -> serde_json::Value {
        serde_json::to_value(
            EventStore::new(&self.db)
                .load_session(self.session_id)
                .unwrap(),
        )
        .unwrap()
    }

    fn content(&self) -> String {
        std::fs::read_to_string(self.target()).unwrap()
    }

    fn rewind(&self, sequence: u64) {
        AppService::from_shared(self.db.clone())
            .rewind_session(&self.session_id.to_string(), sequence)
            .unwrap();
    }

    fn reopen(&mut self) {
        let replacement = Arc::new(Database::open_in_memory().unwrap());
        drop(std::mem::replace(&mut self.db, replacement));
        self.db = Arc::new(Database::open(self.data.path().join("branch.sqlite")).unwrap());
    }

    fn make_legacy(&self, id: &str) -> deepagent_persistence::checkpoint_store::CheckpointRecord {
        let store = CheckpointStore::new(&self.db);
        let mut record = store.get(id).unwrap().unwrap();
        let mut manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
        manifest.as_object_mut().unwrap().remove("branch_anchor");
        manifest["legacy_extra"] = serde_json::json!({"preserve": true});
        record.manifest = manifest.to_string();
        store.put(&record).unwrap();
        record
    }
}

#[test]
fn branch_rewind_keeps_surviving_prefix_across_reopen_and_reused_sequences() {
    let mut fixture = BranchFixture::new();
    let (prefix, _, prefix_tail) = fixture.edit("prefix", "prefix-content");
    let (discarded, old_sequence, _) = fixture.edit("discarded", "old-content");
    let old_record = CheckpointStore::new(&fixture.db)
        .get(&discarded)
        .unwrap()
        .unwrap();
    fixture.rewind(prefix_tail);
    assert_eq!(fixture.content(), "prefix-content");
    fixture.reopen();
    let (current, new_sequence, current_tail) = fixture.edit("current", "new-content");
    assert_eq!(old_sequence, new_sequence);
    let store = CheckpointStore::new(&fixture.db);
    let current_record = store.get(&current).unwrap().unwrap();
    let old_manifest: serde_json::Value = serde_json::from_str(&old_record.manifest).unwrap();
    let current_manifest: serde_json::Value =
        serde_json::from_str(&current_record.manifest).unwrap();
    assert_ne!(
        old_manifest["branch_anchor"],
        current_manifest["branch_anchor"]
    );
    // Qualification must precede backup I/O: a retired branch with a missing
    // backup must neither block nor participate in a current-branch restore.
    let retired_backup =
        std::path::PathBuf::from(old_manifest["entries"][0]["backup"].as_str().unwrap());
    assert!(retired_backup.starts_with(fixture.storage.path()));
    std::fs::remove_file(retired_backup).unwrap();
    fixture.text("text-only tail");
    fixture.rewind(current_tail);
    assert_eq!(fixture.content(), "new-content");
    fixture.reopen();
    fixture.rewind(prefix_tail);
    assert_eq!(fixture.content(), "prefix-content");
    fixture.rewind(0);
    assert_eq!(fixture.content(), "base");
    assert_eq!(fixture.tail(), 0);
    let store = CheckpointStore::new(&fixture.db);
    assert_eq!(store.get(&discarded).unwrap().unwrap(), old_record);
    assert_eq!(store.get(&current).unwrap().unwrap(), current_record);
    assert!(store.get(&prefix).unwrap().is_some());
    assert_eq!(store.list_for_run("discarded").unwrap().len(), 1);
}

#[test]
fn branch_fork_filters_discarded_captures_and_separates_child_runs() {
    let mut fixture = BranchFixture::new();
    fixture.edit("old-first", "old-first");
    let (old_checkpoint, _, _) = fixture.edit("old-second", "old-second");
    fixture.rewind(0);
    let (_, _, kept) = fixture.edit("source-current", "source-current");
    fixture.text("source text tail");
    let source_id = fixture.session_id;
    let source_history = fixture.history();
    let fork = AppService::from_shared(fixture.db.clone())
        .fork_session(&source_id.to_string(), kept)
        .unwrap();
    assert!(fork.restored_paths.is_empty());
    assert_eq!(fixture.content(), "source-current");
    assert_eq!(fixture.history(), source_history);
    fixture.session_id = fork.new_session_id.parse().unwrap();
    assert_eq!(fixture.tail(), kept);
    assert!(fixture.history().to_string().contains("source-current"));
    assert!(!fixture.history().to_string().contains("source text tail"));
    let (child_checkpoint, _, _) = fixture.edit("child", "child-content");
    fixture.reopen();
    fixture.rewind(kept);
    assert_eq!(fixture.content(), "source-current");
    let store = CheckpointStore::new(&fixture.db);
    assert!(store.get(&old_checkpoint).unwrap().is_some());
    assert!(store.get(&child_checkpoint).unwrap().is_some());
    assert!(store
        .list_for_session_after_sequence(&source_id.to_string(), 0)
        .unwrap()
        .iter()
        .all(|record| record.run_id != "child"));
    fixture.session_id = source_id;
    assert_eq!(fixture.history(), source_history);
}

#[test]
fn branch_anchor_is_fixed_before_first_capture_on_a_reused_sequence() {
    let fixture = BranchFixture::new();
    let old_sequence = fixture.text("old turn");
    let old_event = EventStore::new(&fixture.db)
        .load_session(fixture.session_id)
        .unwrap()
        .pop()
        .unwrap();
    RunStore::new(&fixture.db)
        .create(
            "delayed-capture",
            &fixture.session_id.to_string(),
            None,
            1_000,
        )
        .unwrap();
    let manager = CheckpointManager::new(
        fixture.db.clone(),
        "delayed-capture",
        old_sequence as i64,
        fixture.workspace.path(),
        fixture.storage.path(),
    )
    .unwrap();
    fixture.rewind(0);
    assert_eq!(fixture.text("new turn"), old_sequence);
    manager.capture_before(fixture.target()).unwrap();
    std::fs::write(fixture.target(), "new-content").unwrap();
    let record = CheckpointStore::new(&fixture.db)
        .get(manager.id())
        .unwrap()
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
    assert_eq!(
        manifest["branch_anchor"]["event_id"],
        serde_json::to_value(old_event.id).unwrap()
    );
    fixture.rewind(0);
    assert_eq!(fixture.content(), "new-content");
}

#[test]
fn branch_manager_never_rebinds_its_anchor_when_committed_after_rewind() {
    let fixture = BranchFixture::new();
    fixture.text("original turn");
    RunStore::new(&fixture.db)
        .create("late-manager", &fixture.session_id.to_string(), None, 1_000)
        .unwrap();
    let manager = CheckpointManager::new(
        fixture.db.clone(),
        "late-manager",
        fixture.tail() as i64,
        fixture.workspace.path(),
        fixture.storage.path(),
    )
    .unwrap();
    manager.capture_before(fixture.target()).unwrap();
    let original = CheckpointStore::new(&fixture.db)
        .get(manager.id())
        .unwrap()
        .unwrap();
    fixture.rewind(0);
    fixture.text("replacement turn");
    std::fs::write(fixture.target(), "replacement-content").unwrap();
    manager.commit().unwrap();
    let updated = CheckpointStore::new(&fixture.db)
        .get(manager.id())
        .unwrap()
        .unwrap();
    assert_eq!(updated.manifest, original.manifest);
    fixture.rewind(0);
    assert_eq!(fixture.content(), "replacement-content");
}

#[test]
fn legacy_branch_binding_preserves_unknown_fields_and_surviving_prefix() {
    let fixture = BranchFixture::new();
    let (prefix, _, prefix_tail) = fixture.edit("legacy-prefix", "prefix");
    let (tail, _, _) = fixture.edit("legacy-tail", "tail");
    fixture.make_legacy(&prefix);
    fixture.make_legacy(&tail);
    fixture.rewind(prefix_tail);
    assert_eq!(fixture.content(), "prefix");
    for id in [&prefix, &tail] {
        let record = CheckpointStore::new(&fixture.db).get(id).unwrap().unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
        assert!(manifest["branch_anchor"]["event_id"].is_string());
        assert_eq!(manifest["legacy_extra"]["preserve"], true);
    }
    fixture.text("reuse removed sequence without edits");
    fixture.rewind(prefix_tail);
    assert_eq!(fixture.content(), "prefix");
    fixture.rewind(0);
    assert_eq!(fixture.content(), "base");
}

#[test]
fn legacy_branch_with_unknown_identity_refuses_both_operations_without_mutation() {
    use deepagent_core::error::CoreError;
    let mut fixture = BranchFixture::new();
    let (old, _, _) = fixture.edit("unknown-old", "old");
    fixture.rewind(0);
    let legacy = fixture.make_legacy(&old);
    fixture.edit("new-branch", "new-content");
    // SQLite is only a projection. A stale generation=1 must not authorize
    // guessing identity from a reused sequence in the actual generation=2 log.
    fixture
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE session_file_state SET generation=1", [])
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    let before = fixture.history();
    for operation in [Operation::Rewind, Operation::Fork] {
        let app = AppService::from_shared(fixture.db.clone());
        let result = match operation {
            Operation::Rewind => app
                .rewind_session(&fixture.session_id.to_string(), 0)
                .map(|_| ()),
            Operation::Fork => app
                .fork_session(&fixture.session_id.to_string(), 0)
                .map(|_| ()),
        };
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no branch identity"));
        assert_eq!(fixture.content(), "new-content");
        assert_eq!(fixture.history(), before);
        assert_eq!(app.list_sessions().unwrap().len(), 1);
        assert_eq!(
            CheckpointStore::new(&fixture.db)
                .get(&old)
                .unwrap()
                .unwrap(),
            legacy
        );
    }
    fixture.reopen();
    assert_eq!(fixture.content(), "new-content");
    assert_eq!(fixture.history(), before);
    // Explicit checkpoint-by-ID recovery stays available for operator review;
    // rejecting automatic branch qualification does not destroy old backups.
    CheckpointManager::restore(&fixture.db, &old).unwrap();
    assert_eq!(fixture.content(), "base");
}

#[test]
fn legacy_branch_without_source_event_refuses_even_in_original_generation() {
    let fixture = BranchFixture::new();
    let (id, _, _) = fixture.edit("missing-legacy-source", "current");
    let mut record = fixture.make_legacy(&id);
    record.session_sequence = fixture.tail() as i64 + 10;
    CheckpointStore::new(&fixture.db).put(&record).unwrap();
    let before = fixture.history();
    let error = AppService::from_shared(fixture.db.clone())
        .rewind_session(&fixture.session_id.to_string(), 0)
        .unwrap_err();
    assert!(error.to_string().contains("source event missing"));
    assert_eq!(fixture.content(), "current");
    assert_eq!(fixture.history(), before);
    assert_eq!(
        CheckpointStore::new(&fixture.db).get(&id).unwrap().unwrap(),
        record
    );
}

#[test]
fn legacy_branch_binding_failure_rolls_back_metadata_and_allows_retry() {
    use deepagent_core::error::CoreError;
    let mut fixture = BranchFixture::new();
    let (prefix, _, _) = fixture.edit("binding-prefix", "prefix");
    let (tail, _, _) = fixture.edit("binding-tail", "tail");
    let old_prefix = fixture.make_legacy(&prefix);
    let old_tail = fixture.make_legacy(&tail);
    let before = fixture.history();
    fixture.db.with_conn(|conn| {
        conn.execute_batch("CREATE TEMP TRIGGER reject_anchor BEFORE UPDATE ON checkpoints WHEN OLD.run_id='binding-prefix' BEGIN SELECT RAISE(ABORT, 'injected anchor write failure'); END;").map_err(|error| CoreError::Persistence(error.to_string()))?;
        Ok(())
    }).unwrap();
    let error = AppService::from_shared(fixture.db.clone())
        .rewind_session(&fixture.session_id.to_string(), 0)
        .unwrap_err();
    assert!(error.to_string().contains("injected anchor write failure"));
    assert_eq!(fixture.content(), "tail");
    assert_eq!(fixture.history(), before);
    let store = CheckpointStore::new(&fixture.db);
    assert_eq!(store.get(&prefix).unwrap().unwrap(), old_prefix);
    assert_eq!(store.get(&tail).unwrap().unwrap(), old_tail);
    fixture
        .db
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_anchor")
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    fixture.reopen();
    fixture.rewind(0);
    assert_eq!(fixture.content(), "base");
    assert_eq!(fixture.tail(), 0);
}

#[test]
fn busy_history_control_refuses_before_touching_workspace_or_checkpoint_metadata() {
    for fork in [false, true] {
        let mut fixture = BranchFixture::new();
        let (checkpoint_id, _, _) = fixture.edit("busy-control-edit", "current");
        // Legacy metadata makes even an otherwise successful preflight have a
        // visible mutation. Busy refusal must happen before that preflight.
        let checkpoint_before = fixture.make_legacy(&checkpoint_id);
        let history_before = fixture.history();
        let sessions_before = EventStore::new(&fixture.db).list_sessions().unwrap().len();
        let activity = EventStore::new(&fixture.db)
            .acquire_activity(fixture.session_id)
            .unwrap();
        let app = AppService::from_shared(fixture.db.clone());
        let result = if fork {
            app.fork_session(&fixture.session_id.to_string(), 0)
                .map(|_| ())
        } else {
            app.rewind_session(&fixture.session_id.to_string(), 0)
                .map(|_| ())
        };
        assert!(result.unwrap_err().to_string().contains("is busy"));
        assert_eq!(fixture.content(), "current");
        assert_eq!(fixture.history(), history_before);
        assert_eq!(
            CheckpointStore::new(&fixture.db)
                .get(&checkpoint_id)
                .unwrap()
                .unwrap(),
            checkpoint_before
        );
        assert_eq!(
            EventStore::new(&fixture.db).list_sessions().unwrap().len(),
            sessions_before
        );
        drop(activity);
        drop(app);
        fixture.reopen();
        let app = AppService::from_shared(fixture.db.clone());
        if fork {
            let child = app
                .fork_session(&fixture.session_id.to_string(), 0)
                .unwrap();
            assert!(!child.new_session_id.is_empty());
            assert_eq!(fixture.history(), history_before);
        } else {
            app.rewind_session(&fixture.session_id.to_string(), 0)
                .unwrap();
            assert_eq!(fixture.tail(), 0);
        }
        assert_eq!(fixture.content(), "base");
    }
}
