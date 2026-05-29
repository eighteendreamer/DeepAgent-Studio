//! A model-driven [`Agent`] implementation.
//!
//! [`ModelAgent`] turns the scripted demo brain into a real one backed by a
//! [`ModelClient`]. It maintains the running conversation, advertises the
//! available tools, and translates the model's streamed [`ChatResponse`] into
//! the runtime's [`AgentDecision`] vocabulary.
//!
//! Thinking Mode persistence (开发计划.md Phase 2 §5): when the model returns a
//! turn that contains tool calls, the assistant message — including its
//! `reasoning_content` — is appended to the conversation so it is replayed on
//! the next request, exactly as DeepSeek's Thinking Mode protocol requires.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role};
use deepagent_models::chat::FinishReason;
use deepagent_models::{ChatRequest, ModelClient, ToolSchema};
use deepagent_tools::ToolInvocation;

use crate::agent::{Agent, AgentDecision, Observation};

/// An [`Agent`] that delegates decision-making to a model.
pub struct ModelAgent {
    client: Arc<ModelClient>,
    model: String,
    /// Running conversation, seeded with system + user messages.
    messages: Vec<Message>,
    /// Tool schemas advertised to the model each turn.
    tools: Vec<ToolSchema>,
    /// Most recent tool call id, used to correlate the next tool result.
    pending_tool_call_id: Option<String>,
}

impl ModelAgent {
    /// Build a model agent.
    ///
    /// `system` is the system prompt; `goal` is the user's task. `tools` are the
    /// schemas the model may call (typically derived from the
    /// `ToolRegistry`'s visible set for the agent's permissions).
    pub fn new(
        client: Arc<ModelClient>,
        model: impl Into<String>,
        system: impl Into<String>,
        goal: impl Into<String>,
        tools: Vec<ToolSchema>,
    ) -> Self {
        Self {
            client,
            model: model.into(),
            messages: vec![Message::system(system), Message::user(goal)],
            tools,
            pending_tool_call_id: None,
        }
    }

    /// The current conversation (for inspection / persistence).
    pub fn conversation(&self) -> &[Message] {
        &self.messages
    }

    /// Record a tool observation as a `tool` role message correlated to the
    /// pending tool-call id.
    fn record_observation(&mut self, obs: &Observation) {
        let call_id = self
            .pending_tool_call_id
            .take()
            .unwrap_or_else(|| format!("call_{}", obs.tool));
        let content = serde_json::to_string(&obs.output).unwrap_or_else(|_| obs.output.to_string());
        self.messages.push(Message::tool_result(call_id, content));
    }
}

#[async_trait]
impl Agent for ModelAgent {
    async fn think(&mut self, _step: usize, last: Option<&Observation>) -> Result<AgentDecision> {
        // Feed back the previous tool result, if any.
        if let Some(obs) = last {
            self.record_observation(obs);
        }

        let request = ChatRequest::new(self.model.clone(), self.messages.clone())
            .with_tools(self.tools.clone());
        let response = self.client.stream_chat(request).await?;

        // Persist the assistant turn. Thinking Mode: keep reasoning_content when
        // the turn carries tool calls so it is replayed next request.
        let mut assistant = Message::text(Role::Assistant, response.message.content.clone());
        assistant.tool_calls = response.message.tool_calls.clone();
        if response.message.has_tool_calls() {
            assistant.reasoning_content = response.message.reasoning_content.clone();
        }
        self.messages.push(assistant);

        // Decide the next action.
        if let Some(call) = response.message.tool_calls.first() {
            self.pending_tool_call_id = Some(call.id.clone());
            return Ok(AgentDecision::CallTool(ToolInvocation::new(
                call.name.clone(),
                call.arguments.clone(),
            )));
        }

        match response.finish_reason {
            Some(FinishReason::ContentFilter) => {
                Err(CoreError::other("model stopped due to content filter"))
            }
            _ => Ok(AgentDecision::Complete(response.message.content)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::{MockTransport, ModelConfig};

    fn client(events: Vec<String>) -> Arc<ModelClient> {
        let transport = Arc::new(MockTransport::new(events));
        Arc::new(ModelClient::new(transport, ModelConfig::deepseek("test")))
    }

    #[tokio::test]
    async fn completes_when_model_returns_text() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"All done."},"finish_reason":"stop"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let mut agent = ModelAgent::new(client(events), "deepseek-chat", "sys", "do it", vec![]);
        let decision = agent.think(0, None).await.unwrap();
        assert_eq!(decision, AgentDecision::Complete("All done.".to_string()));
        // System + user + assistant.
        assert_eq!(agent.conversation().len(), 3);
    }

    #[tokio::test]
    async fn requests_tool_then_persists_reasoning() {
        let events = vec![
            r#"{"choices":[{"delta":{"reasoning_content":"I should add."}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let mut agent = ModelAgent::new(client(events), "deepseek-reasoner", "sys", "add", vec![]);
        let decision = agent.think(0, None).await.unwrap();
        match decision {
            AgentDecision::CallTool(inv) => {
                assert_eq!(inv.name, "add");
                assert_eq!(inv.arguments, serde_json::json!({"a":1,"b":2}));
            }
            other => panic!("expected CallTool, got {other:?}"),
        }
        // The assistant turn retained reasoning_content (tool-call turn).
        let assistant = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Assistant)
            .unwrap();
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("I should add.")
        );
        assert_eq!(agent.pending_tool_call_id.as_deref(), Some("c1"));
    }

    #[tokio::test]
    async fn feeds_observation_back_as_tool_message() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"sum is 3"},"finish_reason":"stop"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let mut agent = ModelAgent::new(client(events), "deepseek-chat", "sys", "add", vec![]);
        agent.pending_tool_call_id = Some("c1".to_string());
        let obs = Observation {
            tool: "add".to_string(),
            ok: true,
            output: serde_json::json!({"sum": 3}),
        };
        agent.think(1, Some(&obs)).await.unwrap();
        // A tool-role message correlated to c1 was inserted.
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("c1"));
        assert!(tool_msg.content.contains("sum"));
    }
}
