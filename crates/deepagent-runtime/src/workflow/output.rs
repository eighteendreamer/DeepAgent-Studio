//! Compiled v1 end contract. Resolve once and validate the whole object before
//! the existing node/Kernel publication boundary can observe success.
use std::collections::BTreeMap;

use deepagent_core::error::{CoreError, Result};
use serde_json::{Map, Value};

use super::{graph, value_type::ValueType, values, WorkflowNodeSpec};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EndPlan {
    node_id: String,
    bindings: Value,
    fields: BTreeMap<String, ValueType>,
}

impl EndPlan {
    pub(super) fn compile(node: &WorkflowNodeSpec) -> Result<Self> {
        let mut bindings = Map::new();
        let mut fields = BTreeMap::new();
        let declarations = match node.config.get("outputVariables") {
            None => &[][..],
            Some(Value::Array(items)) => items,
            _ => {
                return Err(CoreError::invalid(
                    "WorkflowOutputSchemaInvalid: outputVariables must be an array",
                ))
            }
        };
        for declaration in declarations {
            let name = graph::declaration_name(declaration, "workflow output declaration")?;
            let invalid = |reason| {
                CoreError::invalid(format!(
                    "WorkflowOutputSchemaInvalid: node {} field {name} {reason}",
                    node.id
                ))
            };
            let object = declaration
                .as_object()
                .ok_or_else(|| invalid("requires an object"))?;
            // v1 has no optional/nested schema contract. Do not silently accept
            // schema keywords that this boundary cannot actually enforce.
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "name" | "type" | "value" | "variable" | "description" | "label" | "title"
                )
            }) {
                return Err(invalid("has an unsupported declaration field"));
            }
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .and_then(ValueType::parse)
                .ok_or_else(|| invalid("requires a supported type"))?;
            let binding = object
                .get("value")
                .ok_or_else(|| invalid("requires a value binding"))?;
            if fields.insert(name.to_owned(), kind).is_some() {
                return Err(invalid("is duplicated"));
            }
            bindings.insert(name.to_owned(), binding.clone());
        }
        Ok(Self {
            node_id: node.id.clone(),
            bindings: Value::Object(bindings),
            fields,
        })
    }

    pub(super) fn evaluate(&self, previous: &BTreeMap<String, Value>) -> Result<Value> {
        // One resolver budget for all fields; upstream strings remain data.
        let output = values::resolve(&self.bindings, previous).map_err(|error| {
            CoreError::invalid(format!(
                "InvalidWorkflowOutput: node {} output binding unavailable: {error}",
                self.node_id
            ))
        })?;
        self.validate(&output)?;
        Ok(output)
    }

    fn validate(&self, output: &Value) -> Result<()> {
        values::check_value(output, "end output")?;
        let object = output.as_object().ok_or_else(|| {
            CoreError::invalid(format!(
                "InvalidWorkflowOutput: node {} expected object actual {}",
                self.node_id,
                ValueType::actual_name(output)
            ))
        })?;
        for (name, kind) in &self.fields {
            let value = object.get(name);
            if !value.is_some_and(|value| kind.accepts(value)) {
                return Err(CoreError::invalid(format!(
                    "InvalidWorkflowOutput: node {} field {name} expected {} actual {}",
                    self.node_id,
                    kind.name(),
                    value.map_or("missing", ValueType::actual_name)
                )));
            }
        }
        if object.keys().any(|name| !self.fields.contains_key(name)) {
            return Err(CoreError::invalid(format!(
                "InvalidWorkflowOutput: node {} has an undeclared output field",
                self.node_id
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(declarations: Value) -> Result<EndPlan> {
        EndPlan::compile(
            &serde_json::from_value(
                json!({"id":"end","kind":"end","config":{"outputVariables":declarations}}),
            )
            .unwrap(),
        )
    }

    #[test]
    fn h197_sealed_output_object_rejects_missing_extra_and_wrong_values() {
        let p = plan(json!([{"name":"report","type":"number","value":0}])).unwrap();
        p.validate(&json!({"report":0})).unwrap();
        for (value, message) in [
            (json!({}), "field report expected number actual missing"),
            (
                json!({"report":null}),
                "field report expected number actual null",
            ),
            (
                json!({"report":"PRIVATE_CANARY"}),
                "field report expected number actual string",
            ),
            (
                json!({"report":0,"PRIVATE_KEY":"PRIVATE_CANARY"}),
                "undeclared output field",
            ),
            (json!([]), "expected object actual array"),
        ] {
            let error = p.validate(&value).unwrap_err().to_string();
            assert!(error.contains(message), "{error}");
            assert!(!error.contains("PRIVATE_CANARY") && !error.contains("PRIVATE_KEY"));
        }
    }

    #[test]
    fn h197_output_schema_rejects_missing_binding_and_unenforced_schema_keywords() {
        let valid = json!({"name":"report","type":"object","value":{}});
        for field in [
            "schema",
            "properties",
            "items",
            "required",
            "nullable",
            "default",
            "additionalProperties",
        ] {
            let mut declaration = valid.clone();
            declaration[field] = json!({"nested":"PRIVATE_CANARY"});
            let error = plan(json!([declaration])).unwrap_err().to_string();
            assert!(error.contains("unsupported declaration field"));
            assert!(!error.contains("PRIVATE_CANARY"));
        }
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("value");
        assert!(plan(json!([missing]))
            .unwrap_err()
            .to_string()
            .contains("requires a value binding"));
        assert!(plan(json!([valid.clone(), valid]))
            .unwrap_err()
            .to_string()
            .contains("duplicated"));
        assert_eq!(
            plan(json!([])).unwrap().evaluate(&BTreeMap::new()).unwrap(),
            json!({})
        );
        let annotations = json!([{"name":"report","variable":"ignored_legacy_name","type":"object","value":{"x":[null,false]},"label":"Display","title":"Annotation","description":"Literal annotation"}]);
        assert_eq!(
            plan(annotations)
                .unwrap()
                .evaluate(&BTreeMap::new())
                .unwrap(),
            json!({"report":{"x":[null,false]}})
        );
    }

    #[test]
    fn h197_fields_share_serialized_budget_and_preserve_inserted_strings_as_data() {
        let p = plan(json!([
            {"name":"a","type":"string","value":"{{#source.a#}}"},
            {"name":"b","type":"string","value":"{{#source.b#}}"}
        ]))
        .unwrap();
        let previous = |bytes| {
            BTreeMap::from([(
                "source".into(),
                json!({"a":"\"".repeat(bytes),"b":"\"".repeat(bytes)}),
            )])
        };
        assert!(p
            .evaluate(&previous(300_000))
            .unwrap_err()
            .to_string()
            .contains("1 MiB"));
        let output = p.evaluate(&previous(200_000)).unwrap();
        assert_eq!(output["a"], "\"".repeat(200_000));
        let inserted = "{{#missing.output#}}";
        let p =
            plan(json!([{"name":"report","type":"string","value":"{{#source.text#}}"}])).unwrap();
        assert_eq!(
            p.evaluate(&BTreeMap::from([(
                "source".into(),
                json!({"text":inserted})
            )]))
            .unwrap(),
            json!({"report":inserted})
        );
    }

    #[test]
    fn h197_nested_output_data_still_obeys_the_shared_depth_limit() {
        let p = plan(json!([{"name":"report","type":"object","value":"{{#source.payload#}}"}]))
            .unwrap();
        let mut data = json!({});
        for _ in 0..65 {
            data = json!({"nested":data});
        }
        assert!(p
            .evaluate(&BTreeMap::from([(
                "source".into(),
                json!({"payload":data})
            )]))
            .unwrap_err()
            .to_string()
            .contains("depth 64"));
        assert!(p
            .evaluate(&BTreeMap::from([("source".into(), json!({}))]))
            .unwrap_err()
            .to_string()
            .contains("output binding unavailable"));
    }
}
