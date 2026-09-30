//! Multi-agent control plane: LLM-driven plan generation + the production
//! [`deepagent_builtins::PlanExecutor`].
//!
//! This is the runtime half of the `plan_execute` tool (D1). It:
//! 1. asks the model to decompose a goal into a validated [`PlanDag`]
//!    ([`plan_dag_via_model`]) — a real, goal-tailored decomposition, not the
//!    model-free [`deepagent_planner::HeuristicPlanner`] canned template, and
//! 2. runs that DAG through [`deepagent_subagents::DagScheduler`] over
//!    [`crate::subagent_runner::DagSubagentExecutor`], i.e. the SAME sub-agent
//!    loop the `task` tool uses — one executor, not a second orchestration
//!    stack.
//!
//! Worktree isolation is owned by the scheduler (fan-out/fan-in with per-node
//! git worktrees); per-node lifecycle events reach the UI through the sub-agent
//! runner's existing `Subagent*` [`deepagent_runtime::RuntimeEvent`]s.

use async_trait::async_trait;
use serde::Deserialize;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::chat::ResponseRequest;
use deepagent_models::{ModelClient, ThinkingDepth};
use deepagent_planner::{PlanDag, PlanNode};
use deepagent_subagents::{DagScheduler, GitWorktrees};

use crate::subagent_runner::{ChatSubagentRunner, DagSubagentExecutor};

const PLAN_SYSTEM_PROMPT: &str = concat!(
    "You are the planning module of a coding agent. Decompose the user's goal into a ",
    "directed acyclic graph (DAG) of concrete, self-contained subtasks that specialized ",
    "sub-agents can each execute independently.\n",
    "Output rules (STRICT):\n",
    "- Output ONLY a JSON array. No prose, no markdown code fences.\n",
    "- Each element: {\"id\": string, \"goal\": string, \"role\": string|null, \"depends_on\": [string]}.\n",
    "- ids are short, unique, lowercase, no spaces (e.g. \"schema\", \"api\", \"review\").\n",
    "- depends_on lists ids that MUST finish before this node; keep the graph acyclic.\n",
    "- Subtasks with no ordering constraint MUST have empty depends_on so they run in parallel.\n",
    "- Prefer 2-6 nodes. Each goal must be fully self-contained: a sub-agent sees only that ",
    "goal and the summaries of its upstream dependencies, nothing else.\n",
);

/// One planned subtask as emitted by the model.
#[derive(Debug, Deserialize)]
struct PlannedNode {
    id: String,
    goal: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

/// Ask the model to decompose `goal` into a validated [`PlanDag`].
///
/// Errors (rather than falling back to a canned plan) when the model output is
/// not parseable or the resulting graph is invalid, so a bad plan surfaces
/// instead of silently running the wrong work.
pub(crate) async fn plan_dag_via_model(
    client: &ModelClient,
    model: &str,
    goal: &str,
) -> Result<PlanDag> {
    let request = ResponseRequest::with_instructions_and_user_input(
        model.to_string(),
        PLAN_SYSTEM_PROMPT,
        format!("Goal:\n{goal}"),
    )
    .streaming()
    .with_max_output_tokens(1024)
    .with_thinking_depth(ThinkingDepth::Simple);
    let response = client.stream_response(request).await?;
    let planned = parse_planned_nodes(&response.output_text_projection())?;
    if planned.is_empty() {
        return Err(CoreError::invalid("planner returned no subtasks"));
    }
    let nodes = planned.into_iter().map(|planned| {
        let mut node = PlanNode::new(planned.id, planned.goal).depends_on(planned.depends_on);
        if let Some(role) = planned.role.filter(|role| !role.trim().is_empty()) {
            node = node.with_role(role);
        }
        node
    });
    PlanDag::new(nodes)
}

/// Extract and parse the JSON array of planned nodes from a raw model response,
/// tolerating accidental markdown fences or surrounding prose.
fn parse_planned_nodes(text: &str) -> Result<Vec<PlannedNode>> {
    let start = text.find('[');
    let end = text.rfind(']');
    let body = match (start, end) {
        (Some(start), Some(end)) if end >= start => &text[start..=end],
        _ => {
            return Err(CoreError::invalid(
                "planner output did not contain a JSON array",
            ))
        }
    };
    serde_json::from_str::<Vec<PlannedNode>>(body)
        .map_err(|error| CoreError::invalid(format!("planner output was not valid JSON: {error}")))
}

/// The production [`PlanExecutor`](deepagent_builtins::PlanExecutor): plan a
/// goal with the model, then run the DAG over the shared sub-agent loop.
pub(crate) struct ChatPlanExecutor {
    runner: ChatSubagentRunner,
    max_concurrency: usize,
}

impl ChatPlanExecutor {
    pub(crate) fn new(runner: ChatSubagentRunner) -> Self {
        Self {
            runner,
            max_concurrency: deepagent_subagents::scheduler::DEFAULT_LAYER_CONCURRENCY,
        }
    }
}

#[async_trait]
impl deepagent_builtins::PlanExecutor for ChatPlanExecutor {
    async fn execute_plan(&self, goal: String) -> Result<String> {
        let dag = plan_dag_via_model(&self.runner.client, &self.runner.model, &goal).await?;
        let worktrees = GitWorktrees::new(&self.runner.root, self.runner.worktree_base());
        let executor = DagSubagentExecutor::new(self.runner.clone());
        let report = DagScheduler::new(&executor, &worktrees)
            .with_max_concurrency(self.max_concurrency)
            .run(&dag)
            .await?;
        Ok(format_report(&dag, &report))
    }
}

/// Render a [`ScheduleReport`](deepagent_subagents::ScheduleReport) as a compact
/// human-readable summary for the calling agent.
fn format_report(dag: &PlanDag, report: &deepagent_subagents::ScheduleReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Plan executed: {} node(s), {}.\n",
        dag.len(),
        if report.all_succeeded {
            "all succeeded"
        } else {
            "some nodes failed or were skipped"
        }
    ));
    for node in dag.nodes() {
        if let Some(result) = report.results.get(&node.id) {
            out.push_str(&format!(
                "\n- [{}] {}: {}",
                if result.ok { "ok" } else { "failed" },
                node.id,
                result.summary.trim()
            ));
        } else if report.skipped.contains(&node.id) {
            out.push_str(&format!(
                "\n- [skipped] {}: an upstream dependency did not succeed",
                node.id
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_array() {
        let text = r#"[{"id":"a","goal":"do a","depends_on":[]},
                       {"id":"b","goal":"do b","role":"review","depends_on":["a"]}]"#;
        let nodes = parse_planned_nodes(text).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].depends_on, vec!["a".to_string()]);
        assert_eq!(nodes[1].role.as_deref(), Some("review"));
    }

    #[test]
    fn tolerates_prose_and_code_fences_around_array() {
        let text = "Here is the plan:\n```json\n[{\"id\":\"x\",\"goal\":\"g\"}]\n```\nDone.";
        let nodes = parse_planned_nodes(text).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "x");
    }

    #[test]
    fn rejects_output_without_array() {
        assert!(parse_planned_nodes("I cannot plan this.").is_err());
    }

    #[test]
    fn parsed_nodes_build_a_valid_dag() {
        let text = r#"[{"id":"a","goal":"do a"},{"id":"b","goal":"do b","depends_on":["a"]}]"#;
        let planned = parse_planned_nodes(text).unwrap();
        let nodes = planned
            .into_iter()
            .map(|planned| PlanNode::new(planned.id, planned.goal).depends_on(planned.depends_on));
        let dag = PlanDag::new(nodes).unwrap();
        assert_eq!(dag.len(), 2);
        // b depends on a → two topological layers.
        assert_eq!(dag.topological_layers().unwrap().len(), 2);
    }
}
