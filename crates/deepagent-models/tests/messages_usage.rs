//! Numeric Messages extensions must survive the shared diagnostic boundary.
use deepagent_models::usage::sanitized_provider_usage;
use serde_json::json;

#[test]
fn messages_thinking_usage_is_preserved_without_provider_text() {
    assert_eq!(
        sanitized_provider_usage(&json!({
            "input_tokens": 11,
            "output_tokens": 7,
            "cache_read_input_tokens": 13,
            "cache_creation_input_tokens": 5,
            "output_tokens_details": {
                "thinking_tokens": 3,
                "reasoning_tokens": 2,
                "prompt": "private-canary"
            },
            "api_key": "private-canary"
        })),
        json!({
            "input_tokens": 11,
            "output_tokens": 7,
            "cache_read_input_tokens": 13,
            "cache_creation_input_tokens": 5,
            "output_tokens_details": {"thinking_tokens": 3, "reasoning_tokens": 2}
        })
    );
}
