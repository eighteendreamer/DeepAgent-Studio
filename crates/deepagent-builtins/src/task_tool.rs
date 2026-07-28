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
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

/// The tool name advertised to the model.
pub const TASK_TOOL_NAME: &str = "task";

/// A specialized sub-agent type advertised by the `task` tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentType {
    /// The exact `subagent_type` value the model may pass.
    pub name: String,
    /// Natural-language guidance for when to choose this agent type.
    pub description: String,
}

impl TaskAgentType {
    /// Create an advertised sub-agent type.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }

    fn name_only(name: impl Into<String>) -> Self {
        Self::new(name, "")
    }
}

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
    /// Optional per-child tool allowlist. Empty inherits the selected agent
    /// profile (or the parent registry when the profile is unrestricted).
    pub allowed_tools: Vec<String>,
    /// Optional provider model override for this child.
    pub model: Option<String>,
    /// Optional reasoning effort (`simple`, `medium`, or `deep`).
    pub effort: Option<String>,
    /// Skill ids whose full instructions must be loaded before the first turn.
    pub skills: Vec<String>,
    /// Execution isolation (`shared` or `worktree`).
    pub isolation: String,
}

/// Handle returned when a sub-agent is launched without blocking its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundSubagent {
    /// Durable id used for status and cancellation requests.
    pub id: String,
    /// Initial state, normally `running`.
    pub state: String,
}

/// Durable status projection for one sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentStatus {
    /// Durable sub-agent id.
    pub id: String,
    /// Current lifecycle state.
    pub state: String,
    /// Final summary when terminal.
    pub summary: Option<String>,
    /// Retained worktree path, when isolated execution was requested.
    pub worktree_path: Option<String>,
}

/// Runs a sub-agent to completion and returns its final result text.
///
/// The default impl reports that sub-agents aren't configured; the desktop app
/// supplies a runner that executes a nested agent loop rooted at the project.
#[async_trait]
pub trait SubagentRunner: Send + Sync {
    /// Execute `request` and return the sub-agent's final answer.
    async fn run(&self, request: SubagentRequest) -> Result<String>;

    /// Execute with cancellation inherited from the parent run.
    async fn run_controlled(
        &self,
        request: SubagentRequest,
        context: ToolExecutionContext,
    ) -> Result<String> {
        if context.is_cancelled() {
            return Err(deepagent_core::error::CoreError::other(
                "sub-agent cancelled before start",
            ));
        }
        self.run(request).await
    }

    /// Start a child run and return immediately.
    async fn start_background(
        &self,
        _request: SubagentRequest,
        _context: ToolExecutionContext,
    ) -> Result<BackgroundSubagent> {
        Err(deepagent_core::error::CoreError::other(
            "background sub-agents are not supported by this runner",
        ))
    }

    /// Query one previously started background child.
    async fn status(&self, _id: &str) -> Result<SubagentStatus> {
        Err(deepagent_core::error::CoreError::other(
            "sub-agent status is not supported by this runner",
        ))
    }

    /// Request cancellation without cancelling the parent run.
    async fn cancel(&self, _id: &str) -> Result<bool> {
        Err(deepagent_core::error::CoreError::other(
            "sub-agent cancellation is not supported by this runner",
        ))
    }

    /// Continue a durable terminal child with additional instructions.
    async fn resume(
        &self,
        _id: &str,
        _prompt: &str,
        _context: ToolExecutionContext,
    ) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "sub-agent resume is not supported by this runner",
        ))
    }

    /// Remove a retained worktree for a terminal child.
    async fn cleanup(&self, _id: &str) -> Result<bool> {
        Err(deepagent_core::error::CoreError::other(
            "sub-agent cleanup is not supported by this runner",
        ))
    }
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
    agent_types: Vec<TaskAgentType>,
}

impl<R: SubagentRunner> TaskTool<R> {
    /// Build the tool with a runner and the list of available agent types.
    pub fn new(runner: R, agent_types: impl IntoIterator<Item = String>) -> Self {
        Self::new_with_agent_types(
            runner,
            agent_types.into_iter().map(TaskAgentType::name_only),
        )
    }

    /// Build the tool with named agent types and their descriptions.
    pub fn new_with_agent_types(
        runner: R,
        agent_types: impl IntoIterator<Item = TaskAgentType>,
    ) -> Self {
        Self {
            runner,
            agent_types: agent_types.into_iter().collect(),
        }
    }

    fn agent_type_names(&self) -> Vec<String> {
        self.agent_types
            .iter()
            .map(|agent| agent.name.clone())
            .collect()
    }
}

#[async_trait]
impl<R: SubagentRunner> Tool for TaskTool<R> {
    fn descriptor(&self) -> ToolDescriptor {
        let types_note = if self.agent_types.is_empty() {
            "Only the default general-purpose agent is available; omit subagent_type.".to_string()
        } else {
            let mut lines = Vec::with_capacity(self.agent_types.len());
            for agent in &self.agent_types {
                let description = agent.description.trim();
                if description.is_empty() {
                    lines.push(format!("- `{}`", agent.name));
                } else {
                    lines.push(format!("- `{}`: {description}", agent.name));
                }
            }
            format!(
                "Available subagent_type values:\n{}\nPick the one whose specialty matches the task.",
                lines.join("\n")
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
                Use `background=true` for independent long-running work. Poll it with `operation=status` and `subagent_id`; cancel only that child with `operation=cancel`.\n\
                \n\
                {types_note}"
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["run", "status", "cancel", "resume", "cleanup"],
                        "default": "run"
                    },
                    "subagent_id": {
                        "type": "string",
                        "description": "Required for status/cancel/resume/cleanup operations."
                    },
                    "background": {
                        "type": "boolean",
                        "default": false,
                        "description": "Return a durable id immediately while the child continues running."
                    },
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
                    },
                    "allowed_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tool allowlist for this child."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override for this child."
                    },
                    "effort": {
                        "type": "string",
                        "enum": ["simple", "medium", "deep"],
                        "description": "Optional reasoning effort override."
                    },
                    "skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Skill ids to preload into the child context."
                    },
                    "isolation": {
                        "type": "string",
                        "enum": ["shared", "worktree"],
                        "default": "shared",
                        "description": "Use a detached Git worktree when filesystem isolation is required."
                    }
                },
                "additionalProperties": false
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
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let operation = args
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("run");
        if matches!(operation, "status" | "cancel" | "resume" | "cleanup") {
            let Some(id) = args.get("subagent_id").and_then(serde_json::Value::as_str) else {
                return Ok(ToolOutput::failure(
                    "missing 'subagent_id' for status/cancel/resume/cleanup operation",
                ));
            };
            return match operation {
                "status" => match self.runner.status(id).await {
                    Ok(status) => Ok(ToolOutput::success(serde_json::json!({
                        "subagent_id": status.id,
                        "state": status.state,
                        "summary": status.summary,
                        "worktree_path": status.worktree_path,
                    }))),
                    Err(error) => Ok(ToolOutput::failure(format!(
                        "sub-agent status failed: {error}"
                    ))),
                },
                "cancel" => match self.runner.cancel(id).await {
                    Ok(accepted) => Ok(ToolOutput::success(serde_json::json!({
                        "subagent_id": id,
                        "cancel_accepted": accepted,
                    }))),
                    Err(error) => Ok(ToolOutput::failure(format!(
                        "sub-agent cancellation failed: {error}"
                    ))),
                },
                "resume" => {
                    let Some(prompt) = args
                        .get("prompt")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|prompt| !prompt.is_empty())
                    else {
                        return Ok(ToolOutput::failure(
                            "missing non-empty 'prompt' for resume operation",
                        ));
                    };
                    match self.runner.resume(id, prompt, context).await {
                        Ok(result) => Ok(ToolOutput::success(serde_json::json!({
                            "subagent_id": id,
                            "resumed": true,
                            "result": result,
                        }))),
                        Err(error) => Ok(ToolOutput::failure(format!(
                            "sub-agent resume failed: {error}"
                        ))),
                    }
                }
                "cleanup" => match self.runner.cleanup(id).await {
                    Ok(removed) => Ok(ToolOutput::success(serde_json::json!({
                        "subagent_id": id,
                        "worktree_removed": removed,
                    }))),
                    Err(error) => Ok(ToolOutput::failure(format!(
                        "sub-agent cleanup failed: {error}"
                    ))),
                },
                _ => unreachable!(),
            };
        }
        if operation != "run" {
            return Ok(ToolOutput::failure(format!(
                "unknown task operation: {operation}"
            )));
        }
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
        let string_array = |key: &str| -> Result<Vec<String>> {
            let Some(value) = args.get(key) else {
                return Ok(Vec::new());
            };
            let Some(values) = value.as_array() else {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "'{key}' must be an array of strings"
                )));
            };
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| {
                            deepagent_core::error::CoreError::invalid(format!(
                                "'{key}' must contain only non-empty strings"
                            ))
                        })
                })
                .collect()
        };
        let allowed_tools = match string_array("allowed_tools") {
            Ok(values) => values,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        let skills = match string_array("skills") {
            Ok(values) => values,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        let model = args
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let effort = args
            .get("effort")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let isolation = args
            .get("isolation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("shared")
            .trim()
            .to_ascii_lowercase();
        if !matches!(isolation.as_str(), "shared" | "worktree") {
            return Ok(ToolOutput::failure(
                "'isolation' must be either 'shared' or 'worktree'",
            ));
        }

        // Validate the requested type against the available set (if any).
        if let Some(t) = &subagent_type {
            if !self.agent_types.is_empty() && !self.agent_types.iter().any(|a| a.name == *t) {
                let available = self.agent_type_names();
                return Ok(ToolOutput::failure(format!(
                    "unknown subagent_type '{t}'; available: {}",
                    available.join(", ")
                )));
            }
        }

        let request = SubagentRequest {
            description,
            prompt: prompt.to_string(),
            subagent_type,
            allowed_tools,
            model,
            effort,
            skills,
            isolation,
        };
        if args
            .get("background")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return match self.runner.start_background(request, context).await {
                Ok(handle) => Ok(ToolOutput::success(serde_json::json!({
                    "subagent_id": handle.id,
                    "state": handle.state,
                    "background": true,
                }))),
                Err(error) => Ok(ToolOutput::failure(format!(
                    "background sub-agent failed to start: {error}"
                ))),
            };
        }
        match self.runner.run_controlled(request, context).await {
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

    struct ControlRunner;

    #[async_trait]
    impl SubagentRunner for ControlRunner {
        async fn run(&self, _request: SubagentRequest) -> Result<String> {
            Ok("sync".into())
        }

        async fn start_background(
            &self,
            _request: SubagentRequest,
            _context: ToolExecutionContext,
        ) -> Result<BackgroundSubagent> {
            Ok(BackgroundSubagent {
                id: "sub-1".into(),
                state: "running".into(),
            })
        }

        async fn status(&self, id: &str) -> Result<SubagentStatus> {
            Ok(SubagentStatus {
                id: id.into(),
                state: "succeeded".into(),
                summary: Some("done".into()),
                worktree_path: None,
            })
        }

        async fn cancel(&self, id: &str) -> Result<bool> {
            Ok(id == "sub-1")
        }

        async fn resume(
            &self,
            id: &str,
            prompt: &str,
            _context: ToolExecutionContext,
        ) -> Result<String> {
            Ok(format!("resumed {id}: {prompt}"))
        }

        async fn cleanup(&self, id: &str) -> Result<bool> {
            Ok(id == "sub-1")
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
    async fn background_status_and_cancel_operations_roundtrip() {
        let tool = TaskTool::new(ControlRunner, Vec::<String>::new());
        let started = tool
            .invoke(serde_json::json!({
                "description": "background",
                "prompt": "do it",
                "background": true
            }))
            .await
            .unwrap();
        assert!(started.ok);
        assert_eq!(started.value["subagent_id"], "sub-1");

        let status = tool
            .invoke(serde_json::json!({
                "operation": "status",
                "subagent_id": "sub-1"
            }))
            .await
            .unwrap();
        assert_eq!(status.value["state"], "succeeded");
        assert_eq!(status.value["summary"], "done");

        let cancelled = tool
            .invoke(serde_json::json!({
                "operation": "cancel",
                "subagent_id": "sub-1"
            }))
            .await
            .unwrap();
        assert_eq!(cancelled.value["cancel_accepted"], true);

        let resumed = tool
            .invoke(serde_json::json!({
                "operation": "resume",
                "subagent_id": "sub-1",
                "prompt": "continue"
            }))
            .await
            .unwrap();
        assert_eq!(resumed.value["resumed"], true);
        assert_eq!(resumed.value["result"], "resumed sub-1: continue");

        let cleaned = tool
            .invoke(serde_json::json!({
                "operation": "cleanup",
                "subagent_id": "sub-1"
            }))
            .await
            .unwrap();
        assert_eq!(cleaned.value["worktree_removed"], true);
    }

    #[tokio::test]
    async fn descriptor_lists_agent_types() {
        let tool = TaskTool::new_with_agent_types(
            EchoRunner,
            [
                TaskAgentType::new("explore", "Survey unfamiliar areas"),
                TaskAgentType::new("review", "Review code paths"),
            ],
        );
        let d = tool.descriptor();
        assert_eq!(d.name, TASK_TOOL_NAME);
        assert!(d.description.contains("explore"));
        assert!(d.description.contains("Survey unfamiliar areas"));
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

    #[tokio::test]
    async fn descriptor_exposes_child_execution_overrides_and_rejects_bad_arrays() {
        let tool = TaskTool::new(EchoRunner, Vec::<String>::new());
        let descriptor = tool.descriptor();
        let properties = &descriptor.parameters["properties"];
        assert!(properties.get("allowed_tools").is_some());
        assert!(properties.get("model").is_some());
        assert!(properties.get("effort").is_some());
        assert!(properties.get("skills").is_some());
        assert!(properties["operation"]["enum"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("resume")));

        let output = tool
            .invoke(serde_json::json!({
                "prompt": "do it",
                "allowed_tools": "read_file"
            }))
            .await
            .unwrap();
        assert!(!output.ok);
        assert!(output.value["error"]
            .as_str()
            .unwrap()
            .contains("array of strings"));
    }
}
