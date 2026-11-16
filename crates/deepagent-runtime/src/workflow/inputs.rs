//! Local v1 input contract. Compilation validates defaults; admission resolves
//! exactly once before any graph root can execute. Values are data, not bindings.
use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde_json::{json, Map, Value};

use super::{graph, value_type::ValueType, values, WorkflowNodeSpec};

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct InputPlan {
    fields: BTreeMap<String, InputField>,
}

#[derive(Debug, Clone, PartialEq)]
struct InputField {
    name: String,
    kind: ValueType,
    required: bool,
    default: Option<Value>,
    options: Vec<String>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    minimum: Option<Value>,
    maximum: Option<Value>,
}

pub(super) struct AdmittedInputs {
    pub(super) output: Value,
    pub(super) updates: BTreeMap<String, Value>,
}

fn config_error() -> CoreError {
    CoreError::invalid("WorkflowInputSchemaInvalid: malformed input declaration or constraints")
}

impl InputPlan {
    pub(super) fn compile(start: &WorkflowNodeSpec) -> Result<Self> {
        let declarations = match start.config.get("inputVariables") {
            None => return Ok(Self::default()),
            Some(Value::Array(items)) => items,
            _ => return Err(config_error()),
        };
        let mut fields = BTreeMap::new();
        for declaration in declarations {
            let field = InputField::compile(declaration)?;
            if fields.insert(field.name.clone(), field).is_some() {
                return Err(config_error());
            }
        }
        Ok(Self { fields })
    }

    pub(super) fn admit(
        &self,
        inputs: Map<String, Value>,
        revision: &str,
    ) -> Result<AdmittedInputs> {
        let raw = Value::Object(inputs);
        values::check_value(&raw, "inputs")?;
        let Value::Object(mut inputs) = raw else {
            unreachable!()
        };
        if inputs.keys().any(|name| !self.fields.contains_key(name)) {
            // Never echo attacker-controlled keys or values into logs/events.
            return Err(CoreError::invalid(
                "UnknownWorkflowInput: undeclared input field",
            ));
        }
        let mut output = Map::new();
        let mut sources = Map::new();
        for field in self.fields.values() {
            let (value, source) = if let Some(value) = inputs.remove(&field.name) {
                (value, "provided")
            } else if let Some(default) = &field.default {
                (default.clone(), "default")
            } else if field.required {
                return Err(CoreError::invalid(format!(
                    "MissingWorkflowInput: required field {}",
                    field.name
                )));
            } else {
                (Value::Null, "absent")
            };
            field.validate(&value)?;
            output.insert(field.name.clone(), value);
            sources.insert(field.name.clone(), Value::from(source));
        }
        let output = Value::Object(output);
        values::check_value(&output, "normalized inputs")?;
        let metadata = json!({"version":1,"revision":revision,"sources":sources});
        values::check_value(&metadata, "input provenance")?;
        Ok(AdmittedInputs {
            output,
            updates: BTreeMap::from([("inputs".into(), metadata)]),
        })
    }
}

impl InputField {
    fn compile(declaration: &Value) -> Result<Self> {
        values::check_value(declaration, "input declaration")?;
        let location = "workflow input declaration";
        let name = graph::declaration_name(declaration, location)?.to_owned();
        graph::validate_type(declaration.get("type"), location)?;
        let object = declaration.as_object().ok_or_else(config_error)?;
        // Preserve the graph's existing annotation/legacy-metadata tolerance.
        // Only the canonical name and published constraint fields have meaning;
        // metadata never adds an input key or another execution path.
        let required = match object.get("required") {
            None => false,
            Some(Value::Bool(required)) => *required,
            _ => return Err(config_error()),
        };
        let mut field = InputField {
            name,
            kind: object["type"]
                .as_str()
                .and_then(ValueType::parse)
                .ok_or_else(config_error)?,
            required,
            default: object.get("default").cloned(),
            options: Vec::new(),
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
        };
        field.compile_constraints(object)?;
        if let Some(default) = &field.default {
            field.validate(default).map_err(|_| config_error())?;
        }
        Ok(field)
    }

    fn compile_constraints(&mut self, object: &Map<String, Value>) -> Result<()> {
        // The desktop retains hidden fields when switching input type.
        // Only the constraints of the active type apply, just as in its
        // x-visible-when schema; they are never coerced into another type.
        match self.kind {
            ValueType::String => {
                if let Some(options) = object.get("options") {
                    let mut unique = BTreeSet::new();
                    for option in options.as_array().ok_or_else(config_error)? {
                        let option = option.as_str().ok_or_else(config_error)?;
                        if !unique.insert(option) {
                            return Err(config_error());
                        }
                        self.options.push(option.to_owned());
                    }
                }
                let length = |key| {
                    object
                        .get(key)
                        .map(|value| value.as_u64().ok_or_else(config_error))
                        .transpose()
                };
                self.min_length = length("minLength")?;
                self.max_length = length("maxLength")?;
                if matches!((self.min_length, self.max_length), (Some(min),Some(max)) if min > max)
                {
                    return Err(config_error());
                }
            }
            ValueType::Number => {
                let bound = |key| {
                    object
                        .get(key)
                        .map(|value| {
                            if value.is_number() {
                                Ok(value.clone())
                            } else {
                                Err(config_error())
                            }
                        })
                        .transpose()
                };
                self.minimum = bound("minimum")?;
                self.maximum = bound("maximum")?;
                if let (Some(min), Some(max)) = (&self.minimum, &self.maximum) {
                    if values::compare(min, "gt", max)? {
                        return Err(config_error());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn invalid(&self) -> CoreError {
        CoreError::invalid(format!(
            "InvalidWorkflowInput: field {} violates its declared type or constraints",
            self.name
        ))
    }

    fn validate(&self, value: &Value) -> Result<()> {
        if value.is_null() {
            return if self.required {
                Err(self.invalid())
            } else {
                Ok(())
            };
        }
        if !self.kind.accepts(value) {
            return Err(self.invalid());
        }
        let valid = match (self.kind, value) {
            (ValueType::String, Value::String(text)) => {
                let length = text.chars().count() as u64;
                (self.options.is_empty() || self.options.contains(text))
                    && self.min_length.is_none_or(|min| length >= min)
                    && self.max_length.is_none_or(|max| length <= max)
            }
            (ValueType::Number, Value::Number(_)) => {
                let min = self
                    .minimum
                    .as_ref()
                    .map(|min| values::compare(value, "gte", min))
                    .transpose()?;
                let max = self
                    .maximum
                    .as_ref()
                    .map(|max| values::compare(value, "lte", max))
                    .transpose()?;
                min.unwrap_or(true) && max.unwrap_or(true)
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(self.invalid())
        }
    }
}
