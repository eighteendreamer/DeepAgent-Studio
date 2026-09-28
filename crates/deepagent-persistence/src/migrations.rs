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
    // V11: Chat Completions -> Responses cutover. Old conversation payloads
    // cannot be replayed as Responses items without inventing lost item
    // metadata. Remove conversation/run history while retaining session rows
    // as cost-ledger foreign-key owners. Empty legacy sessions are hidden by
    // the application projection.
    r#"
    DELETE FROM events;
    DELETE FROM tasks;
    DELETE FROM runs;
    UPDATE sessions SET ended_at = COALESCE(ended_at, updated_at), title = NULL;
    "#,
    // V12: bridge the already-applied V11 reset into the separate runtime log
    // database exactly once. Keeping this as a new migration also covers dev
    // databases that reached V11 before the diagnostic event was introduced.
    r#"
    CREATE TABLE IF NOT EXISTS migration_notices (
        key TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('subsec') * 1000)
    );
    INSERT OR IGNORE INTO migration_notices(key) VALUES ('responses_history_reset_completed');
    "#,
    // V13: encrypted secret records. Ciphertext and the separately wrapped
    // master-key record live in SQLite; plaintext is never persisted.
    r#"
    CREATE TABLE secret_records (
        name        TEXT PRIMARY KEY NOT NULL,
        ciphertext  BLOB NOT NULL,
        nonce       BLOB NOT NULL,
        key_version INTEGER NOT NULL DEFAULT 1,
        updated_at  INTEGER NOT NULL
    );
    "#,
    // V14: metadata inventory for large files kept in managed directories.
    r#"
    CREATE TABLE managed_files (
        category     TEXT NOT NULL,
        relative_path TEXT NOT NULL,
        root_path    TEXT NOT NULL,
        byte_size    INTEGER NOT NULL DEFAULT 0,
        modified_at  INTEGER,
        digest       TEXT,
        status       TEXT NOT NULL DEFAULT 'present',
        updated_at   INTEGER NOT NULL,
        PRIMARY KEY (category, relative_path)
    );
    CREATE INDEX idx_managed_files_category_status
        ON managed_files(category, status, updated_at);
    "#,
    // V15: durable run control projections. These tables do not replace
    // runs/run_events; they make action, approval and execution-lease state
    // queryable and recoverable while run_events remains the ordered ledger.
    r#"
    CREATE TABLE run_actions (
        run_id            TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
        turn_id           TEXT NOT NULL,
        call_id           TEXT NOT NULL,
        sequence          INTEGER NOT NULL,
        tool_name         TEXT NOT NULL,
        arguments_hash    TEXT NOT NULL,
        state             TEXT NOT NULL,
        risk              TEXT NOT NULL,
        approval_id       TEXT,
        attempt           INTEGER NOT NULL DEFAULT 0,
        lease_owner       TEXT,
        lease_expires_at  INTEGER,
        started_at        INTEGER,
        finished_at       INTEGER,
        result_ref        TEXT,
        blocked_reason    TEXT,
        parent_action_id  TEXT,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,
        PRIMARY KEY (run_id, call_id),
        UNIQUE (run_id, sequence),
        CHECK (state IN ('received','prepared','queued','blocked','running','completed','failed','cancelled','expired','denied'))
    );
    CREATE INDEX idx_run_actions_state
        ON run_actions(run_id, state, sequence);

    CREATE TABLE run_approvals (
        approval_id       TEXT PRIMARY KEY NOT NULL,
        run_id            TEXT NOT NULL,
        call_id           TEXT NOT NULL,
        state             TEXT NOT NULL DEFAULT 'pending',
        scope             TEXT NOT NULL,
        risk              TEXT NOT NULL,
        reason            TEXT,
        policy_snapshot   TEXT,
        expires_at        INTEGER,
        decided_at        INTEGER,
        decided_by        TEXT,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,
        UNIQUE (run_id, call_id, approval_id),
        FOREIGN KEY (run_id, call_id) REFERENCES run_actions(run_id, call_id) ON DELETE CASCADE,
        CHECK (state IN ('pending','approved','denied','expired','cancelled'))
    );
    CREATE INDEX idx_run_approvals_pending
        ON run_approvals(run_id, state, expires_at);

    CREATE TABLE execution_leases (
        lease_id          TEXT PRIMARY KEY NOT NULL,
        resource_kind     TEXT NOT NULL,
        resource_id       TEXT NOT NULL,
        owner             TEXT NOT NULL,
        epoch             INTEGER NOT NULL,
        fencing_token_hash TEXT NOT NULL,
        acquired_at       INTEGER NOT NULL,
        expires_at        INTEGER NOT NULL,
        renewed_at        INTEGER,
        revoked_at        INTEGER,
        revoke_reason     TEXT,
        UNIQUE (resource_kind, resource_id, epoch)
    );
    CREATE UNIQUE INDEX idx_execution_leases_active_resource
        ON execution_leases(resource_kind, resource_id)
        WHERE revoked_at IS NULL;
    CREATE INDEX idx_execution_leases_expiry
        ON execution_leases(expires_at, revoked_at);
    "#,
    // V16: durable terminal read cursors. PTY bytes remain backend-owned, but
    // reconnecting clients can resume from the last acknowledged offset.
    r#"
    CREATE TABLE terminal_session_cursors (
        session_id TEXT PRIMARY KEY NOT NULL,
        cursor     INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL
    );
    "#,
    // V17: `tool_artifacts` only ever described run-scoped tool output, so
    // canvas media (which has no owning run) could not be indexed. Generalize
    // it to one `artifacts` table instead of adding a second media index:
    // run/call ownership become optional and workspace + kind become explicit.
    r#"
    CREATE TABLE artifacts (
        id           TEXT PRIMARY KEY NOT NULL,
        kind         TEXT NOT NULL,
        run_id       TEXT REFERENCES runs(id) ON DELETE CASCADE,
        call_id      TEXT,
        workspace_id TEXT,
        path         TEXT NOT NULL,
        media_type   TEXT,
        byte_size    INTEGER NOT NULL,
        digest       TEXT,
        created_at   INTEGER NOT NULL
    );
    INSERT INTO artifacts (id, kind, run_id, call_id, workspace_id, path, media_type, byte_size, digest, created_at)
        SELECT id, 'tool_result', run_id, call_id, NULL, path, media_type, byte_size, digest, created_at
        FROM tool_artifacts;
    DROP TABLE tool_artifacts;
    CREATE INDEX idx_artifacts_run ON artifacts(run_id, call_id);
    CREATE INDEX idx_artifacts_workspace ON artifacts(workspace_id, kind, created_at);
    "#,
    // V18: move the authoritative conversation event stream out of SQLite.
    // `session_file_state` is only a rebuildable cursor/projection; the
    // per-session compressed JSONL generations are the source of truth. Old
    // event rows are intentionally not dual-read because their protocol era
    // cannot be replayed safely through the current Responses adapter.
    r#"
    CREATE TABLE session_file_state (
        session_id    TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        generation    INTEGER NOT NULL,
        last_sequence INTEGER,
        byte_size     INTEGER NOT NULL DEFAULT 0,
        updated_at    INTEGER NOT NULL
    );
    DELETE FROM events;
    INSERT OR IGNORE INTO migration_notices(key) VALUES ('session_files_v1_cutover_completed');
    INSERT OR IGNORE INTO migration_notices(key) VALUES ('session_files_v1_maintenance_pending');
    "#,
    // V19: rebuildable local full-text index for file-backed conversations.
    // The FTS table stores its own derived text so it can be deleted and
    // rebuilt independently of the authoritative session files.
    r#"
    CREATE TABLE session_search_docs (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        sequence     INTEGER NOT NULL,
        role         TEXT,
        timestamp    INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        UNIQUE(session_id, sequence)
    );
    CREATE INDEX idx_session_search_docs_session_seq
        ON session_search_docs(session_id, sequence);

    CREATE VIRTUAL TABLE session_search_fts USING fts5(
        doc_id UNINDEXED,
        content,
        tokenize='trigram'
    );

    CREATE TABLE session_search_cjk_bigrams (
        doc_id INTEGER NOT NULL REFERENCES session_search_docs(id) ON DELETE CASCADE,
        gram   TEXT NOT NULL,
        PRIMARY KEY(doc_id, gram)
    );
    CREATE INDEX idx_session_search_cjk_gram
        ON session_search_cjk_bigrams(gram, doc_id);

    CREATE TABLE session_search_cursors (
        session_id    TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        generation    INTEGER NOT NULL,
        last_sequence INTEGER,
        dirty         INTEGER NOT NULL DEFAULT 0,
        updated_at    INTEGER NOT NULL
    );
    "#,
    // V20: tool-result previews changed. Mark the disposable search projection
    // dirty so Database::open rebuilds it from authoritative session files.
    r#"
    UPDATE session_search_cursors SET dirty = 1;
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
        return ensure_session_search_projection(conn);
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

    ensure_session_search_projection(conn)
}

/// The session search schema is a disposable projection. Recreate the entire
/// group when any one of its tables has been removed, so stale cursors cannot
/// falsely report a complete index after a manual reset or interrupted repair.
fn ensure_session_search_projection(conn: &Connection) -> Result<()> {
    if current_version(conn)? != LATEST_VERSION {
        return Ok(());
    }
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'view')
             AND name IN ('session_search_docs', 'session_search_fts',
                          'session_search_cjk_bigrams', 'session_search_cursors')",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    if present == 4 {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN;
         DROP TABLE IF EXISTS session_search_fts;
         DROP TABLE IF EXISTS session_search_cjk_bigrams;
         DROP TABLE IF EXISTS session_search_cursors;
         DROP TABLE IF EXISTS session_search_docs;",
    )
    .map_err(map_sqlite)?;
    let result = conn.execute_batch(MIGRATIONS[18]).map_err(map_sqlite);
    match result {
        Ok(()) => conn.execute_batch("COMMIT;").map_err(map_sqlite),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(error)
        }
    }
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
            "artifacts",
            "subagent_runs",
            "run_actions",
            "run_approvals",
            "execution_leases",
            "secret_records",
            "managed_files",
            "session_file_state",
            "session_search_docs",
            "session_search_cjk_bigrams",
            "session_search_cursors",
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
        let fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_search_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts, 1);
    }

    /// Upgrading an installed database must keep its tool-output index rather
    /// than stranding it in the dropped table.
    #[test]
    fn v17_moves_existing_tool_artifact_rows() {
        let conn = Connection::open_in_memory().unwrap();
        for script in &MIGRATIONS[..16] {
            conn.execute_batch(script).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 16;").unwrap();
        // No owning run row exists in this fixture, so the foreign key is
        // checked only by the real application path.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO tool_artifacts (id, run_id, call_id, path, media_type, byte_size, digest, created_at) \
             VALUES ('a1', 'r1', 'c1', '/tmp/r1.txt', NULL, 10, NULL, 5)",
            [],
        )
        .unwrap();
        conn.execute_batch(MIGRATIONS[16]).unwrap();

        let (kind, run_id, path): (String, String, String) = conn
            .query_row(
                "SELECT kind, run_id, path FROM artifacts WHERE id='a1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(kind, "tool_result");
        assert_eq!(run_id, "r1");
        assert_eq!(path, "/tmp/r1.txt");
        let legacy: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='tool_artifacts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0);
    }

    #[test]
    fn v18_direct_cut_clears_legacy_event_bodies() {
        let conn = Connection::open_in_memory().unwrap();
        for script in &MIGRATIONS[..17] {
            conn.execute_batch(script).unwrap();
        }
        conn.execute(
            "INSERT INTO sessions
                (id, title, created_at, updated_at, ended_at, mode, project)
             VALUES ('ses_legacy', 'legacy', 1, 1, NULL, 'normal', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (id, session_id, sequence, kind, timestamp, payload)
             VALUES ('evt_legacy', 'ses_legacy', 0, 'note', 1, '{}')",
            [],
        )
        .unwrap();

        conn.execute_batch(MIGRATIONS[17]).unwrap();

        let events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(events, 0);
        let state_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='table' AND name='session_file_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state_table, 1);
    }
}
