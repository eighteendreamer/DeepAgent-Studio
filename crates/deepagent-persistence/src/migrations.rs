//! Versioned, idempotent schema migrations.
//!
//! Migrations are an ordered list of SQL scripts. The applied version is tracked
//! via SQLite's `user_version` pragma, which is cheap and avoids a bespoke
//! bookkeeping table. `run` applies every migration whose index is greater than
//! the stored version, inside a transaction.

use deepagent_core::error::Result;
use rusqlite::Connection;

use crate::map_sqlite;

/// Ordered migration scripts. Index `i` (0-based) corresponds to schema
/// version `i + 1`. **Never edit or reorder an existing entry** — only append.
const MIGRATIONS: &[&str] = &[
    // V1: sessions + append-only events.
    r#"
    CREATE TABLE sessions (
        id          TEXT PRIMARY KEY NOT NULL,
        title       TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL,
        ended_at    INTEGER
    );

    CREATE TABLE events (
        id          TEXT PRIMARY KEY NOT NULL,
        session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        sequence    INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        timestamp   INTEGER NOT NULL,
        payload     TEXT NOT NULL,            -- tagged JSON of EventPayload
        UNIQUE (session_id, sequence)
    );

    CREATE INDEX idx_events_session_seq ON events(session_id, sequence);
    CREATE INDEX idx_events_kind        ON events(kind);
    "#,
    // V2: tasks projection (rebuildable from the event stream, kept for queries).
    r#"
    CREATE TABLE tasks (
        id          TEXT PRIMARY KEY NOT NULL,
        session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        goal        TEXT NOT NULL,
        state       TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );

    CREATE INDEX idx_tasks_session ON tasks(session_id);
    CREATE INDEX idx_tasks_state   ON tasks(state);
    "#,
    // V3: a generic document store with optional embeddings. Used by
    // deepagent-memory for cross-session persistence + semantic retrieval, but
    // kept domain-agnostic here so persistence has no upward dependencies.
    r#"
    CREATE TABLE documents (
        id          TEXT NOT NULL,
        collection  TEXT NOT NULL,            -- namespace, e.g. "memory"
        body        TEXT NOT NULL,            -- JSON payload (opaque to this layer)
        embedding   BLOB,                     -- optional little-endian f32 vector
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL,
        PRIMARY KEY (collection, id)
    );

    CREATE INDEX idx_documents_collection ON documents(collection);
    "#,
];

/// The highest schema version defined by this build.
pub const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

/// Read the current schema version from `PRAGMA user_version`.
pub fn current_version(conn: &Connection) -> Result<i64> {
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(map_sqlite)?;
    Ok(v)
}

/// Apply all pending migrations. Idempotent: re-running on an up-to-date
/// database is a no-op.
pub fn run(conn: &Connection) -> Result<()> {
    let mut version = current_version(conn)?;
    if version >= LATEST_VERSION {
        return Ok(());
    }

    while (version as usize) < MIGRATIONS.len() {
        let script = MIGRATIONS[version as usize];
        let next = version + 1;
        tracing::info!(from = version, to = next, "applying migration");

        // Each migration + version bump runs atomically.
        conn.execute_batch("BEGIN;").map_err(map_sqlite)?;
        let result = (|| -> Result<()> {
            conn.execute_batch(script).map_err(map_sqlite)?;
            // user_version does not accept bound params; format is safe (i64).
            conn.execute_batch(&format!("PRAGMA user_version = {next};"))
                .map_err(map_sqlite)?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT;").map_err(map_sqlite)?;
                version = next;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_reaches_latest() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LATEST_VERSION);
    }

    #[test]
    fn run_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        run(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LATEST_VERSION);
    }

    #[test]
    fn expected_tables_exist() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        for table in ["sessions", "events", "tasks", "documents"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} should exist");
        }
    }
}
