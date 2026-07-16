//! # deepagent-runtime
//!
//! The Agent Runtime Kernel (开发提示词.md §22; 开发计划.md Phase 2 §1).
//!
//! This crate models the canonical runtime loop:
//!
//! ```text
//! LOAD SESSION -> BUILD CONTEXT -> PLAN -> THINK -> SELECT TOOLS
//!   -> EXECUTE -> OBSERVE -> VERIFY -> REFLECT -> COMPACT -> SAVE -> LOOP
//! ```
//!
//! The loop here is the *control structure*: it drives a pluggable
//! [`agent::Agent`] (the "brain", which in production wraps a DeepSeek model)
//! through discrete steps, persisting every decision to the append-only event
//! store via [`deepagent_session`]. Because all state changes go through the
//! event log, a run can be stopped and resumed/replayed at any step.
//!
//! Model providers, planners, verification and reflection engines are layered
//! in over later phases behind the traits defined in [`agent`].

pub mod agent;
pub mod approval;
pub mod empty_stub;
pub mod events;
pub mod loop_engine;
pub mod model_agent;
pub mod phase;
pub mod tool_budget;
pub mod tool_result_decorator;

pub use agent::{Agent, AgentDecision, Observation};
pub use approval::{
    ApprovalDecision, ApprovalGate, ApprovalRequest, AutoApproveGate, AutoDenyGate,
};
pub use events::{
    tool_ui_metadata, ChannelSink, NullEventSink, RuntimeEvent, RuntimeEventSink, ToolUiMetadata,
};
pub use loop_engine::{PromptDecision, RunOutcome, RuntimeConfig, RuntimeEngine, VerificationPlan};
pub use model_agent::ModelAgent;
pub use phase::LoopPhase;
pub use tool_budget::ToolResultBudgetConfig;
pub use tool_result_decorator::{ChainDecorator, ToolResultDecorator};
