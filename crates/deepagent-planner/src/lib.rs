//! # deepagent-planner
//!
//! The planning engine (开发计划.md Phase 6 §1, §4).
//!
//! Decomposes a high-level goal into a validated **DAG** of subtasks that the
//! sub-agent scheduler executes. The DAG ([`dag::PlanDag`]) is the shared
//! foundation: it validates dependencies, rejects cycles, and groups nodes into
//! parallel [`dag::PlanDag::topological_layers`] for fan-out / fan-in execution.
//!
//! [`Planner`] supports plan-execute / reflective / multi-agent strategies; the
//! model-free [`HeuristicPlanner`] is the default/fallback implementation.

pub mod dag;
pub mod planner;

pub use dag::{NodeId, PlanDag, PlanNode};
pub use planner::{HeuristicPlanner, PlanStrategy, Planner};
