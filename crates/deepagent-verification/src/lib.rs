//! # deepagent-verification
//!
//! The self-healing Verification Loop (开发计划.md Phase 7; 开发提示词.md §10).
//!
//! Claude Code's real strength is not generation but **self-verification**. This
//! crate implements that loop:
//!
//! ```text
//! VERIFY -> (pass? done) -> REFLECT -> FIX -> RETRY -> VERIFY -> ...
//! ```
//!
//! Components:
//! - [`runner`]     — [`runner::CommandRunner`] abstraction (real process runner
//!   + offline mock) so verification is testable without spawning processes.
//! - [`verifier`]   — runs an ordered suite of build/test/lint
//!   [`verifier::VerificationStep`]s and classifies failures.
//! - [`reflection`] — turns failures into structured [`reflection::Reflection`]s
//!   and performs **loop detection** (the "无限循环可检测" criterion).
//! - [`healing`]    — the [`healing::SelfHealingLoop`] that drives a pluggable
//!   [`healing::Fixer`] until verification passes or it gives up.
//!
//! The runtime wires these into the loop's VERIFY / REFLECT phases and the
//! `VerificationFailed` hook.

pub mod healing;
pub mod reflection;
pub mod runner;
pub mod verifier;

pub use healing::{Fixer, HealingOutcome, SelfHealingLoop};
pub use reflection::{NextAction, Reflection, ReflectionEngine};
pub use runner::{Command, CommandOutput, CommandRunner, MockRunner, SystemCommandRunner};
pub use verifier::{FailureKind, StepResult, VerificationReport, VerificationStep, Verifier};
