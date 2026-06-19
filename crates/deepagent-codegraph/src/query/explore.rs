//! Symbol exploration query.
//!
//! `Explorer` turns one or more user-provided symbol names into a compact
//! answer bundle:
//!
//! 1. anchor symbols with exact name lookup, falling back to FTS5;
//! 2. bridge anchors through the `calls` subgraph with a bounded BFS;
//! 3. read source snippets for the discovered symbols;
//! 4. group snippets by file and apply an output budget.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::store::GraphStore;
use crate::types::{EdgeKind, Node};

/// Output and search limits for [`Explorer::explore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExploreBudget {
    /// Maximum number of anchor candidates per input symbol (`0` = unlimited).
    pub max_hits_per_symbol: usize,
    /// Maximum number of call hops used to bridge anchors.
    pub max_bridge_hops: usize,
    /// Maximum number of file groups returned (`0` = unlimited).
    pub max_files: usize,
    /// Maximum number of symbol snippets returned per file (`0` = unlimited).
    pub max_symbols_per_file: usize,
    /// Maximum number of flow hops returned (`0` = unlimited).
    pub max_flow_hops: usize,
}

impl Default for ExploreBudget {
    fn default() -> Self {
        Self {
            max_hits_per_symbol: 5,
            max_bridge_hops: 3,
            max_files: 8,
            max_symbols_per_file: 8,
            max_flow_hops: 32,
        }
    }
}

/// A call-flow hop included in an exploration result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowHop {
    pub from_node_id: String,
    pub to_node_id: String,
    pub line: u32,
}

/// A symbol plus the source text sliced from its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSource {
    pub node_id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub source: String,
}

/// Source snippets grouped by file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileGroup {
    pub file_path: String,
    pub symbols: Vec<SymbolSource>,
    pub truncated: bool,
}

/// Result of an exploration query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResult {
    /// Input symbol names.
    pub query: Vec<String>,
    /// Concrete graph nodes selected as anchors.
    pub anchors: Vec<Node>,
    /// Bounded call-flow hops connecting or expanding from anchors.
    pub flow: Vec<FlowHop>,
    /// Source snippets grouped by file.
    pub files: Vec<FileGroup>,
    /// True when file, symbol, or flow budgets removed data from the result.
    pub truncated: bool,
}

/// High-level exploration query over a [`GraphStore`].
pub struct Explorer<'a> {
    store: &'a GraphStore,
    project_root: PathBuf,
}

impl<'a> Explorer<'a> {
    /// Build an explorer rooted at `project_root` for reading source files.
    pub fn new(store: &'a GraphStore, project_root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            project_root: project_root.into(),
        }
    }

    /// Explore one or more symbol names.
    ///
    /// Missing names produce an empty result rather than an error. Exact
    /// `qualified_name` / `name` matches are preferred; FTS5 search is used
    /// only when exact lookup finds nothing.
    pub fn explore(
        &self,
        symbols: &[impl AsRef<str>],
        budget: ExploreBudget,
    ) -> Result<ExploreResult> {
        let query: Vec<String> = symbols
            .iter()
            .map(|symbol| symbol.as_ref().trim().to_string())
            .filter(|symbol| !symbol.is_empty())
            .collect();

        let anchors = self.locate_anchors(&query, budget.max_hits_per_symbol)?;
        let (flow, flow_truncated) = self.bridge_call_flow(&anchors, budget)?;
        let mut source_nodes = anchors.clone();
        for hop in &flow {
            self.push_node_if_missing(&mut source_nodes, &hop.from_node_id)?;
            self.push_node_if_missing(&mut source_nodes, &hop.to_node_id)?;
        }

        let (files, files_truncated) = self.group_sources(&source_nodes, budget)?;

        Ok(ExploreResult {
            query,
            anchors,
            flow,
            files,
            truncated: flow_truncated || files_truncated,
        })
    }

    fn locate_anchors(&self, names: &[String], limit: usize) -> Result<Vec<Node>> {
        let mut anchors = Vec::new();
        let mut seen = BTreeSet::new();
        for name in names {
            let mut hits = self.store.nodes_by_name(name)?;
            if hits.is_empty() {
                hits = self.store.search_fts(name, None, limit)?;
            }
            for node in hits.into_iter().take(limit_or_all(limit)) {
                if seen.insert(node.id.clone()) {
                    anchors.push(node);
                }
            }
        }
        Ok(anchors)
    }

    fn bridge_call_flow(
        &self,
        anchors: &[Node],
        budget: ExploreBudget,
    ) -> Result<(Vec<FlowHop>, bool)> {
        if anchors.is_empty() || budget.max_bridge_hops == 0 {
            return Ok((Vec::new(), false));
        }

        let anchor_ids: BTreeSet<String> = anchors.iter().map(|node| node.id.clone()).collect();
        let mut collected = Vec::new();
        let mut seen_hops = BTreeSet::new();
        let mut truncated = false;

        for anchor in anchors {
            let mut queue = VecDeque::from([(anchor.id.clone(), 0usize)]);
            let mut visited = BTreeSet::from([anchor.id.clone()]);
            while let Some((node_id, depth)) = queue.pop_front() {
                if depth >= budget.max_bridge_hops {
                    continue;
                }
                let edges = self.store.edges_from(&node_id, EdgeKind::Calls)?;
                for edge in edges {
                    let target = edge.target.clone();
                    let hop = FlowHop {
                        from_node_id: edge.source.clone(),
                        to_node_id: target.clone(),
                        line: edge.line.unwrap_or(0),
                    };
                    if seen_hops.insert((
                        hop.from_node_id.clone(),
                        hop.to_node_id.clone(),
                        hop.line,
                    )) {
                        if budget.max_flow_hops != 0 && collected.len() >= budget.max_flow_hops {
                            truncated = true;
                            return Ok((collected, truncated));
                        }
                        collected.push(hop);
                    }

                    if anchor_ids.contains(&target) {
                        continue;
                    }
                    if visited.insert(target.clone()) {
                        queue.push_back((target, depth + 1));
                    }
                }
            }
        }

        Ok((collected, truncated))
    }

    fn group_sources(
        &self,
        nodes: &[Node],
        budget: ExploreBudget,
    ) -> Result<(Vec<FileGroup>, bool)> {
        let mut by_file: BTreeMap<String, Vec<Node>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for node in nodes {
            if seen.insert(node.id.clone()) {
                by_file
                    .entry(node.file_path.clone())
                    .or_default()
                    .push(node.clone());
            }
        }

        let mut groups = Vec::new();
        let mut truncated = false;
        for (file_path, mut file_nodes) in by_file {
            if budget.max_files != 0 && groups.len() >= budget.max_files {
                truncated = true;
                break;
            }
            file_nodes.sort_by_key(|node| (node.start_line, node.end_line, node.id.clone()));
            let source = read_source_file(&self.project_root, &file_path)?;
            let mut symbols = Vec::new();
            let mut file_truncated = false;
            for node in file_nodes {
                if budget.max_symbols_per_file != 0 && symbols.len() >= budget.max_symbols_per_file
                {
                    truncated = true;
                    file_truncated = true;
                    break;
                }
                symbols.push(symbol_source(&node, &source));
            }
            groups.push(FileGroup {
                file_path,
                symbols,
                truncated: file_truncated,
            });
        }

        Ok((groups, truncated))
    }

    fn push_node_if_missing(&self, nodes: &mut Vec<Node>, node_id: &str) -> Result<()> {
        if nodes.iter().any(|node| node.id == node_id) {
            return Ok(());
        }
        if let Some(node) = self.store.node_by_id(node_id)? {
            nodes.push(node);
        }
        Ok(())
    }
}

fn read_source_file(project_root: &Path, file_path: &str) -> Result<String> {
    let path = project_root.join(file_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    match fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(CoreError::Persistence(err.to_string())),
    }
}

fn symbol_source(node: &Node, file_source: &str) -> SymbolSource {
    SymbolSource {
        node_id: node.id.clone(),
        name: node.name.clone(),
        qualified_name: node.qualified_name.clone(),
        kind: node.kind.as_str().to_string(),
        start_line: node.start_line,
        end_line: node.end_line,
        source: slice_lines(file_source, node.start_line, node.end_line),
    }
}

fn slice_lines(source: &str, start_line: u32, end_line: u32) -> String {
    if source.is_empty() {
        return String::new();
    }
    let start = start_line.max(1) as usize;
    let end = end_line.max(start_line).max(1) as usize;
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

fn limit_or_all(limit: usize) -> usize {
    if limit == 0 {
        usize::MAX
    } else {
        limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, Language, NodeKind};

    #[test]
    fn explore_uses_fts_when_exact_lookup_misses() {
        let fixture = Fixture::new();
        fixture
            .write("src/lib.rs", "fn parse_config() {}\n")
            .unwrap();
        let node = func("f:parse", "parse_config", "src/lib.rs", 1, 1);
        fixture.store.insert_nodes(&[node]).unwrap();

        let result = fixture
            .explorer()
            .explore(&["config"], ExploreBudget::default())
            .unwrap();

        assert_eq!(ids(&result.anchors), vec!["f:parse"]);
        assert_eq!(result.files[0].symbols[0].source, "fn parse_config() {}");
    }

    #[test]
    fn explore_bridges_symbols_through_calls() {
        let fixture = Fixture::new();
        fixture
            .write(
                "src/lib.rs",
                "fn handler() {\n    service();\n}\nfn service() {\n    save();\n}\nfn save() {}\n",
            )
            .unwrap();
        let handler = func("f:handler", "handler", "src/lib.rs", 1, 3);
        let service = func("f:service", "service", "src/lib.rs", 4, 6);
        let save = func("f:save", "save", "src/lib.rs", 7, 7);
        fixture
            .store
            .insert_nodes(&[handler, service, save])
            .unwrap();
        fixture
            .store
            .insert_edges(&[
                calls("f:handler", "f:service", 2),
                calls("f:service", "f:save", 5),
            ])
            .unwrap();

        let result = fixture
            .explorer()
            .explore(&["handler", "save"], ExploreBudget::default())
            .unwrap();

        assert_eq!(
            result
                .flow
                .iter()
                .map(|hop| (hop.from_node_id.as_str(), hop.to_node_id.as_str()))
                .collect::<Vec<_>>(),
            vec![("f:handler", "f:service"), ("f:service", "f:save")]
        );
        assert_eq!(
            result.files[0]
                .symbols
                .iter()
                .map(|symbol| symbol.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["f:handler", "f:service", "f:save"]
        );
    }

    #[test]
    fn explore_groups_source_by_file() {
        let fixture = Fixture::new();
        fixture.write("src/a.rs", "fn a() {}\n").unwrap();
        fixture.write("src/b.rs", "fn b() {}\n").unwrap();
        fixture
            .store
            .insert_nodes(&[
                func("f:a", "a", "src/a.rs", 1, 1),
                func("f:b", "b", "src/b.rs", 1, 1),
            ])
            .unwrap();
        fixture
            .store
            .insert_edges(&[calls("f:a", "f:b", 1)])
            .unwrap();

        let result = fixture
            .explorer()
            .explore(&["a"], ExploreBudget::default())
            .unwrap();

        assert_eq!(
            result
                .files
                .iter()
                .map(|group| group.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs"]
        );
    }

    #[test]
    fn explore_truncates_by_file_budget() {
        let fixture = Fixture::new();
        fixture.write("src/a.rs", "fn a() {}\n").unwrap();
        fixture.write("src/b.rs", "fn b() {}\n").unwrap();
        fixture
            .store
            .insert_nodes(&[
                func("f:a", "a", "src/a.rs", 1, 1),
                func("f:b", "b", "src/b.rs", 1, 1),
            ])
            .unwrap();
        fixture
            .store
            .insert_edges(&[calls("f:a", "f:b", 1)])
            .unwrap();

        let result = fixture
            .explorer()
            .explore(
                &["a"],
                ExploreBudget {
                    max_files: 1,
                    ..ExploreBudget::default()
                },
            )
            .unwrap();

        assert!(result.truncated);
        assert_eq!(result.files.len(), 1);
    }

    fn ids(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|node| node.id.as_str()).collect()
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

        fn explorer(&self) -> Explorer<'_> {
            Explorer::new(&self.store, self.dir.path())
        }

        fn write(&self, path: &str, source: &str) -> std::io::Result<()> {
            let path = self
                .dir
                .path()
                .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, source)
        }
    }
}
