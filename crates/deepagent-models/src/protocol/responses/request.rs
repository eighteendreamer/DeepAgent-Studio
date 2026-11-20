//! Responses SDK projection. DeepSeek-only shapes stay explicit at this seam.
use async_openai::types::responses as sdk;
use deepagent_core::error::Result;
use serde::{Serialize, Serializer};
use serde_json::{json, Value};

use crate::chat::{ResponseRequest, ToolSchema};
use crate::ResponseInputItem;
use deepagent_core::error::CoreError;
use deepagent_core::message::MessageAttachment;
use deepagent_core::tool_content::{ToolResultBody, ToolResultPart};
use std::collections::BTreeMap;

/// Use the SDK envelope and standard item/tool types, then overlay only fields
/// whose existing DeepSeek contract differs from the pinned SDK. In particular,
/// never round-trip an entire input through SDK InputItem: its untagged
/// ItemReference alternative can consume an item with an id and erase content.
pub(crate) fn serialize<S: Serializer>(
    request: &ResponseRequest,
    serializer: S,
    effort: Option<&str>,
) -> std::result::Result<S::Ok, S::Error> {
    request
        .validate_wire_parameters()
        .map_err(serde::ser::Error::custom)?;
    let result = project(request, effort).map_err(serde::ser::Error::custom)?;
    result.serialize(serializer)
}

#[derive(Serialize)]
struct WireRequest {
    #[serde(flatten)]
    body: Value,
    // A Value converts f32 to f64 before printing (0.95 -> 0.949999988...).
    // Keep the SDK's floats typed until the final bounded serialization.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
}

fn project(request: &ResponseRequest, effort: Option<&str>) -> Result<WireRequest> {
    let mut body = sdk::CreateResponse {
        model: Some(request.model.clone()),
        input: sdk::InputParam::Items(Vec::new()),
        instructions: request.instructions.clone(),
        stream: Some(request.stream),
        temperature: request.temperature,
        top_p: request.top_p,
        max_output_tokens: request.max_output_tokens,
        top_logprobs: request.top_logprobs,
        text: request
            .text
            .clone()
            .map(serde_json::from_value)
            .transpose()?,
        tool_choice: request
            .tool_choice
            .clone()
            .map(serde_json::from_value)
            .transpose()?,
        // Tianshu's forced-function projection is a single-call contract.
        // Keep the named choice; some compatible routes reject it with the
        // protocol's implicit parallel-call default.
        parallel_tool_calls: request
            .tool_choice
            .as_ref()
            .is_some_and(Value::is_object)
            .then_some(false),
        ..Default::default()
    };
    // SDK effort enums intentionally do not cover custom provider wire levels.
    // Standard levels still go through the SDK; only missing levels are overlaid.
    if let Some(effort) = effort {
        if let Ok(effort) = serde_json::from_value::<sdk::ReasoningEffort>(json!(effort)) {
            body.reasoning = Some(sdk::Reasoning {
                effort: Some(effort),
                ..Default::default()
            });
        }
    }
    let temperature = body.temperature.take();
    let top_p = body.top_p.take();
    let mut value = serde_json::to_value(&body)?;
    value["input"] = response_input_items_to_wire(
        &request.input,
        &request.attachment_data_urls,
        &request.image_file_ids,
    )?
    .into();
    if !request.tools.is_empty() {
        value["tools"] = response_tools_to_wire(&request.tools).into();
    }
    if body.reasoning.is_none() {
        if let Some(effort) = effort {
            value["reasoning"] = json!({"effort": effort});
        }
    }
    // DeepSeek still uses user; it is not OpenAI's safety_identifier.
    if let Some(user) = &request.user {
        value["user"] = json!(user);
    }
    Ok(WireRequest {
        body: value,
        temperature,
        top_p,
    })
}

pub(crate) fn response_tools_to_wire(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| match tool.kind.as_str() {
            "function" => serde_json::to_value(sdk::Tool::Function(sdk::FunctionTool {
                name: tool.function.name.clone(),
                description: Some(tool.function.description.clone()),
                parameters: Some(tool.function.parameters.clone()),
                ..Default::default()
            }))
            .expect("SDK function tool contains serializable JSON"),
            "web_search" => {
                serde_json::to_value(sdk::Tool::WebSearch(sdk::WebSearchTool::default()))
                    .expect("SDK web search defaults are serializable")
            }
            // DeepSeek custom tools omit the SDK's required format and description.
            "custom" => json!({"type":"custom","name":tool.function.name}),
            _ => json!({"type":tool.kind}),
        })
        .collect()
}

fn sdk_item(item: sdk::Item) -> Result<Value> {
    Ok(serde_json::to_value(item)?)
}

/// File references intentionally omit detail in the existing DeepSeek contract;
/// the SDK otherwise inserts its auto default. Inline images retain auto.
fn retain_file_reference_shape(value: &mut Value) {
    for field in ["content", "output"] {
        if let Some(parts) = value.get_mut(field).and_then(Value::as_array_mut) {
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("input_image")
                    && part.get("file_id").is_some()
                {
                    part.as_object_mut().expect("image part").remove("detail");
                }
            }
        }
    }
}

pub(crate) fn response_input_items_to_wire(
    items: &[ResponseInputItem],
    data_urls: &BTreeMap<String, String>,
    file_ids: &BTreeMap<String, String>,
) -> Result<Vec<Value>> {
    items.iter().map(|item| {
        let mut value = match item {
            ResponseInputItem::Message { role, content } => message(role, sdk::EasyInputContent::Text(content.clone()))?,
            ResponseInputItem::InputMessage { role, content, attachments } => {
                let mut parts = Vec::new();
                for attachment in attachments.iter().filter(|attachment| attachment.kind == "image") {
                    crate::WireMode::Responses.validate_image_role(role)?;
                    if !file_ids.contains_key(&attachment.id)
                        && data_urls.get(&attachment.id).is_none_or(|url| url.trim().is_empty())
                    {
                        return Err(CoreError::invalid("image_input_message_unresolved: image message requires a resolved image reference; no image was sent"));
                    }
                    parts.push(image_part(attachment, data_urls, file_ids));
                }
                let content = if parts.is_empty() {
                    sdk::EasyInputContent::Text(content.clone())
                } else {
                    if !content.is_empty() { parts.insert(0, text_part(content)); }
                    sdk::EasyInputContent::ContentList(parts)
                };
                message(role, content)?
            }
            ResponseInputItem::FunctionCall { call_id, name, arguments } => sdk_item(sdk::Item::FunctionCall(sdk::FunctionToolCall {
                call_id: call_id.clone(), name: name.clone(), arguments: arguments.clone(), ..Default::default()
            }))?,
            ResponseInputItem::FunctionCallOutput { call_id, output } => sdk_item(sdk::Item::FunctionCallOutput(sdk::FunctionCallOutputItemParam {
                call_id: Some(call_id.clone()), output: tool_output(output, data_urls, file_ids),
                id: None, status: None, name: None, namespace: None, caller: None,
            }))?,
            ResponseInputItem::CustomToolCallOutput { call_id, output } => {
                let output = match tool_output(output, data_urls, file_ids) {
                    sdk::FunctionCallOutput::Text(text) => sdk::CustomToolCallOutputOutput::Text(text),
                    sdk::FunctionCallOutput::Content(parts) => sdk::CustomToolCallOutputOutput::List(parts),
                };
                sdk_item(sdk::Item::CustomToolCallOutput(sdk::CustomToolCallOutput { call_id: call_id.clone(), output, id: None, caller: None }))?
            }
            // DeepSeek reasoning omits the SDK-required summary. Its custom
            // calls omit id; native search payloads retain provider extensions.
            ResponseInputItem::Reasoning { id, content, .. } => {
                let mut value = json!({"type":"reasoning", "content":[sdk::ReasoningItemContent::ReasoningText(sdk::ReasoningTextContent { text: content.clone() })]});
                if let Some(id) = id { value["id"] = json!(id); }
                value
            }
            ResponseInputItem::CustomToolCall { call_id, name, input } => json!({"type":"custom_tool_call","call_id":call_id,"name":name,"input":input}),
            ResponseInputItem::WebSearchCall { id, status, action } => {
                let mut value = json!({"type":"web_search_call","id":id,"status":status});
                if let Some(action) = action { value["action"] = action.clone(); }
                value
            }
            ResponseInputItem::ToolSearchCall { .. } | ResponseInputItem::ToolSearchOutput { .. } => serde_json::to_value(item)?,
        };
        retain_file_reference_shape(&mut value);
        Ok(value)
    }).collect()
}

fn message(role: &str, content: sdk::EasyInputContent) -> Result<Value> {
    let role = match role {
        "user" => sdk::Role::User,
        "assistant" => sdk::Role::Assistant,
        "system" => sdk::Role::System,
        "developer" => sdk::Role::Developer,
        _ => {
            return Err(CoreError::invalid(
                "Responses message role must be user, assistant, system, or developer",
            ))
        }
    };
    Ok(serde_json::to_value(sdk::EasyInputMessage {
        role,
        content,
        ..Default::default()
    })?)
}

fn text_part(text: &str) -> sdk::InputContent {
    sdk::InputContent::InputText(sdk::InputTextContent {
        text: text.to_owned(),
        prompt_cache_breakpoint: None,
    })
}

fn image_part(
    attachment: &MessageAttachment,
    data_urls: &BTreeMap<String, String>,
    file_ids: &BTreeMap<String, String>,
) -> sdk::InputContent {
    let file_id = file_ids.get(&attachment.id).cloned();
    sdk::InputContent::InputImage(sdk::InputImageContent {
        image_url: file_id
            .is_none()
            .then(|| data_urls.get(&attachment.id).cloned())
            .flatten(),
        file_id,
        ..Default::default()
    })
}

fn tool_output(
    output: &ToolResultBody,
    data_urls: &BTreeMap<String, String>,
    file_ids: &BTreeMap<String, String>,
) -> sdk::FunctionCallOutput {
    match output {
        ToolResultBody::Text(text) => sdk::FunctionCallOutput::Text(text.clone()),
        ToolResultBody::Parts(parts) => sdk::FunctionCallOutput::Content(
            parts
                .iter()
                .map(|part| match part {
                    ToolResultPart::Text { text } => text_part(text),
                    ToolResultPart::Image { attachment } => {
                        image_part(attachment, data_urls, file_ids)
                    }
                })
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_effort_preserves_omission_off_and_non_sdk_values() {
        let request = ResponseRequest::new("fixture", Vec::new());
        for effort in [None, Some("none"), Some("ultra")] {
            let body = serde_json::to_value(project(&request, effort).unwrap()).unwrap();
            assert_eq!(body["reasoning"]["effort"].as_str(), effort);
            assert_eq!(body.get("reasoning").is_some(), effort.is_some());
        }
    }
}
