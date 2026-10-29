use super::*;
use crate::artifact_store::ArtifactKind;
use crate::Database;

const BYTES: &[u8] = b"synthetic published artifact";

fn fixture(root: &Path) -> (Database, ArtifactRecord) {
    let db = Database::open(root.join("state.db")).unwrap();
    let path = root.join("art_h254.bin");
    std::fs::write(&path, BYTES).unwrap();
    let record = ArtifactRecord {
        id: "art_h254".into(),
        kind: ArtifactKind::Document,
        run_id: None,
        call_id: None,
        workspace_id: Some("ws-a".into()),
        path: path.to_string_lossy().into_owned(),
        media_type: None,
        byte_size: BYTES.len() as i64,
        digest: Some(format!("sha256:{:x}", Sha256::digest(BYTES))),
        created_at: 1,
    };
    ArtifactStore::new(&db).put_new(&record).unwrap();
    (db, record)
}

fn sql(db: &Database, sql: &str) {
    db.with_conn(|conn| conn.execute_batch(sql).map_err(map_sqlite))
        .unwrap();
}

fn block_receipt(db: &Database) {
    sql(
        db,
        "CREATE TRIGGER reject_receipt BEFORE DELETE ON documents
        WHEN OLD.collection='artifact_deletions_v1'
        BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;",
    );
}

#[test]
fn h254_deletion_intent_requires_full_sync_and_restores_connection_policy() {
    for initial in [0, 1, 2, 3] {
        let root = tempfile::tempdir().unwrap();
        let (db, record) = fixture(root.path());
        sql(&db, &format!("PRAGMA synchronous={initial};"));
        sql(
            &db,
            "CREATE TRIGGER require_durable_intent BEFORE INSERT ON documents
            WHEN NEW.collection='artifact_deletions_v1'
             AND (SELECT synchronous FROM pragma_synchronous)<2
            BEGIN SELECT RAISE(ABORT,'deletion intent requires FULL sync'); END;",
        );
        let store = ArtifactStore::new(&db);
        assert!(store
            .delete_file(&record, verify_file_for_deletion)
            .unwrap());
        let actual: i64 = db
            .with_conn(|conn| {
                conn.query_row("PRAGMA synchronous", [], |row| row.get(0))
                    .map_err(map_sqlite)
            })
            .unwrap();
        assert_eq!(
            actual, initial,
            "do not silently change the connection policy"
        );
    }
}

#[test]
fn h254_journal_and_index_failures_leave_original_publication_intact() {
    for statement in [
        "CREATE TRIGGER fail BEFORE INSERT ON documents BEGIN SELECT RAISE(ABORT,'journal failure'); END;",
        "CREATE TRIGGER fail BEFORE DELETE ON artifacts BEGIN SELECT RAISE(ABORT,'index failure'); END;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (db, record) = fixture(root.path());
        let store = ArtifactStore::new(&db);
        sql(&db, statement);
        assert!(store.delete_file(&record, verify_file_for_deletion).is_err());
        assert_eq!(db.with_conn(synchronous).unwrap(), 1,
            "a failed transaction must also restore the connection policy");
        assert_eq!(store.state(&record.id).unwrap(), Some(ArtifactState::Active(record.clone())));
        assert!(store.pending_deletions().unwrap().is_empty());
        assert_eq!(std::fs::read(&record.path).unwrap(), BYTES);
        sql(&db, "DROP TRIGGER fail;");
        assert!(store.delete_file(&record, verify_file_for_deletion).unwrap());
        assert!(!store.delete_file(&record, verify_file_for_deletion).unwrap());
    }
}

#[test]
fn h254_receipt_failure_blocks_identity_and_path_reuse_until_retry() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    let store = ArtifactStore::new(&db);
    block_receipt(&db);
    let error = store
        .delete_file(&record, verify_file_for_deletion)
        .unwrap_err();
    assert!(matches!(error, CoreError::ArtifactPendingDelete { .. }));
    assert!(!Path::new(&record.path).exists());
    assert!(store.get(&record.id).unwrap().is_none());
    assert_eq!(
        store.state(&record.id).unwrap(),
        Some(ArtifactState::PendingDelete(record.clone()))
    );
    assert!(store.list(None).unwrap().is_empty());
    assert!(store.put_new(&record).is_err());
    let mut replacement = record.clone();
    replacement.workspace_id = Some("ws-b".into());
    replacement.path = root.path().join("new.bin").to_string_lossy().into_owned();
    assert!(store.put(&replacement).is_err());
    let mut alias = record.clone();
    alias.id = "another_id".into();
    assert!(store.put_new(&alias).is_err());
    let intent = db
        .with_conn(|conn| intent_on(conn, &record.id))
        .unwrap()
        .unwrap();
    assert_eq!(intent.attempts, 1);
    assert_eq!(intent.record, record);
    sql(&db, "DROP TRIGGER reject_receipt;");
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .unwrap());
    assert!(store.state(&record.id).unwrap().is_none());
    assert!(store.pending_deletions().unwrap().is_empty());
}

#[test]
fn h254_pending_cleanup_refuses_changed_bytes_and_preserves_intent() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    let store = ArtifactStore::new(&db);
    block_receipt(&db);
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .is_err());
    sql(&db, "DROP TRIGGER reject_receipt;");
    std::fs::write(&record.path, b"replacement").unwrap();
    let error = store
        .delete_file(&record, verify_file_for_deletion)
        .unwrap_err();
    assert!(matches!(error, CoreError::ArtifactPendingDelete { .. }));
    assert_eq!(std::fs::read(&record.path).unwrap(), b"replacement");
    assert_eq!(store.pending_deletions().unwrap(), [record]);
}

#[test]
fn h254_stale_metadata_and_shared_paths_do_not_delete_other_publications() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    let store = ArtifactStore::new(&db);
    let mut changed = record.clone();
    changed.workspace_id = Some("ws-b".into());
    store.put(&changed).unwrap();
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .is_err());
    let mut alias = changed.clone();
    alias.id = "art_alias".into();
    store.put_new(&alias).unwrap();
    assert!(store
        .delete_file(&changed, verify_file_for_deletion)
        .is_err());
    assert_eq!(std::fs::read(&record.path).unwrap(), BYTES);
    assert!(store.pending_deletions().unwrap().is_empty());
}

#[test]
fn h254_concurrent_deleters_commit_only_once_across_connections() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    drop(db);
    let connections: Vec<_> = (0..4)
        .map(|_| Database::open(root.path().join("state.db")).unwrap())
        .collect();
    let barrier = std::sync::Barrier::new(connections.len());
    std::thread::scope(|threads| {
        let workers: Vec<_> = connections
            .iter()
            .map(|db| {
                let barrier = &barrier;
                let record = &record;
                threads.spawn(move || {
                    barrier.wait();
                    ArtifactStore::new(db)
                        .delete_file(record, verify_file_for_deletion)
                        .unwrap()
                })
            })
            .collect();
        let deleted: usize = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum();
        assert_eq!(deleted, 1);
    });
    // All scoped workers must complete before checking both durable surfaces.
    let store = ArtifactStore::new(&connections[0]);
    assert!(store.state(&record.id).unwrap().is_none());
    assert!(store.pending_deletions().unwrap().is_empty());
    assert!(!Path::new(&record.path).exists());
}

#[test]
fn h254_crash_child() {
    let Some(root) = std::env::var_os("DEEPAGENT_H254_CHILD_ROOT") else {
        return;
    };
    let db = Database::open(Path::new(&root).join("state.db")).unwrap();
    let store = ArtifactStore::new(&db);
    let record = store.get("art_h254").unwrap().unwrap();
    store
        .delete_file(&record, verify_file_for_deletion)
        .unwrap();
    panic!("the requested process-exit checkpoint was not reached");
}

#[test]
fn h254_process_exit_at_each_commit_boundary_recovers_without_dangling_active_row() {
    for stage in ["before_commit", "after_commit", "after_unlink"] {
        let root = tempfile::tempdir().unwrap();
        let (db, record) = fixture(root.path());
        drop(db);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "artifact_store::deletion::tests::h254_crash_child",
                "--nocapture",
            ])
            .env("DEEPAGENT_H254_CHILD_ROOT", root.path())
            .env("DEEPAGENT_H254_CRASH_STAGE", stage)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "stage={stage}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reopened = Database::open(root.path().join("state.db")).unwrap();
        let store = ArtifactStore::new(&reopened);
        let expected_state = if stage == "before_commit" {
            ArtifactState::Active(record.clone())
        } else {
            ArtifactState::PendingDelete(record.clone())
        };
        assert_eq!(store.state(&record.id).unwrap(), Some(expected_state));
        assert_eq!(Path::new(&record.path).exists(), stage != "after_unlink");
        assert!(store
            .delete_file(&record, verify_file_for_deletion)
            .unwrap());
        assert!(store.state(&record.id).unwrap().is_none());
        assert!(!Path::new(&record.path).exists());
    }
}

#[test]
fn h254_unknown_journal_version_is_not_interpreted_as_permission_to_delete() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    let body = serde_json::to_string(&DeletionIntent {
        version: 2,
        record: record.clone(),
        started_at: 1,
        attempts: 1,
    })
    .unwrap();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO documents(collection,id,body,created_at,updated_at) VALUES (?1,?2,?3,1,1)",
            params![COLLECTION, record.id, body],
        )
        .map_err(map_sqlite)?;
        Ok(())
    })
    .unwrap();
    let store = ArtifactStore::new(&db);
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .is_err());
    assert!(store.pending_deletions().is_err());
    assert_eq!(std::fs::read(&record.path).unwrap(), BYTES);
}

#[test]
fn h254_legacy_unverifiable_record_is_retained_and_missing_verified_bytes_are_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let (db, record) = fixture(root.path());
    let store = ArtifactStore::new(&db);
    let mut legacy = record.clone();
    legacy.digest = None;
    store.put(&legacy).unwrap();
    assert!(!store
        .delete_file(&legacy, verify_file_for_deletion)
        .unwrap());
    assert_eq!(store.get(&record.id).unwrap(), Some(legacy));
    assert_eq!(std::fs::read(&record.path).unwrap(), BYTES);
    store.put(&record).unwrap();
    std::fs::remove_file(&record.path).unwrap();
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .unwrap());
    assert!(store.state(&record.id).unwrap().is_none());
}

#[cfg(windows)]
#[test]
fn h254_pending_cleanup_refuses_junction_replacement_even_with_identical_bytes() {
    use std::os::windows::process::CommandExt;
    let root = tempfile::tempdir().unwrap();
    let (db, mut record) = fixture(root.path());
    let directory = root.path().join("media");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("art_h254.bin");
    std::fs::rename(&record.path, &path).unwrap();
    record.path = path.to_string_lossy().into_owned();
    let store = ArtifactStore::new(&db);
    store.put(&record).unwrap();
    block_receipt(&db);
    assert!(store
        .delete_file(&record, verify_file_for_deletion)
        .is_err());
    sql(&db, "DROP TRIGGER reject_receipt;");
    std::fs::remove_dir(&directory).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let foreign = outside.path().join("art_h254.bin");
    std::fs::write(&foreign, BYTES).unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&directory)
        .arg(outside.path())
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(output.status.success(), "junction fixture: {output:?}");
    assert!(matches!(
        store
            .delete_file(&record, verify_file_for_deletion)
            .unwrap_err(),
        CoreError::ArtifactPendingDelete { .. }
    ));
    assert_eq!(std::fs::read(&foreign).unwrap(), BYTES);
    assert_eq!(store.pending_deletions().unwrap(), [record]);
}
