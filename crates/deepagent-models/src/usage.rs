//! Bounded, numeric-only provider usage diagnostics. Never retain arbitrary
//! response fields: a provider may put prompt text or secrets inside `usage`.

use serde_json::{Map, Value};

/// Keep known token counters and their reported presence, without copying
/// provider-controlled strings, unknown keys or unbounded nested objects.
pub fn sanitized_provider_usage(value: &Value) -> Value {
    const COUNTERS: &[&str] = &[
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "prompt_cache_hit_tokens",
        "prompt_cache_miss_tokens",
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "promptTokenCount",
        "cachedContentTokenCount",
        "candidatesTokenCount",
        "toolUsePromptTokenCount",
        "thoughtsTokenCount",
        "totalTokenCount",
    ];
    let mut out = Map::new();
    copy_counters(value, &mut out, COUNTERS);
    for (key, counters) in [
        ("prompt_tokens_details", &["cached_tokens"][..]),
        ("completion_tokens_details", &["reasoning_tokens"][..]),
        (
            "cache_creation",
            &["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"][..],
        ),
    ] {
        if let Some(details) = value.get(key) {
            if !details.is_object() {
                out.insert(key.into(), Value::Null);
                continue;
            }
            let mut nested = Map::new();
            copy_counters(details, &mut nested, counters);
            out.insert(key.into(), Value::Object(nested));
        }
    }
    Value::Object(out)
}

fn copy_counters(value: &Value, out: &mut Map<String, Value>, counters: &[&str]) {
    for key in counters {
        if let Some(value) = value.get(*key) {
            // Null marks a reported-but-invalid counter without retaining its
            // provider-controlled string. Absence remains distinguishable.
            out.insert(
                (*key).into(),
                value.as_u64().map(Value::from).unwrap_or(Value::Null),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retains_numeric_presence_without_provider_text_or_unknown_fields() {
        assert_eq!(
            sanitized_provider_usage(&json!({
                "prompt_tokens": 0,
                "completion_tokens": "secret-canary",
                "api_key": "secret-canary",
                "prompt_tokens_details": {"cached_tokens": 0, "prompt": "secret-canary"},
                "unknown": {"tokens": 123}
            })),
            json!({"prompt_tokens": 0, "completion_tokens": null, "prompt_tokens_details": {"cached_tokens": 0}})
        );
    }
}
