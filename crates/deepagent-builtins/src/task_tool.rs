//! `task` — delegate a self-contained piece of work to a sub-agent, modeled on
//! Claude Code's `Agent`/`Task` tool (same JSON schema shape: a short
//! `description`, a full `prompt`, and an optional `subagent_type`).
//!
//! A sub-agent runs autonomously with its own context and returns a single
//! result string to the calling agent. This protects the main context window
//! from large intermediate output (file dumps, search results) and lets the
//! agent parallelize independent research. Actual execution is delegated to a
//! pluggable [`SubagentRunner`] the host wires up (the desktop app runs a nested
//! agent loop; headless/tests use a stub).

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// The tool name advertised to the model.
pub const TASK_TOOL_NAME: &str = "task";

/// A request to run a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRequest {
    /// A short (3-5 word) description of the task, for display.
    pub description: String,
    /// The full, self-contained instruction for the sub-agent. Because the
    /// sub-agent is stateless, this must include everything it needs.
    pub prompt: String,
    /// Which specialized agent to use (e.g. "general", "explore", "review").
    /// `None` means the default general-purpose agent.
    pub subagent_type: Option<String>,
}

/// Runs a sub-agent to completion and returns its final result text.
///
/// The default impl reports that sub-agents aren't configured; the desktop app
/// supplies a runner that executes a nested agent loop rooted at the project.
#[async_trait]
pub trait SubagentRunner: Send + Sync {
    /// Execute `request` and return the sub-agent's final answer.
    async fn run(&self, request: SubagentRequest) -> Result<String>;
}

/// A runner that reports sub-agents are unavailable (headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableSubagentRunner;

#[async_trait]
impl SubagentRunner for UnavailableSubagentRunner {
    async fn run(&self, _request: SubagentRequest) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "sub-agents are not configured in this environment",
        ))
    }
}

/// The `task` tool over a pluggable [`SubagentRunner`].
pub struct TaskTool<R: SubagentRunner> {
    runner: R,
    /// Allowed `subagent_type` values, surfaced in the description so the model
    /// picks a real one. Empty means only the default general agent.
    agent_types: Vec<String>,
}

impl<R: SubagentRunner> TaskTool<R> {
    /// Build the tool with a runner and the list of available agent types.
    pub fn new(runner: R, agent_types: impl IntoIterator<Item = String>) -> Self {
        Self {
            runner,
            agent_types: agent_types.into_iter().collect(),
        }
    }
}

#[async_trait]
impl<R: SubagentRunner> Tool for TaskTool<R> {
    fn descriptor(&self) -> ToolDescriptor {
        let types_note = if self.agent_types.is_empty() {
            "Only the default general-purpose agent is available; omit subagent_type.".to_string()
        } else {
            format!(
                "Available subagent_type values: {}. Pick the one whose specialty matches the task.",
                self.agent_types.join(", ")
            )
        };
        ToolDescriptor {
            name: TASK_TOOL_NAME.into(),
            description: format!(
                "Delegate a self-contained task to a sub-agent that runs autonomously and returns a single result. Brief the sub-agent like a colleague who just walked into the room: include all context they need (files, prior decisions, expected output shape) — they have NONE of your conversation history.\n\
                \n\
                ## When to use\n\
                - Broad exploration where intermediate output would flood your context (audit a whole subsystem, survey many files, gather background on an unfamiliar area).\n\
                - Multiple independent investigation threads that can run concurrently — issue several `task` calls in a SINGLE assistant message and they execute in parallel.\n\
                - Long-output tasks where only the final summary matters (test runs, large diffs, exhaustive searches).\n\
                \n\
                ## When NOT to use\n\
                - You already know the specific file path → call `read_file` directly.\n\
                - A single `grep` / `glob` / `code_map_search` would answer the question.\n\
                - You're modifying a known small set of files → drive the edits yourself.\n\
                - The next step is one tool call away — sub-agent overhead is wasted there.\n\
                \n\
                ## Hard rule: never delegate understanding\n\
                Do NOT phrase prompts like \"based on your findings, fix the bug\" / \"based on your research, implement the feature\". Synthesizing across multiple sources is YOUR job; the sub-agent should return raw observations, summaries, or single-step results that you can then reason over. Delegating the comprehension step robs you of the context needed to make good decisions on the next turn.\n\
                \n\
                ## Concurrency example\n\
                User: \"check the auth, billing, and notifications modules in parallel.\"\n\
                Correct: emit THREE `task` tool_calls in one assistant message — one per module — each with its own focused prompt and expected return shape. The runtime drives them concurrently; you receive three separate summaries on the next turn.\n\
                \n\
                ## Sub-agent statelessness\n\
                The sub-agent has no memory of your conversation, no access to your todo list, and no view of files you've read. The `prompt` field MUST be self-contained: name the file paths, restate the goal, and specify EXACTLY what the sub-agent should return (a JSON shape, a list of file paths, a written summary in 5 bullets, etc.). Vague prompts produce vague answers.\n\
                \n\
                {types_note}"
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "A short (3-5 word) description of the task, for display."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The full, self-contained instruction for the sub-agent. Include all context and state exactly what the sub-agent should return."
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "Which specialized agent to use. Omit for the default general-purpose agent."
                    }
                },
                "required": ["description", "prompt"]
            }),
            // Sub-agents can do real work (incl. writes via their own tools), so
            // running one is Medium risk and requires explicit permission.
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([
                Permission::ReadOnly,
                Permission::Subagent,
            ]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(prompt) = args.get("prompt").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'prompt'"));
        };
        if prompt.trim().is_empty() {
            return Ok(ToolOutput::failure("'prompt' must not be empty"));
        }
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let subagent_type = args
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        // Validate the requested type against the available set (if any).
        if let Some(t) = &subagent_type {
            if !self.agent_types.is_empty() && !self.agent_types.iter().any(|a| a == t) {
                return Ok(ToolOutput::failure(format!(
                    "unknown subagent_type '{t}'; available: {}",
                    self.agent_types.join(", ")
                )));
            }
        }

        let request = SubagentRequest {
            description,
            prompt: prompt.to_string(),
            subagent_type,
        };
        match self.runner.run(request).await {
            Ok(result) => Ok(ToolOutput::success(serde_json::json!({
                "result": result,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("sub-agent failed: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoRunner;
    #[async_trait]
    impl SubagentRunner for EchoRunner {
        async fn run(&self, request: SubagentRequest) -> Result<String> {
            Ok(format!(
                "[{}] did: {}",
                request.subagent_type.as_deref().unwrap_or("general"),
                request.prompt
            ))
        }
    }

    #[tokio::test]
    async fn runs_subagent_and_returns_result() {
        let tool = TaskTool::new(EchoRunner, ["general".to_string(), "explore".to_string()]);
        let out = tool
            .invoke(serde_json::json!({
                "description": "find usages",
                "prompt": "Find all callers of foo()",
                "subagent_type": "explore"
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.value["result"].as_str().unwrap().contains("explore"));
    }

    #[tokio::test]
    async fn rejects_unknown_subagent_type() {
        let tool = TaskTool::new(EchoRunner, ["general".to_string()]);
        let out = tool
            .invoke(serde_json::json!({
                "description": "x",
                "prompt": "do it",
                "subagent_type": "ghost"
            }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn missing_prompt_fails() {
        let tool = TaskTool::new(EchoRunner, []);
        let out = tool
            .invoke(serde_json::json!({"description": "x"}))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn unavailable_runner_reports_failure() {
        let tool = TaskTool::new(UnavailableSubagentRunner, []);
        let out = tool
            .invoke(serde_json::json!({"description": "x", "prompt": "do it"}))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn descriptor_lists_agent_types() {
        let tool = TaskTool::new(EchoRunner, ["explore".to_string(), "review".to_string()]);
        let d = tool.descriptor();
        assert_eq!(d.name, TASK_TOOL_NAME);
        assert!(d.description.contains("explore"));
        assert!(d.description.contains("review"));
    }

    #[test]
    fn descriptor_carries_phase_5b_guidance() {
        // Phase 5B: the task tool description must surface the four key
        // signals so the model uses sub-agents correctly and only when warranted.
        let tool = TaskTool::new(EchoRunner, ["explore".to_string()]);
        let d = tool.descriptor();
        // "Brief like a colleague" framing.
        assert!(d.description.contains("colleague"));
        // When-NOT-to-use section with concrete signals.
        assert!(d.description.contains("When NOT to use"));
        assert!(d.description.contains("read_file"));
        assert!(d.description.contains("grep"));
        // Hard rule: never delegate understanding.
        assert!(d.description.contains("never delegate understanding"));
        assert!(d.description.contains("based on your"));
        // Concurrency example.
        assert!(d.description.contains("THREE `task` tool_calls"));
        // Statelessness contract.
        assert!(d.description.contains("STATELESS") || d.description.contains("statelessness"));
        assert!(d.description.contains("self-contained"));
    }
}
