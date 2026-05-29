//! The planning engine (开发计划.md Phase 6 §1 "Planner Engine").
//!
//! A [`Planner`] decomposes a high-level goal into a validated [`PlanDag`] of
//! subtasks. The trait supports the strategies from the plan — plan-execute,
//! reflective, and multi-agent — selected via [`PlanStrategy`]. The built-in
//! [`HeuristicPlanner`] is model-free and deterministic (good as a fallback and
//! for tests); a model-backed planner implements the same trait.

use deepagent_core::error::Result;

use crate::dag::{PlanDag, PlanNode};

/// Planning strategy (开发计划.md Phase 6 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStrategy {
    /// Produce a fixed plan up front, then execute it.
    PlanExecute,
    /// Plan, execute, then re-plan based on results (iterative).
    Reflective,
    /// Decompose into role-specialized sub-agents (architect/backend/...).
    MultiAgent,
}

/// Decomposes goals into plans.
pub trait Planner {
    /// Produce a validated plan DAG for `goal` using `strategy`.
    fn plan(&self, goal: &str, strategy: PlanStrategy) -> Result<PlanDag>;
}

/// A deterministic, model-free planner.
///
/// It does not understand the goal semantically; it produces a sensible *shape*
/// of plan for the chosen strategy so the scheduler and the rest of the system
/// can be exercised end-to-end without a model. A model-backed planner replaces
/// it by implementing the same trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicPlanner;

impl Planner for HeuristicPlanner {
    fn plan(&self, goal: &str, strategy: PlanStrategy) -> Result<PlanDag> {
        match strategy {
            PlanStrategy::PlanExecute => self.plan_execute(goal),
            PlanStrategy::Reflective => self.reflective(goal),
            PlanStrategy::MultiAgent => self.multi_agent(goal),
        }
    }
}

impl HeuristicPlanner {
    /// Linear analyze -> implement -> verify chain.
    fn plan_execute(&self, goal: &str) -> Result<PlanDag> {
        PlanDag::new([
            PlanNode::new("analyze", format!("Analyze requirements for: {goal}")),
            PlanNode::new("implement", format!("Implement: {goal}"))
                .depends_on(["analyze".to_string()]),
            PlanNode::new("verify", format!("Verify the implementation of: {goal}"))
                .depends_on(["implement".to_string()]),
        ])
    }

    /// Plan-execute with an explicit reflect/re-plan node at the end.
    fn reflective(&self, goal: &str) -> Result<PlanDag> {
        PlanDag::new([
            PlanNode::new("plan", format!("Draft a plan for: {goal}")),
            PlanNode::new("execute", format!("Execute the plan for: {goal}"))
                .depends_on(["plan".to_string()]),
            PlanNode::new("reflect", format!("Reflect on results and refine: {goal}"))
                .depends_on(["execute".to_string()]),
        ])
    }

    /// Role-specialized fan-out: architect -> {backend, frontend, database} ->
    /// review. Demonstrates the DAG's parallel layers.
    fn multi_agent(&self, goal: &str) -> Result<PlanDag> {
        PlanDag::new([
            PlanNode::new("architect", format!("Design the architecture for: {goal}"))
                .with_role("architect"),
            PlanNode::new("backend", format!("Build the backend for: {goal}"))
                .with_role("backend")
                .depends_on(["architect".to_string()]),
            PlanNode::new("frontend", format!("Build the frontend for: {goal}"))
                .with_role("frontend")
                .depends_on(["architect".to_string()]),
            PlanNode::new("database", format!("Design the database for: {goal}"))
                .with_role("database")
                .depends_on(["architect".to_string()]),
            PlanNode::new(
                "review",
                format!("Review and integrate everything for: {goal}"),
            )
            .with_role("review")
            .depends_on([
                "backend".to_string(),
                "frontend".to_string(),
                "database".to_string(),
            ]),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_execute_is_linear() {
        let dag = HeuristicPlanner
            .plan("build X", PlanStrategy::PlanExecute)
            .unwrap();
        let layers = dag.topological_layers().unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["analyze"]);
    }

    #[test]
    fn reflective_has_reflect_node() {
        let dag = HeuristicPlanner
            .plan("build X", PlanStrategy::Reflective)
            .unwrap();
        assert!(dag.node("reflect").is_some());
    }

    #[test]
    fn multi_agent_fans_out_and_in() {
        let dag = HeuristicPlanner
            .plan("build product", PlanStrategy::MultiAgent)
            .unwrap();
        let layers = dag.topological_layers().unwrap();
        // architect | {backend, frontend, database} | review
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["architect"]);
        assert_eq!(layers[1].len(), 3);
        assert_eq!(layers[2], vec!["review"]);
        assert_eq!(
            dag.node("backend").unwrap().role.as_deref(),
            Some("backend")
        );
    }
}
