//! The DAG scheduler (开发计划.md Phase 6 §4).
//!
//! Executes a [`PlanDag`] layer by layer:
//! - **fan-out**: all nodes in a topological layer are independent and are
//!   executed together,
//! - **fan-in**: the next layer starts only once the previous completes, and
//!   each node receives the summaries of its upstream dependencies,
//! - **isolation**: every node gets its own git worktree (no code clobbering).
//!
//! On a sub-agent failure the scheduler stops launching dependents of the
//! failed node (they can never satisfy their dependencies) but reports a full
//! [`ScheduleReport`]. Worktrees are always cleaned up.

use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::Result;
use deepagent_planner::PlanDag;

use crate::subagent::{context_for, SubAgentExecutor, SubAgentResult};
use crate::worktree::WorktreeProvider;

/// The result of scheduling an entire plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleReport {
    /// Per-node results, keyed by node id.
    pub results: BTreeMap<String, SubAgentResult>,
    /// Node ids that were skipped because an upstream dependency failed.
    pub skipped: Vec<String>,
    /// Whether every node completed successfully.
    pub all_succeeded: bool,
}

impl ScheduleReport {
    /// The ids of nodes that failed.
    pub fn failed_nodes(&self) -> Vec<&String> {
        self.results
            .iter()
            .filter(|(_, r)| !r.ok)
            .map(|(id, _)| id)
            .collect()
    }
}

/// Schedules a plan DAG across isolated sub-agents.
pub struct DagScheduler<'a> {
    executor: &'a dyn SubAgentExecutor,
    worktrees: &'a dyn WorktreeProvider,
}

impl<'a> DagScheduler<'a> {
    /// Build a scheduler from an executor and a worktree provider.
    pub fn new(executor: &'a dyn SubAgentExecutor, worktrees: &'a dyn WorktreeProvider) -> Self {
        Self {
            executor,
            worktrees,
        }
    }

    /// Execute `dag` to completion (or until a failure blocks progress).
    pub async fn run(&self, dag: &PlanDag) -> Result<ScheduleReport> {
        let layers = dag.topological_layers()?;
        let mut results: BTreeMap<String, SubAgentResult> = BTreeMap::new();
        let mut failed: BTreeSet<String> = BTreeSet::new();
        let mut skipped: Vec<String> = Vec::new();

        for layer in layers {
            for node_id in layer {
                let node = dag.node(&node_id).expect("layer node exists in dag");

                // Skip if any dependency failed or was skipped.
                let blocked = node
                    .depends_on
                    .iter()
                    .any(|d| failed.contains(d) || skipped.contains(d));
                if blocked {
                    tracing::warn!(node = %node_id, "skipping: upstream dependency unmet");
                    skipped.push(node_id.clone());
                    continue;
                }

                // Gather upstream result summaries (fan-in).
                let upstream: Vec<String> = node
                    .depends_on
                    .iter()
                    .filter_map(|d| results.get(d).map(|r| r.summary.clone()))
                    .collect();

                // Provision an isolated worktree.
                let worktree = self.worktrees.create(&node_id).await?;
                let ctx = context_for(node, worktree, upstream);

                // Execute the sub-agent, always cleaning up the worktree after.
                let exec_result = self.executor.execute(ctx).await;
                let _ = self.worktrees.remove(&node_id).await;

                let result = exec_result?;
                if !result.ok {
                    failed.insert(node_id.clone());
                }
                results.insert(node_id.clone(), result);
            }
        }

        let all_succeeded = failed.is_empty() && skipped.is_empty();
        Ok(ScheduleReport {
            results,
            skipped,
            all_succeeded,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::{SubAgentContext, SubAgentResult};
    use crate::worktree::InMemoryWorktrees;
    use async_trait::async_trait;
    use deepagent_planner::{HeuristicPlanner, PlanNode, PlanStrategy, Planner};
    use std::sync::Mutex;

    /// Records execution order and worktree paths; succeeds for all nodes.
    #[derive(Default)]
    struct RecordingExecutor {
        order: Mutex<Vec<String>>,
        seen_paths: Mutex<Vec<String>>,
        upstream_seen: Mutex<Vec<(String, usize)>>,
    }

    #[async_trait]
    impl SubAgentExecutor for RecordingExecutor {
        async fn execute(&self, ctx: SubAgentContext) -> Result<SubAgentResult> {
            self.order.lock().unwrap().push(ctx.node_id.clone());
            self.seen_paths
                .lock()
                .unwrap()
                .push(ctx.worktree.path.clone());
            self.upstream_seen
                .lock()
                .unwrap()
                .push((ctx.node_id.clone(), ctx.upstream_results.len()));
            Ok(SubAgentResult::success(ctx.node_id, "ok"))
        }
    }

    #[tokio::test]
    async fn runs_full_multi_agent_plan() {
        let dag = HeuristicPlanner
            .plan("build product", PlanStrategy::MultiAgent)
            .unwrap();
        let executor = RecordingExecutor::default();
        let worktrees = InMemoryWorktrees::new("/tmp/wt");
        let scheduler = DagScheduler::new(&executor, &worktrees);

        let report = scheduler.run(&dag).await.unwrap();
        assert!(report.all_succeeded);
        assert_eq!(report.results.len(), 5);

        // architect runs before backend/frontend/database; review runs last.
        let order = executor.order.lock().unwrap().clone();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("architect") < pos("backend"));
        assert!(pos("architect") < pos("frontend"));
        assert!(pos("review") > pos("database"));

        // All worktrees were cleaned up.
        assert!(worktrees.active().is_empty());
    }

    #[tokio::test]
    async fn review_receives_upstream_summaries() {
        let dag = HeuristicPlanner
            .plan("x", PlanStrategy::MultiAgent)
            .unwrap();
        let executor = RecordingExecutor::default();
        let worktrees = InMemoryWorktrees::new("/tmp/wt");
        DagScheduler::new(&executor, &worktrees)
            .run(&dag)
            .await
            .unwrap();

        let upstream = executor.upstream_seen.lock().unwrap().clone();
        let review = upstream.iter().find(|(id, _)| id == "review").unwrap();
        // review depends on backend, frontend, database = 3 upstream summaries.
        assert_eq!(review.1, 3);
    }

    #[tokio::test]
    async fn each_node_gets_isolated_worktree_path() {
        let dag = PlanDag::new([
            PlanNode::new("a", "a"),
            PlanNode::new("b", "b").depends_on(["a".to_string()]),
        ])
        .unwrap();
        let executor = RecordingExecutor::default();
        let worktrees = InMemoryWorktrees::new("/tmp/wt");
        DagScheduler::new(&executor, &worktrees)
            .run(&dag)
            .await
            .unwrap();
        let paths = executor.seen_paths.lock().unwrap().clone();
        assert!(paths.contains(&"/tmp/wt/a".to_string()));
        assert!(paths.contains(&"/tmp/wt/b".to_string()));
        // Distinct worktrees -> no clobbering.
        assert_ne!(paths[0], paths[1]);
    }

    #[tokio::test]
    async fn failure_skips_dependents() {
        // a -> b -> c; a fails, so b and c are skipped.
        let dag = PlanDag::new([
            PlanNode::new("a", "a"),
            PlanNode::new("b", "b").depends_on(["a".to_string()]),
            PlanNode::new("c", "c").depends_on(["b".to_string()]),
        ])
        .unwrap();

        struct FailFirst;
        #[async_trait]
        impl SubAgentExecutor for FailFirst {
            async fn execute(&self, ctx: SubAgentContext) -> Result<SubAgentResult> {
                if ctx.node_id == "a" {
                    Ok(SubAgentResult::failure("a", "boom"))
                } else {
                    Ok(SubAgentResult::success(ctx.node_id, "ok"))
                }
            }
        }

        let worktrees = InMemoryWorktrees::new("/tmp/wt");
        let report = DagScheduler::new(&FailFirst, &worktrees)
            .run(&dag)
            .await
            .unwrap();
        assert!(!report.all_succeeded);
        assert_eq!(report.failed_nodes(), vec![&"a".to_string()]);
        let mut skipped = report.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, vec!["b".to_string(), "c".to_string()]);
        // Worktrees cleaned up even on failure.
        assert!(worktrees.active().is_empty());
    }
}
