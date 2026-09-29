//! # deepagent-prompts
//!
//! Prompt engineering (开发提示词.md §6, §21; 对齐ClaudeCode计划 维度 6).
//!
//! This crate consolidates the *authoring* side of prompts — parsing
//! command/agent `.md` files and assembling a structured system prompt — on top
//! of the context Prompt AST ([`deepagent_context::PromptFragment`]) and the
//! input-dispatch [`deepagent_intent::CommandDef`].
//!
//! ## Pieces
//!
//! - [`frontmatter`] — a dependency-light YAML-frontmatter splitter supporting
//!   scalars, inline CSV lists, and YAML block lists.
//! - [`agent_def`] — Claude Code `agents/<name>.md` (name/description/tools/
//!   model/color plus Teammate fields `disallowedTools`/`permissionMode`/
//!   `maxTurns`/`effort`/`skills`/`background`/`isolation`, and the system-prompt
//!   body).
//! - [`command_loader`] — load `commands/<name>.md` into
//!   [`deepagent_intent::CommandDef`] (`$ARGUMENTS`, `allowed-tools`,
//!   `disable-model-invocation`).
//! - `builder` — **退役**（Teammate-1b）：layered `SystemPromptBuilder` had no
//!   production consumer; the live assembly path is `deepagent-context`'s
//!   [`deepagent_context::ContextAssembler::assemble`] (via `system_context.rs`).
//!   The module stays `#[doc(hidden)]` for one release so its tests keep
//!   compiling; do not export it.
//!
//! Everything here is synchronous, IO-light, and offline-testable; the actual
//! model call lives in `deepagent-models`, and budget-fitted assembly in
//! `deepagent-context`.

#![warn(missing_docs)]

pub mod agent_def;
#[doc(hidden)]
pub mod builder;
pub mod canvas_prompt;
pub mod command_loader;
pub mod frontmatter;

pub use agent_def::{AgentDef, EffortLevel, IsolationChoice, ModelPref, PermissionMode};
pub use command_loader::{discover_commands, load_command_file, parse_command};
pub use frontmatter::Frontmatter;

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_context::HeuristicTokenizer;

    /// End-to-end: parse an agent + a command, assemble a system prompt that
    /// includes the command's rendered body as context.
    #[test]
    fn end_to_end_assembly() {
        let agent = AgentDef::parse(
            "---\nname: reviewer\ndescription: Reviews code\ntools: Read, Grep\nmodel: deepseek-v4-pro\n---\nYou are a meticulous code reviewer.",
        )
        .unwrap();
        assert_eq!(agent.model, ModelPref::Named("deepseek-v4-pro".into()));

        let cmd = parse_command(
            "review",
            "---\ndescription: Review a diff\nallowed-tools: Read\n---\nReview this:\n$ARGUMENTS",
        );
        let rendered = cmd.render("diff --git a/x b/x");

        let counter = HeuristicTokenizer::new();
        let compiled = crate::builder::SystemPromptBuilder::new()
            .core("You are DeepAgent, a verifiable agent runtime.")
            .safety("Never exfiltrate secrets.")
            .with_agent(&agent)
            .context(rendered)
            .user_goal("Produce a review.")
            .compile(&counter);

        assert!(compiled.text.contains("meticulous code reviewer"));
        assert!(compiled.text.contains("Read, Grep"));
        assert!(compiled.text.contains("diff --git"));
        assert!(compiled.text.contains("Produce a review."));
    }
}
