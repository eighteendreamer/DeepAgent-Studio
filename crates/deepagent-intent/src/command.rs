//! Slash-command definitions and registry.
//!
//! Aligned with Claude Code's `commands/<name>.md`: each command has a name, a
//! `description`, an optional `allowed-tools` allow-list, and a body template
//! that may contain the `$ARGUMENTS` placeholder. A command may opt out of
//! autonomous model invocation (`disable_model_invocation`).
//!
//! This module models the *definition* + a [`CommandRegistry`] for lookup. The
//! frontmatter/body of real `.md` files is parsed by `deepagent-prompts`
//! (Phase 11); here we keep the runtime-facing shape so the router can resolve
//! `/name args` to a concrete command.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The `$ARGUMENTS` placeholder substituted with the user's argument string.
pub const ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";

/// A slash-command definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDef {
    /// Command name (without the leading `/`).
    pub name: String,
    /// Human/model-facing description (from frontmatter `description`).
    pub description: String,
    /// Body template; `$ARGUMENTS` is replaced with the invocation arguments.
    #[serde(default)]
    pub body: String,
    /// Allow-listed tool patterns (frontmatter `allowed-tools`), e.g.
    /// `Bash(git:*)`. Empty means "inherit session defaults".
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// If true, the model may not invoke this command autonomously (it is
    /// user-only). Mirrors Claude Code's `disable-model-invocation`.
    #[serde(default)]
    pub disable_model_invocation: bool,
}

impl CommandDef {
    /// Build a minimal command (name + description).
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: String::new(),
            allowed_tools: Vec::new(),
            disable_model_invocation: false,
        }
    }

    /// Set the body template (builder-style).
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Set the allowed-tools allow-list (builder-style).
    pub fn with_allowed_tools(mut self, tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = tools.into_iter().collect();
        self
    }

    /// Mark the command user-only (builder-style).
    pub fn user_only(mut self) -> Self {
        self.disable_model_invocation = true;
        self
    }

    /// Render the body with `$ARGUMENTS` replaced by `args`. If the body has no
    /// placeholder and `args` is non-empty, the args are appended on a new line
    /// (matching Claude Code's "context goes after the prompt" behavior).
    pub fn render(&self, args: &str) -> String {
        if self.body.contains(ARGUMENTS_PLACEHOLDER) {
            self.body.replace(ARGUMENTS_PLACEHOLDER, args)
        } else if args.is_empty() {
            self.body.clone()
        } else if self.body.is_empty() {
            args.to_string()
        } else {
            format!("{}\n\n{}", self.body, args)
        }
    }
}

/// A registry of slash commands keyed by name.
#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, CommandDef>,
}

impl CommandRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert/replace a command. Returns the registry for chaining.
    pub fn register(&mut self, def: CommandDef) -> &mut Self {
        self.commands.insert(def.name.clone(), def);
        self
    }

    /// Look up a command by name (without the leading `/`).
    pub fn get(&self, name: &str) -> Option<&CommandDef> {
        self.commands.get(name)
    }

    /// Whether a command with `name` exists.
    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// All command names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    /// Commands the model is allowed to invoke autonomously.
    pub fn model_invokable(&self) -> Vec<&CommandDef> {
        self.commands
            .values()
            .filter(|c| !c.disable_model_invocation)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_placeholder() {
        let cmd = CommandDef::new("triage", "Triage issues")
            .with_body("Analyze the issue.\n\nContext:\n$ARGUMENTS");
        let rendered = cmd.render("issue #42");
        assert!(rendered.contains("issue #42"));
        assert!(!rendered.contains(ARGUMENTS_PLACEHOLDER));
    }

    #[test]
    fn render_appends_when_no_placeholder() {
        let cmd = CommandDef::new("review", "Review").with_body("Review the code.");
        assert_eq!(cmd.render("foo.rs"), "Review the code.\n\nfoo.rs");
        assert_eq!(cmd.render(""), "Review the code.");
    }

    #[test]
    fn registry_lookup_and_filtering() {
        let mut reg = CommandRegistry::new();
        reg.register(CommandDef::new("a", "A"));
        reg.register(CommandDef::new("b", "B").user_only());
        assert!(reg.contains("a"));
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.model_invokable().len(), 1);
        assert_eq!(reg.model_invokable()[0].name, "a");
    }

    #[test]
    fn names_are_sorted() {
        let mut reg = CommandRegistry::new();
        reg.register(CommandDef::new("zeta", "z"));
        reg.register(CommandDef::new("alpha", "a"));
        assert_eq!(reg.names(), vec!["alpha", "zeta"]);
    }

    #[test]
    fn allowed_tools_builder() {
        let cmd = CommandDef::new("gh", "GitHub")
            .with_allowed_tools(["Bash(gh:*)".to_string(), "read_file".to_string()]);
        assert_eq!(cmd.allowed_tools.len(), 2);
    }
}
