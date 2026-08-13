//! # deepagent-persistence
//!
//! The persistence layer for DeepAgent Studio (开发计划.md Phase 1 §4–§5).
//!
//! Responsibilities:
//! - Open/configure a SQLite database (WAL mode, foreign keys, busy timeout).
//! - Run idempotent, versioned [`migrations`].
//! - Provide the append-only [`event_store::EventStore`] — the source of truth
//!   for session replay & crash recovery.
//!
//! Concurrency model: a single [`Connection`] guarded by a `Mutex`. SQLite in
//! WAL mode permits concurrent readers, but the runtime's event-append path is
//! naturally serialized per-session, so a single guarded connection keeps the
//! invariants (gapless sequence numbers) simple and correct. A connection pool
//! can be layered in later without changing the repository API.

pub mod artifact_store;
pub mod checkpoint_store;
pub mod cost_store;
pub mod document_store;
pub mod event_store;
pub mod migrations;
pub mod run_store;
pub mod runtime_log_store;
pub mod subagent_store;

use std::path::Path;
use std::sync::Mutex;

use deepagent_core::error::{CoreError, Result};
use rusqlite::Connection;

/// Convert a [`rusqlite::Error`] into a [`CoreError::Persistence`].
pub(crate) fn map_sqlite(e: rusqlite::Error) -> CoreError {
    CoreError::Persistence(e.to_string())
}

/// A handle to the SQLite database.
///
/// Cheap to pass by reference; not `Clone` (wrap in `Arc` if shared ownership
/// is needed).
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (creating if necessary) a database at `path`, applying pragmas and
    /// running all pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(map_sqlite)?;
        let db = Self::from_connection(conn)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        subagent_store::SubagentRunStore::new(&db).cancel_orphaned(now)?;
        Ok(db)
    }

    /// Open an in-memory database (used in tests). Each call is isolated.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        configure_pragmas(&conn)?;
        migrations::run(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Run a closure with exclusive access to the underlying connection.
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Persistence("connection mutex poisoned".into()))?;
        f(&guard)
    }

    /// The current schema version (number of applied migrations).
    pub fn schema_version(&self) -> Result<i64> {
        self.with_conn(migrations::current_version)
    }

    /// Atomically consume a one-shot migration notice. The desktop uses this
    /// to bridge a main-database migration into the separate diagnostic log.
    pub fn take_migration_notice(&self, key: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn
                .execute("DELETE FROM migration_notices WHERE key = ?1", [key])
                .map_err(map_sqlite)?;
            Ok(changed > 0)
        })
    }
}

fn configure_pragmas(conn: &Connection) -> Result<()> {
    // WAL for concurrent reads; NORMAL sync is the standard WAL pairing.
    // foreign_keys ON to enforce referential integrity between sessions/events.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(map_sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_memory_and_migrates() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), migrations::LATEST_VERSION);
    }

    #[test]
    fn opens_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), migrations::LATEST_VERSION);
        // Reopening should not re-run migrations destructively.
        drop(db);
        let db2 = Database::open(&path).unwrap();
        assert_eq!(db2.schema_version().unwrap(), migrations::LATEST_VERSION);
    }

    #[test]
    fn reopening_disk_database_cancels_orphaned_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.db");
        {
            let db = Database::open(&path).unwrap();
            db.with_conn(|connection| {
                connection
                    .execute(
                        "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                        [],
                    )
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .unwrap();
            run_store::RunStore::new(&db)
                .create("parent", "s1", None, 1)
                .unwrap();
            subagent_store::SubagentRunStore::new(&db)
                .create(&subagent_store::SubagentRunRecord {
                    id: "child".into(),
                    parent_run_id: "parent".into(),
                    origin_parent_run_id: "parent".into(),
                    state: "running".into(),
                    agent_type: "general".into(),
                    transcript_path: None,
                    worktree_path: None,
                    summary: None,
                    created_at: 2,
                    updated_at: 2,
                    finished_at: None,
                    resume_count: 0,
                })
                .unwrap();
        }

        let reopened = Database::open(&path).unwrap();
        let child = subagent_store::SubagentRunStore::new(&reopened)
            .get("child")
            .unwrap()
            .unwrap();
        assert_eq!(child.state, "cancelled");
        assert!(child.finished_at.is_some());
        assert!(child.summary.unwrap().contains("restarted"));
    }
}
