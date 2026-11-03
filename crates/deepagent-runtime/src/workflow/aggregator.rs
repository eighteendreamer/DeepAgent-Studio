//! Local version-1 aggregation contract, compiled once at the graph boundary.
//! Arrays collect values without flattening; other types select the first live
//! branch. Only a scheduler-confirmed Skipped source is absent, never JSON null.
use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{graph, value_type::ValueType, values, NodeExecutionStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutputType {
    String,
    Number,
    Object,
    Array,
}

impl OutputType {
    fn value_type(self) -> ValueType {
        match self {
            Self::String => ValueType::String,
            Self::Number => ValueType::Number,
            Self::Object => ValueType::Object,
            Self::Array => ValueType::Array,
        }
    }

    fn name(self) -> &'static str {
        self.value_type().name()
    }

    fn accepts(self, value: &Value) -> bool {
        self.value_type().accepts(value)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AggregatorConfig {
    aggregator_output_type: OutputType,
    aggregator_variables: Vec<String>,
    #[serde(rename = "description")]
    _description: Option<String>,
    #[serde(rename = "label")]
    _label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Binding {
    reference: String,
    path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AggregatorPlan {
    output_type: OutputType,
    bindings: Vec<Binding>,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::invalid(format!("AggregatorConfigInvalid: {reason}"))
}

pub(super) fn validate_output_type(value: &Value) -> Result<()> {
    serde_json::from_value::<OutputType>(value.clone())
        .map(|_| ())
        .map_err(|_| invalid("aggregatorOutputType must be string, number, object or array"))
}

impl AggregatorPlan {
    pub(super) fn compile(
        config: &Map<String, Value>,
        definition: &graph::WorkflowDefinition,
    ) -> Result<Self> {
        if config.contains_key("variables") {
            return Err(invalid(
                "legacy variables is not a version-1 aggregation field; explicitly migrate to aggregatorVariables",
            ));
        }
        let config: AggregatorConfig = serde_json::from_value(Value::Object(config.clone()))
            .map_err(|_| {
                invalid("requires aggregatorOutputType, aggregatorVariables and no unknown fields")
            })?;
        if config.aggregator_variables.is_empty() {
            return Err(invalid("aggregatorVariables requires at least one binding"));
        }
        let mut seen = BTreeSet::new();
        let mut bindings = Vec::with_capacity(config.aggregator_variables.len());
        for reference in config.aggregator_variables {
            let path = values::reference_path(&reference)?;
            if !seen.insert(path.clone()) {
                return Err(invalid("duplicate source binding"));
            }
            // Only a top-level declared type is statically knowable. Nested
            // paths and untyped builtins remain subject to runtime validation.
            if config.aggregator_output_type != OutputType::Array && path.len() == 2 {
                if let Some(source) = definition.nodes.iter().find(|node| node.id == path[0]) {
                    if let Some(declared) = graph::declared_output_type(source, &path[1])? {
                        if declared != config.aggregator_output_type.name() {
                            return Err(invalid(
                                "source declaration does not match aggregatorOutputType",
                            ));
                        }
                    }
                }
            }
            bindings.push(Binding { reference, path });
        }
        Ok(Self {
            output_type: config.aggregator_output_type,
            bindings,
        })
    }

    pub(super) fn evaluate(
        &self,
        outputs: &BTreeMap<String, Value>,
        statuses: &BTreeMap<String, NodeExecutionStatus>,
    ) -> Result<(Value, BTreeMap<String, Value>)> {
        let mut selected = Vec::new();
        let mut skipped = Vec::new();
        for binding in &self.bindings {
            match statuses.get(&binding.path[0]) {
                Some(NodeExecutionStatus::Skipped) => skipped.push(binding.path.clone()),
                Some(NodeExecutionStatus::Completed) => {
                    if self.output_type == OutputType::Array || selected.is_empty() {
                        selected.push(binding);
                    }
                }
                _ => return Err(invalid("source is not completed or explicitly skipped")),
            }
        }
        if selected.is_empty() {
            return Err(invalid("no completed source is available"));
        }
        let sources: Vec<_> = selected.iter().map(|binding| &binding.path).collect();
        let references = selected
            .iter()
            .map(|binding| Value::String(binding.reference.clone()))
            .collect();
        // One shared budget for all selected bindings, not one allowance per
        // value. resolve() never parses upstream strings a second time.
        let Value::Array(mut resolved) = values::resolve(&Value::Array(references), outputs)?
        else {
            return Err(invalid("internal binding shape"));
        };
        let output = if self.output_type == OutputType::Array {
            Value::Array(resolved)
        } else {
            resolved.remove(0)
        };
        if !self.output_type.accepts(&output) {
            return Err(invalid(
                "selected value does not match aggregatorOutputType",
            ));
        }
        let output = json!({"output":output});
        values::check_value(&output, "aggregation output")?;
        let metadata = json!({
            "version":1,"outputType":self.output_type.name(),
            "configuredCount":self.bindings.len(),"selectedCount":sources.len(),
            "selectedSources":sources,"skippedSources":skipped
        });
        values::check_value(&metadata, "aggregation provenance")?;
        let updates = BTreeMap::from([("aggregation".into(), metadata)]);
        Ok((output, updates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: &str) -> AggregatorPlan {
        let definition: graph::WorkflowDefinition = serde_json::from_value(json!({
            "version":1,"nodes":[],"edges":[]
        }))
        .unwrap();
        AggregatorPlan::compile(
            &serde_json::from_value(json!({
                "aggregatorOutputType":kind,"aggregatorVariables":["{{#source.output#}}"]
            }))
            .unwrap(),
            &definition,
        )
        .unwrap()
    }

    #[test]
    fn output_without_completed_scheduler_provenance_is_not_available() {
        let outputs = BTreeMap::from([("source".into(), json!({"output":"value"}))]);
        for status in [
            None,
            Some(NodeExecutionStatus::Pending),
            Some(NodeExecutionStatus::Running),
            Some(NodeExecutionStatus::Failed),
            Some(NodeExecutionStatus::Cancelled),
            Some(NodeExecutionStatus::Skipped),
        ] {
            let statuses = status
                .map(|status| BTreeMap::from([("source".into(), status)]))
                .unwrap_or_default();
            assert!(
                plan("string").evaluate(&outputs, &statuses).is_err(),
                "{status:?}"
            );
        }
        let statuses = BTreeMap::from([("source".into(), NodeExecutionStatus::Completed)]);
        assert_eq!(
            plan("string").evaluate(&outputs, &statuses).unwrap().0,
            json!({"output":"value"})
        );
    }

    #[test]
    fn canonical_binding_parser_reuses_the_value_grammar() {
        assert_eq!(
            values::reference_path("{{#node-1.output.0.key#}}").unwrap(),
            ["node-1", "output", "0", "key"]
        );
        for invalid in [
            "",
            "node.output",
            "{{#node.output#}",
            " {{#node.output#}}",
            "{{#node.__proto__#}}",
            "{{#node.output#}}{{#node.output#}}",
        ] {
            assert!(values::reference_path(invalid).is_err(), "{invalid}");
        }
    }
}
