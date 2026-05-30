//! The normalized [`ExecutionRequest`] — the single artifact the input
//! dispatch layer produces and hands to the runtime.
//!
//! Raw user input (text + attachments) is classified into an [`Intent`] and
//! packaged here. The runtime never sees raw input; it only sees an
//! `ExecutionRequest`, which guarantees slash commands have been resolved,
//! attachments normalized, and the effective prompt computed.

use serde::{Deserialize, Serialize};

use crate::attachment::Attachment;

/// What the user's input was classified as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    /// Ordinary conversational/agent input — goes to the model/agent loop.
    Chat,
    /// A resolved slash command invocation.
    SlashCommand {
        /// The command name (without the leading `/`).
        name: String,
        /// The raw argument string after the command name.
        arguments: String,
    },
    /// A slash command was typed but no such command is registered.
    UnknownCommand {
        /// The attempted command name.
        name: String,
    },
}

impl Intent {
    /// Stable label for tracing.
    pub fn label(&self) -> &'static str {
        match self {
            Intent::Chat => "chat",
            Intent::SlashCommand { .. } => "slash_command",
            Intent::UnknownCommand { .. } => "unknown_command",
        }
    }

    /// Whether this intent is a successfully resolved slash command.
    pub fn is_command(&self) -> bool {
        matches!(self, Intent::SlashCommand { .. })
    }
}

/// The normalized request handed to the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// The classified intent.
    pub intent: Intent,
    /// The raw text the user submitted (verbatim).
    pub raw_text: String,
    /// The effective prompt to drive the run: for a slash command this is the
    /// rendered command body; for chat it equals the raw text (mentions
    /// stripped of nothing — kept verbatim).
    pub prompt: String,
    /// Normalized attachments lifted from the input.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Tool allow-list contributed by a resolved command (empty = session
    /// defaults). Mirrors Claude Code's per-command `allowed-tools`.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

impl ExecutionRequest {
    /// Whether the request carries any attachments.
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    /// Whether the request restricts tools to a command-provided allow-list.
    pub fn restricts_tools(&self) -> bool {
        !self.allowed_tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_labels() {
        assert_eq!(Intent::Chat.label(), "chat");
        assert_eq!(
            Intent::SlashCommand {
                name: "x".into(),
                arguments: String::new()
            }
            .label(),
            "slash_command"
        );
    }

    #[test]
    fn serde_roundtrip() {
        let req = ExecutionRequest {
            intent: Intent::SlashCommand {
                name: "triage".into(),
                arguments: "issue #1".into(),
            },
            raw_text: "/triage issue #1".into(),
            prompt: "Analyze issue #1".into(),
            attachments: vec![Attachment::file("a.rs")],
            allowed_tools: vec!["Bash(gh:*)".into()],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ExecutionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
        assert!(back.has_attachments());
        assert!(back.restricts_tools());
    }
}
