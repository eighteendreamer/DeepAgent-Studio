//! # deepagent-core
//!
//! Foundational primitives for the DeepAgent Runtime Kernel.
//!
//! This crate is intentionally dependency-light and has **no** async, IO, or
//! database dependencies. Everything else in the workspace (`deepagent-session`,
//! `deepagent-runtime`, `deepagent-context`, ...) builds on the types defined
//! here so that the domain model stays consistent across the whole system.
//!
//! Modules:
//! - [`id`]      — strongly-typed identifiers (session, task, event, agent).
//! - [`error`]   — the unified [`error::CoreError`] / [`Result`] types.
//! - [`clock`]   — monotonic-ish timestamps and a testable [`clock::Clock`].
//! - [`event`]   — the append-only [`event::Event`] envelope and payloads.
//! - [`task`]    — the [`task::TaskState`] machine used by the runtime.
//! - [`message`] — conversation [`message::Message`] / role primitives.

pub mod clock;
pub mod error;
pub mod event;
pub mod id;
pub mod message;
pub mod task;

pub use error::{CoreError, Result};
