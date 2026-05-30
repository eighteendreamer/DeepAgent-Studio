//! The [`SystemPromptBuilder`] — assembles a Claude-Code-structured system
//! prompt over the context Prompt AST ([`deepagent_context::PromptFragment`]).
//!
//! Claude Code's system prompt follows a stable layered structure
//! (System Core → Safety → Workspace Rules → Agent Identity → Tool Rules →
//! Memory → Context → User Goal). That maps one-to-one onto the existing
//! [`deepagent_context::PromptSource`] ordering, so this builder just collects
//! typed sections and emits ordered [`PromptFragment`]s that the
//! [`deepagent_context::ContextPipeline`] / budgeter can consume directly.
//!
//! An [`AgentDef`] contributes the `AgentIdentity` section (its body) and its
//! tool allow-list feeds the `ToolRules` section, so a sub-agent's persona and
//! permitted tools are assembled consistently.

use deepagent_context::prompt::PromptCompiler;
use deepagent_context::{CompiledPrompt, PromptFragment, PromptSource, TokenCounter};

use crate::agent_def::AgentDef;

/// Priorities for the assembled sections (higher survives budget pressure).
mod prio {
    pub const CORE: u8 = u8::MAX;
    pub const SAFETY: u8 = u8::MAX;
    pub const WORKSPACE: u8 = 200;
    pub const IDENTITY: u8 = 190;
    pub const TOOLS: u8 = 180;
    pub const MEMORY: u8 = 90;
    pub const CONTEXT: u8 = 120;
    pub const USER_GOAL: u8 = u8::MAX;
}

/// Builds a layered system prompt from typed sections.
#[derive(Debug, Default)]
pub struct SystemPromptBuilder {
    core: Option<String>,
    safety: Vec<String>,
    workspace: Vec<String>,
    identity: Option<String>,
    tool_rules: Vec<String>,
    memory: Vec<String>,
    context: Vec<String>,
    user_goal: Option<String>,
}

impl SystemPromptBuilder {
    /// New empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the immutable system-core identity (never dropped).
    pub fn core(mut self, text: impl Into<String>) -> Self {
        self.core = Some(text.into());
        self
    }

    /// Add a safety/guardrail rule (never dropped).
    pub fn safety(mut self, text: impl Into<String>) -> Self {
        self.safety.push(text.into());
        self
    }

    /// Add a workspace rule (steering / project convention).
    pub fn workspace_rule(mut self, text: impl Into<String>) -> Self {
        self.workspace.push(text.into());
        self
    }

    /// Set the agent identity section directly.
    pub fn identity(mut self, text: impl Into<String>) -> Self {
        self.identity = Some(text.into());
        self
    }

    /// Add a tool-usage rule.
    pub fn tool_rule(mut self, text: impl Into<String>) -> Self {
        self.tool_rules.push(text.into());
        self
    }

    /// Add an injected memory block.
    pub fn memory(mut self, text: impl Into<String>) -> Self {
        self.memory.push(text.into());
        self
    }

    /// Add a retrieved/conversational context block.
    pub fn context(mut self, text: impl Into<String>) -> Self {
        self.context.push(text.into());
        self
    }

    /// Set the user goal for this turn (never dropped).
    pub fn user_goal(mut self, text: impl Into<String>) -> Self {
        self.user_goal = Some(text.into());
        self
    }

    /// Apply an [`AgentDef`]: its body becomes the identity section and its
    /// declared tools become a tool-rules line.
    pub fn with_agent(mut self, agent: &AgentDef) -> Self {
        self.identity = Some(agent.body.clone());
        if agent.restricts_tools() {
            self.tool_rules.push(format!(
                "You may only use these tools: {}.",
                agent.tools.join(", ")
            ));
        }
        self
    }

    /// Collect the typed sections into ordered [`PromptFragment`]s.
    pub fn fragments(&self) -> Vec<PromptFragment> {
        let mut out = Vec::new();
        if let Some(core) = &self.core {
            out.push(PromptFragment::new(
                PromptSource::SystemCore,
                prio::CORE,
                core,
            ));
        }
        for s in &self.safety {
            out.push(PromptFragment::new(
                PromptSource::SafetyRules,
                prio::SAFETY,
                s,
            ));
        }
        for w in &self.workspace {
            out.push(PromptFragment::new(
                PromptSource::WorkspaceRules,
                prio::WORKSPACE,
                w,
            ));
        }
        if let Some(id) = &self.identity {
            out.push(PromptFragment::new(
                PromptSource::AgentIdentity,
                prio::IDENTITY,
                id,
            ));
        }
        for t in &self.tool_rules {
            out.push(PromptFragment::new(PromptSource::ToolRules, prio::TOOLS, t));
        }
        for m in &self.memory {
            out.push(PromptFragment::new(PromptSource::Memory, prio::MEMORY, m));
        }
        for c in &self.context {
            out.push(PromptFragment::new(PromptSource::Context, prio::CONTEXT, c));
        }
        if let Some(goal) = &self.user_goal {
            out.push(PromptFragment::new(
                PromptSource::UserGoal,
                prio::USER_GOAL,
                goal,
            ));
        }
        out
    }

    /// Compile the assembled prompt (canonical order) without budget pressure.
    pub fn compile(&self, counter: &dyn TokenCounter) -> CompiledPrompt {
        let mut compiler = PromptCompiler::new();
        for frag in self.fragments() {
            compiler.push(frag);
        }
        compiler.compile(counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_context::HeuristicTokenizer;

    #[test]
    fn assembles_in_canonical_order() {
        let counter = HeuristicTokenizer::new();
        let compiled = SystemPromptBuilder::new()
            .core("You are DeepAgent.")
            .safety("Never run destructive commands without approval.")
            .workspace_rule("Match the project's style.")
            .identity("You are a Rust backend specialist.")
            .tool_rule("Prefer read tools before write tools.")
            .memory("User prefers small modules.")
            .context("Recent: building Phase 11.")
            .user_goal("Add prompt assembly.")
            .compile(&counter);

        let sources: Vec<_> = compiled.fragments.iter().map(|f| f.source).collect();
        assert_eq!(
            sources,
            vec![
                PromptSource::SystemCore,
                PromptSource::SafetyRules,
                PromptSource::WorkspaceRules,
                PromptSource::AgentIdentity,
                PromptSource::ToolRules,
                PromptSource::Memory,
                PromptSource::Context,
                PromptSource::UserGoal,
            ]
        );
        assert!(compiled.tokens > 0);
        assert!(compiled.text.contains("You are DeepAgent."));
    }

    #[test]
    fn with_agent_sets_identity_and_tools() {
        let counter = HeuristicTokenizer::new();
        let agent = AgentDef::parse(
            "---\nname: arch\ndescription: d\ntools: Read, Grep\nmodel: inherit\n---\nYou are an architect.",
        )
        .unwrap();
        let compiled = SystemPromptBuilder::new()
            .core("core")
            .with_agent(&agent)
            .user_goal("design it")
            .compile(&counter);

        assert!(compiled.text.contains("You are an architect."));
        assert!(compiled.text.contains("Read, Grep"));
        // Identity precedes tool rules precedes user goal.
        let id_pos = compiled.text.find("architect").unwrap();
        let tool_pos = compiled.text.find("only use these tools").unwrap();
        let goal_pos = compiled.text.find("design it").unwrap();
        assert!(id_pos < tool_pos && tool_pos < goal_pos);
    }

    #[test]
    fn empty_builder_yields_no_fragments() {
        assert!(SystemPromptBuilder::new().fragments().is_empty());
    }
}
