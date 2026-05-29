//! SubAgent execution model (开发计划.md Phase 6 §2; 开发提示词.md §5, §11).
//!
//! Each sub-agent runs with **isolated** context, memory scope, and workspace
//! so concurrent agents do not pollute each other (开发提示词.md §5 "Context
//! Isolation"). This module defines the per-sub-agent [`SubAgentContext`] and
//! the [`SubAgentExecutor`] trait the scheduler drives; the actual agent loop
//! is supplied by the caller (the runtime), keeping this crate free of a
//! dependency cycle on `deepagent-runtime`.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_planner::PlanNode;

use crate::worktree::Worktree;

/// The isolated execution context handed to a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentContext {
    /// The plan node this sub-agent is responsible for.
    pub node_id: String,
    /// The goal (from the node).
    pub goal: String,
    /// Role hint, if any.
    pub role: Option<String>,
    /// The isolated worktree assigned to this sub-agent.
    pub worktree: Worktree,
    /// Summaries of upstream dependencies' results, injected so this agent can
    /// build on prior work without sharing mutable context.
    pub upstream_results: Vec<String>,
}

/// The outcome of running a single sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentResult {
    /// The node id this result is for.
    pub node_id: String,
    /// Whether the sub-agent succeeded.
    pub ok: bool,
    /// A short summary of what it produced (fed to downstream agents).
    pub summary: String,
}

impl SubAgentResult {
    /// A successful result.
    pub fn success(node_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            ok: true,
            summary: summary.into(),
        }
    }

    /// A failed result.
    pub fn failure(node_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            ok: false,
            summary: summary.into(),
        }
    }
}

/// Executes a single sub-agent given its isolated context.
///
/// Implementations wrap the runtime's agent loop. The scheduler calls this once
/// per ready node; the executor is `&self` so it can be shared across
/// concurrently scheduled nodes.
#[async_trait]
pub trait SubAgentExecutor: Send + Sync {
    /// Run the sub-agent for `context`, returning its result.
    async fn execute(&self, context: SubAgentContext) -> Result<SubAgentResult>;
}

/// Helper to build a [`SubAgentContext`] from a plan node + worktree + upstream
/// results.
pub fn context_for(
    node: &PlanNode,
    worktree: Worktree,
    upstream_results: Vec<String>,
) -> SubAgentContext {
    SubAgentContext {
        node_id: node.id.clone(),
        goal: node.goal.clone(),
        role: node.role.clone(),
        worktree,
        upstream_results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_constructors() {
        let ok = SubAgentResult::success("n1", "did it");
        assert!(ok.ok);
        let fail = SubAgentResult::failure("n2", "broke");
        assert!(!fail.ok);
    }

    #[test]
    fn context_from_node() {
        let node = PlanNode::new("backend", "build api").with_role("backend");
        let wt = Worktree {
            name: "backend".into(),
            path: "/tmp/backend".into(),
        };
        let ctx = context_for(&node, wt, vec!["arch done".into()]);
        assert_eq!(ctx.node_id, "backend");
        assert_eq!(ctx.role.as_deref(), Some("backend"));
        assert_eq!(ctx.upstream_results, vec!["arch done".to_string()]);
    }
}
