//! # deepagent-intent
//!
//! Slash-command dispatch: the two registries the runtime actually consults
//! before a prompt reaches the model.
//!
//! - [`SlashRegistry`] — code-defined commands (`/help`, `/compact`, …) that
//!   return a [`SlashAction`] the caller executes.
//! - [`CommandRegistry`] / [`CommandDef`] — prompt-template commands loaded from
//!   `.md` files (`deepagent-prompts` parses frontmatter into `CommandDef`),
//!   including `$ARGUMENTS` rendering and the allowed-tools restriction.
//!
//! ## What this crate deliberately is not
//!
//! There is no intent classifier, slot filler, or "router" producing an
//! `ExecutionRequest`. The reference implementation this project aligns with
//! routes input by *what the model chooses to call*, not by pattern-matching the
//! user's text; an earlier version of this crate carried such a layer plus an
//! `#file` mention extractor with zero production callers, and both were removed
//! rather than left to rot alongside the live path. Input normalization lives in
//! `deepagent_runtime::InputIngress`, and `@`-mention extraction stays where it
//! has always been: the composer.

#![warn(missing_docs)]

pub mod command;
pub mod slash;

pub use command::{CommandDef, CommandRegistry, ARGUMENTS_PLACEHOLDER};
pub use slash::{
    CommandContext, CommandResult, SlashAction, SlashCommand, SlashHandler, SlashRegistry,
};
