//! Local version-1 iteration contract derived from the published item/done ports.
//! This plans scopes, never starts a tool, creates a run or retries an item.
//! Item contexts use ordinary WorkflowAgents through the same owning Kernel.

use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::graph::{output_names, WorkflowDefinition, WorkflowNodeSpec};
use super::values;

/// Matches nodeSchemas.ts; parallel=false always has exactly one active item.
pub(super) const MAX_CONCURRENCY: usize = 50;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum ErrorHandling {
    Terminate,
    Continue,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OutputSelector {
    Item,
    Reference(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct IterationPlan {
    pub node_id: String,
    pub input: Value,
    pub output: OutputSelector,
    pub concurrency: usize,
    pub error_handling: ErrorHandling,
    pub flatten: bool,
    /// Exclusive item descendants. A done-reachable join is outside this body.
    pub body: BTreeSet<String>,
    pub roots: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Options {
    input_variable: String,
    output_variable: String,
    parallel: bool,
    max_concurrency: usize,
    error_handling: ErrorHandling,
    flatten: bool,
}

fn invalid(message: &str) -> CoreError {
    CoreError::invalid(format!("IterationPlanInvalid: {message}"))
}

fn reference(value: &Value) -> Result<Vec<String>> {
    let paths = values::references(value)?;
    let path = paths.first().filter(|_| paths.len() == 1).ok_or_else(|| {
        invalid("input/output selector must be one canonical node-output reference")
    })?;
    if value.as_str() != Some(format!("{{{{#{}#}}}}", path.join(".")).as_str()) {
        return Err(invalid(
            "selector cannot contain a template or a variable alias",
        ));
    }
    Ok(path.clone())
}

fn reachable(definition: &WorkflowDefinition, roots: BTreeSet<String>) -> BTreeSet<String> {
    let mut pending: Vec<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if seen.insert(id.clone()) {
            pending.extend(
                definition
                    .edges
                    .iter()
                    .filter(|edge| edge.source == id)
                    .map(|edge| edge.target.clone()),
            );
        }
    }
    seen
}

impl IterationPlan {
    fn compile(node: &WorkflowNodeSpec, definition: &WorkflowDefinition) -> Result<Self> {
        if [
            "iterationCollection",
            "iteratorVariable",
            "iterationOperation",
            "iterationTemplate",
        ]
        .iter()
        .any(|key| node.config.contains_key(*key))
        {
            return Err(invalid(
                "unpublished legacy map fields are not iteration configuration",
            ));
        }
        // Select executable fields; unrelated node labels/UI metadata are not options.
        let options: Options = serde_json::from_value(json!({
            "inputVariable": node.config.get("inputVariable"),
            "outputVariable": node.config.get("outputVariable").cloned().unwrap_or(json!("")),
            "parallel": node.config.get("parallel").cloned().unwrap_or(json!(false)),
            "maxConcurrency": node.config.get("maxConcurrency").cloned().unwrap_or(json!(1)),
            "errorHandling": node.config.get("errorHandling").cloned().unwrap_or(json!("terminate")),
            "flatten": node.config.get("flatten").cloned().unwrap_or(json!(false))
        })).map_err(|_| invalid("invalid input, concurrency, error policy or flatten type"))?;
        if !(1..=MAX_CONCURRENCY).contains(&options.max_concurrency) {
            return Err(invalid("maxConcurrency must be in 1..=50"));
        }
        let input = Value::String(options.input_variable);
        reference(&input)?;
        let roots = |handle| {
            definition
                .edges
                .iter()
                .filter(|edge| {
                    edge.source == node.id && edge.source_handle.as_deref() == Some(handle)
                })
                .map(|edge| edge.target.clone())
                .collect::<BTreeSet<_>>()
        };
        let item_roots = roots("item");
        let done = reachable(definition, roots("done"));
        let body: BTreeSet<_> = reachable(definition, item_roots.clone())
            .difference(&done)
            .cloned()
            .collect();
        for edge in &definition.edges {
            if body.contains(&edge.target)
                && !body.contains(&edge.source)
                && !(edge.source == node.id && edge.source_handle.as_deref() == Some("item"))
            {
                return Err(invalid(
                    "item body has an incoming control edge from outside its scope",
                ));
            }
        }
        let output = if matches!(options.output_variable.as_str(), "" | "item") {
            OutputSelector::Item
        } else {
            let selected = Value::String(options.output_variable);
            let path = reference(&selected)?;
            if !body.contains(&path[0]) {
                return Err(invalid(
                    "output selector must belong to this iteration body",
                ));
            }
            let source = definition
                .nodes
                .iter()
                .find(|source| source.id == path[0])
                .ok_or_else(|| invalid("output selector source is missing"))?;
            if !output_names(source)?.contains(&path[1]) {
                return Err(invalid("output selector names an undeclared body output"));
            }
            OutputSelector::Reference(selected)
        };
        Ok(Self {
            node_id: node.id.clone(),
            input,
            output,
            concurrency: if options.parallel {
                options.max_concurrency
            } else {
                1
            },
            error_handling: options.error_handling,
            flatten: options.flatten,
            roots: item_roots.intersection(&body).cloned().collect(),
            body,
        })
    }

    pub fn items(&self, outputs: &BTreeMap<String, Value>) -> Result<Vec<Value>> {
        let value = values::resolve(&self.input, outputs)?;
        values::check_value(&value, "iteration input")?;
        match value {
            Value::Array(items) => Ok(items),
            _ => Err(invalid(
                "inputVariable must resolve to an array, not an empty fallback",
            )),
        }
    }

    pub fn select(&self, item: &Value, outputs: &BTreeMap<String, Value>) -> Result<Value> {
        match &self.output {
            OutputSelector::Item => Ok(item.clone()),
            OutputSelector::Reference(reference) => values::resolve(reference, outputs),
        }
    }
}

pub(super) fn compile_plans(
    definition: &WorkflowDefinition,
) -> Result<BTreeMap<String, IterationPlan>> {
    let mut plans = BTreeMap::new();
    for node in definition
        .nodes
        .iter()
        .filter(|node| node.kind == "iteration")
    {
        plans.insert(node.id.clone(), IterationPlan::compile(node, definition)?);
    }
    let ordered: Vec<_> = plans.values().collect();
    for (index, left) in ordered.iter().enumerate() {
        for right in ordered.iter().skip(index + 1) {
            if left.body.is_disjoint(&right.body) {
                continue;
            }
            let nested = (left.body.contains(&right.node_id) && right.body.is_subset(&left.body))
                || (right.body.contains(&left.node_id) && left.body.is_subset(&right.body));
            if !nested || left.body == right.body {
                return Err(invalid(
                    "iteration bodies overlap without a strict parent/child scope",
                ));
            }
        }
    }
    for plan in plans.values() {
        if let OutputSelector::Reference(value) = &plan.output {
            let source = &reference(value)?[0];
            let mut expected = scope_of(&plans, &plan.node_id);
            expected.push(plan.node_id.clone());
            if scope_of(&plans, source) != expected {
                return Err(invalid(
                    "select a nested aggregate, not an individual nested item's output",
                ));
            }
        }
    }
    Ok(plans)
}

/// Scope identity is derived from ports, not an independently maintained tree.
pub(super) fn scope_of(plans: &BTreeMap<String, IterationPlan>, node: &str) -> Vec<String> {
    let mut owners: Vec<_> = plans
        .values()
        .filter(|plan| plan.body.contains(node))
        .collect();
    owners.sort_by_key(|plan| std::cmp::Reverse(plan.body.len()));
    owners
        .into_iter()
        .map(|plan| plan.node_id.clone())
        .collect()
}

#[cfg(test)]
mod tests;
