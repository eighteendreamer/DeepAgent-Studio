//! Closed tool choices with explicit DeepSeek Responses and Chat wire shapes.
use deepagent_core::error::{CoreError, Result};
use deepagent_tools::validate_tool_name;
use serde_json::Value;

use crate::ToolSchema;

/// Keep public JSON fields source/storage compatible, but interpret them only
/// through this closed protocol type. Never include rejected values in errors.
enum ToolChoice<'a> {
    Auto,
    None,
    Required,
    Function { name: &'a str },
}

#[derive(Clone, Copy)]
enum ChoiceFormat {
    Responses,
    Chat,
}

impl<'a> ToolChoice<'a> {
    fn parse(value: &'a Value, format: ChoiceFormat) -> Result<Self> {
        match value {
            Value::String(mode) => match mode.as_str() {
                "auto" => Ok(Self::Auto),
                "none" => Ok(Self::None),
                "required" => Ok(Self::Required),
                _ => Err(invalid_shape(format)),
            },
            Value::Object(object)
                if object.len() == 2
                    && object.get("type").and_then(Value::as_str) == Some("function") =>
            {
                let name = match format {
                    ChoiceFormat::Responses => object.get("name"),
                    ChoiceFormat::Chat => object
                        .get("function")
                        .and_then(Value::as_object)
                        .filter(|function| function.len() == 1)
                        .and_then(|function| function.get("name")),
                }
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_shape(format))?;
                validate_tool_name(name)?;
                Ok(Self::Function { name })
            }
            _ => Err(invalid_shape(format)),
        }
    }
}

fn invalid_shape(format: ChoiceFormat) -> CoreError {
    CoreError::invalid(match format {
        ChoiceFormat::Responses => "Responses tool_choice must be auto, none, required, or a flat function choice with name",
        ChoiceFormat::Chat => "Chat tool_choice must be auto, none, required, or a nested function choice with name",
    })
}

/// Settings can validate syntax, but only the request knows its final tools.
pub fn validate_responses_tool_choice(value: &Value) -> Result<()> {
    ToolChoice::parse(value, ChoiceFormat::Responses).map(|_| ())
}

/// Canonical request/default choices are Responses-shaped, never native Chat.
pub(crate) fn responses_tool_choice_to_chat(value: &Value) -> Result<Value> {
    Ok(match ToolChoice::parse(value, ChoiceFormat::Responses)? {
        ToolChoice::Auto => Value::String("auto".into()),
        ToolChoice::None => Value::String("none".into()),
        ToolChoice::Required => Value::String("required".into()),
        ToolChoice::Function { name } => {
            serde_json::json!({"type":"function","function":{"name":name}})
        }
    })
}

pub(crate) fn validate_chat_tool_choice(value: Option<&Value>) -> Result<()> {
    value
        .map(|value| ToolChoice::parse(value, ChoiceFormat::Chat))
        .transpose()
        .map(|_| ())
}

pub(crate) fn validate_final_responses_tool_choice(
    value: Option<&Value>,
    tools: &[ToolSchema],
) -> Result<()> {
    if let Some(ToolChoice::Function { name }) = value
        .map(|value| ToolChoice::parse(value, ChoiceFormat::Responses))
        .transpose()?
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
