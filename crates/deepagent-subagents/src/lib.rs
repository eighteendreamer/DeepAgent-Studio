//! # deepagent-subagents
//!
//! The SubAgent runtime (开发计划.md Phase 6 §2–§4; 开发提示词.md §5, §11, §12).
//!
//! Turns a [`deepagent_planner::PlanDag`] into coordinated execution across
//! isolated sub-agents:
//!
//! - [`scheduler`] — the [`scheduler::DagScheduler`] runs the DAG layer by layer
//!   (fan-out within a layer, fan-in between layers), feeding each node its
//!   upstream dependencies' summaries.
//! - [`subagent`]  — the per-agent [`subagent::SubAgentContext`] (isolated
//!   context/role/worktree) and the [`subagent::SubAgentExecutor`] trait the
//!   scheduler drives.
//! - [`worktree`]  — [`worktree::WorktreeProvider`] for git-worktree isolation
//!   so parallel agents never clobber each other's code.
//!
//! This crate depends only on `deepagent-planner` (for the DAG); the concrete
//! agent loop is injected by the runtime via [`subagent::SubAgentExecutor`],
//! avoiding a dependency cycle.

pub mod scheduler;
pub mod subagent;
pub mod worktree;

pub use scheduler::{DagScheduler, ScheduleReport};
pub use subagent::{context_for, SubAgentContext, SubAgentExecutor, SubAgentResult};
pub use worktree::{GitWorktrees, InMemoryWorktrees, Worktree, WorktreeProvider};
