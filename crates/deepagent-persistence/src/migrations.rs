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
    // V4: session run mode (复刻规范 §5 "运行模式是一等公民"). Stored on the
    // session row so the sidebar can show it without loading the event stream.
    // Existing rows default to "normal".
    r#"
    ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'normal';
    "#,
    // V5: project association. A session belongs to a project (a folder, keyed
    // by its absolute root path) so the sidebar can group sessions by project
    // and the agent's file operations default to that folder. Nullable so
    // legacy/unscoped sessions remain valid.
    r#"
    ALTER TABLE sessions ADD COLUMN project TEXT;
    CREATE INDEX idx_sessions_project ON sessions(project);
    "#,
    // V6: cost tracking. Records per-request token cost so the UI can show
    // cumulative spend and enforce budget limits.
    r#"
    CREATE TABLE costs (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        timestamp   INTEGER NOT NULL,
        model       TEXT NOT NULL,
        input_tokens    INTEGER NOT NULL DEFAULT 0,
        output_tokens   INTEGER NOT NULL DEFAULT 0,
        cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
        total_tokens    INTEGER NOT NULL DEFAULT 0,
        cost_yuan       REAL NOT NULL DEFAULT 0.0
    );

    CREATE INDEX idx_costs_session ON costs(session_id);
    CREATE INDEX idx_costs_timestamp ON costs(timestamp);
    "#,
    // V7: reset the old USD ledger and switch `cost_yuan` to RMB semantics.
    // Cache-miss tokens are now stored from provider usage directly.
    r#"
    DELETE FROM costs;
    ALTER TABLE costs ADD COLUMN cache_miss_tokens INTEGER NOT NULL DEFAULT 0;
    "#,
    // V8: Agent Kernel v2 run ledger. These tables are append-only diagnostics
    // and recovery metadata; the existing session/event model remains intact.
    r#"
    CREATE TABLE runs (
        id              TEXT PRIMARY KEY NOT NULL,
        session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        task_id         TEXT,
        state           TEXT NOT NULL,
        terminal_kind   TEXT,
        terminal_reason TEXT,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL,
        finished_at     INTEGER
    );

    CREATE INDEX idx_runs_session ON runs(session_id, created_at);
    CREATE INDEX idx_runs_state ON runs(state);

    CREATE TABLE run_events (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        sequence    INTEGER NOT NULL,
        timestamp   INTEGER NOT NULL,
        phase       TEXT NOT NULL,
        status      TEXT NOT NULL,
        event_type  TEXT NOT NULL,
        data        TEXT NOT NULL,
        UNIQUE(run_id, sequence)
    );

    CREATE INDEX idx_run_events_run_seq ON run_events(run_id, sequence);

    CREATE TABLE checkpoints (
        id              TEXT PRIMARY KEY NOT NULL,
        run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        session_sequence INTEGER NOT NULL,
        workspace_root  TEXT NOT NULL,
        manifest        TEXT NOT NULL,
        created_at      INTEGER NOT NULL
    );

    CREATE TABLE tool_artifacts (
        id          TEXT PRIMARY KEY NOT NULL,
        run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        call_id     TEXT NOT NULL,
        path        TEXT NOT NULL,
        media_type  TEXT,
        byte_size   INTEGER NOT NULL,
        digest      TEXT,
        created_at  INTEGER NOT NULL
    );

    CREATE INDEX idx_tool_artifacts_run ON tool_artifacts(run_id, call_id);

    CREATE TABLE subagent_runs (
        id              TEXT PRIMARY KEY NOT NULL,
        parent_run_id   TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        state           TEXT NOT NULL,
        agent_type      TEXT NOT NULL,
        transcript_path TEXT,
        worktree_path   TEXT,
        summary         TEXT,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL,
        finished_at     INTEGER
    );

    CREATE INDEX idx_subagent_runs_parent ON subagent_runs(parent_run_id, created_at);
    "#,
    // V9: preserve child lineage when the same sub-agent is resumed from a
    // later parent run in the same conversation.
    r#"
    ALTER TABLE subagent_runs ADD COLUMN origin_parent_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE;
    ALTER TABLE subagent_runs ADD COLUMN resume_count INTEGER NOT NULL DEFAULT 0;
    UPDATE subagent_runs SET origin_parent_run_id=parent_run_id WHERE origin_parent_run_id IS NULL;
    CREATE INDEX idx_subagent_runs_origin_parent ON subagent_runs(origin_parent_run_id, created_at);
    "#,
    // V10: speed up the sidebar's newest-first session list. Without this,
    // SQLite scans `sessions` and builds a temporary B-tree for
    // `ORDER BY updated_at DESC` once the session count grows.
    r#"
    CREATE INDEX idx_sessions_updated_at_desc ON sessions(updated_at DESC);
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
        for table in [
            "sessions",
            "events",
            "tasks",
            "documents",
            "runs",
            "run_events",
            "checkpoints",
            "tool_artifacts",
            "subagent_runs",
        ] {
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
