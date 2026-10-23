//! Shared normalization for provider errors received from HTTP bodies and
//! semantic stream events.

use deepagent_core::error::CoreError;

pub(crate) fn from_json_error(
    status: Option<u16>,
    error: Option<&serde_json::Value>,
    fallback_code: Option<&str>,
    fallback_message: &str,
    retry_after_ms: Option<u64>,
    request_id: Option<String>,
) -> CoreError {
    let code = error
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            error
                .and_then(|value| value.get("type"))
                .and_then(serde_json::Value::as_str)
        })
        .or(fallback_code)
        .map(str::to_string);
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback_message);

    CoreError::provider_with_metadata(
        status,
        code,
        sanitize_message(message),
        retry_after_ms,
        request_id,
    )
}

fn sanitize_message(message: &str) -> String {
    let mut redact_next = false;
    let mut sanitized = Vec::new();
    for word in message.split_whitespace() {
        if redact_next {
            sanitized.push("[redacted]".to_string());
            redact_next = false;
        } else if word.eq_ignore_ascii_case("bearer") {
            sanitized.push(word.to_string());
            redact_next = true;
        } else {
            sanitized.push(redact_token_like_key(word));
        }
    }
    let mut sanitized = sanitized.join(" ");
    if sanitized.chars().count() > 2_000 {
        sanitized = sanitized.chars().take(2_000).collect();
        sanitized.push('…');
    }
    if sanitized.is_empty() {
        "provider request failed without an error message".to_string()
    } else {
        sanitized
    }
}

fn redact_token_like_key(word: &str) -> String {
    let Some(start) = word.find("sk-") else {
        return word.to_string();
    };
    let mut redacted = String::with_capacity(start + 4);
    redacted.push_str(&word[..start]);
    redacted.push_str("sk-…");
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_stream_errors_keep_code_and_share_redaction() {
        let error = serde_json::json!({
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "message": "Bearer secret-value sk-secret-value"
        });
        let normalized = from_json_error(
            None,
            Some(&error),
            Some("response_failed"),
            "fallback",
            None,
            None,
        );
        assert!(matches!(
            normalized,
            CoreError::Provider {
                code: Some(ref code),
                ref message,
                ..
            } if code == "context_length_exceeded"
                && !message.contains("secret-value")
        ));
    }
}
