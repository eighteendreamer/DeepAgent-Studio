//! Durable metadata for run-scoped workspace checkpoints.

use std::collections::HashMap;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::{EventId, SessionId};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{event_store::EventStore, map_sqlite, run_store::RunStore, session_files, Database};

/// Stable identity of the source event, not its reusable numeric position.
/// Stored inside the existing manifest; no SQLite schema migration is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointAnchor {
    pub session_id: SessionId,
    pub event_id: EventId,
}

#[derive(Deserialize)]
struct ManifestAnchor {
    #[serde(default)]
    branch_anchor: Option<CheckpointAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub id: String,
    pub run_id: String,
    pub session_sequence: i64,
    pub workspace_root: String,
    pub manifest: String,
    pub created_at: i64,
}

pub struct CheckpointStore<'db> {
    db: &'db Database,
}

impl<'db> CheckpointStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Resolve once when a manager is created, before its first file mutation.
    /// Detached managers without a run may be used for local evidence, but
    /// cannot persist a checkpoint (the existing run FK still applies).
    pub fn capture_anchor(&self, run_id: &str, sequence: i64) -> Result<Option<CheckpointAnchor>> {
        let Some(run) = RunStore::new(self.db).get(run_id)? else {
            return Ok(None);
        };
        let session_id = run.session_id.parse::<SessionId>()?;
        let event = EventStore::new(self.db)
            .load_session(session_id)?
            .into_iter()
            .find(|event| i64::try_from(event.sequence).ok() == Some(sequence))
            .ok_or_else(|| {
                CoreError::Persistence(format!(
                    "checkpoint source event missing: run {run_id}, sequence {sequence}"
                ))
            })?;
        Ok(Some(CheckpointAnchor {
            session_id,
            event_id: event.id,
        }))
    }

    pub fn put(&self, record: &CheckpointRecord) -> Result<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO checkpoints (id, run_id, session_sequence, workspace_root, manifest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.id,
                    record.run_id,
                    record.session_sequence,
                    record.workspace_root,
                    record.manifest,
                    record.created_at
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<CheckpointRecord>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT id, run_id, session_sequence, workspace_root, manifest, created_at FROM checkpoints WHERE id=?1",
                [id],
                |row| {
                    Ok(CheckpointRecord {
                        id: row.get(0)?,
                        run_id: row.get(1)?,
                        session_sequence: row.get(2)?,
                        workspace_root: row.get(3)?,
                        manifest: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
    }

    /// Checkpoints recorded by one run, newest first. Used by startup crash
    /// recovery to surface "files can be rolled back" metadata for runs that
    /// died before finishing (incremental commits keep this populated even
    /// when the process crashed mid-run).
    pub fn list_for_run(&self, run_id: &str) -> Result<Vec<CheckpointRecord>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, run_id, session_sequence, workspace_root, manifest, created_at
                     FROM checkpoints WHERE run_id = ?1
                     ORDER BY created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([run_id], |row| {
                    Ok(CheckpointRecord {
                        id: row.get(0)?,
                        run_id: row.get(1)?,
                        session_sequence: row.get(2)?,
                        workspace_root: row.get(3)?,
                        manifest: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    /// Select only checkpoints whose source identity is still in the active
    /// history. Legacy metadata can be bound only against generation 1; this
    /// deliberately refuses ambiguous automatic recovery after a rewrite.
    /// The gate covers qualification/backfill, not the later workspace restore
    /// or an entire running turn (active-turn exclusion is a separate boundary).
    pub fn list_for_session_after_sequence(
        &self,
        session_id: &str,
        after_sequence: i64,
    ) -> Result<Vec<CheckpointRecord>> {
        let session_id = session_id.parse::<SessionId>()?;
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        let current = session_files::load(&self.db.session_files_root(), session_id)?
            .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;
        let active: HashMap<_, _> = current
            .events
            .iter()
            .map(|event| (event.sequence as i64, event.id))
            .collect();
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.run_id, c.session_sequence, c.workspace_root, c.manifest, c.created_at
                     FROM checkpoints c
                     INNER JOIN runs r ON r.id = c.run_id
                     WHERE r.session_id = ?1
                     ORDER BY c.session_sequence DESC, c.created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([session_id.to_string()], |row| {
                    Ok(CheckpointRecord {
                        id: row.get(0)?,
                        run_id: row.get(1)?,
                        session_sequence: row.get(2)?,
                        workspace_root: row.get(3)?,
                        manifest: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(map_sqlite)?;
            let records = rows.collect::<std::result::Result<Vec<_>, _>>().map_err(map_sqlite)?;
            drop(stmt);
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let mut selected = Vec::new();
            for mut record in records {
                let Some(anchor) = restore_anchor(
                    &tx, &mut record, session_id, current.generation, &active, after_sequence,
                )? else {
                    continue;
                };
                if record.session_sequence > after_sequence
                    && anchor.session_id == session_id
                    && active.get(&record.session_sequence) == Some(&anchor.event_id)
                {
                    selected.push(record);
                }
            }
            tx.commit().map_err(map_sqlite)?;
            Ok(selected)
        })
    }
}

fn restore_anchor(
    tx: &rusqlite::Transaction<'_>,
    record: &mut CheckpointRecord,
    session_id: SessionId,
    generation: u64,
    active: &HashMap<i64, EventId>,
    after_sequence: i64,
) -> Result<Option<CheckpointAnchor>> {
    let source = serde_json::from_str::<ManifestAnchor>(&record.manifest).map_err(|error| {
        CoreError::Persistence(format!("checkpoint {} anchor: {error}", record.id))
    })?;
    if let Some(anchor) = source.branch_anchor {
        return Ok(Some(anchor));
    }
    if generation != 1 {
        if record.session_sequence <= after_sequence {
            return Ok(None);
        }
        // An unbound capture might still be needed. Silently skipping it could
        // truncate history while retaining the wrong files, so fail preflight.
        return Err(CoreError::Persistence(format!(
            "legacy checkpoint {} has no branch identity after session rewrite; automatic restore refused",
            record.id
        )));
    }
    let Some(event_id) = active.get(&record.session_sequence) else {
        if record.session_sequence <= after_sequence {
            return Ok(None);
        }
        return Err(CoreError::Persistence(format!(
            "legacy checkpoint {} has no branch identity: source event missing; automatic restore refused",
            record.id
        )));
    };
    // Bind ALL surviving legacy captures against the never-rewritten original
    // log, including the prefix, so a later deeper rewind remains possible.
    let anchor = CheckpointAnchor {
        session_id,
        event_id: *event_id,
    };
    let mut manifest: serde_json::Value = serde_json::from_str(&record.manifest)
        .map_err(|error| CoreError::Persistence(error.to_string()))?;
    manifest["branch_anchor"] =
        serde_json::to_value(&anchor).map_err(|error| CoreError::Persistence(error.to_string()))?;
    record.manifest = serde_json::to_string(&manifest)
        .map_err(|error| CoreError::Persistence(error.to_string()))?;
    tx.execute(
        "UPDATE checkpoints SET manifest=?2 WHERE id=?1",
        params![record.id, record.manifest],
    )
    .map_err(map_sqlite)?;
    Ok(Some(anchor))
}
