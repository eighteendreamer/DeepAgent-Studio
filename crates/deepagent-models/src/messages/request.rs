//! Messages request projection from the canonical Responses item surface.
use anthropic_sdk::{MessageCreateParams, OutputConfig, SystemPrompt, ThinkingConfig, ToolChoice};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::response_item::ResponseItem;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::chat::{ResponseRequest, ToolSchema};

pub(crate) fn encode_request(request: &ResponseRequest, max_tokens: u32) -> Result<String> {
    let (system, messages) = project_messages(request)?;
    let mut body = MessageCreateParams {
        model: request.model.clone(),
        max_tokens: i64::from(max_tokens),
        messages: serde_json::from_value(Value::Array(messages))
            .map_err(|_| invalid("content does not match the Messages SDK schema"))?,
        stream: Some(true),
        system: system
            .filter(|value| !value.is_empty())
            .map(SystemPrompt::Text),
        temperature: request.temperature.map(f64::from),
        top_p: request.top_p.map(f64::from),
        ..Default::default()
    };
    if let Some(effort) = request.reasoning_effort {
        if effort == crate::ReasoningEffort::None {
            body.thinking = Some(ThinkingConfig::Disabled);
        } else {
            body.thinking = Some(ThinkingConfig::Adaptive);
            body.output_config = Some(OutputConfig {
                effort: Some(serde_json::from_value(json!(effort.label()))?),
                format: None,
            });
        }
    }
    if !request.tools.is_empty() {
        body.tools = Some(
            serde_json::from_value(json!(project_tools(&request.tools)))
                .map_err(|_| invalid("tools do not match the Messages SDK schema"))?,
        );
    }
    if let Some(choice) = request.tool_choice.as_ref() {
        body.tool_choice = Some(messages_tool_choice(choice)?);
    }
    Ok(serde_json::to_string(&body)?)
}

pub(crate) fn project_tools(tools: &[ToolSchema]) -> Vec<Value> {
    tools.iter().map(tool_to_wire).collect()
}

fn tool_to_wire(tool: &ToolSchema) -> Value {
    json!({
        "name": tool.function.name,
        "description": tool.function.description,
        "input_schema": tool.function.parameters,
    })
}

fn messages_tool_choice(choice: &Value) -> Result<ToolChoice> {
    match choice {
        Value::String(value) if value == "auto" => Ok(ToolChoice::Auto {
            disable_parallel_tool_use: None,
        }),
        Value::String(value) if value == "none" => Ok(ToolChoice::None),
        Value::String(value) if value == "required" => Ok(ToolChoice::Any {
            disable_parallel_tool_use: None,
        }),
        Value::Object(value)
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) =>
        {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid("named Messages tool choice requires a name"))?;
            Ok(ToolChoice::Tool {
                name: name.to_owned(),
                disable_parallel_tool_use: None,
            })
        }
        _ => Err(invalid("unsupported Messages tool choice")),
    }
}

fn project_messages(request: &ResponseRequest) -> Result<(Option<String>, Vec<Value>)> {
    let mut systems = Vec::new();
    if let Some(instructions) = request
        .instructions
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        systems.push(instructions.to_owned());
    }
    let mut messages = Vec::<Value>::new();
    let mut pending_tool_ids = Vec::new();
    let mut index = 0;
    while index < request.input.len() {
        let item = &request.input[index];
        match item {
            ResponseItem::Message { role, content } => {
                ensure_no_pending_tools(&pending_tool_ids)?;
                push_message(&mut messages, role, json!({"type":"text","text":content}))?;
            }
            ResponseItem::InputMessage {
                role,
                content,
                attachments,
            } => {
                ensure_no_pending_tools(&pending_tool_ids)?;
                if !attachments.is_empty() {
                    return Err(invalid("Messages image attachments are not implemented"));
                }
                push_message(&mut messages, role, json!({"type":"text","text":content}))?;
            }
            ResponseItem::Reasoning {
                content, signature, ..
            } => {
                ensure_no_pending_tools(&pending_tool_ids)?;
                let mut block = json!({"type":"thinking","thinking":content});
                if let Some(signature) = signature {
                    block["signature"] = Value::String(signature.clone());
                }
                push_message(&mut messages, "assistant", block)?;
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                let input: Value = serde_json::from_str(arguments)
                    .map_err(|_| invalid("Messages function arguments must be valid JSON"))?;
                if !input.is_object() {
                    return Err(invalid("Messages function arguments must be a JSON object"));
                }
                pending_tool_ids.push(call_id.clone());
                push_message(
                    &mut messages,
                    "assistant",
                    json!({"type":"tool_use","id":call_id,"name":name,"input":input}),
                )?;
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
            } => {
                pending_tool_ids.push(call_id.clone());
                push_message(
                    &mut messages,
                    "assistant",
                    json!({"type":"tool_use","id":call_id,"name":name,
                        "input":{"patch":input}}),
                )?;
            }
            ResponseItem::FunctionCallOutput { .. } | ResponseItem::CustomToolCallOutput { .. } => {
                if pending_tool_ids.is_empty() {
                    return Err(invalid(
                        "Messages tool_result must follow an assistant tool_use",
                    ));
                }
                let mut results = Vec::new();
                while index < request.input.len() {
                    let (call_id, output) = match &request.input[index] {
                        ResponseItem::FunctionCallOutput { call_id, output }
                        | ResponseItem::CustomToolCallOutput { call_id, output } => {
                            (call_id.clone(), output)
                        }
                        _ => break,
                    };
                    if output.attachments().next().is_some() {
                        return Err(invalid("Messages tool result images are not implemented"));
                    }
                    results.push((call_id, output.text()));
                    index += 1;
                }
                let order: HashMap<&str, usize> = pending_tool_ids
                    .iter()
                    .enumerate()
                    .map(|(position, call_id)| (call_id.as_str(), position))
                    .collect();
                if results.len() != pending_tool_ids.len()
                    || results
                        .iter()
                        .map(|(call_id, _)| call_id)
                        .collect::<HashSet<_>>()
                        .len()
                        != results.len()
                    || results
                        .iter()
                        .any(|(call_id, _)| !order.contains_key(call_id.as_str()))
                    || order.len() != results.len()
                {
                    return Err(invalid(
                        "Messages tool_result set must match the preceding tool_use set",
                    ));
                }
                results.sort_by_key(|(call_id, _)| order[call_id.as_str()]);
                for (call_id, content) in results {
                    push_message(
                        &mut messages,
                        "user",
                        json!({"type":"tool_result","tool_use_id":call_id,"content":content}),
                    )?;
                }
                pending_tool_ids.clear();
                continue;
            }
            ResponseItem::WebSearchCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. } => {
                ensure_no_pending_tools(&pending_tool_ids)?;
                return Err(invalid("Messages does not support host discovery items"));
            }
        }
        index += 1;
    }
    if !pending_tool_ids.is_empty() {
        return Err(invalid(
            "Messages assistant tool_use set has no matching tool_result set",
        ));
    }
    if messages.is_empty() {
        return Err(invalid(
            "Messages request requires at least one user or assistant message",
        ));
    }
    Ok((
        (!systems.is_empty()).then(|| systems.join("\n\n")),
        messages,
    ))
}

fn ensure_no_pending_tools(pending_tool_ids: &[String]) -> Result<()> {
    if pending_tool_ids.is_empty() {
        Ok(())
    } else {
        Err(invalid(
            "Messages tool_result must immediately follow the assistant tool_use set",
        ))
    }
}

fn push_message(messages: &mut Vec<Value>, role: &str, block: Value) -> Result<()> {
    let role = match role {
        "user" | "assistant" => role,
        "system" => {
            return Err(invalid(
                "Messages system items must be supplied as instructions",
            ))
        }
        "tool" => return Err(invalid("Messages tool role must use tool_result blocks")),
        _ => return Err(invalid("Messages message role is unsupported")),
    };
    let needs_new = messages
        .last()
        .and_then(Value::as_object)
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        != Some(role);
    if needs_new {
        messages.push(json!({"role":role,"content":[block]}));
    } else {
        messages
            .last_mut()
            .and_then(Value::as_object_mut)
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid("Messages internal content projection failed"))?
            .push(block);
    }
    Ok(())
}

fn invalid(message: &str) -> CoreError {
    CoreError::invalid(format!("Messages request: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::responses::ResponseInputItem;

    #[test]
    fn projects_system_reasoning_tool_use_and_results_without_lossy_chat_conversion() {
        let mut request = ResponseRequest::from_response_items(
            "claude-fixture",
            None,
            vec![
                ResponseInputItem::Message {
                    role: "user".into(),
                    content: "read".into(),
                },
                ResponseInputItem::Reasoning {
                    id: None,
                    content: "think".into(),
                    signature: Some("sig".into()),
                },
                ResponseInputItem::FunctionCall {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"a.txt"}"#.into(),
                },
                ResponseInputItem::FunctionCallOutput {
                    call_id: "call-1".into(),
                    output: deepagent_core::tool_content::ToolResultBody::Text("ok".into()),
                },
            ],
        );
        request.instructions = Some("rules".into());
        request.tools = vec![ToolSchema::function(
            "read_file",
            "Read",
            json!({"type":"object"}),
        )];
        let body: Value = serde_json::from_str(&encode_request(&request, 128).unwrap()).unwrap();
        assert_eq!(body["system"], "rules");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][1]["content"][0]["signature"], "sig");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn orders_multiple_tool_results_by_the_preceding_tool_use_order() {
        let request = ResponseRequest::from_response_items(
            "claude-fixture",
            None,
            vec![
                ResponseInputItem::Message {
                    role: "user".into(),
                    content: "read".into(),
                },
                ResponseInputItem::FunctionCall {
                    call_id: "call-a".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
                ResponseInputItem::FunctionCall {
                    call_id: "call-b".into(),
                    name: "list_directory".into(),
                    arguments: "{}".into(),
                },
                ResponseInputItem::FunctionCallOutput {
                    call_id: "call-b".into(),
                    output: deepagent_core::tool_content::ToolResultBody::Text("b".into()),
                },
                ResponseInputItem::FunctionCallOutput {
                    call_id: "call-a".into(),
                    output: deepagent_core::tool_content::ToolResultBody::Text("a".into()),
                },
            ],
        );
        let body: Value = serde_json::from_str(&encode_request(&request, 128).unwrap()).unwrap();
        let results = body["messages"][2]["content"].as_array().unwrap();
        assert_eq!(results[0]["tool_use_id"], "call-a");
        assert_eq!(results[1]["tool_use_id"], "call-b");
    }

    #[test]
    fn rejects_a_tool_result_set_that_does_not_match_the_tool_uses() {
        let mut request = ResponseRequest::from_response_items("claude-fixture", None, vec![]);
        request.input = vec![
            ResponseInputItem::FunctionCall {
                call_id: "call-a".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "call-unknown".into(),
                output: deepagent_core::tool_content::ToolResultBody::Text("x".into()),
            },
        ];
        assert!(encode_request(&request, 128)
            .unwrap_err()
            .to_string()
            .contains("must match"));
    }

    #[test]
    fn rejects_duplicate_tool_results_even_when_the_count_matches() {
        let request = ResponseRequest::from_response_items(
            "claude-fixture",
            None,
            vec![
                ResponseInputItem::FunctionCall {
                    call_id: "call-a".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
                ResponseInputItem::FunctionCall {
                    call_id: "call-b".into(),
                    name: "list_directory".into(),
                    arguments: "{}".into(),
                },
                ResponseInputItem::FunctionCallOutput {
                    call_id: "call-a".into(),
                    output: deepagent_core::tool_content::ToolResultBody::Text("a".into()),
                },
                ResponseInputItem::FunctionCallOutput {
                    call_id: "call-a".into(),
                    output: deepagent_core::tool_content::ToolResultBody::Text("duplicate".into()),
                },
            ],
        );
        assert!(encode_request(&request, 128)
            .unwrap_err()
            .to_string()
            .contains("must match"));
    }

    #[test]
    fn required_tool_choice_uses_messages_any_variant() {
        let mut request = ResponseRequest::new(
            "claude-fixture",
            vec![deepagent_core::message::Message::user("read")],
        );
        request.tools = vec![ToolSchema::function(
            "read_file",
            "Read",
            json!({"type":"object"}),
        )];
        request.tool_choice = Some(json!("required"));
        let body: Value = serde_json::from_str(&encode_request(&request, 128).unwrap()).unwrap();
        assert_eq!(body["tool_choice"], json!({"type":"any"}));
    }

    #[test]
    fn explicit_none_disables_thinking_without_an_invalid_effort() {
        let mut request = ResponseRequest::new(
            "claude-fixture",
            vec![deepagent_core::message::Message::user("hello")],
        );
        request.reasoning_effort = Some(crate::ReasoningEffort::None);
        let body: Value = serde_json::from_str(&encode_request(&request, 128).unwrap()).unwrap();
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert!(body.get("output_config").is_none());
    }
}
