//! DeepSeek Responses choice contract; Chat has its own wire shape.
use deepagent_core::error::{CoreError, Result};
use deepagent_tools::validate_tool_name;
use serde_json::Value;

use crate::ToolSchema;

/// Keep public JSON fields source/storage compatible, but interpret them only
/// through this closed protocol type. Never include rejected values in errors.
enum ResponsesToolChoice<'a> {
    Auto,
    None,
    Required,
    Function { name: &'a str },
}

impl<'a> ResponsesToolChoice<'a> {
    fn parse(value: &'a Value) -> Result<Self> {
        match value {
            Value::String(mode) => match mode.as_str() {
                "auto" => Ok(Self::Auto),
                "none" => Ok(Self::None),
                "required" => Ok(Self::Required),
                _ => Err(invalid_shape()),
            },
            Value::Object(object)
                if object.len() == 2
                    && object.get("type").and_then(Value::as_str) == Some("function") =>
            {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_shape)?;
                validate_tool_name(name)?;
                Ok(Self::Function { name })
            }
            _ => Err(invalid_shape()),
        }
    }
}

fn invalid_shape() -> CoreError {
    CoreError::invalid(
        "Responses tool_choice must be auto, none, required, or a flat function choice with name",
    )
}

/// Settings can validate syntax, but only the request knows its final tools.
pub fn validate_responses_tool_choice(value: &Value) -> Result<()> {
    ResponsesToolChoice::parse(value).map(|_| ())
}

pub(crate) fn validate_final_responses_tool_choice(
    value: Option<&Value>,
    tools: &[ToolSchema],
) -> Result<()> {
    if let Some(ResponsesToolChoice::Function { name }) =
        value.map(ResponsesToolChoice::parse).transpose()?
    {
        let matches = tools
            .iter()
            .filter(|tool| tool.kind == "function" && tool.function.name == name)
            .count();
        if matches != 1 {
            return Err(CoreError::invalid(
                "Responses tool_choice must reference exactly one function in the final tools",
            ));
        }
    }
    Ok(())
}
