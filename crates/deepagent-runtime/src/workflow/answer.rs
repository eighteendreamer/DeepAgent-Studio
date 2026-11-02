//! Local version-1 answer contract, matching the published desktop fields.
//! Templates are code; resolved bindings and inserted output are data. Rendering
//! happens exactly once through the existing bounded workflow value boundary.
use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{graph, values};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AnswerConfig {
    answer_template: String,
    #[serde(default)]
    answer_variables: Vec<Value>,
    // Accepted graph metadata is never executable and never rendered.
    #[serde(rename = "description")]
    _description: Option<String>,
    #[serde(rename = "label")]
    _label: Option<String>,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::invalid(format!("AnswerConfigInvalid: {reason}"))
}

impl AnswerConfig {
    pub(super) fn parse(config: &Map<String, Value>) -> Result<Self> {
        if config.contains_key("text") || config.contains_key("variables") {
            return Err(invalid(
                "legacy text/variables are not version-1 answer fields; explicitly migrate to answerTemplate/answerVariables",
            ));
        }
        let parsed: Self = serde_json::from_value(Value::Object(config.clone())).map_err(|_| {
            invalid("expected answerTemplate string, optional answerVariables array and no unknown fields")
        })?;
        if parsed.answer_template.trim().is_empty() {
            return Err(invalid("answerTemplate must contain non-whitespace text"));
        }
        let mut names = BTreeSet::new();
        for binding in &parsed.answer_variables {
            let object = binding
                .as_object()
                .ok_or_else(|| invalid("binding must be an object"))?;
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "name" | "value"))
            {
                return Err(invalid("binding permits only name and value"));
            }
            let name = graph::declaration_name(binding, "answer binding")?;
            if !names.insert(name) || !object.contains_key("value") {
                return Err(invalid(
                    "binding requires a unique name and an explicit value",
                ));
            }
        }
        values::validate_template(&parsed.answer_template, &names)?;
        Ok(parsed)
    }

    pub(super) fn render(&self, outputs: &BTreeMap<String, Value>) -> Result<Value> {
        let raw_bindings = self
            .answer_variables
            .iter()
            .map(|binding| {
                // parse() already checked names and required values, including null.
                let name = graph::declaration_name(binding, "answer binding")?;
                Ok((name.to_owned(), binding["value"].clone()))
            })
            .collect::<Result<Map<String, Value>>>()?;
        // Resolve the entire namespace under one existing budget. Independent
        // per-binding budgets would permit unused aliases to multiply a large
        // upstream value before the renderer had a chance to enforce its cap.
        let bindings = values::resolve(&Value::Object(raw_bindings), outputs)?;
        let bindings = bindings
            .as_object()
            .ok_or_else(|| invalid("bindings must resolve to an object"))?;
        let answer = values::render_template(&self.answer_template, bindings, outputs)?;
        if answer.trim().is_empty() {
            return Err(invalid("rendered answer must contain non-whitespace text"));
        }
        let output = json!({"answer":answer});
        values::check_value(&output, "answer output")?;
        Ok(output)
    }
}
