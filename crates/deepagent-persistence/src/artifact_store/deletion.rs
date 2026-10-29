//! Recoverable deletion over the existing artifacts/documents tables. The
//! owner supplies authorization/retention policy; this module owns the one
//! transaction and byte-cleanup protocol, not a second artifact store.

use std::io::{self, Read};
use std::path::Path;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{row_to_record, ArtifactRecord, ArtifactStore, COLUMNS};
use crate::{fs_commit, map_sqlite};

const COLLECTION: &str = "artifact_deletions_v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactState {
    Active(ArtifactRecord),
    PendingDelete(ArtifactRecord),
}

impl ArtifactState {
    pub fn record(&self) -> &ArtifactRecord {
        match self {
            Self::Active(record) | Self::PendingDelete(record) => record,
        }
    }

    pub fn require_active(self) -> Result<ArtifactRecord> {
        match self {
            Self::Active(record) => Ok(record),
            Self::PendingDelete(record) => Err(pending_error(
                &record.id,
                CoreError::other("committed deletion has not finished cleanup"),
            )),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletionIntent {
    version: u32,
    record: ArtifactRecord,
    started_at: i64,
    attempts: u64,
}

fn decode_intent(id: &str, body: &str) -> Result<DeletionIntent> {
    let intent: DeletionIntent = serde_json::from_str(body)?;
    if intent.version != 1 || intent.record.id != id || intent.attempts == 0 {
        return Err(CoreError::Persistence(
            "invalid artifact deletion intent".into(),
        ));
    }
    ArtifactStore::validate_id(id)?;
    Ok(intent)
}

fn intent_on(conn: &Connection, id: &str) -> Result<Option<DeletionIntent>> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body FROM documents WHERE collection=?1 AND id=?2",
            params![COLLECTION, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    body.map(|body| decode_intent(id, &body)).transpose()
}

fn pending_on(conn: &Connection) -> Result<Vec<ArtifactRecord>> {
    let mut statement = conn
        .prepare("SELECT id, body FROM documents WHERE collection=?1 ORDER BY id")
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([COLLECTION], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(map_sqlite)?;
    rows.map(|row| {
        let (id, body) = row.map_err(map_sqlite)?;
        Ok(decode_intent(&id, &body)?.record)
    })
    .collect()
}

fn active_on(conn: &Connection, id: &str) -> Result<Option<ArtifactRecord>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM artifacts WHERE id=?1"),
        [id],
        row_to_record,
    )
    .optional()
    .map_err(map_sqlite)
}

fn state_on(conn: &Connection, id: &str) -> Result<Option<ArtifactState>> {
    if let Some(intent) = intent_on(conn, id)? {
        return Ok(Some(ArtifactState::PendingDelete(intent.record)));
    }
    Ok(active_on(conn, id)?.map(ArtifactState::Active))
}

pub(super) fn check_publication(conn: &Connection, record: &ArtifactRecord) -> Result<()> {
    for pending in pending_on(conn)? {
        if pending.id == record.id || same_path(&pending.path, &record.path) {
            return Err(pending_error(
                &pending.id,
                CoreError::other("cannot republish an identity or path pending deletion"),
            ));
        }
    }
    Ok(())
}

fn same_path(left: &str, right: &str) -> bool {
    if Path::new(left) == Path::new(right) {
        return true;
    }
    match (
        fs_commit::canonicalize_with_missing_suffix(Path::new(left)),
        fs_commit::canonicalize_with_missing_suffix(Path::new(right)),
    ) {
        (Ok(left), Ok(right)) => {
            #[cfg(windows)]
            {
                left.to_string_lossy()
                    .eq_ignore_ascii_case(&right.to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                left == right
            }
        }
        _ => false,
    }
}

impl ArtifactStore<'_> {
    /// Read active/pending metadata in one consistent SQLite snapshot. The
    /// owner must authorize `record()` before exposing either state.
    pub fn state(&self, id: &str) -> Result<Option<ArtifactState>> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let state = state_on(&tx, id)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(state)
        })
    }

    /// Host-internal recovery inventory, not an unscoped UI query.
    pub fn pending_deletions(&self) -> Result<Vec<ArtifactRecord>> {
        self.db.with_conn(pending_on)
    }

    /// Delete a published file only after its intent and index removal commit
    /// together. `verify` runs under the existing cross-process path gate and
    /// must enforce the caller's managed-root/ownership/retention policy.
    /// `false` retains an active artifact; pending failures remain explicit.
    pub fn delete_file(
        &self,
        expected: &ArtifactRecord,
        verify: impl FnOnce(&ArtifactRecord) -> Result<bool>,
    ) -> Result<bool> {
        let _operation =
            fs_commit::lock_path_operation(Path::new(&expected.path)).map_err(io_error)?;
        let Some(state) = self.state(&expected.id)? else {
            return Ok(false);
        };
        if state.record() != expected {
            return Err(CoreError::Persistence(
                "artifact changed before deletion".into(),
            ));
        }
        let pending = matches!(state, ArtifactState::PendingDelete(_));
        match verify(expected) {
            Ok(true) => {}
            Ok(false) if !pending => return Ok(false),
            result => {
                let error = result
                    .err()
                    .unwrap_or_else(|| CoreError::other("file identity check refused cleanup"));
                return Err(if pending {
                    pending_error(&expected.id, error)
                } else {
                    error
                });
            }
        }
        let Some(body) = self.commit_deletion(expected).map_err(|error| {
            if pending {
                pending_error(&expected.id, error)
            } else {
                error
            }
        })?
        else {
            return Ok(false);
        };
        #[cfg(test)]
        crash_checkpoint("after_commit");

        let cleanup = (|| {
            match std::fs::remove_file(&expected.path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
            #[cfg(test)]
            crash_checkpoint("after_unlink");
            self.db.with_conn(|conn| {
                let removed = conn
                    .execute(
                        "DELETE FROM documents WHERE collection=?1 AND id=?2 AND body=?3",
                        params![COLLECTION, expected.id, body],
                    )
                    .map_err(map_sqlite)?;
                if removed != 1 {
                    return Err(CoreError::Persistence(
                        "artifact deletion receipt changed".into(),
                    ));
                }
                Ok(())
            })
        })();
        cleanup.map_err(|error| {
            tracing::warn!(artifact_id=%expected.id, %error, "artifact cleanup remains pending");
            pending_error(&expected.id, error)
        })?;
        tracing::debug!(artifact_id=%expected.id, "artifact deletion completed");
        Ok(true)
    }

    fn commit_deletion(&self, expected: &ArtifactRecord) -> Result<Option<String>> {
        self.db.with_conn(|conn| {
            // The database normally uses WAL + NORMAL. That is sufficient for
            // process crashes, but the intent must reach the WAL sync boundary
            // before an irreversible file unlink. Strengthen only this commit,
            // while holding the connection mutex, and restore the caller policy.
            // https://www.sqlite.org/pragma.html#pragma_synchronous
            let original = synchronous(conn)?;
            if original < 2 {
                conn.execute_batch("PRAGMA synchronous=FULL;")
                    .map_err(map_sqlite)?;
            }
            let result = commit_deletion_on(conn, expected);
            if original < 2 {
                if let Err(restore) = conn.execute_batch(&format!("PRAGMA synchronous={original};"))
                {
                    tracing::error!(artifact_id=%expected.id, %restore,
                        "failed to restore artifact deletion connection policy");
                    return Err(match result {
                        Ok(Some(_)) => pending_error(&expected.id, map_sqlite(restore)),
                        Ok(None) => map_sqlite(restore),
                        Err(error) => CoreError::Persistence(format!(
                            "{error}; restoring synchronous policy also failed: {restore}"
                        )),
                    });
                }
            }
            result
        })
    }
}

fn synchronous(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(map_sqlite)
}

fn commit_deletion_on(conn: &Connection, expected: &ArtifactRecord) -> Result<Option<String>> {
    if synchronous(conn)? < 2 {
        return Err(CoreError::Persistence(
            "artifact deletion requires FULL synchronous commit".into(),
        ));
    }
    let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
    let Some(state) = state_on(&tx, &expected.id)? else {
        return Ok(None);
    };
    if state.record() != expected {
        return Err(CoreError::Persistence(
            "artifact changed during deletion".into(),
        ));
    }
    // Publication cannot claim a pending path. Also refuse older metadata
    // aliases before retiring the shared bytes.
    let mut statement = tx
        .prepare("SELECT path FROM artifacts WHERE id<>?1")
        .map_err(map_sqlite)?;
    let paths = statement
        .query_map([&expected.id], |row| row.get::<_, String>(0))
        .map_err(map_sqlite)?;
    for path in paths {
        if same_path(&path.map_err(map_sqlite)?, &expected.path) {
            return Err(CoreError::Persistence(
                "artifact path has another index owner".into(),
            ));
        }
    }
    drop(statement);
    let now = SystemClock.now().as_millis();
    let mut intent = intent_on(&tx, &expected.id)?.unwrap_or_else(|| DeletionIntent {
        version: 1,
        record: expected.clone(),
        started_at: now,
        attempts: 0,
    });
    intent.attempts = intent
        .attempts
        .checked_add(1)
        .ok_or_else(|| CoreError::Persistence("artifact deletion attempt overflow".into()))?;
    let body = serde_json::to_string(&intent)?;
    tx.execute(
        "INSERT INTO documents (collection,id,body,created_at,updated_at) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(collection,id) DO UPDATE SET body=excluded.body,updated_at=excluded.updated_at",
        params![COLLECTION, expected.id, body, intent.started_at, now],
    ).map_err(map_sqlite)?;
    tx.execute("DELETE FROM artifacts WHERE id=?1", [&expected.id])
        .map_err(map_sqlite)?;
    #[cfg(test)]
    crash_checkpoint("before_commit");
    tx.commit().map_err(map_sqlite)?;
    tracing::debug!(artifact_id=%expected.id, attempt=intent.attempts, "artifact deletion intent committed");
    Ok(Some(body))
}
fn pending_error(id: &str, source: CoreError) -> CoreError {
    CoreError::ArtifactPendingDelete {
        artifact_id: id.into(),
        source: Box::new(source),
    }
}

fn io_error(error: io::Error) -> CoreError {
    CoreError::Persistence(format!("artifact deletion: {error}"))
}

/// Check the immutable published identity before cleanup, without mutating
/// bytes. A missing file is recoverable; links and changed bytes are retained.
/// This advisory preflight does not exclude arbitrary external filesystem writers.
pub fn verify_file_for_deletion(record: &ArtifactRecord) -> Result<bool> {
    let path = Path::new(&record.path);
    if !deletion_metadata_is_valid(record) {
        tracing::warn!(artifact_id=%record.id, "artifact cleanup retained invalid publication metadata");
        return Ok(false);
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => {
                tracing::warn!(artifact_id=%record.id, "artifact cleanup refused a replaced link");
                return Ok(false);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && ancestor == path => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(io_error(error)),
    };
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let len = file.read(&mut buffer).map_err(io_error)?;
        if len == 0 {
            break;
        }
        digest.update(&buffer[..len]);
        bytes = bytes.saturating_add(len as u64);
        if bytes > record.byte_size as u64 {
            tracing::warn!(artifact_id=%record.id, "artifact cleanup retained an enlarged file");
            return Ok(false);
        }
    }
    let unchanged = bytes == record.byte_size as u64
        && record.digest.as_deref() == Some(format!("sha256:{:x}", digest.finalize()).as_str());
    if !unchanged {
        tracing::warn!(artifact_id=%record.id, "artifact cleanup retained changed bytes");
    }
    Ok(unchanged)
}

/// Metadata-only policy check, safe before attempting path ownership or I/O.
/// GC retains malformed historical rows instead of aborting an entire batch.
pub(crate) fn deletion_metadata_is_valid(record: &ArtifactRecord) -> bool {
    let path = Path::new(&record.path);
    path.is_absolute()
        && path.file_stem().and_then(|stem| stem.to_str()) == Some(&record.id)
        && record.digest.is_some()
        && record.byte_size >= 0
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

#[cfg(test)]
fn crash_checkpoint(stage: &str) {
    if std::env::var("DEEPAGENT_H254_CRASH_STAGE").as_deref() == Ok(stage) {
        std::process::exit(86);
    }
}

#[cfg(test)]
mod tests;
