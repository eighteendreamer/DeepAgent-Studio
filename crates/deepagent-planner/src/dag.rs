//! The plan DAG (开发计划.md Phase 6 §4 "DAG Scheduler").
//!
//! A [`PlanDag`] is a directed acyclic graph of [`PlanNode`]s. An edge `a -> b`
//! means "b depends on a" (a must complete before b). The DAG is validated on
//! construction — unknown dependencies and cycles are rejected — and exposes:
//!
//! - [`PlanDag::topological_layers`] — groups nodes into layers that can run in
//!   parallel (fan-out), where each layer depends only on earlier layers
//!   (fan-in). This is what the sub-agent scheduler consumes.
//! - [`PlanDag::ready_nodes`] — given a set of completed nodes, which nodes are
//!   now unblocked.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

/// Identifier for a node within a plan. Stable, human-meaningful strings keep
/// plans readable and serializable.
pub type NodeId = String;

/// A single unit of work in a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanNode {
    /// Unique id within the plan.
    pub id: NodeId,
    /// What this node should accomplish (becomes a sub-agent's goal).
    pub goal: String,
    /// Ids of nodes that must complete before this one.
    #[serde(default)]
    pub depends_on: Vec<NodeId>,
    /// Optional role/agent hint (e.g. "backend", "frontend", "review").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl PlanNode {
    /// A node with no dependencies.
    pub fn new(id: impl Into<NodeId>, goal: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            goal: goal.into(),
            depends_on: Vec::new(),
            role: None,
        }
    }

    /// Add dependencies (builder style).
    pub fn depends_on(mut self, deps: impl IntoIterator<Item = NodeId>) -> Self {
        self.depends_on = deps.into_iter().collect();
        self
    }

    /// Set a role hint (builder style).
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }
}

/// A validated directed acyclic graph of plan nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDag {
    nodes: BTreeMap<NodeId, PlanNode>,
}

impl PlanDag {
    /// Build and validate a DAG from a set of nodes.
    ///
    /// Errors if any dependency references an unknown node, a node depends on
    /// itself, duplicate ids exist, or the graph contains a cycle.
    pub fn new(nodes: impl IntoIterator<Item = PlanNode>) -> Result<Self> {
        let mut map = BTreeMap::new();
        for node in nodes {
            if map.contains_key(&node.id) {
                return Err(CoreError::invalid(format!(
                    "duplicate node id: {}",
                    node.id
                )));
            }
            map.insert(node.id.clone(), node);
        }

        // Validate dependency references and self-loops.
        for node in map.values() {
            for dep in &node.depends_on {
                if dep == &node.id {
                    return Err(CoreError::invalid(format!(
                        "node '{}' depends on itself",
                        node.id
                    )));
                }
                if !map.contains_key(dep) {
                    return Err(CoreError::invalid(format!(
                        "node '{}' depends on unknown node '{}'",
                        node.id, dep
                    )));
                }
            }
        }

        let dag = Self { nodes: map };
        dag.check_acyclic()?;
        Ok(dag)
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the plan is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Look up a node by id.
    pub fn node(&self, id: &str) -> Option<&PlanNode> {
        self.nodes.get(id)
    }

    /// All nodes (stable order by id).
    pub fn nodes(&self) -> impl Iterator<Item = &PlanNode> {
        self.nodes.values()
    }

    /// Kahn's algorithm: returns an error if the graph has a cycle.
    fn check_acyclic(&self) -> Result<()> {
        self.topological_order().map(|_| ())
    }

    /// A flat topological ordering (Kahn's algorithm). Errors on cycle.
    pub fn topological_order(&self) -> Result<Vec<NodeId>> {
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        for node in self.nodes.values() {
            in_degree.entry(&node.id).or_insert(0);
            for _dep in &node.depends_on {
                *in_degree.entry(&node.id).or_insert(0) += 1;
            }
        }

        // Queue nodes with no remaining dependencies (stable by id).
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();

        // Build reverse adjacency: dep -> dependents.
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for node in self.nodes.values() {
            for dep in &node.depends_on {
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }

        let mut order = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.to_string());
            if let Some(children) = dependents.get(id) {
                let mut newly_ready: Vec<&str> = Vec::new();
                for &child in children {
                    let d = in_degree.get_mut(child).expect("child in in_degree");
                    *d -= 1;
                    if *d == 0 {
                        newly_ready.push(child);
                    }
                }
                newly_ready.sort_unstable();
                for c in newly_ready {
                    queue.push_back(c);
                }
            }
        }

        if order.len() != self.nodes.len() {
            return Err(CoreError::invalid("plan DAG contains a cycle".to_string()));
        }
        Ok(order)
    }

    /// Group nodes into topological *layers*. Layer 0 has no dependencies;
    /// every node in layer `k` depends only on nodes in layers `< k`. Nodes
    /// within a layer are independent and may run in parallel (fan-out); the
    /// next layer fans in once the previous completes.
    pub fn topological_layers(&self) -> Result<Vec<Vec<NodeId>>> {
        // Ensure acyclic first (gives a clear error).
        self.topological_order()?;

        let mut remaining: BTreeSet<&str> = self.nodes.keys().map(|s| s.as_str()).collect();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut layers: Vec<Vec<NodeId>> = Vec::new();

        while !remaining.is_empty() {
            // Nodes whose deps are all done form the next layer.
            let layer: Vec<&str> = remaining
                .iter()
                .filter(|id| {
                    self.nodes[**id]
                        .depends_on
                        .iter()
                        .all(|d| done.contains(d.as_str()))
                })
                .copied()
                .collect();

            if layer.is_empty() {
                // Should not happen on an acyclic graph, but guard anyway.
                return Err(CoreError::invalid("plan DAG contains a cycle".to_string()));
            }

            for id in &layer {
                remaining.remove(id);
            }
            for id in &layer {
                done.insert(id);
            }
            layers.push(layer.into_iter().map(|s| s.to_string()).collect());
        }

        Ok(layers)
    }

    /// Given the set of already-completed node ids, return the nodes that are
    /// now ready (all dependencies satisfied and not yet completed).
    pub fn ready_nodes(&self, completed: &BTreeSet<NodeId>) -> Vec<NodeId> {
        self.nodes
            .values()
            .filter(|n| !completed.contains(&n.id))
            .filter(|n| n.depends_on.iter().all(|d| completed.contains(d)))
            .map(|n| n.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, deps: &[&str]) -> PlanNode {
        PlanNode::new(id, format!("do {id}")).depends_on(deps.iter().map(|s| s.to_string()))
    }

    #[test]
    fn builds_valid_dag() {
        let dag = PlanDag::new([
            node("a", &[]),
            node("b", &["a"]),
            node("c", &["a"]),
            node("d", &["b", "c"]),
        ])
        .unwrap();
        assert_eq!(dag.len(), 4);
    }

    #[test]
    fn rejects_unknown_dependency() {
        let err = PlanDag::new([node("a", &["ghost"])]).unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn rejects_self_dependency() {
        let err = PlanDag::new([node("a", &["a"])]).unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let err = PlanDag::new([node("a", &[]), node("a", &[])]).unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn detects_cycle() {
        // a -> b -> c -> a
        let err =
            PlanDag::new([node("a", &["c"]), node("b", &["a"]), node("c", &["b"])]).unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn topological_layers_group_parallel_work() {
        // a -> {b, c} -> d  (diamond)
        let dag = PlanDag::new([
            node("a", &[]),
            node("b", &["a"]),
            node("c", &["a"]),
            node("d", &["b", "c"]),
        ])
        .unwrap();
        let layers = dag.topological_layers().unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["a"]);
        assert_eq!(layers[1], vec!["b", "c"]); // parallel fan-out
        assert_eq!(layers[2], vec!["d"]); // fan-in
    }

    #[test]
    fn topological_order_respects_dependencies() {
        let dag = PlanDag::new([node("a", &[]), node("b", &["a"]), node("c", &["b"])]).unwrap();
        let order = dag.topological_order().unwrap();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
    }

    #[test]
    fn ready_nodes_progresses_with_completion() {
        let dag = PlanDag::new([node("a", &[]), node("b", &["a"]), node("c", &["a"])]).unwrap();
        let mut completed = BTreeSet::new();
        assert_eq!(dag.ready_nodes(&completed), vec!["a"]);
        completed.insert("a".to_string());
        let mut ready = dag.ready_nodes(&completed);
        ready.sort();
        assert_eq!(ready, vec!["b", "c"]);
    }
}
