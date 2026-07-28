//! Durable metadata for run-scoped workspace checkpoints.

use deepagent_core::error::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

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

    pub fn list_for_session_after_sequence(
        &self,
        session_id: &str,
        after_sequence: i64,
    ) -> Result<Vec<CheckpointRecord>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.run_id, c.session_sequence, c.workspace_root, c.manifest, c.created_at
                     FROM checkpoints c
                     INNER JOIN runs r ON r.id = c.run_id
                     WHERE r.session_id = ?1 AND c.session_sequence > ?2
                     ORDER BY c.session_sequence DESC, c.created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![session_id, after_sequence], |row| {
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
}
