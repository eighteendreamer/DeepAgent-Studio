//! The agent "brain" abstraction.
//!
//! The runtime loop is model-agnostic: it drives anything implementing
//! [`Agent`]. In production an [`Agent`] wraps a DeepSeek client (with Thinking
//! Mode reasoning persistence); in tests it is a deterministic stub. This keeps
//! the loop logic fully unit-testable without network access.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::ToolInvocation;

/// What the agent decided to do on a given `think` step.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentDecision {
    /// Invoke a tool. The loop will execute it and feed back an [`Observation`].
    CallTool(ToolInvocation),
    /// The task is finished; carry a final message.
    Complete(String),
    /// The agent cannot proceed and needs human input / approval.
    NeedsApproval(String),
}

/// Feedback handed back to the agent after a tool runs (开发提示词.md §17).
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// The tool that produced this observation.
    pub tool: String,
    /// Whether the tool succeeded.
    pub ok: bool,
    /// JSON output / error detail.
    pub output: serde_json::Value,
}

/// The agent brain. Implementations decide the next [`AgentDecision`] given the
/// accumulated observations so far.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Produce the next decision. `step` is the 0-based iteration index;
    /// `last` is the most recent observation (None on the first step).
    async fn think(&mut self, step: usize, last: Option<&Observation>) -> Result<AgentDecision>;
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted agent that returns a fixed sequence of decisions, for tests.
    pub struct ScriptedAgent {
        pub script: VecDeque<AgentDecision>,
        pub observations: Vec<Observation>,
    }

    impl ScriptedAgent {
        pub fn new(decisions: impl IntoIterator<Item = AgentDecision>) -> Self {
            Self {
                script: decisions.into_iter().collect(),
                observations: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl Agent for ScriptedAgent {
        async fn think(
            &mut self,
            _step: usize,
            last: Option<&Observation>,
        ) -> Result<AgentDecision> {
            if let Some(obs) = last {
                self.observations.push(obs.clone());
            }
            Ok(self
                .script
                .pop_front()
                .unwrap_or_else(|| AgentDecision::Complete("script exhausted".into())))
        }
    }
}
