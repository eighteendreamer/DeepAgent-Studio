//! The [`IntentRouter`] — the entry point of the input dispatch layer.
//!
//! It takes raw user input (text + any pre-attached items), classifies it, and
//! produces a normalized [`ExecutionRequest`]:
//!
//! 1. **Slash detection** — input starting with `/` is parsed as `/name args`
//!    and resolved against the [`CommandRegistry`]. Resolved commands render
//!    their body (with `$ARGUMENTS`) into the effective prompt and contribute
//!    their `allowed-tools`.
//! 2. **Mention lifting** — `#path` / `#File:path` tokens in the text are
//!    lifted into [`Attachment`]s (merged with any caller-supplied ones).
//! 3. **Fallthrough** — everything else is `Intent::Chat`.
//!
//! The router is deliberately deterministic and dependency-light: semantic
//! skill matching lives in `deepagent-skills`, and frontmatter parsing in
//! `deepagent-prompts`. This keeps the dispatch layer testable in isolation.

use crate::attachment::{extract_mentions, Attachment};
use crate::command::CommandRegistry;
use crate::request::{ExecutionRequest, Intent};

/// Classifies raw input into an [`ExecutionRequest`].
#[derive(Debug, Default)]
pub struct IntentRouter {
    commands: CommandRegistry,
    /// Whether to lift `#path` mentions in text into attachments.
    lift_mentions: bool,
}

impl IntentRouter {
    /// Build a router over a command registry, with mention-lifting enabled.
    pub fn new(commands: CommandRegistry) -> Self {
        Self {
            commands,
            lift_mentions: true,
        }
    }

    /// Disable lifting `#path` mentions into attachments (builder-style).
    pub fn without_mention_lifting(mut self) -> Self {
        self.lift_mentions = false;
        self
    }

    /// The command registry (for inspection).
    pub fn commands(&self) -> &CommandRegistry {
        &self.commands
    }

    /// Route raw input with no caller-supplied attachments.
    pub fn route(&self, input: &str) -> ExecutionRequest {
        self.route_with(input, Vec::new())
    }

    /// Route raw input plus any caller-supplied attachments (e.g. dragged-in
    /// images). Mentions in chat text are appended to these. Slash-command
    /// input is not mention-lifted (a `#` there is typically an argument such
    /// as an issue number, not a file reference).
    pub fn route_with(&self, input: &str, attachments: Vec<Attachment>) -> ExecutionRequest {
        let raw_text = input.to_string();
        let trimmed = input.trim_start();

        // Slash command? Require a non-space char immediately after `/`.
        if let Some(rest) = trimmed.strip_prefix('/') {
            let (name, args) = split_command(rest);
            // Empty `/` or `/ …` (no command name) is just chat.
            if !name.is_empty() {
                if let Some(def) = self.commands.get(name) {
                    let prompt = def.render(args);
                    tracing::debug!(command = name, "routed slash command");
                    return ExecutionRequest {
                        intent: Intent::SlashCommand {
                            name: name.to_string(),
                            arguments: args.to_string(),
                        },
                        raw_text,
                        prompt,
                        attachments,
                        allowed_tools: def.allowed_tools.clone(),
                    };
                }
                tracing::debug!(command = name, "unknown slash command");
                return ExecutionRequest {
                    intent: Intent::UnknownCommand {
                        name: name.to_string(),
                    },
                    raw_text: raw_text.clone(),
                    prompt: raw_text,
                    attachments,
                    allowed_tools: Vec::new(),
                };
            }
        }

        // Plain chat: lift `#path` mentions from the text into file attachments.
        let mut attachments = attachments;
        if self.lift_mentions {
            for path in extract_mentions(input) {
                let exists = attachments
                    .iter()
                    .any(|a| a.value == path && a.kind == crate::attachment::AttachmentKind::File);
                if !exists {
                    attachments.push(Attachment::file(path));
                }
            }
        }

        ExecutionRequest {
            intent: Intent::Chat,
            prompt: raw_text.clone(),
            raw_text,
            attachments,
            allowed_tools: Vec::new(),
        }
    }
}

/// Split `name args` (already stripped of the leading `/`) into `(name, args)`.
/// The name runs to the first whitespace; args is the trimmed remainder.
fn split_command(rest: &str) -> (&str, &str) {
    match rest.find(char::is_whitespace) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandDef;

    fn router() -> IntentRouter {
        let mut reg = CommandRegistry::new();
        reg.register(
            CommandDef::new("triage", "Triage issues")
                .with_body("Analyze the issue.\n\nContext:\n$ARGUMENTS")
                .with_allowed_tools(["Bash(gh:*)".to_string()]),
        );
        IntentRouter::new(reg)
    }

    #[test]
    fn plain_text_is_chat() {
        let req = router().route("how do I add a test?");
        assert_eq!(req.intent, Intent::Chat);
        assert_eq!(req.prompt, "how do I add a test?");
        assert!(req.allowed_tools.is_empty());
    }

    #[test]
    fn resolves_known_slash_command() {
        let req = router().route("/triage issue #42 is flaky");
        assert_eq!(
            req.intent,
            Intent::SlashCommand {
                name: "triage".into(),
                arguments: "issue #42 is flaky".into()
            }
        );
        assert!(req.prompt.contains("issue #42 is flaky"));
        assert!(req.prompt.contains("Analyze the issue"));
        assert_eq!(req.allowed_tools, vec!["Bash(gh:*)".to_string()]);
    }

    #[test]
    fn unknown_slash_command_is_flagged() {
        let req = router().route("/nope do something");
        assert_eq!(
            req.intent,
            Intent::UnknownCommand {
                name: "nope".into()
            }
        );
        // Prompt falls back to the raw text so nothing is lost.
        assert_eq!(req.prompt, "/nope do something");
    }

    #[test]
    fn command_with_no_args() {
        let req = router().route("/triage");
        assert_eq!(
            req.intent,
            Intent::SlashCommand {
                name: "triage".into(),
                arguments: String::new()
            }
        );
    }

    #[test]
    fn lifts_hash_mentions_into_attachments() {
        let req = router().route("please review #src/main.rs and #Cargo.toml");
        assert_eq!(req.intent, Intent::Chat);
        assert_eq!(req.attachments.len(), 2);
        assert!(req.attachments.iter().any(|a| a.value == "src/main.rs"));
    }

    #[test]
    fn merges_caller_attachments_without_duplicates() {
        let req = router().route_with(
            "review #src/main.rs",
            vec![
                Attachment::file("src/main.rs"),
                Attachment::image("diagram.png"),
            ],
        );
        // The mention duplicates an existing file attachment → not added twice.
        let files = req
            .attachments
            .iter()
            .filter(|a| a.value == "src/main.rs")
            .count();
        assert_eq!(files, 1);
        assert_eq!(req.attachments.len(), 2);
    }

    #[test]
    fn mention_lifting_can_be_disabled() {
        let mut reg = CommandRegistry::new();
        reg.register(CommandDef::new("x", "x"));
        let router = IntentRouter::new(reg).without_mention_lifting();
        let req = router.route("review #src/main.rs");
        assert!(req.attachments.is_empty());
    }

    #[test]
    fn bare_slash_is_chat() {
        let req = router().route("/ this is not a command");
        assert_eq!(req.intent, Intent::Chat);
    }

    #[test]
    fn leading_whitespace_before_slash() {
        let req = router().route("   /triage hi");
        assert!(req.intent.is_command());
    }
}
