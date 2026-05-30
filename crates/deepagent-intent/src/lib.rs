//! # deepagent-intent
//!
//! The **input dispatch / intent recognition layer** — the Prompt Submission
//! layer from the 复刻规范 (原则 3 / P1) and the 对齐ClaudeCode计划 维度 1.
//!
//! Claude Code never feeds raw user input straight to the model. Input first
//! passes through a dispatch layer that:
//! 1. detects and resolves **slash commands** (`/triage …`) against a
//!    [`CommandRegistry`],
//! 2. normalizes **attachments** (`#File:…` mentions, images, pasted text),
//! 3. produces a single [`ExecutionRequest`] the runtime can act on.
//!
//! This crate provides exactly that, deterministically and without async or IO
//! dependencies, so it is trivially testable. Semantic skill matching lives in
//! `deepagent-skills`; frontmatter parsing for `.md` command files lives in
//! `deepagent-prompts`. The router consumes the *resolved* shapes.
//!
//! ## Flow
//!
//! ```text
//! raw input ──► IntentRouter::route ──► ExecutionRequest { intent, prompt, attachments, allowed_tools }
//!                     │
//!                     ├─ "/name args"   → Intent::SlashCommand  (body rendered, allowed-tools applied)
//!                     ├─ "/unknown …"   → Intent::UnknownCommand (falls back to raw text)
//!                     └─ anything else  → Intent::Chat           (#path mentions lifted to attachments)
//! ```
//!
//! The produced [`ExecutionRequest`] is what the runtime turns into a task,
//! firing the `UserPromptSubmit` hook along the way (wired in the runtime).

#![warn(missing_docs)]

pub mod attachment;
pub mod command;
pub mod request;
pub mod router;

pub use attachment::{extract_mentions, Attachment, AttachmentKind};
pub use command::{CommandDef, CommandRegistry, ARGUMENTS_PLACEHOLDER};
pub use request::{ExecutionRequest, Intent};
pub use router::IntentRouter;

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end: a registry + router resolve a command and a chat turn.
    #[test]
    fn end_to_end_dispatch() {
        let mut reg = CommandRegistry::new();
        reg.register(
            CommandDef::new("commit", "Create a commit")
                .with_body("Create a conventional commit for:\n$ARGUMENTS")
                .with_allowed_tools(["Bash(git:*)".to_string()]),
        );
        let router = IntentRouter::new(reg);

        // Command path.
        let cmd = router.route("/commit fix the parser bug");
        assert!(cmd.intent.is_command());
        assert!(cmd.prompt.contains("fix the parser bug"));
        assert!(cmd.restricts_tools());

        // Chat path with a file mention.
        let chat = router.route("explain #src/parser.rs to me");
        assert_eq!(chat.intent, Intent::Chat);
        assert!(chat.has_attachments());
        assert_eq!(chat.attachments[0].value, "src/parser.rs");
    }
}
