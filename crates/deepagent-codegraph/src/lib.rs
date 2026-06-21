//! # deepagent-codegraph
//!
//! Native Rust code-graph engine for DeepAgent Studio.
//!
//! It parses a project's source with tree-sitter, builds a symbol-level
//! knowledge graph in SQLite (with an FTS5 full-text index), and serves two
//! consumers from a single extraction:
//!
//! - **AI consumer**: `codegraph_*` tools query SQLite directly for precise,
//!   structured answers (symbol source + call chains).
//! - **Human consumer**: a projector down-projects the rich graph into the
//!   existing `.understand-anything/knowledge-graph.json` for the front-end
//!   project-map panel.
//!
//! The same persisted graph powers both experiences, so indexing work is done
//! once and reused across the UI, AI tools, and future watcher-triggered syncs.
//! The crate currently ships precise tree-sitter extractors for Rust,
//! TypeScript, JavaScript, Python, and Go, plus a broad heuristic extractor for
//! additional source languages until dedicated grammars are available.

pub mod error_locator;
pub mod extraction;
pub mod projection;
pub mod query;
pub mod resolution;
pub mod scanner;
pub mod store;
pub mod types;
pub mod watcher;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};

use crate::extraction::{ExtractedFile, Extractor};
use crate::projection::{ProjectionStats, Projector};
use crate::query::{
    CallSite, ExploreBudget, ExploreResult, ImpactResult, NodeDetail, NodeHit, QueryManager,
};
use crate::resolution::import_resolver::ImportResolver;
use crate::resolution::Resolver;
use crate::store::GraphStore;
use crate::types::{Edge, FileRecord, Node, NodeKind, UnresolvedRef};

pub use scanner::{FileScanner, ScannedFile};

/// Directory (relative to the project root) holding the code-graph database and
/// the projected UA `knowledge-graph.json`. Keeping both artifacts together
/// also means the existing stale-check skip-list (which already ignores
/// `.understand-anything`) never treats the database as a project change.
const CODEGRAPH_DIR: &str = ".understand-anything";
/// SQLite database file name inside [`CODEGRAPH_DIR`].
const CODEGRAPH_DB: &str = "codegraph.db";
/// `project_metadata` key holding the git HEAD commit recorded at index time.
const GIT_COMMIT_KEY: &str = "git_commit_hash";
/// `project_metadata` key holding the most recent incremental change set
/// (JSON array of POSIX relative paths). Recorded by [`CodeGraph::sync`].
const CHANGED_FILES_KEY: &str = "last_changed_files";

/// Statistics describing one indexing run (full or incremental).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStats {
    /// Number of files scanned and processed in this run.
    pub files_indexed: usize,
    /// Total nodes in the graph after the run.
    pub nodes: usize,
    /// Total edges in the graph after the run.
    pub edges: usize,
    /// Wall-clock duration of the run.
    pub duration: Duration,
    /// `true` when produced by [`CodeGraph::sync`] (incremental), `false` for a
    /// full [`CodeGraph::index_all`].
    pub is_incremental: bool,
}

/// Unified facade over the code-graph engine.
///
/// Owns a [`GraphStore`] (SQLite, opened under `.understand-anything/`) and the
/// canonical project root. It orchestrates the extraction pipeline
/// (scan -> extract -> store -> resolve imports) for both full and incremental
/// runs, and projects the rich graph into the UA `knowledge-graph.json`
/// consumed by the existing front-end project-map panel.
pub struct CodeGraph {
    store: GraphStore,
    project_root: PathBuf,
}

impl std::fmt::Debug for CodeGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeGraph")
            .field("project_root", &self.project_root)
            .finish_non_exhaustive()
    }
}

impl CodeGraph {
    /// Open (creating if necessary) the code-graph database for `project_root`.
    ///
    /// The root is canonicalised; the database lives at
    /// `<root>/.understand-anything/codegraph.db`. The containing directory is
    /// created if missing.
    pub fn open(project_root: &Path) -> Result<Self> {
        let root = project_root.canonicalize().map_err(|e| {
            CoreError::invalid(format!(
                "cannot resolve project root {}: {e}",
                project_root.display()
            ))
        })?;
        let db_dir = root.join(CODEGRAPH_DIR);
        std::fs::create_dir_all(&db_dir).map_err(|e| {
            CoreError::Persistence(format!(
                "failed to create code-graph directory {}: {e}",
                db_dir.display()
            ))
        })?;
        let store = GraphStore::open(&db_dir.join(CODEGRAPH_DB))?;
        Ok(Self {
            store,
            project_root: root,
        })
    }

    /// The canonical project root this graph indexes.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Borrow the underlying store (read-only access for queries/projection).
    pub fn store(&self) -> &GraphStore {
        &self.store
    }

    /// Whether the database already holds an index (any tracked files).
    ///
    /// Drives the choice between [`CodeGraph::index_all`] (first run) and
    /// [`CodeGraph::sync`] (subsequent runs). A read error is treated as "no
    /// index" so a corrupt/empty database falls back to a full rebuild.
    pub fn has_existing_index(&self) -> bool {
        self.store.file_count().map(|c| c > 0).unwrap_or(false)
    }

    /// Full index: scan the whole tree, extract every file, persist nodes/
    /// edges/files/unresolved-refs, then resolve cross-file `imports` edges.
    ///
    /// Extraction is sequential in phase one (tree-sitter `Parser` is `!Sync`);
    /// parallelism is a later performance task. Returns post-run [`IndexStats`]
    /// with `is_incremental = false`.
    pub fn index_all(&mut self) -> Result<IndexStats> {
        let start = Instant::now();

        let scanner = FileScanner::new(&self.project_root)?;
        let scanned = scanner.scan(&self.project_root)?;
        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut refs: Vec<UnresolvedRef> = Vec::new();
        let mut files: Vec<FileRecord> = Vec::new();

        for extracted in extract_files_parallel(scanned.clone())? {
            let file_record = file_record(&extracted.file_info);
            nodes.extend(extracted.nodes);
            edges.extend(extracted.edges);
            refs.extend(extracted.unresolved_refs);
            files.push(file_record);
        }

        // Persist extraction output. Nodes first so edge/ref foreign keys hold.
        self.store.insert_nodes(&nodes)?;
        self.store.insert_edges(&edges)?;
        self.store.insert_unresolved_refs(&refs)?;
        self.store.upsert_files(&files)?;

        // Resolve cross-file imports over the freshly extracted node set and
        // persist the file -> file `imports` edges.
        let import_edges = ImportResolver::default().resolve_edges(&nodes).edges;
        self.store.insert_edges(&import_edges)?;

        // Resolve cross-file calls: match parked call references against the
        // persisted callable definitions and persist heuristic `calls` edges.
        // Parked references only hold calls the extractor could not resolve
        // within a single file, so this never duplicates extraction-time edges.
        Resolver::new(&self.store).resolve_all()?;

        self.record_git_commit();
        self.store.set_metadata(CHANGED_FILES_KEY, "[]")?;

        Ok(IndexStats {
            files_indexed: scanned.len(),
            nodes: self.store.node_count()?,
            edges: self.store.edge_count()?,
            duration: start.elapsed(),
            is_incremental: false,
        })
    }

    /// Incremental sync: diff the current tree against the stored `files` table
    /// by content hash, drop graph data for deleted/modified files, re-extract
    /// added/modified files, and re-resolve imports for the changed files.
    ///
    /// The changed-file set is recorded in project metadata (best-effort change
    /// marking). Returns post-run [`IndexStats`] with `is_incremental = true`.
    pub fn sync(&mut self) -> Result<IndexStats> {
        let start = Instant::now();

        let scanner = FileScanner::new(&self.project_root)?;
        let scanned = scanner.scan(&self.project_root)?;
        let change = self.store.changed_files(&scanned);

        // Remove graph data for files that vanished or changed; the file node's
        // FK cascade also clears its outgoing edges and unresolved refs.
        for path in change.deleted.iter().chain(change.modified.iter()) {
            self.store.delete_file_cascade(path)?;
        }

        // Re-extract added + modified files.
        let touched: std::collections::BTreeSet<&String> =
            change.added.iter().chain(change.modified.iter()).collect();
        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut refs: Vec<UnresolvedRef> = Vec::new();
        let mut files: Vec<FileRecord> = Vec::new();
        let mut to_extract: Vec<ScannedFile> = Vec::new();

        for file in &scanned {
            let rel = posix_relative(file);
            if !touched.contains(&rel) {
                continue;
            }
            to_extract.push(file.clone());
        }

        for extracted in extract_files_parallel(to_extract)? {
            let file_record = file_record(&extracted.file_info);
            nodes.extend(extracted.nodes);
            edges.extend(extracted.edges);
            refs.extend(extracted.unresolved_refs);
            files.push(file_record);
        }

        self.store.insert_nodes(&nodes)?;
        self.store.insert_edges(&edges)?;
        self.store.insert_unresolved_refs(&refs)?;
        self.store.upsert_files(&files)?;

        // Local import resolution: resolve over the full current node set (so
        // targets in unchanged files are visible) but only persist edges whose
        // source is a changed file. Changed files had their old edges cascade-
        // deleted above, so no duplicates are introduced.
        if !nodes.is_empty() {
            let changed_sources: std::collections::BTreeSet<String> = nodes
                .iter()
                .filter(|n| n.kind == NodeKind::File)
                .map(|n| n.id.clone())
                .collect();
            let all_nodes = self.store.all_nodes()?;
            let import_edges: Vec<Edge> = ImportResolver::default()
                .resolve_edges(&all_nodes)
                .edges
                .into_iter()
                .filter(|e| changed_sources.contains(&e.source))
                .collect();
            self.store.insert_edges(&import_edges)?;
        }

        // Local call resolution: re-resolve only the call references that live
        // in the changed files. Definition candidates still come from the whole
        // graph (so calls into unchanged files resolve), and the changed files'
        // old `calls` edges were cascade-deleted above, so no duplicates arise.
        let changed_files: Vec<String> = touched.iter().map(|s| (*s).clone()).collect();
        Resolver::new(&self.store).resolve_for_files(&changed_files)?;

        self.record_git_commit();
        self.record_changed_files(&change);

        Ok(IndexStats {
            files_indexed: touched.len(),
            nodes: self.store.node_count()?,
            edges: self.store.edge_count()?,
            duration: start.elapsed(),
            is_incremental: true,
        })
    }

    /// Project the rich graph into the UA `knowledge-graph.json` at `out_path`.
    pub fn project_ua_json(&self, out_path: &Path) -> Result<ProjectionStats> {
        Projector::new(&self.store).project(&self.project_root, out_path)
    }

    /// Search indexed symbols through the query manager.
    pub fn search(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: usize,
    ) -> Result<Vec<NodeHit>> {
        self.query_manager().search(query, kind, limit)
    }

    /// Explore named symbols and related call-flow/source context.
    pub fn explore(&self, symbols: &[String], budget: ExploreBudget) -> Result<ExploreResult> {
        self.query_manager().explore(symbols, budget)
    }

    /// Direct callers of a node id, qualified name, or bare symbol name.
    pub fn callers(&self, symbol: &str, limit: usize) -> Result<Vec<CallSite>> {
        self.query_manager().callers(symbol, limit)
    }

    /// Direct callees of a node id, qualified name, or bare symbol name.
    pub fn callees(&self, symbol: &str, limit: usize) -> Result<Vec<CallSite>> {
        self.query_manager().callees(symbol, limit)
    }

    /// Change-impact radius for a node id, qualified name, or bare symbol name.
    pub fn impact(&self, symbol: &str, depth: usize) -> Result<Option<ImpactResult>> {
        self.query_manager().impact(symbol, depth)
    }

    /// Detail view for a node id, qualified name, or bare symbol name.
    pub fn node(&self, target: &str) -> Result<Option<NodeDetail>> {
        self.query_manager().node(target)
    }

    fn query_manager(&self) -> QueryManager<'_> {
        QueryManager::new(&self.store, &self.project_root)
    }

    /// Record the current git HEAD commit in project metadata, if available.
    ///
    /// Gracefully degrades when git is missing or the project is not a
    /// repository: the metadata key is simply left unset.
    fn record_git_commit(&self) {
        if let Some(hash) = git_head_commit(&self.project_root) {
            if let Err(err) = self.store.set_metadata(GIT_COMMIT_KEY, &hash) {
                tracing::warn!(error = %err, "failed to record git commit hash");
            }
        }
    }

    /// Record the incremental change set (added/modified/deleted POSIX paths)
    /// in project metadata. Best-effort: a serialisation/store failure is
    /// logged, not propagated.
    fn record_changed_files(&self, change: &crate::store::ChangeSet) {
        let mut changed: Vec<&String> = Vec::new();
        changed.extend(change.added.iter());
        changed.extend(change.modified.iter());
        changed.extend(change.deleted.iter());
        match serde_json::to_string(&changed) {
            Ok(json) => {
                if let Err(err) = self.store.set_metadata(CHANGED_FILES_KEY, &json) {
                    tracing::warn!(error = %err, "failed to record changed files");
                }
            }
            Err(err) => tracing::warn!(error = %err, "failed to serialise changed files"),
        }
    }
}

/// Read a source file as UTF-8, lossily decoding invalid bytes.
///
/// The scanner already filters binary extensions and oversized files, so this
/// rarely needs the lossy path; on a read error it returns an empty string so
/// the file still gets registered as a `file` node.
fn read_source(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to read file; indexing as empty");
            String::new()
        }
    }
}

/// Extract files in parallel while keeping SQLite writes centralised.
fn extract_files_parallel(files: Vec<ScannedFile>) -> Result<Vec<ExtractedFile>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, files.len());
    let chunk_size = files.len().div_ceil(workers);
    let indexed: Vec<(usize, ScannedFile)> = files.into_iter().enumerate().collect();

    let mut extracted: Vec<(usize, ExtractedFile)> = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in indexed.chunks(chunk_size) {
            let chunk = chunk.to_vec();
            handles.push(scope.spawn(move || -> Result<Vec<(usize, ExtractedFile)>> {
                let extractor = Extractor::new();
                let mut out = Vec::with_capacity(chunk.len());
                for (index, file) in chunk {
                    let source = read_source(&file.path);
                    out.push((index, extractor.extract(&file, &source)?));
                }
                Ok(out)
            }));
        }

        for handle in handles {
            let mut batch = handle
                .join()
                .map_err(|_| CoreError::other("extractor worker panicked"))??;
            extracted.append(&mut batch);
        }
        Ok::<(), CoreError>(())
    })?;

    extracted.sort_by_key(|(index, _)| *index);
    Ok(extracted.into_iter().map(|(_, file)| file).collect())
}

/// The POSIX-style relative path string for a scanned file.
fn posix_relative(file: &ScannedFile) -> String {
    file.relative_path.to_string_lossy().replace('\\', "/")
}

/// Build a [`FileRecord`] for the `files` table from a scanned file.
fn file_record(file: &ScannedFile) -> FileRecord {
    let now = current_timestamp();
    let modified_at = std::fs::metadata(&file.path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(now);
    FileRecord {
        path: posix_relative(file),
        content_hash: file.content_hash.clone(),
        language: file.language,
        size: file.size,
        modified_at,
        indexed_at: now,
    }
}

/// Current wall-clock time as a Unix timestamp in seconds (0 before the epoch).
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve the git HEAD commit for `root` via `git rev-parse HEAD`.
///
/// Returns `None` when git is unavailable, the directory is not a repository,
/// or the command fails for any reason (graceful degradation).
fn git_head_commit(root: &Path) -> Option<String> {
    let mut command = Command::new("git");
    hide_command_window(&mut command);
    let output = command
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!hash.is_empty()).then_some(hash)
}

/// Suppress the console window git would otherwise flash on Windows.
#[cfg(target_os = "windows")]
fn hide_command_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// No-op on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
fn hide_command_window(_command: &mut Command) {}
