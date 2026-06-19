//! Guided tour generation for projected project maps.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::projection::layers::Layer;
use crate::types::{Edge, EdgeKind, Node};

/// One guided-tour step in the projected knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TourStep {
    pub order: u32,
    pub title: String,
    pub description: String,
    pub node_ids: Vec<String>,
}

/// Generate a deterministic guided tour over the graph.
///
/// If non-empty layers are supplied, each layer becomes one step with its node
/// ids sorted according to the graph's topological order. If no layers are
/// supplied, each graph node becomes its own step.
pub fn generate_tour(nodes: &[Node], edges: &[Edge], layers: &[Layer]) -> Vec<TourStep> {
    let topo_rank = topological_ranks(nodes, edges);
    if layers.is_empty() {
        return nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, node_id)| TourStep {
                order: (index + 1) as u32,
                title: title_for_node(nodes, &node_id),
                description: "Review this graph node and its local relationships.".to_string(),
                node_ids: vec![node_id],
            })
            .collect();
    }

    layers
        .iter()
        .filter(|layer| !layer.node_ids.is_empty())
        .enumerate()
        .map(|(index, layer)| {
            let mut node_ids = layer.node_ids.clone();
            node_ids.sort_by(|left, right| {
                topo_rank
                    .get(left)
                    .unwrap_or(&usize::MAX)
                    .cmp(topo_rank.get(right).unwrap_or(&usize::MAX))
                    .then_with(|| left.cmp(right))
            });
            node_ids.dedup();
            TourStep {
                order: (index + 1) as u32,
                title: layer.name.clone(),
                description: layer.description.clone(),
                node_ids,
            }
        })
        .collect()
}

fn topological_ranks(nodes: &[Node], edges: &[Edge]) -> BTreeMap<String, usize> {
    let node_ids: BTreeSet<String> = nodes.iter().map(|node| node.id.clone()).collect();
    let mut outgoing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indegree: BTreeMap<String, usize> = node_ids
        .iter()
        .map(|node_id| (node_id.clone(), 0usize))
        .collect();

    for edge in edges {
        if !node_ids.contains(&edge.source) || !node_ids.contains(&edge.target) {
            continue;
        }
        let (from, to) = tour_edge_direction(edge);
        if from == to {
            continue;
        }
        if outgoing
            .entry(from.to_string())
            .or_default()
            .insert(to.to_string())
        {
            *indegree.entry(to.to_string()).or_insert(0) += 1;
        }
    }

    let mut ready: VecDeque<String> = indegree
        .iter()
        .filter_map(|(node_id, degree)| (*degree == 0).then_some(node_id.clone()))
        .collect();
    let mut ordered = Vec::with_capacity(node_ids.len());

    while let Some(node_id) = ready.pop_front() {
        ordered.push(node_id.clone());
        if let Some(targets) = outgoing.get(&node_id) {
            for target in targets {
                if let Some(degree) = indegree.get_mut(target) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        ready.push_back(target.clone());
                    }
                }
            }
        }
    }

    if ordered.len() < node_ids.len() {
        let seen: BTreeSet<String> = ordered.iter().cloned().collect();
        for node_id in &node_ids {
            if !seen.contains(node_id) {
                ordered.push(node_id.clone());
            }
        }
    }

    ordered
        .into_iter()
        .enumerate()
        .map(|(rank, node_id)| (node_id, rank))
        .collect()
}

fn tour_edge_direction(edge: &Edge) -> (&str, &str) {
    match edge.kind {
        EdgeKind::Imports | EdgeKind::Extends | EdgeKind::Implements | EdgeKind::TypeOf => {
            (&edge.target, &edge.source)
        }
        EdgeKind::Contains | EdgeKind::Calls | EdgeKind::Exports | EdgeKind::References => {
            (&edge.source, &edge.target)
        }
    }
}

fn title_for_node(nodes: &[Node], node_id: &str) -> String {
    nodes
        .iter()
        .find(|node| node.id == node_id)
        .map(|node| {
            if node.name.is_empty() {
                node.id.clone()
            } else {
                node.name.clone()
            }
        })
        .unwrap_or_else(|| node_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::layers::Layer;
    use crate::types::{Language, NodeKind};

    #[test]
    fn tour_order_is_contiguous_for_layers() {
        let nodes = vec![
            node("file:src/data/db.rs", "db", "src/data/db.rs"),
            node("file:src/api/users.rs", "users", "src/api/users.rs"),
            node("file:src/core.rs", "core", "src/core.rs"),
        ];
        let layers = vec![
            layer("api", "API", vec!["file:src/api/users.rs"]),
            layer("data", "Data", vec!["file:src/data/db.rs"]),
            layer("core", "Core", vec!["file:src/core.rs"]),
        ];

        let tour = generate_tour(&nodes, &[], &layers);

        assert_eq!(orders(&tour), vec![1, 2, 3]);
        assert_eq!(tour[0].title, "API");
    }

    #[test]
    fn tour_order_is_contiguous_without_layers() {
        let nodes = vec![
            node("file:b.rs", "b", "b.rs"),
            node("file:a.rs", "a", "a.rs"),
        ];

        let tour = generate_tour(&nodes, &[], &[]);

        assert_eq!(orders(&tour), vec![1, 2]);
        assert_eq!(tour.len(), 2);
        assert!(tour.iter().all(|step| step.node_ids.len() == 1));
    }

    #[test]
    fn layer_node_ids_follow_topological_rank() {
        let nodes = vec![
            node("file:src/api/users.rs", "users", "src/api/users.rs"),
            node("file:src/data/db.rs", "db", "src/data/db.rs"),
        ];
        let edges = vec![Edge {
            source: "file:src/api/users.rs".to_string(),
            target: "file:src/data/db.rs".to_string(),
            kind: EdgeKind::Imports,
            metadata: None,
            line: None,
            provenance: None,
        }];
        let layers = vec![layer(
            "core",
            "Core",
            vec!["file:src/api/users.rs", "file:src/data/db.rs"],
        )];

        let tour = generate_tour(&nodes, &edges, &layers);

        assert_eq!(
            tour[0].node_ids,
            vec![
                "file:src/data/db.rs".to_string(),
                "file:src/api/users.rs".to_string()
            ]
        );
    }

    fn orders(tour: &[TourStep]) -> Vec<u32> {
        tour.iter().map(|step| step.order).collect()
    }

    fn layer(id: &str, name: &str, node_ids: Vec<&str>) -> Layer {
        Layer {
            id: id.to_string(),
            name: name.to_string(),
            description: format!("{name} layer"),
            node_ids: node_ids.into_iter().map(str::to_string).collect(),
        }
    }

    fn node(id: &str, name: &str, file_path: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::File,
            name: name.to_string(),
            qualified_name: name.to_string(),
            file_path: file_path.to_string(),
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
}
