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
use deepagent_models::chat::{FinishReason, ThinkingDepth};
use deepagent_models::{ChatRequest, DeltaObserver, ModelClient, ToolSchema};
use deepagent_tools::ToolInvocation;

use crate::agent::{Agent, AgentDecision, Observation};
use crate::events::{RuntimeEvent, RuntimeEventSink};

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
    /// Optional live event sink: when set, token/reasoning deltas are forwarded
    /// as [`RuntimeEvent`]s for streaming to a UI.
    events: Option<Arc<dyn RuntimeEventSink>>,
    /// Cumulative token usage summed across every model call this run.
    usage: crate::agent::RunUsage,
    /// DeepSeek Thinking Mode depth applied to every request.
    thinking_depth: ThinkingDepth,
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
            events: None,
            usage: crate::agent::RunUsage::default(),
            thinking_depth: ThinkingDepth::default(),
        }
    }

    /// Attach a live event sink so token/reasoning deltas stream out as
    /// [`RuntimeEvent`]s (builder style).
    pub fn with_events(mut self, events: Arc<dyn RuntimeEventSink>) -> Self {
        self.events = Some(events);
        self
    }

    /// Attach the user's DeepSeek Thinking Mode depth.
    pub fn with_thinking_depth(mut self, depth: ThinkingDepth) -> Self {
        self.thinking_depth = depth;
        self
    }

    /// Seed prior conversation turns (builder style) for **session
    /// continuation**: the `history` messages are inserted between the system
    /// prompt and the current user goal, so the model sees the earlier dialog
    /// when resuming an existing session. Pass plain user/assistant turns;
    /// the agent appends the live turn's messages on top as usual.
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        if !history.is_empty() {
            // messages == [system, goal]; reinsert as [system, history.., goal].
            let goal = self.messages.pop().expect("seeded with system + goal");
            self.messages.extend(history);
            self.messages.push(goal);
        }
        self
    }

    /// The current conversation (for inspection / persistence).
    pub fn conversation(&self) -> &[Message] {
        &self.messages
    }

    /// Record a tool observation as a `tool` role message correlated to its
    /// originating tool-call id.
    ///
    /// The result is wrapped in an explicit envelope (`{"status": "ok" |
    /// "error", "result"|"error": ...}`) so the model can reliably tell success
    /// from failure. On failure a short recovery hint is appended, nudging the
    /// model to retry with corrected arguments or try a different tool rather
    /// than giving up — the behaviour that makes tool use feel "smooth".
    fn record_observation(&mut self, obs: &Observation) {
        // Correlate by the observation's own call id (set by the loop engine).
        // Fall back to the single most-recent pending id, then a synthetic one,
        // so older single-call paths keep working.
        let call_id = obs
            .call_id
            .clone()
            .or_else(|| self.pending_tool_call_id.take())
            .unwrap_or_else(|| format!("call_{}", obs.tool));
        let envelope = if obs.ok {
            serde_json::json!({ "status": "ok", "result": obs.output })
        } else {
            serde_json::json!({
                "status": "error",
                "tool": obs.tool,
                "error": obs.output,
                "recovery_hint": "This tool call FAILED. Do not give up. Read the error, then \
                    either retry with corrected arguments or use a different tool/approach to \
                    achieve the same goal. Only report inability after genuinely trying.",
            })
        };
        let content = serde_json::to_string(&envelope).unwrap_or_else(|_| obs.output.to_string());
        self.messages.push(Message::tool_result(call_id, content));
    }
}

/// Forwards model streaming deltas to a [`RuntimeEventSink`] as
/// [`RuntimeEvent`]s, so a UI sees tokens/reasoning live.
struct SinkObserver {
    sink: Arc<dyn RuntimeEventSink>,
}

impl DeltaObserver for SinkObserver {
    fn on_content(&mut self, delta: &str) {
        self.sink.emit(RuntimeEvent::ContentDelta {
            text: delta.to_string(),
        });
    }
    fn on_reasoning(&mut self, delta: &str) {
        self.sink.emit(RuntimeEvent::ReasoningDelta {
            text: delta.to_string(),
        });
    }
    // Tool-call start is emitted by the loop engine (with args + call_id) at the
    // BeforeToolUse gate, so we don't duplicate it here.
}

#[async_trait]
impl Agent for ModelAgent {
    async fn think(&mut self, _step: usize, last: &[Observation]) -> Result<AgentDecision> {
        // Feed back every tool result from the previous step (more than one when
        // the previous turn ran tools in parallel). Order matters: tool results
        // must follow the assistant turn that requested them.
        for obs in last {
            self.record_observation(obs);
        }

        let request = ChatRequest::new(self.model.clone(), self.messages.clone())
            .with_thinking_depth(self.thinking_depth)
            .with_tools(self.tools.clone());

        // Stream the turn, forwarding token/reasoning deltas to the event sink
        // (if any) as they arrive.
        let response = match &self.events {
            Some(sink) => {
                let mut observer = SinkObserver { sink: sink.clone() };
                self.client
                    .stream_chat_observed(request, &mut observer)
                    .await?
            }
            None => self.client.stream_chat(request).await?,
        };

        // Forward token usage to the event sink so the UI can show input/output
        // and DeepSeek cache hit/miss totals for the run. Also accumulate it so
        // the loop can persist the run's total usage at completion.
        if let Some(usage) = response.usage {
            self.usage.prompt_tokens += usage.prompt_tokens;
            self.usage.completion_tokens += usage.completion_tokens;
            self.usage.total_tokens += usage.total_tokens;
            self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
            self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
            if let Some(sink) = &self.events {
                sink.emit(RuntimeEvent::Usage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                    cost_yuan: None,
                });
            }
        }

        // Persist the assistant turn. Thinking Mode: keep reasoning_content when
        // the turn carries tool calls so it is replayed next request.
        let mut assistant = Message::text(Role::Assistant, response.message.content.clone());
        assistant.tool_calls = response.message.tool_calls.clone();
        if response.message.has_tool_calls() {
            assistant.reasoning_content = response.message.reasoning_content.clone();
        }
        self.messages.push(assistant);

        // Decide the next action. The model may emit several tool calls in one
        // turn (parallel tool calling) — carry all of them, each tagged with its
        // own id so results correlate back correctly.
        let calls = &response.message.tool_calls;
        if !calls.is_empty() {
            let invocations: Vec<ToolInvocation> = calls
                .iter()
                .map(|c| {
                    ToolInvocation::new(c.name.clone(), c.arguments.clone()).with_id(c.id.clone())
                })
                .collect();
            // Track the last id for the legacy single-call fallback path.
            self.pending_tool_call_id = calls.last().map(|c| c.id.clone());
            if invocations.len() == 1 {
                return Ok(AgentDecision::CallTool(
                    invocations.into_iter().next().unwrap(),
                ));
            }
            return Ok(AgentDecision::CallTools(invocations));
        }

        match response.finish_reason {
            Some(FinishReason::ContentFilter) => {
                Err(CoreError::other("model stopped due to content filter"))
            }
            _ => Ok(AgentDecision::Complete(response.message.content)),
        }
    }

    fn cumulative_usage(&self) -> Option<crate::agent::RunUsage> {
        // None when nothing was reported (all zero), so callers can skip the
        // persisted usage event for providers that don't return usage.
        if self.usage == crate::agent::RunUsage::default() {
            None
        } else {
            Some(self.usage)
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
        let mut agent =
            ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "do it", vec![]);
        let decision = agent.think(0, &[]).await.unwrap();
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
        let mut agent = ModelAgent::new(client(events), "deepseek-v4-pro", "sys", "add", vec![]);
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CallTool(inv) => {
                assert_eq!(inv.name, "add");
                assert_eq!(inv.arguments, serde_json::json!({"a":1,"b":2}));
                assert_eq!(inv.id.as_deref(), Some("c1"));
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
        let mut agent = ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "add", vec![]);
        let obs = Observation {
            tool: "add".to_string(),
            ok: true,
            output: serde_json::json!({"sum": 3}),
            call_id: Some("c1".to_string()),
        };
        agent.think(1, std::slice::from_ref(&obs)).await.unwrap();
        // A tool-role message correlated to c1 was inserted.
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("c1"));
        // Success is wrapped in an ok envelope carrying the result.
        let v: serde_json::Value = serde_json::from_str(&tool_msg.content).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["result"]["sum"], 3);
    }

    #[tokio::test]
    async fn failed_observation_is_fed_back_with_recovery_hint() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"ok let me retry"},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ];
        let mut agent =
            ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "search", vec![]);
        let obs = Observation {
            tool: "web_search".to_string(),
            ok: false,
            output: serde_json::json!({"error": "search failed: timeout"}),
            call_id: Some("c9".to_string()),
        };
        agent.think(1, std::slice::from_ref(&obs)).await.unwrap();
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&tool_msg.content).unwrap();
        // Failure is explicitly marked and carries a recovery hint.
        assert_eq!(v["status"], "error");
        assert_eq!(v["tool"], "web_search");
        assert!(v["error"]["error"].as_str().unwrap().contains("timeout"));
        assert!(v["recovery_hint"].as_str().unwrap().contains("retry"));
    }
}
