//! Query layer: `QueryManager` and graph traversal powering
//! explore/callers/callees/impact/node/search.
//!
//! The store-facing graph traversal lives in [`traverser`]: [`Traverser`]
//! walks the `calls` edges to answer callers/callees/impact queries. The
//! higher-level [`explore`] module anchors symbol names and builds a compact
//! call-flow/source bundle for AI exploration.

use std::path::{Path, PathBuf};

use deepagent_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::store::GraphStore;
use crate::types::{Edge, EdgeKind, Node, NodeKind};

#[path = "query/explore.rs"]
pub mod explore;

#[path = "query/traverser.rs"]
pub mod traverser;

pub use explore::{ExploreBudget, ExploreResult, Explorer, FileGroup, FlowHop, SymbolSource};
pub use traverser::{CallSite, Direction, ImpactResult, Traverser};

/// Full-text search hit returned by [`QueryManager::search`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeHit {
    pub node: Node,
}

/// Detailed node view used by `codegraph_node`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetail {
    pub node: Node,
    pub source: String,
    pub callers: Vec<CallSite>,
    pub callees: Vec<CallSite>,
    pub outgoing_imports: Vec<Edge>,
    pub incoming_imports: Vec<Edge>,
}

/// Query facade over a [`GraphStore`].
pub struct QueryManager<'a> {
    store: &'a GraphStore,
    project_root: PathBuf,
}

impl<'a> QueryManager<'a> {
    /// Create a query manager. `project_root` is used by `explore` to read
    /// source snippets.
    pub fn new(store: &'a GraphStore, project_root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            project_root: project_root.into(),
        }
    }

    /// Borrowed project root used for source reads.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Search the FTS5 index and wrap nodes as query hits.
    pub fn search(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: usize,
    ) -> Result<Vec<NodeHit>> {
        Ok(self
            .store
            .search_fts(query, kind, limit)?
            .into_iter()
            .map(|node| NodeHit { node })
            .collect())
    }

    /// Explore a set of symbols using exact/FTS location and calls traversal.
    pub fn explore(&self, symbols: &[String], budget: ExploreBudget) -> Result<ExploreResult> {
        Explorer::new(self.store, &self.project_root).explore(symbols, budget)
    }

    /// Direct callers of `symbol`, where `symbol` may be a node id, qualified
    /// name, or bare name.
    pub fn callers(&self, symbol: &str, limit: usize) -> Result<Vec<CallSite>> {
        let Some(node) = self.resolve_symbol(symbol)? else {
            return Ok(Vec::new());
        };
        Traverser::new(self.store).callers(&node.id, limit)
    }

    /// Direct callees of `symbol`, where `symbol` may be a node id, qualified
    /// name, or bare name.
    pub fn callees(&self, symbol: &str, limit: usize) -> Result<Vec<CallSite>> {
        let Some(node) = self.resolve_symbol(symbol)? else {
            return Ok(Vec::new());
        };
        Traverser::new(self.store).callees(&node.id, limit)
    }

    /// Change-impact radius of `symbol`, following reverse `calls` edges.
    pub fn impact(&self, symbol: &str, depth: usize) -> Result<Option<ImpactResult>> {
        let Some(node) = self.resolve_symbol(symbol)? else {
            return Ok(None);
        };
        Traverser::new(self.store).impact(&node.id, depth)
    }

    /// Node details plus local call/import context.
    pub fn node(&self, target: &str) -> Result<Option<NodeDetail>> {
        let Some(node) = self.resolve_symbol(target)? else {
            return Ok(None);
        };
        let traverser = Traverser::new(self.store);
        Ok(Some(NodeDetail {
            source: source_for_node(&self.project_root, &node),
            callers: traverser.callers(&node.id, 0)?,
            callees: traverser.callees(&node.id, 0)?,
            outgoing_imports: self.store.edges_from(&node.id, EdgeKind::Imports)?,
            incoming_imports: self.store.edges_to(&node.id, EdgeKind::Imports)?,
            node,
        }))
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<Option<Node>> {
        if symbol.trim().is_empty() {
            return Ok(None);
        }
        if let Some(node) = self.store.node_by_id(symbol)? {
            return Ok(Some(node));
        }
        if let Some(node) = self.store.nodes_by_name(symbol)?.into_iter().next() {
            return Ok(Some(node));
        }
        Ok(self.store.search_fts(symbol, None, 1)?.into_iter().next())
    }
}

fn source_for_node(project_root: &Path, node: &Node) -> String {
    let path = project_root.join(node.file_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let Ok(source) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let start = node.start_line.max(1) as usize;
    let end = node.end_line.max(node.start_line).max(1) as usize;
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_no = index + 1;
            (line_no >= start && line_no <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, Language};

    #[test]
    fn search_returns_node_hits() {
        let fixture = Fixture::new();
        fixture
            .store
            .insert_nodes(&[func("f:parse", "parse_config", "src/lib.rs", 1, 1)])
            .unwrap();

        let hits = fixture.manager().search("parse_config", None, 10).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node.id, "f:parse");
    }

    #[test]
    fn callers_and_callees_resolve_by_name() {
        let fixture = Fixture::new();
        fixture
            .store
            .insert_nodes(&[
                func("f:handler", "handler", "src/lib.rs", 1, 3),
                func("f:service", "service", "src/lib.rs", 5, 7),
            ])
            .unwrap();
        fixture
            .store
            .insert_edges(&[calls("f:handler", "f:service", 2)])
            .unwrap();

        let callers = fixture.manager().callers("service", 10).unwrap();
        let callees = fixture.manager().callees("handler", 10).unwrap();

        assert_eq!(callers[0].node.id, "f:handler");
        assert_eq!(callees[0].node.id, "f:service");
    }

    #[test]
    fn impact_and_node_detail_resolve_by_id() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "fn handler() {\n    service();\n}\n\nfn service() {}\n",
        )
        .unwrap();
        fixture
            .store
            .insert_nodes(&[
                func("f:handler", "handler", "src/lib.rs", 1, 3),
                func("f:service", "service", "src/lib.rs", 5, 7),
            ])
            .unwrap();
        fixture
            .store
            .insert_edges(&[calls("f:handler", "f:service", 2)])
            .unwrap();

        let impact = fixture
            .manager()
            .impact("f:service", 2)
            .unwrap()
            .expect("service exists");
        let detail = fixture
            .manager()
            .node("f:handler")
            .unwrap()
            .expect("handler exists");

        assert_eq!(impact.direct[0].id, "f:handler");
        assert_eq!(detail.callees[0].node.id, "f:service");
        assert_eq!(detail.source, "fn handler() {\n    service();\n}");
    }

    fn func(id: &str, name: &str, file_path: &str, start_line: u32, end_line: u32) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: format!("crate::{name}"),
            file_path: file_path.to_string(),
            language: Language::Rust,
            start_line,
            end_line,
            start_column: 0,
            end_column: 0,
            signature: Some(format!("fn {name}()")),
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn calls(source: &str, target: &str, line: u32) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind: EdgeKind::Calls,
            metadata: None,
            line: Some(line),
            provenance: None,
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        store: GraphStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                store: GraphStore::open_in_memory().unwrap(),
            }
        }

        fn manager(&self) -> QueryManager<'_> {
            QueryManager::new(&self.store, self.dir.path())
        }
    }
}
