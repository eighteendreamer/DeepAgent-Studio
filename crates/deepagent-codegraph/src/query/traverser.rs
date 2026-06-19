//! Graph traversal over `calls` edges.
//!
//! [`Traverser`] borrows a [`GraphStore`] and answers the structural
//! call-graph questions that power the `callers` / `callees` / `impact`
//! queries:
//!
//! - [`Traverser::callers`] — who calls this symbol (incoming `calls` edges).
//! - [`Traverser::callees`] — what this symbol calls (outgoing `calls` edges).
//! - [`Traverser::impact`] — the change-impact radius: the reverse-`calls`
//!   reachable set split into *direct* (depth 1) and *indirect* (depth 2..=N)
//!   callers.
//!
//! All walks are breadth-first with a `visited` set, so cyclic call graphs
//! terminate instead of looping forever.

use std::collections::HashSet;

use deepagent_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::store::GraphStore;
use crate::types::{EdgeKind, Node};

/// Direction in which to follow edges during a traversal.
///
/// For a `calls` edge `caller -> callee`:
/// - [`Direction::Incoming`] walks toward callers (`edges_to`, neighbor =
///   `source`).
/// - [`Direction::Outgoing`] walks toward callees (`edges_from`, neighbor =
///   `target`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Follow edges that point *at* the node (toward callers).
    Incoming,
    /// Follow edges that point *away from* the node (toward callees).
    Outgoing,
}

/// A single call site: the symbol at one end of a `calls` edge plus the source
/// line the call was recorded on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSite {
    /// The caller (for `callers`) or callee (for `callees`) node.
    pub node: Node,
    /// Line the call edge was recorded on; `0` when the edge has no line.
    pub line: u32,
}

/// Result of an [`Traverser::impact`] query: the change-impact radius of a
/// symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactResult {
    /// The symbol the impact was computed for.
    pub target: Node,
    /// Direct callers (depth 1 in the reverse-`calls` BFS).
    pub direct: Vec<Node>,
    /// Indirect callers (depth 2..=`depth` in the reverse-`calls` BFS).
    pub indirect: Vec<Node>,
}

/// Borrows a [`GraphStore`] to walk its `calls` edges.
pub struct Traverser<'a> {
    store: &'a GraphStore,
}

impl<'a> Traverser<'a> {
    /// Create a traverser over `store`.
    pub fn new(store: &'a GraphStore) -> Self {
        Self { store }
    }

    /// All call sites that call `node_id` (incoming `calls` edges).
    ///
    /// Returns at most `limit` sites (a `limit` of `0` means unlimited). A
    /// node id that does not exist simply yields an empty vector rather than
    /// an error.
    pub fn callers(&self, node_id: &str, limit: usize) -> Result<Vec<CallSite>> {
        let edges = self.store.edges_to(node_id, EdgeKind::Calls)?;
        self.collect_call_sites(edges, Direction::Incoming, limit)
    }

    /// All call sites that `node_id` calls (outgoing `calls` edges).
    ///
    /// Returns at most `limit` sites (a `limit` of `0` means unlimited). A
    /// node id that does not exist simply yields an empty vector rather than
    /// an error.
    pub fn callees(&self, node_id: &str, limit: usize) -> Result<Vec<CallSite>> {
        let edges = self.store.edges_from(node_id, EdgeKind::Calls)?;
        self.collect_call_sites(edges, Direction::Outgoing, limit)
    }

    /// Change-impact radius of `node_id`: the reverse-`calls` reachable set up
    /// to `depth` hops, split into direct (depth 1) and indirect (depth
    /// 2..=`depth`) callers.
    ///
    /// `depth` is clamped to a minimum of `1`. Returns `Ok(None)` when
    /// `node_id` does not exist (callers are guided, not errored — mirroring
    /// the "no error on missing node" contract of the query tools).
    pub fn impact(&self, node_id: &str, depth: usize) -> Result<Option<ImpactResult>> {
        let target = match self.store.node_by_id(node_id)? {
            Some(node) => node,
            None => return Ok(None),
        };

        let depth = depth.max(1);
        let layers = self.bfs_layers(node_id, Direction::Incoming, EdgeKind::Calls, depth)?;

        let direct = layers.first().cloned().unwrap_or_default();
        let indirect = layers
            .iter()
            .skip(1)
            .flat_map(|layer| layer.iter().cloned())
            .collect();

        Ok(Some(ImpactResult {
            target,
            direct,
            indirect,
        }))
    }

    /// Resolve every edge endpoint (per `direction`) into a [`CallSite`],
    /// stopping once `limit` sites have been collected (`0` = unlimited).
    ///
    /// Endpoints that no longer resolve to a node are skipped.
    fn collect_call_sites(
        &self,
        edges: Vec<crate::types::Edge>,
        direction: Direction,
        limit: usize,
    ) -> Result<Vec<CallSite>> {
        let mut sites = Vec::new();
        for edge in edges {
            let endpoint = match direction {
                Direction::Incoming => &edge.source,
                Direction::Outgoing => &edge.target,
            };
            if let Some(node) = self.store.node_by_id(endpoint)? {
                sites.push(CallSite {
                    node,
                    line: edge.line.unwrap_or(0),
                });
                if limit != 0 && sites.len() >= limit {
                    break;
                }
            }
        }
        Ok(sites)
    }

    /// Breadth-first walk from `start` following `kind` edges in `direction`,
    /// returning the nodes discovered at each depth level (index `0` = depth
    /// 1) up to `max_depth` levels.
    ///
    /// A `visited` set (seeded with `start`) guarantees termination on cyclic
    /// graphs and de-duplicates nodes reachable by multiple paths — each node
    /// appears at most once, at the shallowest depth it is found.
    fn bfs_layers(
        &self,
        start: &str,
        direction: Direction,
        kind: EdgeKind,
        max_depth: usize,
    ) -> Result<Vec<Vec<Node>>> {
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(start.to_string());

        let mut layers: Vec<Vec<Node>> = Vec::new();
        let mut frontier: Vec<String> = vec![start.to_string()];

        for _ in 0..max_depth {
            if frontier.is_empty() {
                break;
            }

            let mut next_frontier: Vec<String> = Vec::new();
            let mut layer_nodes: Vec<Node> = Vec::new();

            for id in &frontier {
                let edges = match direction {
                    Direction::Incoming => self.store.edges_to(id, kind)?,
                    Direction::Outgoing => self.store.edges_from(id, kind)?,
                };
                for edge in edges {
                    let neighbor = match direction {
                        Direction::Incoming => edge.source,
                        Direction::Outgoing => edge.target,
                    };
                    if !visited.insert(neighbor.clone()) {
                        continue;
                    }
                    if let Some(node) = self.store.node_by_id(&neighbor)? {
                        layer_nodes.push(node);
                        next_frontier.push(neighbor);
                    }
                }
            }

            if layer_nodes.is_empty() {
                break;
            }
            layers.push(layer_nodes);
            frontier = next_frontier;
        }

        Ok(layers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, Language, NodeKind};

    /// Build a minimal function [`Node`] with the given id.
    fn func_node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: id.to_string(),
            qualified_name: id.to_string(),
            file_path: "src/lib.rs".to_string(),
            language: Language::Rust,
            start_line: 1,
            end_line: 2,
            start_column: 0,
            end_column: 1,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    /// Build a `calls` edge `source -> target` recorded on `line`.
    fn calls_edge(source: &str, target: &str, line: Option<u32>) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind: EdgeKind::Calls,
            metadata: None,
            line,
            provenance: None,
        }
    }

    /// Store with nodes A, B, C and a linear call chain A -> B -> C.
    fn linear_chain_store() -> GraphStore {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[func_node("a"), func_node("b"), func_node("c")])
            .unwrap();
        store
            .insert_edges(&[
                calls_edge("a", "b", Some(10)),
                calls_edge("b", "c", Some(20)),
            ])
            .unwrap();
        store
    }

    fn ids(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn callers_returns_the_direct_caller() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);

        let callers_b = t.callers("b", 10).unwrap();
        assert_eq!(callers_b.len(), 1);
        assert_eq!(callers_b[0].node.id, "a");

        let callers_c = t.callers("c", 10).unwrap();
        assert_eq!(callers_c.len(), 1);
        assert_eq!(callers_c[0].node.id, "b");
    }

    #[test]
    fn callers_of_root_is_empty() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);
        assert!(t.callers("a", 10).unwrap().is_empty());
    }

    #[test]
    fn callees_returns_the_direct_callee() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);

        let callees_a = t.callees("a", 10).unwrap();
        assert_eq!(callees_a.len(), 1);
        assert_eq!(callees_a[0].node.id, "b");

        let callees_b = t.callees("b", 10).unwrap();
        assert_eq!(callees_b.len(), 1);
        assert_eq!(callees_b[0].node.id, "c");
    }

    #[test]
    fn callees_of_leaf_is_empty() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);
        assert!(t.callees("c", 10).unwrap().is_empty());
    }

    #[test]
    fn call_site_line_comes_from_the_edge() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);
        // Edge a -> b was recorded on line 10.
        assert_eq!(t.callers("b", 10).unwrap()[0].line, 10);
        // Edge a -> b from a's perspective (callee b) is also line 10.
        assert_eq!(t.callees("a", 10).unwrap()[0].line, 10);
    }

    #[test]
    fn call_site_line_defaults_to_zero_when_edge_has_no_line() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[func_node("a"), func_node("b")])
            .unwrap();
        store.insert_edges(&[calls_edge("a", "b", None)]).unwrap();

        let t = Traverser::new(&store);
        assert_eq!(t.callers("b", 10).unwrap()[0].line, 0);
    }

    #[test]
    fn callers_respects_limit() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[func_node("a"), func_node("d"), func_node("b")])
            .unwrap();
        store
            .insert_edges(&[calls_edge("a", "b", Some(1)), calls_edge("d", "b", Some(2))])
            .unwrap();

        let t = Traverser::new(&store);
        assert_eq!(t.callers("b", 2).unwrap().len(), 2);
        assert_eq!(t.callers("b", 1).unwrap().len(), 1);
    }

    #[test]
    fn impact_splits_direct_and_indirect_callers() {
        // A -> B -> C, so impacting C: direct = [B], indirect = [A].
        let store = linear_chain_store();
        let t = Traverser::new(&store);

        let impact = t.impact("c", 10).unwrap().expect("c exists");
        assert_eq!(impact.target.id, "c");
        assert_eq!(ids(&impact.direct), vec!["b"]);
        assert_eq!(ids(&impact.indirect), vec!["a"]);
    }

    #[test]
    fn impact_depth_one_returns_only_direct_callers() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);

        let impact = t.impact("c", 1).unwrap().expect("c exists");
        assert_eq!(ids(&impact.direct), vec!["b"]);
        assert!(impact.indirect.is_empty());
    }

    #[test]
    fn impact_terminates_on_a_cycle() {
        // A -> B -> A is a cycle; impact must not loop forever.
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[func_node("a"), func_node("b")])
            .unwrap();
        store
            .insert_edges(&[calls_edge("a", "b", Some(1)), calls_edge("b", "a", Some(2))])
            .unwrap();

        let t = Traverser::new(&store);
        // Reverse-calls from A: direct caller is B (b -> a); from B the only
        // caller is A which is already visited, so indirect is empty.
        let impact = t.impact("a", 10).unwrap().expect("a exists");
        assert_eq!(ids(&impact.direct), vec!["b"]);
        assert!(impact.indirect.is_empty());
    }

    #[test]
    fn missing_node_does_not_error() {
        let store = linear_chain_store();
        let t = Traverser::new(&store);

        assert!(t.callers("does-not-exist", 10).unwrap().is_empty());
        assert!(t.callees("does-not-exist", 10).unwrap().is_empty());
        assert!(t.impact("does-not-exist", 10).unwrap().is_none());
    }
}
