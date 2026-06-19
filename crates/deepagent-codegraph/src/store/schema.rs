//! SQLite schema definition and migration framework for the code-graph store.
//!
//! The on-disk layout mirrors `design.md` (Data Models) and the reference
//! `schema.sql`: six relational tables (`nodes` / `edges` / `files` /
//! `unresolved_refs` / `schema_versions` / `project_metadata`), a `nodes_fts`
//! FTS5 virtual table kept in sync by triggers, and a set of performance
//! indexes.
//!
//! ## Migration model
//!
//! Migrations are an append-only, ordered list of SQL scripts ([`MIGRATIONS`]).
//! Each carries an integer `version`, a human description, and the DDL to bring
//! the schema up to that version. Applied versions are recorded in the
//! `schema_versions` table; [`init_schema`] applies every migration whose
//! version has not yet been recorded, each inside its own transaction.
//!
//! All DDL is written with `IF NOT EXISTS`, so applying the schema to a
//! database that already has it is a harmless no-op — the migration framework
//! adds version gating on top so future migrations only ever run once.

use rusqlite::Connection;

use deepagent_core::error::Result;

use super::map_sqlite;

/// The highest schema version this build knows how to produce.
///
/// Bump this (and append a [`Migration`] to [`MIGRATIONS`]) whenever the schema
/// changes. The reader never downgrades; opening a database written by a newer
/// build leaves the extra versions in place.
pub const CURRENT_VERSION: i64 = 1;

/// A single, versioned schema migration.
///
/// `version` is the schema version reached after `sql` runs successfully.
/// Entries in [`MIGRATIONS`] are append-only and must stay ordered by
/// ascending `version`; never edit or reorder an existing entry.
struct Migration {
    /// Schema version reached after this migration applies.
    version: i64,
    /// Human-readable description recorded in `schema_versions.description`.
    description: &'static str,
    /// The DDL script for this migration (all statements `IF NOT EXISTS`).
    sql: &'static str,
}

/// Ordered, append-only list of migrations.
///
/// Index order is ascending `version`. Adding a new schema revision means
/// pushing a new [`Migration`] with `version = CURRENT_VERSION` after bumping
/// [`CURRENT_VERSION`]. Existing entries are immutable.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    description: "Initial schema",
    sql: SCHEMA_V1,
}];

/// The `schema_versions` bookkeeping table, created before any migration runs
/// so applied-version tracking always has somewhere to write.
const SCHEMA_VERSIONS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_versions (
    version    INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL,
    description TEXT
);
"#;

/// Version 1: the complete initial schema (tables, FTS5 virtual table + sync
/// triggers, and performance indexes).
const SCHEMA_V1: &str = r#"
-- ---------------------------------------------------------------------------
-- Core tables
-- ---------------------------------------------------------------------------

-- Nodes: code symbols (functions, classes, structs, ...).
CREATE TABLE IF NOT EXISTS nodes (
    id             TEXT PRIMARY KEY,
    kind           TEXT NOT NULL,
    name           TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    file_path      TEXT NOT NULL,
    language       TEXT NOT NULL,
    start_line     INTEGER NOT NULL,
    end_line       INTEGER NOT NULL,
    start_column   INTEGER NOT NULL,
    end_column     INTEGER NOT NULL,
    signature      TEXT,
    docstring      TEXT,
    visibility     TEXT,
    is_exported    INTEGER NOT NULL DEFAULT 0,
    is_async       INTEGER NOT NULL DEFAULT 0,
    updated_at     INTEGER NOT NULL
);

-- Edges: directed relationships between nodes.
CREATE TABLE IF NOT EXISTS edges (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    source     TEXT NOT NULL,
    target     TEXT NOT NULL,
    kind       TEXT NOT NULL,
    metadata   TEXT,
    line       INTEGER,
    provenance TEXT DEFAULT NULL,
    FOREIGN KEY (source) REFERENCES nodes(id) ON DELETE CASCADE,
    FOREIGN KEY (target) REFERENCES nodes(id) ON DELETE CASCADE
);

-- Files: tracked source files (drives incremental sync via content_hash).
CREATE TABLE IF NOT EXISTS files (
    path         TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    language     TEXT NOT NULL,
    size         INTEGER NOT NULL,
    modified_at  INTEGER NOT NULL,
    indexed_at   INTEGER NOT NULL,
    node_count   INTEGER NOT NULL DEFAULT 0
);

-- Unresolved references: parked for the resolver after full extraction.
CREATE TABLE IF NOT EXISTS unresolved_refs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    from_node_id   TEXT NOT NULL,
    reference_name TEXT NOT NULL,
    reference_kind TEXT NOT NULL,
    line           INTEGER NOT NULL,
    file_path      TEXT NOT NULL DEFAULT '',
    FOREIGN KEY (from_node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

-- Project metadata: small key/value store for provenance/version info.
CREATE TABLE IF NOT EXISTS project_metadata (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- ---------------------------------------------------------------------------
-- Full-text search (FTS5)
-- ---------------------------------------------------------------------------

-- External-content FTS index over node names/docstrings/signatures. The
-- bundled SQLite ships with FTS5 compiled in, so this works with no extra
-- feature flag. content='nodes' / content_rowid='rowid' make it an
-- external-content table backed by `nodes`.
CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
    id,
    name,
    qualified_name,
    docstring,
    signature,
    content='nodes',
    content_rowid='rowid'
);

-- Triggers keep nodes_fts in sync with nodes (insert / delete / update).
CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
    VALUES (NEW.rowid, NEW.id, NEW.name, NEW.qualified_name, NEW.docstring, NEW.signature);
END;

CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, id, name, qualified_name, docstring, signature)
    VALUES ('delete', OLD.rowid, OLD.id, OLD.name, OLD.qualified_name, OLD.docstring, OLD.signature);
END;

CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, id, name, qualified_name, docstring, signature)
    VALUES ('delete', OLD.rowid, OLD.id, OLD.name, OLD.qualified_name, OLD.docstring, OLD.signature);
    INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
    VALUES (NEW.rowid, NEW.id, NEW.name, NEW.qualified_name, NEW.docstring, NEW.signature);
END;

-- ---------------------------------------------------------------------------
-- Performance indexes
-- ---------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_nodes_kind      ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_name      ON nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_file_path ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_file_line ON nodes(file_path, start_line);

-- (source, kind) / (target, kind) composites also serve source-only /
-- target-only lookups via SQLite's left-prefix scan.
CREATE INDEX IF NOT EXISTS idx_edges_kind        ON edges(kind);
CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source, kind);
CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target, kind);

CREATE INDEX IF NOT EXISTS idx_files_language ON files(language);

CREATE INDEX IF NOT EXISTS idx_unresolved_name      ON unresolved_refs(reference_name);
CREATE INDEX IF NOT EXISTS idx_unresolved_from_node ON unresolved_refs(from_node_id);
"#;

/// Initialise (or migrate) the schema on `conn`.
///
/// Enables `PRAGMA foreign_keys` for the connection (so the `edges` /
/// `unresolved_refs` cascade deletes are enforced), ensures the
/// `schema_versions` table exists, then applies every pending migration in
/// order. Each migration runs in its own transaction and records its version
/// in `schema_versions`.
///
/// Idempotent: calling it again on an up-to-date database is a no-op.
pub fn init_schema(conn: &Connection) -> Result<()> {
    // foreign_keys is a per-connection pragma and is OFF by default; it must be
    // set before any cascade-relying operation. Setting it here covers every
    // connection opened through GraphStore.
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(map_sqlite)?;

    // Bookkeeping table must exist before we can read applied versions.
    conn.execute_batch(SCHEMA_VERSIONS_DDL)
        .map_err(map_sqlite)?;

    apply_pending_migrations(conn)
}

/// Apply every migration whose version is not yet recorded in
/// `schema_versions`. Each migration + its version record commit atomically.
fn apply_pending_migrations(conn: &Connection) -> Result<()> {
    for migration in MIGRATIONS {
        if is_applied(conn, migration.version)? {
            continue;
        }

        tracing::info!(
            version = migration.version,
            description = migration.description,
            "applying codegraph schema migration"
        );

        conn.execute_batch("BEGIN;").map_err(map_sqlite)?;
        let result = (|| -> Result<()> {
            conn.execute_batch(migration.sql).map_err(map_sqlite)?;
            conn.execute(
                "INSERT OR IGNORE INTO schema_versions (version, applied_at, description)
                 VALUES (?1, strftime('%s', 'now'), ?2)",
                rusqlite::params![migration.version, migration.description],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })();

        match result {
            Ok(()) => conn.execute_batch("COMMIT;").map_err(map_sqlite)?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Whether `version` has already been recorded in `schema_versions`.
fn is_applied(conn: &Connection, version: i64) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_versions WHERE version = ?1",
            rusqlite::params![version],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    Ok(count > 0)
}

/// The highest schema version recorded as applied on `conn`, or `0` if none.
///
/// Exposed for callers (and future migrations) that need to inspect the
/// on-disk schema version.
pub fn applied_version(conn: &Connection) -> Result<i64> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_versions",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    Ok(version)
}
