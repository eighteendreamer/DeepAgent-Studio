//! H229: only synthetic authority files, actual SQLite and the public owner.
use super::*;
use deepagent_core::message::Message;

const QUERIES: [&str; 3] = ["检", "缓存", "repairneedle"];

#[test]
fn h229_healthy_history_above_the_old_total_decode_budget_remains_searchable() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db, "A");
    let store = EventStore::new(&db);
    for index in 0..70 {
        store
            .append(
                id,
                EventPayload::Note {
                    text: "n".repeat(1024 * 1024),
                },
                Timestamp::from_millis(102 + index),
            )
            .unwrap();
    }
    let report = store
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(report.hits.len(), 1, "a healthy many-record history must not be permanently excluded by its aggregate decoded size");
    assert_eq!(report.hits[0].session.id, id);
    assert!(report.unavailable.is_empty());
    assert_eq!(report.hits[0].context.len(), 3);
    let projected = || {
        db.with_conn(|c| {
            c.query_row(
                "SELECT id, content_hash FROM session_search_docs WHERE session_id=?1",
                [id.to_string()],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(map_sqlite)
        })
        .unwrap()
    };
    let original_projection = projected();
    // Corruption in an unmatched final record must still prevent using this
    // history. Streaming snippets cannot stop validation after the first hit.
    let path = file(&db, id);
    let mut broken = std::fs::read(&path).unwrap();
    *broken.last_mut().unwrap() ^= 1;
    std::fs::write(&path, &broken).unwrap();
    let bad = store
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert!(bad.hits.is_empty());
    assert_eq!(
        bad.unavailable,
        vec![SessionSearchIssue {
            session_id: id,
            kind: SessionSearchIssueKind::Corrupt
        }]
    );
    assert_eq!(std::fs::read(path).unwrap(), broken);
    assert_eq!(
        projected(),
        original_projection,
        "a late CRC failure must roll back the staged delete/insert, not publish its prefix"
    );
}

#[test]
fn h229_search_does_not_park_on_the_global_connection_mutex() {
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    seed(&db, "A");
    let worker_db = db.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let mut before_release = None;
    let worker = db
        .with_conn(|_| {
            let worker = std::thread::spawn(move || {
                send.send(EventStore::new(&worker_db).search_with_status(
                    "repairneedle",
                    SessionSearchScope::All,
                    10,
                ))
                .unwrap();
            });
            before_release = Some(receive.recv_timeout(std::time::Duration::from_millis(500)));
            Ok(worker)
        })
        .unwrap();
    worker.join().unwrap();
    let error = before_release
        .unwrap()
        .expect("optional search must not park behind the connection owner")
        .unwrap_err();
    assert!(error.to_string().contains("session_search_busy"));
    assert!(EventStore::new(&db)
        .search_file_issues(SessionSearchScope::All)
        .unwrap()
        .is_empty());
    assert_eq!(
        EventStore::new(&db)
            .search("repairneedle", SessionSearchScope::All, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn h229_external_sqlite_writer_is_a_prompt_global_error_and_does_not_change_busy_timeout_or_file_health(
) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let id = seed(&db, "A");
    db.with_conn(|c| {
        c.execute_batch("PRAGMA busy_timeout=5432;")
            .map_err(map_sqlite)
    })
    .unwrap();
    let source = file(&db, id);
    let original = std::fs::read(&source).unwrap();
    let other = rusqlite::Connection::open(&path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let started = std::time::Instant::now();
    let result =
        EventStore::new(&db).search_with_status("repairneedle", SessionSearchScope::All, 10);
    let elapsed = started.elapsed();
    other.execute_batch("ROLLBACK;").unwrap();
    assert!(
        result.is_err(),
        "a global projection write error cannot become a partial success"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "optional SQLite write waited behind another writer: {elapsed:?}"
    );
    let timeout: i64 = db
        .with_conn(|c| {
            c.query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                .map_err(map_sqlite)
        })
        .unwrap();
    assert_eq!(
        timeout, 5432,
        "optional maintenance must restore the exact connection policy"
    );
    let store = EventStore::new(&db);
    assert!(store
        .search_file_issues(SessionSearchScope::All)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(
        store
            .search("repairneedle", SessionSearchScope::All, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn h229_process_io_contention_is_deferred_without_blocking_the_search_worker() {
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let id = seed(&db, "A");
    let guard = db.lock_session_io().unwrap();
    let worker_db = db.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let result = EventStore::new(&worker_db).search_with_status(
            "repairneedle",
            SessionSearchScope::All,
            10,
        );
        send.send(result).unwrap();
    });
    let before_release = receive.recv_timeout(std::time::Duration::from_millis(500));
    drop(guard);
    worker.join().unwrap();
    let report = before_release
        .expect("optional search cannot wait for the process-wide writer gate")
        .unwrap();
    assert!(report.hits.is_empty());
    assert_eq!(
        report.unavailable,
        vec![SessionSearchIssue {
            session_id: id,
            kind: SessionSearchIssueKind::Busy
        }]
    );
    let retry = EventStore::new(&db)
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(retry.hits.len(), 1);
    assert!(retry.unavailable.is_empty());
}

#[test]
fn h229_cancellation_and_byte_budgets_during_real_decode_never_truncate_or_mark_the_source_corrupt()
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    let db = Database::open_in_memory().unwrap();
    let id = seed_text(&db, "A", &"real decode checkpoint ".repeat(128 * 1024));
    let path = file(&db, id);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(&[0x28, 0xb5, 0x2f, 0xfd]);
    std::fs::write(&path, &bytes).unwrap();
    let calls = AtomicUsize::new(0);
    let cancelled = || calls.fetch_add(1, Ordering::SeqCst) >= 20;
    let control = session_files::ReadControl::new(&cancelled, None, u64::MAX, u64::MAX);
    let result = session_files::try_load_for_maintenance(&db.session_files_root(), id, &control);
    assert!(matches!(
        result,
        Err(session_files::MaintenanceReadError::Stopped(
            session_files::ReadStop::Cancelled
        ))
    ));
    let (compressed, decoded) = control.progress();
    assert!(compressed > 0 && decoded > 64 * 1024, "must stop after real source reading/decompression, not only at entry: {compressed}/{decoded}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    for (source_limit, decoded_limit) in [(1, u64::MAX), (u64::MAX, 1024)] {
        let control = session_files::ReadControl::new(&|| false, None, source_limit, decoded_limit);
        assert!(matches!(
            session_files::try_load_for_maintenance(&db.session_files_root(), id, &control),
            Err(session_files::MaintenanceReadError::Stopped(
                session_files::ReadStop::Budget
            ))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    assert!(EventStore::new(&db)
        .search_file_issues(SessionSearchScope::All)
        .unwrap()
        .is_empty());
    // Ordinary replay still uses the same strict parser and may recover the
    // valid torn tail only after the complete source has passed validation.
    assert_eq!(EventStore::new(&db).load_session(id).unwrap().len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), bytes[..bytes.len() - 4]);
}

#[test]
fn h229_pre_cancelled_search_does_not_read_repair_or_quarantine_any_history() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db, "A");
    let path = file(&db, id);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(0xff);
    std::fs::write(&path, &bytes).unwrap();
    let control = SessionSearchControl::new(std::time::Duration::from_secs(5), &|| true);
    let store = EventStore::new(&db);
    let error = store
        .search_with_control("repairneedle", SessionSearchScope::All, 10, &control)
        .unwrap_err();
    assert!(error.to_string().contains("session_search_cancelled"));
    assert!(store
        .search_file_issues(SessionSearchScope::All)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn h229_zero_maintenance_budget_is_explicitly_deferred_not_complete_or_corrupt() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db, "A");
    let control = SessionSearchControl::new(std::time::Duration::ZERO, &|| false);
    let store = EventStore::new(&db);
    let report = store
        .search_with_control("repairneedle", SessionSearchScope::All, 10, &control)
        .unwrap();
    assert!(report.hits.is_empty());
    assert_eq!(
        report.unavailable,
        vec![SessionSearchIssue {
            session_id: id,
            kind: SessionSearchIssueKind::Deferred
        }]
    );
    let retry = store
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(retry.hits.len(), 1);
    assert!(retry.unavailable.is_empty());
}

#[test]
fn h229_short_maintenance_attempts_rotate_the_existing_cursor_instead_of_starving_later_files() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let mut ids: Vec<_> = (0..3).map(|_| seed(&db, "A")).collect();
    ids.sort_unstable_by_key(ToString::to_string);
    for expected in &ids {
        let calls = AtomicUsize::new(0);
        let checkpoint = || {
            // Rendezvous at the first actual file-read checkpoint, after the
            // repository has selected an attempt. No corrupted/fake parser.
            if calls.fetch_add(1, Ordering::SeqCst) == 3 {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            false
        };
        let control = SessionSearchControl::new(std::time::Duration::from_millis(200), &checkpoint);
        let report = EventStore::new(&db)
            .search_with_control("repairneedle", SessionSearchScope::All, 10, &control)
            .unwrap();
        assert!(report.hits.is_empty());
        assert_eq!(report.unavailable.len(), 3);
        assert!(report
            .unavailable
            .iter()
            .all(|issue| issue.kind == SessionSearchIssueKind::Deferred));
        let last: String = db.with_conn(|c| c.query_row("SELECT body FROM documents WHERE collection='session_search_maintenance_v1' AND id='cursor'", [], |r| r.get(0)).map_err(map_sqlite)).unwrap();
        assert_eq!(serde_json::from_str::<SessionId>(&last).unwrap(), *expected);
    }
    drop(db);
    let reopened = Database::open(path).unwrap();
    let report = EventStore::new(&reopened)
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(report.hits.len(), 3);
    assert!(report.unavailable.is_empty());
}

fn seed(db: &Database, project: &str) -> SessionId {
    seed_text(db, project, "检 缓存 repairneedle decision")
}

fn seed_text(db: &Database, project: &str, text: &str) -> SessionId {
    let id = SessionId::new();
    let store = EventStore::new(db);
    store
        .create_session_full(
            id,
            Some("H229 synthetic history"),
            Default::default(),
            Some(project),
            Timestamp::from_millis(100),
        )
        .unwrap();
    store
        .append(
            id,
            EventPayload::MessageAppended {
                message: Message::user(text),
            },
            Timestamp::from_millis(101),
        )
        .unwrap();
    id
}

#[test]
fn h229_real_checksum_failure_in_unmatched_same_size_file_is_not_silently_complete() {
    checksum_failure(false);
}

#[test]
fn h229_same_size_and_restored_mtime_are_not_an_integrity_proof() {
    checksum_failure(true);
}

fn checksum_failure(restore_mtime: bool) {
    let db = Database::open_in_memory().unwrap();
    let good = seed(&db, "A");
    let bad = seed_text(&db, "A", "unrelated synthetic history");
    let store = EventStore::new(&db);
    assert_eq!(
        store
            .search("repairneedle", SessionSearchScope::All, 10)
            .unwrap()
            .len(),
        1
    );
    let path = file(&db, bad);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut broken = std::fs::read(&path).unwrap();
    // encode_records enables Zstd checksums. Flip only its final checksum
    // byte; the length, generation, all magic bytes and all SQLite rows stay.
    *broken.last_mut().unwrap() ^= 1;
    std::fs::write(&path, &broken).unwrap();
    if restore_mtime {
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(file.metadata().unwrap().modified().unwrap(), modified);
    }
    let report = store
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].session.id, good);
    assert_eq!(
        report.unavailable,
        vec![SessionSearchIssue {
            session_id: bad,
            kind: SessionSearchIssueKind::Corrupt
        }]
    );
    assert_eq!(std::fs::read(path).unwrap(), broken);
    assert!(store
        .load_session(bad)
        .unwrap_err()
        .to_string()
        .contains("corrupt zstd frame"));
}

fn file(db: &Database, id: SessionId) -> std::path::PathBuf {
    std::fs::read_dir(db.session_transcript_dir(id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".v1.jsonl.zst"))
        .unwrap()
}

#[test]
fn h229_bad_project_cannot_block_another_scope_or_rewrite_bad_bytes() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path().join("state.sqlite")).unwrap();
    let good = seed(&db, "A");
    let bad = seed(&db, "B");
    let path = file(&db, bad);
    let mut broken = std::fs::read(&path).unwrap();
    broken.push(0xff);
    std::fs::write(&path, &broken).unwrap();
    let store = EventStore::new(&db);
    for query in QUERIES {
        let hits = store
            .search(query, SessionSearchScope::Project("A"), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, good);
    }
    assert!(store
        .load_session(bad)
        .unwrap_err()
        .to_string()
        .contains("invalid zstd frame"));
    assert_eq!(std::fs::read(path).unwrap(), broken);
}

#[test]
fn h229_bad_known_or_orphan_file_does_not_prevent_real_database_reopen() {
    for orphan in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("state.sqlite");
        let db = Database::open(&db_path).unwrap();
        let good = seed(&db, "A");
        let bad = seed(&db, "B");
        let path = file(&db, bad);
        let original = std::fs::read(&path).unwrap();
        let mut broken = original.clone();
        broken.push(0xff);
        std::fs::write(&path, &broken).unwrap();
        if orphan {
            db.with_conn(|c| {
                c.execute("DELETE FROM sessions WHERE id=?1", [bad.to_string()])
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .unwrap();
        }
        drop(db);
        let reopened = Database::open(&db_path).unwrap();
        let store = EventStore::new(&reopened);
        assert_eq!(store.load_session(good).unwrap().len(), 1);
        assert!(store.load_session(bad).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), broken);
        drop(reopened);
        // Restoring the only synthetic authority file must allow normal
        // recovery without clearing SQLite, any index or any quarantine row.
        std::fs::write(&path, original).unwrap();
        let restored = Database::open(&db_path).unwrap();
        assert_eq!(
            EventStore::new(&restored).load_session(bad).unwrap().len(),
            1
        );
        assert_eq!(
            EventStore::new(&restored)
                .search("repairneedle", SessionSearchScope::All, 10)
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn h229_global_sqlite_failure_is_not_downgraded_to_session_quarantine() {
    let db = Database::open_in_memory().unwrap();
    let id = seed(&db, "A");
    db.with_conn(|c| {
        c.execute_batch("DROP TABLE session_search_fts;")
            .map_err(map_sqlite)
    })
    .unwrap();
    let error = EventStore::new(&db)
        .search("repairneedle", SessionSearchScope::All, 10)
        .unwrap_err();
    assert!(error.to_string().contains("session_search_fts"));
    assert_eq!(EventStore::new(&db).load_session(id).unwrap().len(), 1);
}

#[test]
fn h229_multiple_bad_matching_histories_preserve_good_hits_and_source_context_in_every_scope() {
    for project in [Some("A"), None] {
        let db = Database::open_in_memory().unwrap();
        let ids: Vec<_> = (0..3).map(|_| seed(&db, "A")).collect();
        if project.is_none() {
            db.with_conn(|c| {
                c.execute("UPDATE sessions SET project=NULL", [])
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .unwrap();
        }
        let mut failures = Vec::new();
        for id in &ids[1..] {
            let path = file(&db, *id);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.push(0xff);
            std::fs::write(&path, &bytes).unwrap();
            failures.push((path, bytes));
        }
        let scope = project.map_or(SessionSearchScope::NoProject, SessionSearchScope::Project);
        for query in QUERIES {
            let report = EventStore::new(&db)
                .search_with_status(query, scope, 10)
                .unwrap();
            assert_eq!(report.hits.len(), 1);
            assert_eq!(report.hits[0].session.id, ids[0]);
            assert_eq!(report.hits[0].context.len(), 1);
            assert_eq!(report.hits[0].context[0].session_id, ids[0]);
            assert_eq!(report.unavailable.len(), 2);
            assert!(report
                .unavailable
                .iter()
                .all(|issue| issue.kind == SessionSearchIssueKind::Corrupt));
            assert!(EventStore::new(&db).search(query, scope, 10).is_err());
        }
        for (path, bytes) in failures {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn h229_projection_or_quarantine_transaction_failure_propagates_and_never_marks_healthy_source_bad()
{
    for quarantine in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let id = seed(&db, "A");
        let path = file(&db, id);
        if quarantine {
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.push(0xff);
            std::fs::write(&path, bytes).unwrap();
        }
        db.with_conn(|c| {
            c.execute_batch(if quarantine {
                "CREATE TRIGGER fail_health BEFORE INSERT ON documents
                 WHEN NEW.collection='session_file_health_v1'
                 BEGIN SELECT RAISE(ABORT, 'synthetic health transaction failure'); END;"
            } else {
                "CREATE TRIGGER fail_projection BEFORE INSERT ON session_search_docs
                 BEGIN SELECT RAISE(ABORT, 'synthetic projection transaction failure'); END;"
            })
            .map_err(map_sqlite)
        })
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let store = EventStore::new(&db);
        let error = store
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap_err();
        assert!(error.to_string().contains("synthetic"));
        assert!(store
            .search_file_issues(SessionSearchScope::All)
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        if !quarantine {
            assert_eq!(store.load_session(id).unwrap().len(), 1);
        }
    }
}

#[test]
fn h229_missing_authority_is_explicit_and_restored_bytes_clear_health_without_clearing_sqlite() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let good = seed(&db, "A");
    let bad = seed(&db, "A");
    let source = file(&db, bad);
    let bytes = std::fs::read(&source).unwrap();
    // Moving a fixture-owned authority is reversible; no real user data.
    let retained = source.with_extension("h229-retained");
    std::fs::rename(&source, &retained).unwrap();
    let report = EventStore::new(&db)
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].session.id, good);
    assert_eq!(
        report.unavailable,
        vec![SessionSearchIssue {
            session_id: bad,
            kind: SessionSearchIssueKind::Missing
        }]
    );
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        EventStore::new(&reopened)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap()
            .unavailable
            .len(),
        1
    );
    assert_eq!(std::fs::read(&retained).unwrap(), bytes);
    std::fs::rename(&retained, &source).unwrap();
    let recovered = EventStore::new(&reopened)
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    assert_eq!(recovered.hits.len(), 2);
    assert!(recovered.unavailable.is_empty());
    assert_eq!(std::fs::read(source).unwrap(), bytes);
}

#[test]
fn h229_one_busy_file_does_not_serialize_every_other_history_behind_a_five_second_wait() {
    let db = Database::open_in_memory().unwrap();
    let good = seed(&db, "A");
    let busy = seed(&db, "A");
    let guard = session_files::lock_session(&db.session_files_root(), busy).unwrap();
    let started = std::time::Instant::now();
    let report = EventStore::new(&db)
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap();
    let elapsed = started.elapsed();
    drop(guard);
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].session.id, good);
    assert_eq!(report.unavailable.len(), 1);
    assert_eq!(report.unavailable[0].session_id, busy);
    assert_ne!(report.unavailable[0].kind, SessionSearchIssueKind::Corrupt);
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "search must defer a busy history without waiting for its writer: {elapsed:?}"
    );
}

#[test]
fn h229_database_open_does_not_execute_optional_search_projection_transactions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Database::open(&path).unwrap();
    let id = seed(&db, "A");
    db.with_conn(|c| {
        c.execute_batch(
            "UPDATE session_search_cursors SET dirty=1;
             CREATE TRIGGER fail_optional_projection BEFORE INSERT ON session_search_docs
             BEGIN SELECT RAISE(ABORT, 'synthetic optional projection failure'); END;",
        )
        .map_err(map_sqlite)
    })
    .unwrap();
    drop(db);
    let reopened = Database::open(&path).unwrap_or_else(|error| {
        panic!("opening authority state cannot require rebuilding all search indexes: {error}")
    });
    // Search itself must still surface SQLite/projection transaction failures.
    // It cannot achieve startup availability by swallowing this global error.
    let store = EventStore::new(&reopened);
    assert_eq!(store.load_session(id).unwrap().len(), 1);
    assert!(store
        .search_with_status("repairneedle", SessionSearchScope::All, 10)
        .unwrap_err()
        .to_string()
        .contains("synthetic optional projection failure"));
}

#[cfg(windows)]
#[test]
fn h229_exclusive_file_opens_defer_only_the_busy_history_without_retrying_for_five_seconds() {
    use std::os::windows::fs::OpenOptionsExt;
    for lock_file in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let good = seed(&db, "A");
        let busy = seed(&db, "A");
        let path = if lock_file {
            db.data_root()
                .join("files/locks/sessions")
                .join(format!("{busy}.lock"))
        } else {
            file(&db, busy)
        };
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .unwrap();
        let started = std::time::Instant::now();
        let report = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].session.id, good);
        assert_eq!(
            report.unavailable,
            vec![SessionSearchIssue {
                session_id: busy,
                kind: SessionSearchIssueKind::Busy,
            }]
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "busy open must not wait: {:?}",
            started.elapsed()
        );
        drop(guard);
        let recovered = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(recovered.hits.len(), 2);
        assert!(recovered.unavailable.is_empty());
    }
}

#[cfg(windows)]
#[test]
fn h229_actual_access_denied_is_unreadable_not_busy_or_corrupt_and_is_retryable() {
    struct RestorePermissions(std::path::PathBuf, std::fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    for lock_file in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let good = seed(&db, "A");
        let denied = seed(&db, "A");
        let path = if lock_file {
            db.data_root()
                .join("files/locks/sessions")
                .join(format!("{denied}.lock"))
        } else {
            file(&db, denied)
        };
        let original = std::fs::read(file(&db, denied)).unwrap();
        let restore = RestorePermissions(
            path.clone(),
            std::fs::metadata(&path).unwrap().permissions(),
        );
        let mut permissions = restore.1.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        // Real Windows ERROR_ACCESS_DENIED at the same read/write open used
        // by maintenance. This is a file-attribute test, not an ACL simulation.
        assert_eq!(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap_err()
                .raw_os_error(),
            Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
        );
        let started = std::time::Instant::now();
        let report = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].session.id, good);
        assert_eq!(
            report.unavailable,
            vec![SessionSearchIssue {
                session_id: denied,
                kind: SessionSearchIssueKind::Unreadable,
            }]
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(std::fs::read(file(&db, denied)).unwrap(), original);
        drop(restore);
        let recovered = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(recovered.hits.len(), 2);
        assert!(recovered.unavailable.is_empty());
    }
}

#[cfg(windows)]
#[test]
fn h229_real_dacl_denial_is_isolated_and_recovery_restores_search_without_changing_source() {
    // The same local .NET ACL APIs used by the existing file_metadata probe.
    // Only two synthetic TempDir files are changed; SDDL never leaves memory
    // or appears in logs. ReadControl/WriteDac remain available for restoration.
    fn policy(path: &std::path::Path, restore: Option<&str>) -> String {
        let script = r#"
$ErrorActionPreference = 'Stop'
$path = $env:DEEPAGENT_H229_ACL_PATH
$section = [System.Security.AccessControl.AccessControlSections]::Access
$policy = [System.IO.File]::GetAccessControl($path)
$original = $policy.GetSecurityDescriptorSddlForm($section)
if ($env:DEEPAGENT_H229_ACL_RESTORE) {
    $policy.SetSecurityDescriptorSddlForm($env:DEEPAGENT_H229_ACL_RESTORE, $section)
} else {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $rights = [System.Security.AccessControl.FileSystemRights]::ReadData -bor [System.Security.AccessControl.FileSystemRights]::WriteData
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new($identity, $rights, [System.Security.AccessControl.AccessControlType]::Deny)
    $policy.AddAccessRule($rule)
}
[System.IO.File]::SetAccessControl($path, $policy)
[Console]::Write($original)
"#;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("DEEPAGENT_H229_ACL_PATH", path)
            .env("DEEPAGENT_H229_ACL_RESTORE", restore.unwrap_or_default())
            .output()
            .expect("local ACL fixture requires PowerShell");
        assert!(output.status.success(), "local ACL operation failed");
        String::from_utf8(output.stdout).unwrap()
    }
    struct RestoreAcl(std::path::PathBuf, String);
    impl Drop for RestoreAcl {
        fn drop(&mut self) {
            policy(&self.0, Some(&self.1));
        }
    }
    for lock_file in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let good = seed(&db, "A");
        let denied = seed(&db, "A");
        let source = file(&db, denied);
        let original = std::fs::read(&source).unwrap();
        let path = if lock_file {
            db.data_root()
                .join("files/locks/sessions")
                .join(format!("{denied}.lock"))
        } else {
            source.clone()
        };
        let restore = RestoreAcl(path.clone(), policy(&path, None));
        assert_eq!(std::fs::read(&path).unwrap_err().raw_os_error(), Some(5));
        let started = std::time::Instant::now();
        let report = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].session.id, good);
        assert_eq!(
            report.unavailable,
            vec![SessionSearchIssue {
                session_id: denied,
                kind: SessionSearchIssueKind::Unreadable,
            }]
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        drop(restore);
        assert_eq!(std::fs::read(source).unwrap(), original);
        let retry = EventStore::new(&db)
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(retry.hits.len(), 2);
        assert!(retry.unavailable.is_empty());
    }
    for suffix in ["control.lock", "dispatch.lock"] {
        let db = Database::open_in_memory().unwrap();
        let good = seed(&db, "A");
        let denied = seed(&db, "A");
        let store = EventStore::new(&db);
        drop(store.try_begin_recovery(denied).unwrap().unwrap());
        let path = db
            .data_root()
            .join("files/locks/sessions")
            .join(format!("{denied}.{suffix}"));
        let restore = RestoreAcl(path.clone(), policy(&path, None));
        assert!(
            store.try_begin_recovery(denied).is_err(),
            "ownership-gate failure must not be reclassified as a busy or corrupt transcript"
        );
        assert!(store
            .try_begin_recovery(good)
            .unwrap()
            .unwrap()
            .authority_is_available()
            .unwrap());
        assert!(store
            .search_file_issues(SessionSearchScope::All)
            .unwrap()
            .is_empty());
        drop(restore);
        assert!(store
            .try_begin_recovery(denied)
            .unwrap()
            .unwrap()
            .authority_is_available()
            .unwrap());
        assert!(store
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap()
            .unavailable
            .is_empty());
    }
}

#[test]
fn h229_recycled_history_is_not_reported_as_an_unexpected_missing_file_and_restore_revalidates_it()
{
    for corrupt in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let good = seed(&db, "A");
        let recycled = seed(&db, "A");
        let store = EventStore::new(&db);
        if corrupt {
            let path = file(&db, recycled);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.push(0xff);
            std::fs::write(path, bytes).unwrap();
            assert_eq!(
                store
                    .search_with_status("repairneedle", SessionSearchScope::All, 10)
                    .unwrap()
                    .unavailable
                    .len(),
                1
            );
        }
        assert!(store
            .trash_session(recycled, Timestamp::from_millis(200))
            .unwrap());
        let report = store
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].session.id, good);
        assert!(
            report.unavailable.is_empty(),
            "deliberately recycled histories are outside the active search domain"
        );
        assert!(store.restore_trashed_session(recycled).unwrap());
        let restored = store
            .search_with_status("repairneedle", SessionSearchScope::All, 10)
            .unwrap();
        if corrupt {
            assert_eq!(
                restored.unavailable,
                vec![SessionSearchIssue {
                    session_id: recycled,
                    kind: SessionSearchIssueKind::Corrupt
                }]
            );
            assert_eq!(restored.hits.len(), 1);
            assert!(store.load_session(recycled).is_err());
        } else {
            assert!(restored.unavailable.is_empty());
            assert_eq!(restored.hits.len(), 2);
        }
    }
}
