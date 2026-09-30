//! `plan_execute` — decompose a goal into a dependency DAG of sub-tasks and run
//! them across isolated sub-agents (fan-out / fan-in), returning one aggregated
//! report.
//!
//! This is the model-facing surface of the multi-agent control plane. Like the
//! `task` tool, the actual work is delegated to a pluggable [`PlanExecutor`] the
//! host wires up (the app runs an LLM planner + the DAG scheduler over the same
//! sub-agent loop `task` uses; headless/tests use a stub). Keeping the trait
//! here lets `deepagent-builtins` stay free of a dependency on the runtime,
//! planner, or model crates.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

/// The tool name advertised to the model.
pub const PLAN_EXECUTE_TOOL_NAME: &str = "plan_execute";

/// Runs a full plan for a goal: decompose into a subtask DAG, execute the nodes
/// with dependency ordering, and return an aggregated report.
#[async_trait]
pub trait PlanExecutor: Send + Sync {
    /// Plan and execute `goal`, returning a human-readable aggregated report.
    async fn execute_plan(&self, goal: String) -> Result<String>;

    /// Execute with cancellation inherited from the parent run. The default
    /// honors a pre-start cancel then delegates to [`PlanExecutor::execute_plan`].
    async fn execute_plan_controlled(
        &self,
        goal: String,
        context: ToolExecutionContext,
    ) -> Result<String> {
        if context.is_cancelled() {
            return Err(deepagent_core::error::CoreError::other(
                "plan cancelled before start",
            ));
        }
        self.execute_plan(goal).await
    }
}

/// Delegate through a shared handle, so `Arc<dyn PlanExecutor>` (the type the
/// runtime wires) is itself a [`PlanExecutor`] and can back a [`PlanExecuteTool`].
#[async_trait]
impl<T: PlanExecutor + ?Sized> PlanExecutor for Arc<T> {
    async fn execute_plan(&self, goal: String) -> Result<String> {
        (**self).execute_plan(goal).await
    }

    async fn execute_plan_controlled(
        &self,
        goal: String,
        context: ToolExecutionContext,
    ) -> Result<String> {
        (**self).execute_plan_controlled(goal, context).await
    }
}

/// A planner that reports the capability is unavailable (headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailablePlanExecutor;

#[async_trait]
impl PlanExecutor for UnavailablePlanExecutor {
    async fn execute_plan(&self, _goal: String) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "multi-agent planning is not configured in this environment",
        ))
    }
}

/// The `plan_execute` tool over a pluggable [`PlanExecutor`].
pub struct PlanExecuteTool<P: PlanExecutor> {
    executor: P,
}

impl<P: PlanExecutor> PlanExecuteTool<P> {
    /// Build the tool over `executor`.
    pub fn new(executor: P) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl<P: PlanExecutor> Tool for PlanExecuteTool<P> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: PLAN_EXECUTE_TOOL_NAME.into(),
            description: concat!(
                "Decompose a larger goal into a dependency-ordered DAG of subtasks and run them ",
                "across isolated sub-agents (independent subtasks run concurrently; dependents ",
                "receive their upstreams' summaries), then return one aggregated report.\n",
                "\n",
                "## When to use\n",
                "- A goal that splits into several independent or partially-ordered pieces of work ",
                "(e.g. \"build the API, the frontend, and the migration, then review\").\n",
                "- You want the runtime to plan the decomposition and orchestrate the fan-out/fan-in ",
                "for you, rather than issuing and tracking many `task` calls by hand.\n",
                "\n",
                "## When NOT to use\n",
                "- A single self-contained delegation → use `task`.\n",
                "- Work you can do directly in a couple of tool calls — orchestration overhead is wasted.\n",
                "\n",
                "Provide a single, self-contained `goal`. The planner has none of your conversation ",
                "history, so state the objective, constraints, and the shape of the expected result."
            )
            .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "The full, self-contained goal to plan and execute. State the objective, constraints, and expected result shape."
                    }
                },
                "required": ["goal"]
            }),
            // Spawns sub-agents that may run arbitrary tools — Medium risk,
            // same posture as `task`.
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
        let Some(goal) = args
            .get("goal")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
        else {
            return Ok(ToolOutput::failure("missing non-empty 'goal'"));
        };
        match self
            .executor
            .execute_plan_controlled(goal.to_string(), context)
            .await
        {
            Ok(report) => Ok(ToolOutput::success(serde_json::json!({ "report": report }))),
            Err(error) => Ok(ToolOutput::failure(format!("plan_execute failed: {error}"))),
        }
    }

    fn is_concurrency_safe(&self, _arguments: &serde_json::Value) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoPlanner;

    #[async_trait]
    impl PlanExecutor for EchoPlanner {
        async fn execute_plan(&self, goal: String) -> Result<String> {
            Ok(format!("planned+ran: {goal}"))
        }
    }

    #[tokio::test]
    async fn runs_goal_and_returns_report() {
        let tool = PlanExecuteTool::new(EchoPlanner);
        let out = tool
            .invoke(serde_json::json!({ "goal": "build a thing" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["report"], "planned+ran: build a thing");
    }

    #[tokio::test]
    async fn rejects_empty_goal() {
        let tool = PlanExecuteTool::new(EchoPlanner);
        let out = tool
            .invoke(serde_json::json!({ "goal": "  " }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn unavailable_planner_reports_error() {
        let tool = PlanExecuteTool::new(UnavailablePlanExecutor);
        let out = tool
            .invoke(serde_json::json!({ "goal": "x" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
