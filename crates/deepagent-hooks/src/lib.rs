//! # deepagent-hooks
//!
//! The Hook Lifecycle (开发提示词.md §13).
//!
//! Hooks are pluggable units of logic that fire at well-defined points in the
//! runtime loop — from `SessionStart`, through the `BeforeToolUse` /
//! `AfterToolUse` gates around every tool call, to `SessionEnd`. They are the
//! primary extension mechanism for policy, observability, and safety.
//!
//! ## Key concepts
//!
//! - [`HookPoint`] — the lifecycle points (the 8 hooks from the blueprint).
//! - [`Hook`] — the async trait an extension implements.
//! - [`HookOutcome`] — `Continue` or `Deny(reason)`.
//! - [`HookRegistry`] — registers hooks per point and dispatches them, honoring
//!   veto semantics (only "before" gates can halt the loop).
//! - [`builtin`] — ready-made hooks (tool allow-listing, argument guarding).
//!
//! ## Veto / access-control
//!
//! A `BeforeToolUse` hook that returns `Deny` blocks the tool call — this is the
//! access-control pattern. At observational points (`AfterToolUse`, …) a `Deny`
//! cannot halt anything and is downgraded to a warning.

pub mod builtin;
pub mod hook;
pub mod lifecycle;
pub mod registry;

pub use builtin::{ArgumentGuardHook, ToolAllowlistHook};
pub use hook::{Hook, HookOutcome};
pub use lifecycle::{HookContext, HookData, HookPoint};
pub use registry::HookRegistry;
