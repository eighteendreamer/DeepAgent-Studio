//! Resolution layer: import-target resolution and call-reference name matching
//! across files after full extraction.
//!
//! Two concrete pieces live here:
//!
//! - [`import_resolver`] turns language-level import symbols into cross-file
//!   `imports` edges (`file -> file`).
//! - [`name_matcher`] turns parked call [`UnresolvedRef`]s into cross-file
//!   `calls` edges by matching the referenced name against callable definition
//!   nodes.
//!
//! [`Resolver`] is the store-facing orchestrator over the latter: it reads
//! unresolved call references and definition nodes from the [`GraphStore`],
//! runs the name matcher, and persists the resulting `calls` edges. Import
//! resolution stays available through [`ImportResolver`] (driven directly by
//! the facade over the in-memory node set), so the two resolvers coexist
//! without interfering.

#[path = "resolution/import_resolver.rs"]
pub mod import_resolver;
#[path = "resolution/name_matcher.rs"]
pub mod name_matcher;

pub use import_resolver::{ImportAlias, ImportResolver, ImportResolverConfig, ResolveImportReport};
pub use name_matcher::{CallMatchReport, DefinitionIndex};

use deepagent_core::error::Result;

use crate::store::GraphStore;
use crate::types::{Node, NodeKind};

/// Summary of one call-resolution pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveStats {
    /// Number of call references that matched a definition.
    pub resolved: usize,
    /// Number of call references left unresolved (no matching definition).
    pub unresolved: usize,
}

/// Store-facing orchestrator for cross-file call resolution.
///
/// Borrows a [`GraphStore`] and resolves parked call references into heuristic
/// `calls` edges via [`name_matcher`]. Definition candidates are the callable
/// nodes (functions / methods) already persisted in the graph, so a full
/// extraction must have run first.
pub struct Resolver<'a> {
    store: &'a GraphStore,
}

impl<'a> Resolver<'a> {
    /// Construct a resolver over `store`.
    pub fn new(store: &'a GraphStore) -> Self {
        Self { store }
    }

    /// Resolve every parked call reference in the store, persisting the
    /// resulting `calls` edges.
    ///
    /// Definition candidates are all callable nodes in the graph. Unresolved
    /// references are left in place (no error edge). Resolved-reference rows
    /// are intentionally *kept* in `unresolved_refs`: `resolve_all` runs once
    /// on a fresh full index, and incremental syncs re-process only the
    /// references they re-extract, so leaving the rows is the lowest-complexity
    /// choice that never double-emits edges.
    pub fn resolve_all(&self) -> Result<ResolveStats> {
        let index = self.definition_index()?;
        let refs = self.store.all_unresolved_refs()?;
        let report = name_matcher::match_calls(&refs, &index);
        self.store.insert_edges(&report.edges)?;
        Ok(stats(&report))
    }

    /// Resolve only the parked call references whose call site lives in one of
    /// `files`, persisting the resulting `calls` edges.
    ///
    /// Used by incremental sync: definition candidates are still drawn from the
    /// whole graph (so calls into unchanged files resolve), but only references
    /// belonging to the changed files are matched. An empty `files` slice is a
    /// no-op.
    pub fn resolve_for_files(&self, files: &[String]) -> Result<ResolveStats> {
        if files.is_empty() {
            return Ok(ResolveStats::default());
        }
        let index = self.definition_index()?;
        let refs = self.store.unresolved_refs_for_files(files)?;
        let report = name_matcher::match_calls(&refs, &index);
        self.store.insert_edges(&report.edges)?;
        Ok(stats(&report))
    }

    /// Build a [`DefinitionIndex`] over every callable node in the graph.
    fn definition_index(&self) -> Result<DefinitionIndex> {
        let mut callables: Vec<Node> = self.store.nodes_by_kind(NodeKind::Function)?;
        callables.extend(self.store.nodes_by_kind(NodeKind::Method)?);
        Ok(DefinitionIndex::build(&callables))
    }
}

fn stats(report: &CallMatchReport) -> ResolveStats {
    ResolveStats {
        resolved: report.resolved,
        unresolved: report.unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::file_node_id;
    use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

    fn function(name: &str, qualified: &str, file: &str, line: u32) -> Node {
        Node {
            id: format!("function:{file}:{qualified}:{line}"),
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            file_path: file.to_string(),
            language: Language::Rust,
            start_line: line,
            end_line: line + 3,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn file_node(file: &str) -> Node {
        Node {
            id: file_node_id(file),
            kind: NodeKind::File,
            name: file.rsplit('/').next().unwrap_or(file).to_string(),
            qualified_name: file.to_string(),
            file_path: file.to_string(),
            language: Language::Rust,
            start_line: 1,
            end_line: 1,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn call_ref(from: &str, name: &str, file: &str, line: u32) -> UnresolvedRef {
        UnresolvedRef {
            from_node_id: from.to_string(),
            reference_name: name.to_string(),
            reference_kind: name_matcher::CALL_REFERENCE_KIND.to_string(),
            line,
            file_path: file.to_string(),
        }
    }

    fn calls_edges(store: &GraphStore, source: &str) -> Vec<Edge> {
        store.edges_from(source, EdgeKind::Calls).unwrap()
    }

    #[test]
    fn resolve_all_connects_cross_file_call() {
        let store = GraphStore::open_in_memory().unwrap();
        let caller = function("driver", "driver", "src/a.rs", 1);
        let callee = function("compute", "compute", "src/b.rs", 2);
        store
            .insert_nodes(&[
                file_node("src/a.rs"),
                file_node("src/b.rs"),
                caller.clone(),
                callee.clone(),
            ])
            .unwrap();
        store
            .insert_unresolved_refs(&[call_ref(&caller.id, "compute", "src/a.rs", 4)])
            .unwrap();

        let stats = Resolver::new(&store).resolve_all().unwrap();

        assert_eq!(stats.resolved, 1);
        assert_eq!(stats.unresolved, 0);
        let edges = calls_edges(&store, &caller.id);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, callee.id);
    }

    #[test]
    fn resolve_all_keeps_unresolved_reference_without_error() {
        let store = GraphStore::open_in_memory().unwrap();
        let caller = function("driver", "driver", "src/a.rs", 1);
        store
            .insert_nodes(&[file_node("src/a.rs"), caller.clone()])
            .unwrap();
        store
            .insert_unresolved_refs(&[call_ref(&caller.id, "nowhere", "src/a.rs", 4)])
            .unwrap();

        let stats = Resolver::new(&store).resolve_all().unwrap();

        assert_eq!(stats.resolved, 0);
        assert_eq!(stats.unresolved, 1);
        assert!(calls_edges(&store, &caller.id).is_empty());
        // The parked reference is still present.
        assert_eq!(store.all_unresolved_refs().unwrap().len(), 1);
    }

    #[test]
    fn resolve_for_files_only_processes_listed_files() {
        let store = GraphStore::open_in_memory().unwrap();
        let caller_a = function("driver_a", "driver_a", "src/a.rs", 1);
        let caller_b = function("driver_b", "driver_b", "src/b.rs", 1);
        let target = function("shared", "shared", "src/lib.rs", 2);
        store
            .insert_nodes(&[
                file_node("src/a.rs"),
                file_node("src/b.rs"),
                file_node("src/lib.rs"),
                caller_a.clone(),
                caller_b.clone(),
                target.clone(),
            ])
            .unwrap();
        store
            .insert_unresolved_refs(&[
                call_ref(&caller_a.id, "shared", "src/a.rs", 3),
                call_ref(&caller_b.id, "shared", "src/b.rs", 3),
            ])
            .unwrap();

        let stats = Resolver::new(&store)
            .resolve_for_files(&["src/a.rs".to_string()])
            .unwrap();

        assert_eq!(stats.resolved, 1);
        assert_eq!(calls_edges(&store, &caller_a.id).len(), 1);
        // The call in the file we did not ask for stays unresolved.
        assert!(calls_edges(&store, &caller_b.id).is_empty());
    }

    #[test]
    fn resolve_for_files_empty_is_noop() {
        let store = GraphStore::open_in_memory().unwrap();
        let stats = Resolver::new(&store).resolve_for_files(&[]).unwrap();
        assert_eq!(stats, ResolveStats::default());
    }
}
