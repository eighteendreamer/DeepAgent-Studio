//! The DAG scheduler (开发计划.md Phase 6 §4).
//!
//! Executes a [`PlanDag`] layer by layer:
//! - **fan-out**: all runnable nodes in a topological layer are independent, so
//!   they execute concurrently, bounded by [`DagScheduler`]'s concurrency cap,
//! - **fan-in**: the next layer starts only once the previous completes, and
//!   each node receives the summaries of its upstream dependencies,
//! - **isolation**: every node gets its own git worktree (no code clobbering).
//!
//! On a sub-agent failure the scheduler stops launching dependents of the
//! failed node (they can never satisfy their dependencies) but reports a full
//! [`ScheduleReport`]. Worktrees are always cleaned up.

use std::collections::{BTreeMap, BTreeSet};

use futures::stream::StreamExt;

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

/// Default cap on how many sibling sub-agents run at once within a layer.
///
/// Sub-agents are heavyweight — each drives a full agent loop and provisions a
/// git worktree — so the fan-out is bounded rather than unbounded. Override with
/// [`DagScheduler::with_max_concurrency`].
pub const DEFAULT_LAYER_CONCURRENCY: usize = 8;

/// Schedules a plan DAG across isolated sub-agents.
pub struct DagScheduler<'a> {
    executor: &'a dyn SubAgentExecutor,
    worktrees: &'a dyn WorktreeProvider,
    max_concurrency: usize,
}

impl<'a> DagScheduler<'a> {
    /// Build a scheduler from an executor and a worktree provider, using
    /// [`DEFAULT_LAYER_CONCURRENCY`] as the per-layer fan-out cap.
    pub fn new(executor: &'a dyn SubAgentExecutor, worktrees: &'a dyn WorktreeProvider) -> Self {
        Self {
            executor,
            worktrees,
            max_concurrency: DEFAULT_LAYER_CONCURRENCY,
        }
    }

    /// Set the maximum number of sibling nodes executed concurrently within a
    /// layer. Values below 1 are clamped to 1 (fully serial).
    pub fn with_max_concurrency(mut self, max_concurrency: usize) -> Self {
        self.max_concurrency = max_concurrency.max(1);
        self
    }

    /// Execute `dag` to completion (or until a failure blocks progress).
    pub async fn run(&self, dag: &PlanDag) -> Result<ScheduleReport> {
        let layers = dag.topological_layers()?;
        let mut results: BTreeMap<String, SubAgentResult> = BTreeMap::new();
        let mut failed: BTreeSet<String> = BTreeSet::new();
        let mut skipped: Vec<String> = Vec::new();

        for layer in layers {
            // Partition the layer into skipped vs runnable nodes. Within a
            // topological layer no node depends on another, so every read of
            // `results`/`failed`/`skipped` here observes only prior layers and
            // the partition is independent of intra-layer execution order.
            let mut runnable: Vec<(String, Vec<String>)> = Vec::new();
            for node_id in layer {
                let node = dag.node(&node_id).expect("layer node exists in dag");

                // Skip if any dependency failed or was skipped.
                let blocked = node
                    .depends_on
                    .iter()
                    .any(|d| failed.contains(d) || skipped.contains(d));
                if blocked {
                    tracing::warn!(node = %node_id, "skipping: upstream dependency unmet");
                    skipped.push(node_id);
                    continue;
                }

                // Gather upstream result summaries (fan-in).
                let upstream: Vec<String> = node
                    .depends_on
                    .iter()
                    .filter_map(|d| results.get(d).map(|r| r.summary.clone()))
                    .collect();
                runnable.push((node_id, upstream));
            }

            // Fan-out: run the layer's independent nodes concurrently, bounded
            // by `max_concurrency`. Each node gets its own worktree, always torn
            // down before its future resolves (even on executor failure).
            let node_runs = runnable.into_iter().map(|(node_id, upstream)| async move {
                let node = dag.node(&node_id).expect("layer node exists in dag");
                let worktree = match self.worktrees.create(&node_id).await {
                    Ok(worktree) => worktree,
                    Err(error) => return (node_id, Err(error)),
                };
                let ctx = context_for(node, worktree, upstream);
                let exec_result = self.executor.execute(ctx).await;
                let _ = self.worktrees.remove(&node_id).await;
                (node_id, exec_result)
            });
            let mut layer_results: Vec<(String, Result<SubAgentResult>)> =
                futures::stream::iter(node_runs)
                    .buffer_unordered(self.max_concurrency)
                    .collect()
                    .await;

            // Fan-in: aggregate in a stable (node-id) order so the recorded
            // results and any propagated error do not depend on completion
            // timing. `results`/`failed` are ordered collections anyway, but a
            // deterministic order also fixes *which* error surfaces first.
            layer_results.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (node_id, exec_result) in layer_results {
                let result = exec_result?;
                if !result.ok {
                    failed.insert(node_id.clone());
                }
                results.insert(node_id, result);
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
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    /// Records the peak number of sub-agents executing at the same instant.
    /// The `yield_now` between increment and decrement opens an overlap window:
    /// a true fan-out parks every sibling there together, so the gauge climbs to
    /// the layer width; a serial loop would peak at 1.
    #[derive(Default)]
    struct ConcurrencyProbe {
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    #[async_trait]
    impl SubAgentExecutor for ConcurrencyProbe {
        async fn execute(&self, ctx: SubAgentContext) -> Result<SubAgentResult> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::task::yield_now().await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(SubAgentResult::success(ctx.node_id, "ok"))
        }
    }

    #[tokio::test]
    async fn layer_nodes_execute_concurrently() {
        // The MultiAgent plan's middle layer {backend, frontend, database} has
        // three independent nodes; a genuine fan-out overlaps them.
        let dag = HeuristicPlanner
            .plan("x", PlanStrategy::MultiAgent)
            .unwrap();
        let executor = ConcurrencyProbe::default();
        let worktrees = InMemoryWorktrees::new("/tmp/wt");

        let report = DagScheduler::new(&executor, &worktrees)
            .run(&dag)
            .await
            .unwrap();
        assert!(report.all_succeeded);

        let peak = executor.max_in_flight.load(Ordering::SeqCst);
        assert!(
            peak >= 2,
            "expected concurrent layer execution, peak in-flight was {peak}"
        );
    }

    #[tokio::test]
    async fn concurrency_cap_of_one_serializes() {
        // A cap of 1 must degrade to serial execution: peak in-flight is 1.
        let dag = HeuristicPlanner
            .plan("x", PlanStrategy::MultiAgent)
            .unwrap();
        let executor = ConcurrencyProbe::default();
        let worktrees = InMemoryWorktrees::new("/tmp/wt");

        let report = DagScheduler::new(&executor, &worktrees)
            .with_max_concurrency(1)
            .run(&dag)
            .await
            .unwrap();
        assert!(report.all_succeeded);

        let peak = executor.max_in_flight.load(Ordering::SeqCst);
        assert_eq!(peak, 1, "cap=1 must serialize, peak in-flight was {peak}");
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
