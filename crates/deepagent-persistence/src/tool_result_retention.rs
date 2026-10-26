//! Reachability-based retirement over the existing artifact/session stores.
//! No second blob index: detached rows survive their originating run's cascade
//! until the last retained transcript (active or recycled) releases them.

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::Path;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use sha2::{Digest, Sha256};

use crate::artifact_store::{ArtifactRecord, ArtifactStore};
use crate::{map_sqlite, session_files, Database};

pub(crate) fn collect(db: &Database, cutoff: i64) -> Result<u64> {
    let _retention = db.lock_tool_result_retention()?;
    let ids = db.with_conn(|conn| {
        let mut statement = conn
            .prepare(
                "SELECT a.id FROM artifacts a WHERE a.kind='tool_result' AND a.run_id IS NULL
             AND a.created_at<=?1 AND NOT EXISTS
                (SELECT 1 FROM artifacts other WHERE other.path=a.path AND other.id<>a.id)
             ORDER BY a.id",
            )
            .map_err(map_sqlite)?;
        let rows = statement
            .query_map([cutoff], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    })?;
    if ids.is_empty() {
        return Ok(0);
    }
    let referenced = retained_ids(db)?;
    let store = ArtifactStore::new(db);
    let mut removed = 0;
    for id in ids {
        if referenced.contains(&id) || !is_published_identity(&id) {
            continue;
        }
        let Some(record) = store.get(&id)? else {
            continue;
        };
        if remove_verified(&record)? {
            store.delete(&id)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn retained_ids(db: &Database) -> Result<HashSet<String>> {
    // Include invisible/recycled sessions, not EventStore::list_sessions(). A
    // fork cannot publish between this inventory and deletion: both own the
    // retention gate. Trash moves/restores own the same per-session I/O lock.
    let sessions = db.with_conn(|conn| {
        let mut statement = conn
            .prepare("SELECT id FROM sessions")
            .map_err(map_sqlite)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    })?;
    let root = db.session_files_root();
    let mut ids = HashSet::new();
    for session in sessions {
        let session: SessionId = session.parse()?;
        let _io = db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&root, session)?;
        for event in session_files::load_retained_events(&root, session)? {
            // The opaque ASCII stem also survives nested JSON strings in
            // native Responses output, legacy Tool messages and summaries.
            // Mentions conservatively retain; they never prove ownership.
            let payload = serde_json::to_string(&event.payload)?;
            ids.extend(
                payload
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .filter(|word| is_published_identity(word))
                    .map(str::to_owned),
            );
        }
    }
    Ok(ids)
}

fn is_published_identity(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("tool_result_") else {
        return false;
    };
    let parts: Vec<_> = rest.split('_').collect();
    parts.len() == 3
        && parts.iter().zip([16, 16, 32]).all(|(part, len)| {
            part.len() == len && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn remove_verified(record: &ArtifactRecord) -> Result<bool> {
    let path = Path::new(&record.path);
    if !path.is_absolute()
        || path.file_stem().and_then(|s| s.to_str()) != Some(&record.id)
        || path.extension().and_then(|s| s.to_str()) != Some("json")
        || record.digest.is_none()
        || record.byte_size < 0
    {
        tracing::warn!(id=%record.id, "tool result GC retained invalid publication metadata");
        return Ok(false);
    }
    let _operation = crate::fs_commit::lock_path_operation(path).map_err(persistence_error)?;
    // Refuse observed link replacements; this preflight is not a native
    // handle-relative deletion boundary against arbitrary external writers.
    // A digest mismatch is diagnostic and retained for investigation.
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => {
                tracing::warn!(id=%record.id, "tool result GC refused a replaced link");
                return Ok(false);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && ancestor == path => {}
            Err(error) => return Err(persistence_error(error)),
        }
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(persistence_error(error)),
    };
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let len = file.read(&mut buffer).map_err(persistence_error)?;
        if len == 0 {
            break;
        }
        digest.update(&buffer[..len]);
        bytes = bytes.saturating_add(len as u64);
        if bytes > record.byte_size as u64 {
            tracing::warn!(id=%record.id, "tool result GC retained an enlarged file");
            return Ok(false);
        }
    }
    if bytes != record.byte_size as u64
        || record.digest.as_deref() != Some(format!("sha256:{:x}", digest.finalize()).as_str())
    {
        tracing::warn!(id=%record.id, "tool result GC retained changed bytes");
        return Ok(false);
    }
    drop(file);
    std::fs::remove_file(path).map_err(persistence_error)?;
    Ok(true)
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn persistence_error(error: io::Error) -> CoreError {
    CoreError::Persistence(format!("tool result retention: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_store::ArtifactKind;
    use crate::event_store::EventStore;
    use crate::run_store::RunStore;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::event::EventPayload;
    use deepagent_core::response_item::ResponseItem;

    fn result(db: &Database, ordinal: u32) -> ArtifactRecord {
        let id = format!(
            "tool_result_{}_{}_{ordinal:032x}",
            "a".repeat(16),
            "b".repeat(16)
        );
        let path = db.data_root().join(format!("{id}.json"));
        let bytes = format!("{{\"result\":{ordinal}}}");
        std::fs::write(&path, &bytes).unwrap();
        let record = ArtifactRecord {
            id,
            kind: ArtifactKind::ToolResult,
            run_id: None,
            call_id: Some("call".into()),
            workspace_id: None,
            path: path.to_str().unwrap().into(),
            media_type: Some("application/json".into()),
            byte_size: bytes.len() as i64,
            digest: Some(format!("sha256:{:x}", Sha256::digest(bytes.as_bytes()))),
            created_at: 10,
        };
        ArtifactStore::new(db).put_new(&record).unwrap();
        record
    }

    #[test]
    fn retention_is_bounded_and_changed_or_foreign_bytes_are_not_deleted() {
        let db = Database::open_in_memory().unwrap();
        let expired = result(&db, 1);
        let changed = result(&db, 2);
        std::fs::write(&changed.path, "not the published output").unwrap();
        let mut foreign = result(&db, 3);
        ArtifactStore::new(&db).delete(&foreign.id).unwrap();
        foreign.id = "legacy_or_foreign".into();
        ArtifactStore::new(&db).put_new(&foreign).unwrap();
        assert_eq!(collect(&db, 9).unwrap(), 0);
        assert!(Path::new(&expired.path).is_file());
        assert_eq!(collect(&db, 10).unwrap(), 1);
        assert!(ArtifactStore::new(&db).get(&expired.id).unwrap().is_none());
        assert!(!Path::new(&expired.path).exists());
        assert_eq!(
            std::fs::read_to_string(&changed.path).unwrap(),
            "not the published output"
        );
        assert!(ArtifactStore::new(&db).get(&changed.id).unwrap().is_some());
        assert!(Path::new(&foreign.path).is_file());
        assert_eq!(collect(&db, 10).unwrap(), 0);
    }

    #[test]
    fn native_only_and_recycled_references_survive_until_the_last_history_expires() {
        let db = Database::open_in_memory().unwrap();
        let record = result(&db, 1);
        let session = SessionId::new();
        let store = EventStore::new(&db);
        store
            .create_session(session, Some("native only"), Timestamp::from_millis(1))
            .unwrap();
        store
            .append(
                session,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCallOutput {
                        call_id: "call".into(),
                        output:
                            serde_json::json!({"status":"ok","result":{"saved_path":record.path}})
                                .to_string(),
                    },
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        assert_eq!(collect(&db, 100).unwrap(), 0);
        store
            .trash_session(session, Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(
            collect(&db, 100).unwrap(),
            0,
            "recycled history remains restorable"
        );
        assert!(!store.purge_deleted_session_metadata(session).unwrap());
        store
            .purge_trashed_sessions_before(Timestamp::from_millis(20))
            .unwrap();
        assert!(store.purge_deleted_session_metadata(session).unwrap());
        assert_eq!(collect(&db, 100).unwrap(), 1);
    }

    #[test]
    fn owner_is_detached_before_cascade_and_live_fork_keeps_the_original_result() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let source = SessionId::new();
        store
            .create_session(source, None, Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create("owner", &source.to_string(), None, 1)
            .unwrap();
        let mut record = result(&db, 1);
        record.run_id = Some("owner".into());
        ArtifactStore::new(&db).put(&record).unwrap();
        store
            .append(
                source,
                EventPayload::ToolCallCompleted {
                    call_id: "call".into(),
                    ok: true,
                    output: serde_json::json!({"saved_path":record.path}),
                    duration_ms: 1,
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        let child = SessionId::new();
        store
            .fork_session(source, child, 0, Timestamp::from_millis(3))
            .unwrap();
        assert_eq!(collect(&db, 100).unwrap(), 0);
        store
            .trash_session(source, Timestamp::from_millis(20))
            .unwrap();
        store
            .purge_trashed_sessions_before(Timestamp::from_millis(20))
            .unwrap();
        store.purge_deleted_session_metadata(source).unwrap();
        assert!(RunStore::new(&db).get("owner").unwrap().is_none());
        assert!(ArtifactStore::new(&db)
            .get(&record.id)
            .unwrap()
            .unwrap()
            .run_id
            .is_none());
        assert_eq!(collect(&db, 100).unwrap(), 0);
        assert!(Path::new(&record.path).is_file());
        store
            .trash_session(child, Timestamp::from_millis(30))
            .unwrap();
        store
            .purge_trashed_sessions_before(Timestamp::from_millis(30))
            .unwrap();
        store.purge_deleted_session_metadata(child).unwrap();
        assert_eq!(collect(&db, 100).unwrap(), 1);
    }

    #[test]
    fn corrupt_retained_history_fails_closed_without_changing_files_or_index() {
        let db = Database::open_in_memory().unwrap();
        let record = result(&db, 1);
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        let log = std::fs::read_dir(db.session_transcript_dir(session))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|p| p.extension().is_some_and(|s| s == "zst"))
            .unwrap();
        std::fs::write(&log, "corrupt source").unwrap();
        assert!(collect(&db, 100).is_err());
        assert!(Path::new(&record.path).is_file());
        assert!(ArtifactStore::new(&db).get(&record.id).unwrap().is_some());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "corrupt source");
    }

    #[test]
    fn retention_gate_is_shared_by_distinct_database_handles_and_fork_publication() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("retention.sqlite");
        let first = Database::open(&db_path).unwrap();
        let second = Database::open(&db_path).unwrap();
        let source = SessionId::new();
        EventStore::new(&first)
            .create_session(source, None, Timestamp::from_millis(1))
            .unwrap();
        EventStore::new(&first)
            .append(
                source,
                EventPayload::Note {
                    text: "prefix".into(),
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        let gate = first.lock_tool_result_retention().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let child = SessionId::new();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result =
                EventStore::new(&second).fork_session(source, child, 0, Timestamp::from_millis(3));
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(EventStore::new(&first)
            .get_session(child)
            .unwrap()
            .is_none());
        drop(gate);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(
            EventStore::new(&first).load_session(child).unwrap().len(),
            1
        );
    }

    #[test]
    fn missing_retained_source_refuses_gc_without_guessing_its_references() {
        let db = Database::open_in_memory().unwrap();
        let record = result(&db, 1);
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        std::fs::rename(
            db.session_transcript_dir(session),
            db.data_root().join("unavailable-source"),
        )
        .unwrap();
        assert!(collect(&db, 100).is_err());
        assert!(Path::new(&record.path).is_file());
        assert!(ArtifactStore::new(&db).get(&record.id).unwrap().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn observed_junction_replacement_is_not_followed_even_when_digest_matches() {
        use std::os::windows::process::CommandExt;
        let db = Database::open_in_memory().unwrap();
        let mut record = result(&db, 1);
        let physical = db.data_root().join("physical");
        let alias = db.data_root().join("replaced");
        std::fs::create_dir(&physical).unwrap();
        let target = physical.join(format!("{}.json", record.id));
        std::fs::rename(&record.path, &target).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&physical)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction fixture: {output:?}");
        record.path = alias
            .join(format!("{}.json", record.id))
            .to_str()
            .unwrap()
            .into();
        ArtifactStore::new(&db).put(&record).unwrap();
        assert_eq!(collect(&db, 100).unwrap(), 0);
        assert!(target.is_file());
        assert!(ArtifactStore::new(&db).get(&record.id).unwrap().is_some());
    }
}
