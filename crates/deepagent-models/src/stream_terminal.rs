//! One provider-envelope boundary shared by all Responses terminal events.
//! Contract: https://api-docs.deepseek.com/zh-cn/api/create-response/
use deepagent_core::error::{CoreError, Result};
use serde_json::Value;

use crate::chat::FinishReason;

/// A missing cause is not evidence of exhausted output tokens. Call after
/// envelope validation and usage observation, before accepting the terminal.
pub(super) fn incomplete_reason(event: &Value) -> Result<FinishReason> {
    match event["response"]["incomplete_details"]["reason"].as_str() {
        Some("max_output_tokens") => Ok(FinishReason::Length),
        Some("content_filter") => Ok(FinishReason::ContentFilter),
        _ => Err(CoreError::provider(
            None,
            Some("incomplete_reason_unknown".into()),
            "Responses incomplete response has no supported reason; refusing output-limit recovery",
        )),
    }
}

pub(super) fn validate(event: &Value, kind: &str, status: &str) -> Result<()> {
    let invalid = |field: &str, expected: &str| {
        // Never include received values: response ids, models and extensions
        // are untrusted provider data and may contain private text.
        CoreError::provider(
            None,
            Some("malformed_response_terminal".into()),
            format!("{kind}: response{field} must be {expected}"),
        )
    };
    let response = event
        .get("response")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("", "an object"))?;
    for field in ["id", "model"] {
        if !response
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(invalid(&format!(".{field}"), "a nonblank string"));
        }
    }
    if response.get("object").and_then(Value::as_str) != Some("response") {
        return Err(invalid(".object", "the string response"));
    }
    if !response
        .get("created_at")
        .is_some_and(|value| value.is_i64() || value.is_u64())
    {
        return Err(invalid(".created_at", "an integer timestamp"));
    }
    if response.get("status").and_then(Value::as_str) != Some(status) {
        return Err(invalid(".status", "the matching terminal status"));
    }
    if !response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_object))
    {
        return Err(invalid(".output", "an array of objects"));
    }
    for field in ["usage", "error", "incomplete_details"] {
        if response
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_object())
        {
            return Err(invalid(&format!(".{field}"), "an object or null"));
        }
    }
    if let Some(reason) = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
    {
        if !matches!(
            reason.as_str(),
            Some("max_output_tokens" | "content_filter")
        ) {
            return Err(invalid(
                ".incomplete_details.reason",
                "max_output_tokens or content_filter",
            ));
        }
    }
    Ok(())
}
