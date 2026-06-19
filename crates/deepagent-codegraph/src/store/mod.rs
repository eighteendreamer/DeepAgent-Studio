//! Storage layer: [`GraphStore`] over rusqlite.
//!
//! Persists the code graph — `nodes` / `edges` / `files` / `unresolved_refs`
//! plus a `nodes_fts` FTS5 index — into a single SQLite database. This module
//! owns connection setup and schema/migration bootstrapping (see [`schema`]);
//! CRUD, incremental change detection, and query helpers are layered on in
//! later tasks.

pub mod schema;

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use deepagent_core::error::{CoreError, Result};

use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, FileRecord, Language, Node, NodeKind, UnresolvedRef};

/// Outcome of comparing a fresh scan against the `files` table:
/// which paths are new, which changed content, and which vanished.
///
/// All paths are POSIX-style relative paths (the `files.path` primary key
/// format). Vectors are sorted for deterministic ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Paths present in the scan but absent from the store.
    pub added: Vec<String>,
    /// Paths present in both, but with a different `content_hash`.
    pub modified: Vec<String>,
    /// Paths present in the store but absent from the scan.
    pub deleted: Vec<String>,
}

/// Current wall-clock time as a Unix timestamp in seconds.
///
/// Falls back to `0` if the system clock is before the Unix epoch (which
/// cannot happen in practice) so callers never have to handle a clock error.
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Comma-separated `nodes` columns in the canonical order used by
/// [`row_to_node`]. Keeping the SELECT list and the row mapper in lock-step
/// avoids positional drift.
const NODE_COLUMNS: &str = "id, kind, name, qualified_name, file_path, language, \
     start_line, end_line, start_column, end_column, \
     signature, docstring, visibility, is_exported, is_async";

/// Map a `nodes` row (selected via [`NODE_COLUMNS`]) into a [`Node`].
fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let kind: String = row.get("kind")?;
    let language: String = row.get("language")?;
    let is_exported: i64 = row.get("is_exported")?;
    let is_async: i64 = row.get("is_async")?;
    Ok(Node {
        id: row.get("id")?,
        kind: NodeKind::parse(&kind),
        name: row.get("name")?,
        qualified_name: row.get("qualified_name")?,
        file_path: row.get("file_path")?,
        language: Language::parse(&language),
        start_line: row.get("start_line")?,
        end_line: row.get("end_line")?,
        start_column: row.get("start_column")?,
        end_column: row.get("end_column")?,
        signature: row.get("signature")?,
        docstring: row.get("docstring")?,
        visibility: row.get("visibility")?,
        is_exported: is_exported != 0,
        is_async: is_async != 0,
    })
}

/// Map an `edges` row (selected as `source, target, kind, metadata, line,
/// provenance`) into an [`Edge`], parsing the JSON `metadata` string.
fn row_to_edge(row: &rusqlite::Row<'_>) -> Result<Edge> {
    let kind: String = row.get("kind").map_err(map_sqlite)?;
    let metadata_str: Option<String> = row.get("metadata").map_err(map_sqlite)?;
    let metadata = match metadata_str {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };
    Ok(Edge {
        source: row.get("source").map_err(map_sqlite)?,
        target: row.get("target").map_err(map_sqlite)?,
        kind: EdgeKind::parse(&kind),
        metadata,
        line: row.get("line").map_err(map_sqlite)?,
        provenance: row.get("provenance").map_err(map_sqlite)?,
    })
}

/// Map an `unresolved_refs` row (selected as `from_node_id, reference_name,
/// reference_kind, line, file_path`) into an [`UnresolvedRef`].
fn row_to_unresolved_ref(row: &rusqlite::Row<'_>) -> rusqlite::Result<UnresolvedRef> {
    Ok(UnresolvedRef {
        from_node_id: row.get("from_node_id")?,
        reference_name: row.get("reference_name")?,
        reference_kind: row.get("reference_kind")?,
        line: row.get("line")?,
        file_path: row.get("file_path")?,
    })
}

/// Convert a [`rusqlite::Error`] into a [`CoreError::Persistence`].
///
/// Mirrors the workspace convention in `deepagent-persistence` so the store
/// layer surfaces a single, stable error type upward.
pub(crate) fn map_sqlite(e: rusqlite::Error) -> CoreError {
    CoreError::Persistence(e.to_string())
}

/// Build a safe FTS5 `MATCH` expression from arbitrary user input.
///
/// FTS5 query syntax treats characters like `"`, `*`, `:`, `(`, `)`, `^`, `-`
/// and the bareword keywords `AND` / `OR` / `NOT` / `NEAR` specially, so
/// feeding raw user text into `MATCH` risks a syntax error. To make *any* input
/// safe, the text is tokenised into maximal runs of alphanumeric/underscore
/// characters and each token is wrapped in double quotes (an FTS5 string
/// literal). Quoting neutralises operator characters and forces keywords to be
/// matched literally; the tokens are joined with spaces (implicit AND).
///
/// Returns `None` when the input contains no usable tokens (e.g. `""` or
/// `"***"`), in which case the caller should short-circuit to an empty result
/// rather than running a `MATCH` that cannot match anything.
fn build_fts_match(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        // Double any embedded quote defensively; tokens never contain a quote
        // (the split strips them) but this keeps the literal well-formed.
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

/// `nodes` columns prefixed with the `nodes.` table qualifier, for SELECTs that
/// join `nodes` against `nodes_fts` (which shares column names like `id` /
/// `name`). The output column names stay bare (`id`, `kind`, ...) so
/// [`row_to_node`] keeps working unchanged.
fn qualified_node_columns() -> String {
    NODE_COLUMNS
        .split(',')
        .map(|c| format!("nodes.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SQLite-backed store for the code graph.
///
/// A thin owner of a [`Connection`] with the schema initialised. Opening is
/// idempotent: the schema is created on first open and left untouched on
/// subsequent opens of the same database.
pub struct GraphStore {
    /// The underlying SQLite connection (schema initialised, `foreign_keys` on).
    pub(crate) conn: Connection,
}

impl GraphStore {
    /// Open (creating if necessary) the code-graph database at `db_path`,
    /// enabling foreign keys and applying all pending schema migrations.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path).map_err(map_sqlite)?;
        Self::from_connection(conn)
    }

    /// Open an isolated in-memory database with the schema applied.
    ///
    /// Primarily for tests; each call yields a fresh, independent database.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        Self::from_connection(conn)
    }

    /// Initialise pragmas + schema on an already-open connection.
    fn from_connection(conn: Connection) -> Result<Self> {
        schema::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// The highest schema version recorded as applied on this database.
    pub fn schema_version(&self) -> Result<i64> {
        schema::applied_version(&self.conn)
    }

    // -----------------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------------

    /// Insert or replace a tracked file record in the `files` table.
    ///
    /// Keyed on `path` (POSIX relative); re-upserting the same path overwrites
    /// the previous row. `node_count` keeps its column default.
    pub fn upsert_file(&self, file: &FileRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO files
                 (path, content_hash, language, size, modified_at, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    file.path,
                    file.content_hash,
                    file.language.as_str(),
                    file.size as i64,
                    file.modified_at,
                    file.indexed_at,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Batch-insert nodes inside a single transaction with a reused prepared
    /// statement.
    ///
    /// `kind`/`language` are stored via their `as_str()` form; `is_exported` /
    /// `is_async` as `0`/`1`; `updated_at` is set to the current timestamp.
    /// An empty slice is a no-op. The whole batch commits atomically, so a
    /// failure on any row rolls back the entire insert.
    pub fn insert_nodes(&self, nodes: &[Node]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let now = current_timestamp();
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO nodes (
                        id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column,
                        signature, docstring, visibility, is_exported, is_async, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6,
                        ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14, ?15, ?16
                     )",
                )
                .map_err(map_sqlite)?;
            for node in nodes {
                stmt.execute(params![
                    node.id,
                    node.kind.as_str(),
                    node.name,
                    node.qualified_name,
                    node.file_path,
                    node.language.as_str(),
                    node.start_line,
                    node.end_line,
                    node.start_column,
                    node.end_column,
                    node.signature,
                    node.docstring,
                    node.visibility,
                    node.is_exported as i64,
                    node.is_async as i64,
                    now,
                ])
                .map_err(map_sqlite)?;
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }

    /// Batch-insert unresolved references inside a single transaction with a
    /// reused prepared statement.
    ///
    /// These are call/usage references parked for the cross-file name matcher
    /// (a second-phase concern); persisting them in phase one keeps the data
    /// available without yet generating heuristic `calls` edges. An empty slice
    /// is a no-op; the whole batch commits atomically.
    pub fn insert_unresolved_refs(&self, refs: &[UnresolvedRef]) -> Result<()> {
        if refs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO unresolved_refs
                        (from_node_id, reference_name, reference_kind, line, file_path)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(map_sqlite)?;
            for r in refs {
                stmt.execute(params![
                    r.from_node_id,
                    r.reference_name,
                    r.reference_kind,
                    r.line,
                    r.file_path,
                ])
                .map_err(map_sqlite)?;
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }

    /// Batch-insert edges inside a single transaction with a reused prepared
    /// statement.
    ///
    /// `metadata` is serialised to a JSON string (NULL when absent). An empty
    /// slice is a no-op; the whole batch commits atomically.
    pub fn insert_edges(&self, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO edges (source, target, kind, metadata, line, provenance)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(map_sqlite)?;
            for edge in edges {
                let metadata = match &edge.metadata {
                    Some(value) => Some(serde_json::to_string(value)?),
                    None => None,
                };
                stmt.execute(params![
                    edge.source,
                    edge.target,
                    edge.kind.as_str(),
                    metadata,
                    edge.line,
                    edge.provenance,
                ])
                .map_err(map_sqlite)?;
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Deletes
    // -----------------------------------------------------------------------

    /// Remove all graph data associated with `path`.
    ///
    /// Deletes the `files` row, every `nodes` row whose `file_path` matches
    /// (which cascade-deletes their `edges` and `unresolved_refs` via the FK
    /// `ON DELETE CASCADE`), and any remaining `unresolved_refs` keyed on the
    /// file path directly. All steps run in one transaction so the file is
    /// either fully removed or left untouched.
    pub fn delete_file_cascade(&self, path: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        // Nodes first: their FK cascade removes dependent edges and
        // unresolved_refs (those referencing a deleted from_node_id).
        tx.execute("DELETE FROM nodes WHERE file_path = ?1", params![path])
            .map_err(map_sqlite)?;
        // Any unresolved_refs tracked by file path but not tied to a deleted
        // node (defensive: from_node_id cascade may not cover all of them).
        tx.execute(
            "DELETE FROM unresolved_refs WHERE file_path = ?1",
            params![path],
        )
        .map_err(map_sqlite)?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![path])
            .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Incremental diff
    // -----------------------------------------------------------------------

    /// Compare a fresh scan against the stored `files` table by `content_hash`.
    ///
    /// Returns a [`ChangeSet`]: `added` = scanned-but-not-stored,
    /// `modified` = stored-and-scanned-with-different-hash, `deleted` =
    /// stored-but-not-scanned. Paths are compared in their POSIX relative
    /// form. On a read error the stored side is treated as empty (so every
    /// scanned file is reported as added) and a warning is logged, keeping the
    /// signature infallible.
    pub fn changed_files(&self, scanned: &[ScannedFile]) -> ChangeSet {
        let stored = match self.stored_file_hashes() {
            Ok(map) => map,
            Err(err) => {
                tracing::warn!(error = %err, "changed_files: failed to read files table; treating store as empty");
                HashMap::new()
            }
        };

        let mut change = ChangeSet::default();
        let mut scanned_paths: HashMap<String, ()> = HashMap::with_capacity(scanned.len());

        for file in scanned {
            let path = file.relative_path.to_string_lossy().into_owned();
            scanned_paths.insert(path.clone(), ());
            match stored.get(&path) {
                None => change.added.push(path),
                Some(stored_hash) if *stored_hash != file.content_hash => {
                    change.modified.push(path)
                }
                Some(_) => {}
            }
        }

        for stored_path in stored.keys() {
            if !scanned_paths.contains_key(stored_path) {
                change.deleted.push(stored_path.clone());
            }
        }

        change.added.sort();
        change.modified.sort();
        change.deleted.sort();
        change
    }

    /// Load the `path -> content_hash` map from the `files` table.
    fn stored_file_hashes(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, content_hash FROM files")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_sqlite)?;

        let mut map = HashMap::new();
        for row in rows {
            let (path, hash) = row.map_err(map_sqlite)?;
            map.insert(path, hash);
        }
        Ok(map)
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// Fetch a single node by its stable id, or `None` if absent.
    pub fn node_by_id(&self, id: &str) -> Result<Option<Node>> {
        let sql = format!("SELECT {NODE_COLUMNS} FROM nodes WHERE id = ?1");
        self.conn
            .query_row(&sql, params![id], row_to_node)
            .optional()
            .map_err(map_sqlite)
    }

    /// Find the innermost symbol node containing `line` in `file_path`.
    ///
    /// The match is restricted to nodes from the requested POSIX file path
    /// where `start_line <= line <= end_line`. When multiple symbols contain
    /// the line, the smallest line range wins, which returns the innermost
    /// enclosing node. File nodes are ignored so a line outside any symbol
    /// yields `None` rather than the whole file.
    pub fn node_at_location(&self, file_path: &str, line: u32) -> Result<Option<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes
             WHERE file_path = ?1
               AND kind != ?2
               AND start_line <= ?3
               AND end_line >= ?3
             ORDER BY (end_line - start_line) ASC, start_line DESC, id ASC
             LIMIT 1"
        );
        self.conn
            .query_row(
                &sql,
                params![file_path, NodeKind::File.as_str(), line],
                row_to_node,
            )
            .optional()
            .map_err(map_sqlite)
    }

    /// Outgoing edges of `node_id` of the given [`EdgeKind`] (`source = id`).
    pub fn edges_from(&self, node_id: &str, kind: EdgeKind) -> Result<Vec<Edge>> {
        self.query_edges(
            "SELECT source, target, kind, metadata, line, provenance
             FROM edges WHERE source = ?1 AND kind = ?2",
            node_id,
            kind,
        )
    }

    /// Incoming edges of `node_id` of the given [`EdgeKind`] (`target = id`).
    pub fn edges_to(&self, node_id: &str, kind: EdgeKind) -> Result<Vec<Edge>> {
        self.query_edges(
            "SELECT source, target, kind, metadata, line, provenance
             FROM edges WHERE target = ?1 AND kind = ?2",
            node_id,
            kind,
        )
    }

    /// Shared edge query: bind `(node_id, kind)` and map every row to an
    /// [`Edge`].
    fn query_edges(&self, sql: &str, node_id: &str, kind: EdgeKind) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(sql).map_err(map_sqlite)?;
        let mut rows = stmt
            .query(params![node_id, kind.as_str()])
            .map_err(map_sqlite)?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().map_err(map_sqlite)? {
            edges.push(row_to_edge(row)?);
        }
        Ok(edges)
    }

    /// All nodes of kind `file`.
    pub fn all_file_nodes(&self) -> Result<Vec<Node>> {
        self.nodes_by_kind(NodeKind::File)
    }

    /// Number of tracked files in the `files` table.
    ///
    /// Used by the facade to decide between a full index (`0`) and an
    /// incremental sync (`> 0`).
    pub fn file_count(&self) -> Result<usize> {
        self.scalar_count("SELECT COUNT(*) FROM files")
    }

    /// Total number of nodes in the graph.
    pub fn node_count(&self) -> Result<usize> {
        self.scalar_count("SELECT COUNT(*) FROM nodes")
    }

    /// Total number of edges in the graph.
    pub fn edge_count(&self) -> Result<usize> {
        self.scalar_count("SELECT COUNT(*) FROM edges")
    }

    /// Run a `SELECT COUNT(*)` query returning a single integer as `usize`.
    fn scalar_count(&self, sql: &str) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(map_sqlite)?;
        Ok(count.max(0) as usize)
    }

    /// Every node in the graph, ordered by file path then start line.
    ///
    /// Used by the projector to down-project the full rich graph into the UA
    /// `knowledge-graph.json`.
    pub fn all_nodes(&self) -> Result<Vec<Node>> {
        let sql = format!("SELECT {NODE_COLUMNS} FROM nodes ORDER BY file_path, start_line, id");
        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt.query_map([], row_to_node).map_err(map_sqlite)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(map_sqlite)?);
        }
        Ok(nodes)
    }

    /// Every edge in the graph, ordered for deterministic output.
    pub fn all_edges(&self) -> Result<Vec<Edge>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source, target, kind, metadata, line, provenance
                 FROM edges ORDER BY source, target, kind, id",
            )
            .map_err(map_sqlite)?;
        let mut rows = stmt.query([]).map_err(map_sqlite)?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().map_err(map_sqlite)? {
            edges.push(row_to_edge(row)?);
        }
        Ok(edges)
    }

    /// All nodes of a given kind, ordered by file path then start line.
    pub fn nodes_by_kind(&self, kind: NodeKind) -> Result<Vec<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes WHERE kind = ?1 ORDER BY file_path, start_line"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![kind.as_str()], row_to_node)
            .map_err(map_sqlite)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(map_sqlite)?);
        }
        Ok(nodes)
    }

    /// Full-text search over the `nodes_fts` FTS5 index (name / qualified_name
    /// / docstring / signature), returning the matching [`Node`]s ranked by
    /// BM25 relevance (best first).
    ///
    /// `query` is arbitrary user text: it is tokenised and quoted via
    /// [`build_fts_match`] so any input — including FTS5 operator characters or
    /// reserved keywords — is matched literally and never raises a SQL syntax
    /// error. A query with no usable tokens yields an empty result.
    ///
    /// `kind` optionally restricts results to a single [`NodeKind`]. `limit`
    /// caps the number of rows returned (`0` means unlimited). The FTS index is
    /// external-content (`content='nodes'`), so its `rowid` joins straight back
    /// to `nodes.rowid` to recover the full node.
    pub fn search_fts(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: usize,
    ) -> Result<Vec<Node>> {
        let match_expr = match build_fts_match(query) {
            Some(expr) => expr,
            None => return Ok(Vec::new()),
        };

        let cols = qualified_node_columns();
        let mut sql = format!(
            "SELECT {cols} FROM nodes_fts \
             JOIN nodes ON nodes.rowid = nodes_fts.rowid \
             WHERE nodes_fts MATCH ?1"
        );
        if kind.is_some() {
            sql.push_str(" AND nodes.kind = ?2");
        }
        // bm25() ranks better matches lower, so ascending order is best-first.
        sql.push_str(" ORDER BY bm25(nodes_fts), nodes.file_path, nodes.start_line");
        if limit != 0 {
            // `limit` is a usize, so inlining it cannot inject SQL.
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let mut nodes = Vec::new();
        if let Some(kind) = kind {
            let rows = stmt
                .query_map(params![match_expr, kind.as_str()], row_to_node)
                .map_err(map_sqlite)?;
            for row in rows {
                nodes.push(row.map_err(map_sqlite)?);
            }
        } else {
            let rows = stmt
                .query_map(params![match_expr], row_to_node)
                .map_err(map_sqlite)?;
            for row in rows {
                nodes.push(row.map_err(map_sqlite)?);
            }
        }
        Ok(nodes)
    }

    /// Locate nodes by an exact `qualified_name` or `name` match, with
    /// qualified-name matches ranked first.
    ///
    /// Used by `explore` to anchor an input symbol name to concrete graph
    /// nodes (preferring the precise, fully-qualified definition over a bare
    /// name collision). Ties break by file path then start line for
    /// determinism.
    pub fn nodes_by_name(&self, name: &str) -> Result<Vec<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes \
             WHERE qualified_name = ?1 OR name = ?1 \
             ORDER BY (qualified_name = ?1) DESC, file_path, start_line"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![name], row_to_node)
            .map_err(map_sqlite)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(map_sqlite)?);
        }
        Ok(nodes)
    }

    /// Every parked [`UnresolvedRef`] in the `unresolved_refs` table, ordered
    /// for deterministic output.
    ///
    /// Used by the cross-file call-name matcher
    /// ([`crate::resolution::Resolver::resolve_all`]) to turn parked call
    /// references into heuristic `calls` edges.
    pub fn all_unresolved_refs(&self) -> Result<Vec<UnresolvedRef>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT from_node_id, reference_name, reference_kind, line, file_path
                 FROM unresolved_refs ORDER BY file_path, line, reference_name",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], row_to_unresolved_ref)
            .map_err(map_sqlite)?;
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row.map_err(map_sqlite)?);
        }
        Ok(refs)
    }

    /// Parked [`UnresolvedRef`]s whose call site lives in one of `files`
    /// (matched on `unresolved_refs.file_path`), ordered deterministically.
    ///
    /// Used by [`crate::resolution::Resolver::resolve_for_files`] during an
    /// incremental sync so only references in changed files are re-resolved.
    /// An empty `files` slice yields an empty result without touching the
    /// database.
    pub fn unresolved_refs_for_files(&self, files: &[String]) -> Result<Vec<UnresolvedRef>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let wanted: std::collections::BTreeSet<&str> = files.iter().map(String::as_str).collect();
        Ok(self
            .all_unresolved_refs()?
            .into_iter()
            .filter(|r| wanted.contains(r.file_path.as_str()))
            .collect())
    }

    // -----------------------------------------------------------------------
    // Project metadata
    // -----------------------------------------------------------------------

    /// Set (insert or overwrite) a `project_metadata` key/value pair.
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO project_metadata (key, value, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = excluded.updated_at",
                params![key, value, current_timestamp()],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Read a `project_metadata` value by key, or `None` if unset.
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names of every relational table the schema must create.
    const EXPECTED_TABLES: &[&str] = &[
        "nodes",
        "edges",
        "files",
        "unresolved_refs",
        "schema_versions",
        "project_metadata",
    ];

    /// Names of the FTS sync triggers.
    const EXPECTED_TRIGGERS: &[&str] = &["nodes_ai", "nodes_ad", "nodes_au"];

    fn table_exists(store: &GraphStore, name: &str) -> bool {
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .unwrap();
        count == 1
    }

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = GraphStore::open_in_memory().unwrap();
        for table in EXPECTED_TABLES {
            assert!(table_exists(&store, table), "table {table} should exist");
        }
    }

    #[test]
    fn fts_virtual_table_exists() {
        let store = GraphStore::open_in_memory().unwrap();
        // FTS5 virtual tables are registered as type='table' in sqlite_master.
        assert!(
            table_exists(&store, "nodes_fts"),
            "nodes_fts virtual table should exist"
        );
    }

    #[test]
    fn fts_triggers_exist() {
        let store = GraphStore::open_in_memory().unwrap();
        for trigger in EXPECTED_TRIGGERS {
            let count: i64 = store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name=?1",
                    rusqlite::params![trigger],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "trigger {trigger} should exist");
        }
    }

    #[test]
    fn performance_indexes_exist() {
        let store = GraphStore::open_in_memory().unwrap();
        for index in [
            "idx_nodes_kind",
            "idx_nodes_name",
            "idx_nodes_file_path",
            "idx_nodes_file_line",
            "idx_edges_kind",
            "idx_edges_source_kind",
            "idx_edges_target_kind",
            "idx_files_language",
            "idx_unresolved_name",
            "idx_unresolved_from_node",
        ] {
            let count: i64 = store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    rusqlite::params![index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "index {index} should exist");
        }
    }

    #[test]
    fn schema_version_is_current() {
        let store = GraphStore::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::CURRENT_VERSION);
    }

    #[test]
    fn fts_index_is_queryable_and_synced_by_triggers() {
        let store = GraphStore::open_in_memory().unwrap();

        // Insert a node; the nodes_ai trigger should mirror it into nodes_fts.
        store
            .conn
            .execute(
                "INSERT INTO nodes (
                    id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column, updated_at
                 ) VALUES (
                    'function:src/lib.rs:my_handler:10', 'function', 'my_handler',
                    'crate::my_handler', 'src/lib.rs', 'rust',
                    10, 20, 0, 1, 0
                 )",
                [],
            )
            .unwrap();

        // FTS lookup by name finds the inserted node.
        let hit: String = store
            .conn
            .query_row(
                "SELECT id FROM nodes_fts WHERE nodes_fts MATCH ?1",
                rusqlite::params!["my_handler"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit, "function:src/lib.rs:my_handler:10");

        // Deleting the node removes it from the FTS index (nodes_ad trigger).
        store
            .conn
            .execute(
                "DELETE FROM nodes WHERE id = 'function:src/lib.rs:my_handler:10'",
                [],
            )
            .unwrap();
        let remaining: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH ?1",
                rusqlite::params!["my_handler"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "FTS row should be removed with the node");
    }

    #[test]
    fn foreign_keys_pragma_is_enabled() {
        let store = GraphStore::open_in_memory().unwrap();
        let enabled: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1, "foreign_keys pragma should be ON");
    }

    #[test]
    fn foreign_key_cascade_deletes_edges() {
        let store = GraphStore::open_in_memory().unwrap();

        // Two nodes + an edge between them.
        for id in ["a", "b"] {
            store
                .conn
                .execute(
                    "INSERT INTO nodes (
                        id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column, updated_at
                     ) VALUES (?1, 'function', ?1, ?1, 'src/x.rs', 'rust', 1, 2, 0, 1, 0)",
                    rusqlite::params![id],
                )
                .unwrap();
        }
        store
            .conn
            .execute(
                "INSERT INTO edges (source, target, kind) VALUES ('a', 'b', 'calls')",
                [],
            )
            .unwrap();

        // Deleting node 'a' should cascade-delete the edge (FK ON DELETE CASCADE).
        store
            .conn
            .execute("DELETE FROM nodes WHERE id = 'a'", [])
            .unwrap();
        let edge_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(edge_count, 0, "edge should cascade-delete with its source");
    }

    #[test]
    fn reopening_same_database_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codegraph.db");

        let store = GraphStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::CURRENT_VERSION);
        drop(store);

        // Reopening must not error and must not bump the version.
        let store2 = GraphStore::open(&path).unwrap();
        assert_eq!(store2.schema_version().unwrap(), schema::CURRENT_VERSION);

        // schema_versions should hold exactly one row for v1 (no duplicates).
        let rows: i64 = store2
            .conn
            .query_row("SELECT COUNT(*) FROM schema_versions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1, "version should be recorded exactly once");
    }

    #[test]
    fn double_init_on_same_connection_is_idempotent() {
        let store = GraphStore::open_in_memory().unwrap();
        // Running init again should be a harmless no-op.
        schema::init_schema(&store.conn).unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::CURRENT_VERSION);
    }
}

#[cfg(test)]
mod crud_tests {
    use super::*;
    use crate::scanner::ScannedFile;
    use crate::types::{Edge, EdgeKind, FileRecord, Language, Node, NodeKind};
    use std::path::PathBuf;

    /// Build a node with sensible defaults; override fields at the call site.
    fn node(id: &str, kind: NodeKind, file_path: &str) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: id.to_string(),
            qualified_name: format!("crate::{id}"),
            file_path: file_path.to_string(),
            language: Language::Rust,
            start_line: 1,
            end_line: 10,
            start_column: 0,
            end_column: 4,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn edge(source: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind,
            metadata: None,
            line: None,
            provenance: None,
        }
    }

    fn file_record(path: &str, hash: &str) -> FileRecord {
        FileRecord {
            path: path.to_string(),
            content_hash: hash.to_string(),
            language: Language::Rust,
            size: 123,
            modified_at: 1_000,
            indexed_at: 2_000,
        }
    }

    fn scanned(path: &str, hash: &str) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(format!("/abs/{path}")),
            relative_path: PathBuf::from(path),
            language: Language::Rust,
            size: 123,
            content_hash: hash.to_string(),
        }
    }

    #[test]
    fn node_round_trips_all_fields() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut n = node(
            "function:src/lib.rs:handler:5",
            NodeKind::Function,
            "src/lib.rs",
        );
        n.name = "handler".into();
        n.qualified_name = "crate::handler".into();
        n.language = Language::TypeScript;
        n.start_line = 5;
        n.end_line = 42;
        n.start_column = 2;
        n.end_column = 9;
        n.signature = Some("fn handler(req: Req) -> Resp".into());
        n.docstring = Some("Handles a request.".into());
        n.visibility = Some("pub".into());
        n.is_exported = true;
        n.is_async = true;

        store.insert_nodes(&[n.clone()]).unwrap();

        let fetched = store.node_by_id(&n.id).unwrap().expect("node should exist");
        assert_eq!(fetched, n);
    }

    #[test]
    fn node_by_id_returns_none_for_missing() {
        let store = GraphStore::open_in_memory().unwrap();
        assert!(store.node_by_id("does:not:exist").unwrap().is_none());
    }

    #[test]
    fn node_at_location_returns_symbol_containing_line() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut function = node(
            "function:src/lib.rs:handle:3",
            NodeKind::Function,
            "src/lib.rs",
        );
        function.start_line = 3;
        function.end_line = 12;
        store
            .insert_nodes(&[
                node("file:src/lib.rs", NodeKind::File, "src/lib.rs"),
                function.clone(),
            ])
            .unwrap();

        let hit = store
            .node_at_location("src/lib.rs", 7)
            .unwrap()
            .expect("function should contain line 7");

        assert_eq!(hit, function);
    }

    #[test]
    fn node_at_location_prefers_innermost_range() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut class = node(
            "class:src/app.ts:Controller:1",
            NodeKind::Class,
            "src/app.ts",
        );
        class.start_line = 1;
        class.end_line = 40;
        let mut method = node(
            "method:src/app.ts:Controller.handle:10",
            NodeKind::Method,
            "src/app.ts",
        );
        method.start_line = 10;
        method.end_line = 20;
        let mut inner = node(
            "function:src/app.ts:Controller.handle.inner:14",
            NodeKind::Function,
            "src/app.ts",
        );
        inner.start_line = 14;
        inner.end_line = 16;
        store
            .insert_nodes(&[
                node("file:src/app.ts", NodeKind::File, "src/app.ts"),
                class,
                method,
                inner.clone(),
            ])
            .unwrap();

        let hit = store
            .node_at_location("src/app.ts", 15)
            .unwrap()
            .expect("inner function should contain line 15");

        assert_eq!(hit, inner);
    }

    #[test]
    fn node_at_location_returns_none_outside_symbols() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut function = node(
            "function:src/lib.rs:handle:3",
            NodeKind::Function,
            "src/lib.rs",
        );
        function.start_line = 3;
        function.end_line = 12;
        store
            .insert_nodes(&[
                node("file:src/lib.rs", NodeKind::File, "src/lib.rs"),
                function,
            ])
            .unwrap();

        assert!(store.node_at_location("src/lib.rs", 2).unwrap().is_none());
        assert!(store
            .node_at_location("src/missing.rs", 7)
            .unwrap()
            .is_none());
    }

    #[test]
    fn edges_from_and_to_filter_by_kind() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[
                node("a", NodeKind::Function, "src/a.rs"),
                node("b", NodeKind::Function, "src/b.rs"),
                node("c", NodeKind::Function, "src/c.rs"),
            ])
            .unwrap();
        store
            .insert_edges(&[
                edge("a", "b", EdgeKind::Calls),
                edge("a", "c", EdgeKind::Imports),
                edge("c", "a", EdgeKind::Calls),
            ])
            .unwrap();

        let calls_from_a = store.edges_from("a", EdgeKind::Calls).unwrap();
        assert_eq!(calls_from_a.len(), 1);
        assert_eq!(calls_from_a[0].target, "b");

        let imports_from_a = store.edges_from("a", EdgeKind::Imports).unwrap();
        assert_eq!(imports_from_a.len(), 1);
        assert_eq!(imports_from_a[0].target, "c");

        let calls_to_a = store.edges_to("a", EdgeKind::Calls).unwrap();
        assert_eq!(calls_to_a.len(), 1);
        assert_eq!(calls_to_a[0].source, "c");

        // No imports edges point at `a`.
        assert!(store.edges_to("a", EdgeKind::Imports).unwrap().is_empty());
    }

    #[test]
    fn edge_metadata_json_round_trips() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[
                node("a", NodeKind::Function, "src/a.rs"),
                node("b", NodeKind::Function, "src/b.rs"),
            ])
            .unwrap();

        let mut e = edge("a", "b", EdgeKind::Calls);
        e.metadata = Some(serde_json::json!({ "arg_count": 3, "via": "trait" }));
        e.line = Some(17);
        e.provenance = Some("heuristic".into());
        store.insert_edges(&[e.clone()]).unwrap();

        let fetched = store.edges_from("a", EdgeKind::Calls).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0], e);
        assert_eq!(fetched[0].metadata.as_ref().unwrap()["arg_count"], 3);
    }

    #[test]
    fn delete_file_cascade_removes_nodes_edges_and_unresolved_refs() {
        let store = GraphStore::open_in_memory().unwrap();

        // Two files: doomed.rs gets deleted, keep.rs must survive.
        store
            .upsert_file(&file_record("src/doomed.rs", "h1"))
            .unwrap();
        store
            .upsert_file(&file_record("src/keep.rs", "h2"))
            .unwrap();
        store
            .insert_nodes(&[
                node("d1", NodeKind::Function, "src/doomed.rs"),
                node("d2", NodeKind::Function, "src/doomed.rs"),
                node("k1", NodeKind::Function, "src/keep.rs"),
            ])
            .unwrap();
        // Edge within doomed file + an edge bridging to the kept file.
        store
            .insert_edges(&[
                edge("d1", "d2", EdgeKind::Calls),
                edge("d1", "k1", EdgeKind::Calls),
                edge("k1", "k1", EdgeKind::References),
            ])
            .unwrap();
        // unresolved_refs for the doomed file and the kept file.
        store
            .conn
            .execute(
                "INSERT INTO unresolved_refs (from_node_id, reference_name, reference_kind, line, file_path)
                 VALUES ('d1', 'foo', 'call', 3, 'src/doomed.rs'),
                        ('k1', 'bar', 'call', 4, 'src/keep.rs')",
                [],
            )
            .unwrap();

        store.delete_file_cascade("src/doomed.rs").unwrap();

        // Doomed nodes gone, kept node remains.
        assert!(store.node_by_id("d1").unwrap().is_none());
        assert!(store.node_by_id("d2").unwrap().is_none());
        assert!(store.node_by_id("k1").unwrap().is_some());

        // Files row removed for doomed, kept for the other.
        let file_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(file_count, 1);

        // Edges touching deleted nodes cascade away; the keep.rs self-edge stays.
        let edge_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 1, "only the keep.rs self-edge should remain");

        // unresolved_refs for the doomed file cleaned up, kept file's remains.
        let remaining_refs: Vec<String> = {
            let mut stmt = store
                .conn
                .prepare("SELECT file_path FROM unresolved_refs")
                .unwrap();
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            rows
        };
        assert_eq!(remaining_refs, vec!["src/keep.rs".to_string()]);
    }

    #[test]
    fn changed_files_classifies_added_modified_deleted() {
        let store = GraphStore::open_in_memory().unwrap();
        // Store already knows about three files.
        store
            .upsert_file(&file_record("src/same.rs", "hash_same"))
            .unwrap();
        store
            .upsert_file(&file_record("src/edited.rs", "hash_old"))
            .unwrap();
        store
            .upsert_file(&file_record("src/gone.rs", "hash_gone"))
            .unwrap();

        // Scan: same unchanged, edited changed, gone missing, fresh new.
        let scan = vec![
            scanned("src/same.rs", "hash_same"),
            scanned("src/edited.rs", "hash_new"),
            scanned("src/fresh.rs", "hash_fresh"),
        ];

        let change = store.changed_files(&scan);
        assert_eq!(change.added, vec!["src/fresh.rs".to_string()]);
        assert_eq!(change.modified, vec!["src/edited.rs".to_string()]);
        assert_eq!(change.deleted, vec!["src/gone.rs".to_string()]);
    }

    #[test]
    fn changed_files_on_empty_store_reports_all_added() {
        let store = GraphStore::open_in_memory().unwrap();
        let scan = vec![scanned("a.rs", "h1"), scanned("b.rs", "h2")];
        let change = store.changed_files(&scan);
        assert_eq!(change.added, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert!(change.modified.is_empty());
        assert!(change.deleted.is_empty());
    }

    #[test]
    fn all_file_nodes_returns_only_file_kind() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[
                node("file:src/a.rs", NodeKind::File, "src/a.rs"),
                node("file:src/b.rs", NodeKind::File, "src/b.rs"),
                node("function:src/a.rs:f:1", NodeKind::Function, "src/a.rs"),
                node("struct:src/b.rs:S:1", NodeKind::Struct, "src/b.rs"),
            ])
            .unwrap();

        let files = store.all_file_nodes().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|n| n.kind == NodeKind::File));
        let ids: Vec<&str> = files.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["file:src/a.rs", "file:src/b.rs"]);
    }

    #[test]
    fn nodes_by_kind_returns_requested_kind_in_stable_order() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut import_b = node("import:src/b.rs:crate::a:1", NodeKind::Import, "src/b.rs");
        import_b.start_line = 4;
        let mut import_a = node("import:src/a.rs:crate::b:1", NodeKind::Import, "src/a.rs");
        import_a.start_line = 2;
        store
            .insert_nodes(&[
                node("file:src/a.rs", NodeKind::File, "src/a.rs"),
                import_b.clone(),
                import_a.clone(),
                node("function:src/a.rs:f:10", NodeKind::Function, "src/a.rs"),
            ])
            .unwrap();

        let imports = store.nodes_by_kind(NodeKind::Import).unwrap();

        assert_eq!(imports, vec![import_a, import_b]);
    }

    #[test]
    fn metadata_set_get_round_trip_and_overwrite() {
        let store = GraphStore::open_in_memory().unwrap();
        assert!(store.get_metadata("git_commit").unwrap().is_none());

        store.set_metadata("git_commit", "abc123").unwrap();
        assert_eq!(
            store.get_metadata("git_commit").unwrap(),
            Some("abc123".to_string())
        );

        // Overwriting the same key updates the value in place.
        store.set_metadata("git_commit", "def456").unwrap();
        assert_eq!(
            store.get_metadata("git_commit").unwrap(),
            Some("def456".to_string())
        );

        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM project_metadata", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "overwrite must not create a duplicate row");
    }

    #[test]
    fn upsert_file_replaces_existing_record() {
        let store = GraphStore::open_in_memory().unwrap();
        store.upsert_file(&file_record("src/x.rs", "h1")).unwrap();
        store.upsert_file(&file_record("src/x.rs", "h2")).unwrap();

        let (count, hash): (i64, String) = store
            .conn
            .query_row(
                "SELECT COUNT(*), MAX(content_hash) FROM files WHERE path = 'src/x.rs'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "upsert must not duplicate the path");
        assert_eq!(hash, "h2", "content_hash should be the latest value");
    }

    #[test]
    fn batch_insert_nodes_in_single_transaction() {
        let store = GraphStore::open_in_memory().unwrap();
        let nodes: Vec<Node> = (0..500)
            .map(|i| {
                node(
                    &format!("function:src/lib.rs:f{i}:{i}"),
                    NodeKind::Function,
                    "src/lib.rs",
                )
            })
            .collect();

        store.insert_nodes(&nodes).unwrap();

        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 500);
        // Spot-check a couple of round-trips.
        assert!(store
            .node_by_id("function:src/lib.rs:f0:0")
            .unwrap()
            .is_some());
        assert!(store
            .node_by_id("function:src/lib.rs:f499:499")
            .unwrap()
            .is_some());
    }

    #[test]
    fn empty_batch_inserts_are_noops() {
        let store = GraphStore::open_in_memory().unwrap();
        store.insert_nodes(&[]).unwrap();
        store.insert_edges(&[]).unwrap();
        let nodes: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!((nodes, edges), (0, 0));
    }

    // -----------------------------------------------------------------------
    // FTS5 search (search_fts)
    // -----------------------------------------------------------------------

    /// Build a function node carrying a docstring, for full-text search tests.
    fn doc_node(id: &str, name: &str, file_path: &str, docstring: &str) -> Node {
        let mut n = node(id, NodeKind::Function, file_path);
        n.name = name.to_string();
        n.qualified_name = format!("crate::{name}");
        n.docstring = Some(docstring.to_string());
        n
    }

    #[test]
    fn search_fts_matches_by_name() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[
                doc_node("f:1", "parse_config", "src/cfg.rs", "Parses the config."),
                doc_node("f:2", "render_view", "src/ui.rs", "Renders a view."),
            ])
            .unwrap();

        let hits = store.search_fts("parse_config", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "parse_config");
    }

    #[test]
    fn search_fts_matches_by_docstring() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[
                doc_node("f:1", "alpha", "src/a.rs", "Reticulates the splines."),
                doc_node("f:2", "beta", "src/b.rs", "Frobnicates a widget."),
            ])
            .unwrap();

        let hits = store.search_fts("splines", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "alpha");
    }

    #[test]
    fn search_fts_filters_by_kind() {
        let store = GraphStore::open_in_memory().unwrap();
        let mut struct_node = node("s:1", NodeKind::Struct, "src/m.rs");
        struct_node.name = "Widget".into();
        struct_node.qualified_name = "crate::Widget".into();
        let mut fn_node = node("f:1", NodeKind::Function, "src/m.rs");
        fn_node.name = "Widget".into();
        fn_node.qualified_name = "crate::make_Widget".into();
        store.insert_nodes(&[struct_node, fn_node]).unwrap();

        let structs = store
            .search_fts("Widget", Some(NodeKind::Struct), 10)
            .unwrap();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].kind, NodeKind::Struct);

        let funcs = store
            .search_fts("Widget", Some(NodeKind::Function), 10)
            .unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].kind, NodeKind::Function);
    }

    #[test]
    fn search_fts_respects_limit() {
        let store = GraphStore::open_in_memory().unwrap();
        let nodes: Vec<Node> = (0..5)
            .map(|i| {
                doc_node(
                    &format!("f:{i}"),
                    &format!("handler{i}"),
                    "src/lib.rs",
                    "A request handler.",
                )
            })
            .collect();
        store.insert_nodes(&nodes).unwrap();

        // All five share the "handler" docstring term.
        let limited = store.search_fts("handler", None, 2).unwrap();
        assert_eq!(limited.len(), 2);

        let unlimited = store.search_fts("handler", None, 0).unwrap();
        assert_eq!(unlimited.len(), 5);
    }

    #[test]
    fn search_fts_special_characters_do_not_error() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[doc_node(
                "f:1",
                "parse_config",
                "src/cfg.rs",
                "Parses the config.",
            )])
            .unwrap();

        // Each of these would be a syntax error if passed raw into FTS5 MATCH.
        for raw in [
            "parse_config\"",
            "\"unbalanced",
            "parse* OR config",
            "NEAR(foo bar)",
            "a AND b OR (c",
            "::crate::parse_config::",
            "^weird:token-",
            "***",
            "",
            "   ",
        ] {
            let result = store.search_fts(raw, None, 10);
            assert!(result.is_ok(), "query {raw:?} must not raise a SQL error");
        }

        // A query made only of separators yields an empty (not errored) result.
        assert!(store.search_fts("***", None, 10).unwrap().is_empty());
        // A real token embedded in junk still matches.
        assert_eq!(
            store.search_fts("parse_config\"", None, 10).unwrap().len(),
            1
        );
    }
}
