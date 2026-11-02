//! Versioned canvas code contract. Source is literal text, not a workflow
//! template. Only input bindings are resolved. `main(inputs)` must return an
//! object matching the declared outputs; no missing value is fabricated.
//!
//! This is a local execution contract, not a provider API or an interpreter.
//! Preparation returns only an opaque handle and non-secret audit metadata.
//! The Kernel owns permission, approval, cancellation and tool dispatch.

use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{graph, values};

pub const CODE_EXECUTE_TOOL: &str = "canvas_code_execute";
pub const CODE_VERSION: u32 = 1;
pub const MAX_CODE_BYTES: usize = 64 * 1024;
const MAX_VARIABLES: usize = 128;
pub const MAX_CODE_LOG_BYTES: usize = 16 * 1024;
pub const MAX_CODE_RESPONSE_BYTES: usize = values::MAX_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeLanguage {
    Javascript,
    Python,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeInput {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeOutput {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRequest {
    pub version: u32,
    #[serde(rename = "codeLanguage")]
    pub language: CodeLanguage,
    #[serde(rename = "codeScript")]
    pub script: String,
    #[serde(rename = "codeInputVariables")]
    pub inputs: Vec<CodeInput>,
    #[serde(rename = "codeOutputVariables")]
    pub outputs: Vec<CodeOutput>,
}

impl CodeRequest {
    pub fn from_config(
        config: &Map<String, Value>,
        previous_outputs: &BTreeMap<String, Value>,
    ) -> Result<Self> {
        // Select the published fields, not legacy `code`/`codeVariables` or
        // unrelated UI metadata. Never resolve the script, even if it contains
        // a canonical reference marker or an intentionally malformed one.
        let mut request: Self = serde_json::from_value(json!({
            "version": CODE_VERSION,
            "codeLanguage": config.get("codeLanguage").cloned().unwrap_or(json!("javascript")),
            "codeScript": config.get("codeScript"),
            "codeInputVariables": config.get("codeInputVariables").cloned().unwrap_or(json!([])),
            "codeOutputVariables": config.get("codeOutputVariables").cloned().unwrap_or(json!([])),
        }))
        .map_err(|_| CoreError::invalid("CodeInputInvalid: invalid code-node configuration"))?;
        request.validate()?;
        for input in &mut request.inputs {
            input.value = values::resolve(&input.value, previous_outputs)?;
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != CODE_VERSION
            || self.script.trim().is_empty()
            || self.script.len() > MAX_CODE_BYTES
            || self.inputs.len() > MAX_VARIABLES
            || self.outputs.len() > MAX_VARIABLES
        {
            return Err(CoreError::invalid(
                "CodeInputInvalid: unsupported version, empty/oversized source or too many variables",
            ));
        }
        let mut inputs = BTreeSet::new();
        for input in &self.inputs {
            let declaration = json!({"name": input.name});
            graph::declaration_name(&declaration, "code input")?;
            if !inputs.insert(&input.name) {
                return Err(CoreError::invalid("CodeInputInvalid: duplicate input name"));
            }
        }
        let mut outputs = BTreeSet::new();
        for output in &self.outputs {
            let declaration = json!({"name": output.name});
            graph::declaration_name(&declaration, "code output")?;
            graph::validate_type(Some(&json!(output.value_type)), "code output")?;
            if !outputs.insert(&output.name) {
                return Err(CoreError::invalid(
                    "CodeInputInvalid: duplicate output name",
                ));
            }
        }
        values::check_value(&serde_json::to_value(self)?, "code request")
    }

    pub fn input_object(&self) -> Map<String, Value> {
        self.inputs
            .iter()
            .map(|input| (input.name.clone(), input.value.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeInvocation {
    pub request_id: String,
    pub language: CodeLanguage,
    pub script_sha256: String,
}

/// The host keeps source and inputs run-local. This is preparation only:
/// resolving SDKs and starting processes before Kernel approval is forbidden.
pub trait CodeBridge: Send + Sync {
    fn prepare(&self, request: CodeRequest) -> Result<CodeInvocation>;
    fn release(&self, request_id: &str);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeIsolation {
    WindowsLpacV1,
    WindowsAppcontainerV1,
    WindowsSandboxV1,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeReceipt {
    pub language: CodeLanguage,
    pub script_sha256: String,
    pub runtime_version: String,
    pub isolation: CodeIsolation,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeResponse {
    pub version: u32,
    pub outputs: Map<String, Value>,
    pub receipt: CodeReceipt,
}

impl CodeResponse {
    pub fn validate(&self, invocation: &CodeInvocation, declared: &[CodeOutput]) -> Result<()> {
        if self.version != CODE_VERSION
            || self.receipt.language != invocation.language
            || self.receipt.script_sha256 != invocation.script_sha256
            || self.receipt.runtime_version.trim().is_empty()
            || self.receipt.exit_code != 0
            || self.receipt.stdout.len() > MAX_CODE_LOG_BYTES
            || self.receipt.stderr.len() > MAX_CODE_LOG_BYTES
            || self.outputs.len() != declared.len()
        {
            return Err(CoreError::invalid(
                "CodeResultInvalid: invalid execution receipt or output count",
            ));
        }
        for declaration in declared {
            let value = self.outputs.get(&declaration.name);
            let valid = match (declaration.value_type.as_str(), value) {
                ("string", Some(Value::String(_)))
                | ("number", Some(Value::Number(_)))
                | ("boolean", Some(Value::Bool(_)))
                | ("object", Some(Value::Object(_)))
                | ("array", Some(Value::Array(_))) => true,
                // A code node can pass an existing opaque artifact reference,
                // never a host path or inline file bytes. It gets no file grant.
                ("file", Some(Value::String(reference))) => {
                    reference.starts_with("artifact://") && reference.len() > "artifact://".len()
                }
                _ => false,
            };
            if !valid {
                return Err(CoreError::invalid(
                    "CodeResultInvalid: missing or wrongly typed declared output",
                ));
            }
        }
        values::check_value(&serde_json::to_value(self)?, "code response")
    }
}

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod tests;
