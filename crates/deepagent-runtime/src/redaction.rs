//! Recursive secret redaction for durably persisted runtime data (Phase F).
//!
//! Two persistence paths carry runtime events: the diagnostics DB
//! (`runtime-logs.db`, redacted aggressively in `deepagent-app-core`
//! including prompt bodies) and the product `run_events` table consumed by
//! UI replay. Replay NEEDS message text, so this layer scrubs ONLY secret
//! material — bearer tokens, `sk-` keys, `password=`/`secret=` literals and
//! secret-named JSON keys — while leaving prose intact.

/// Recursively scrub secrets from a JSON value. Object keys that look like
/// secrets have their values replaced wholesale; every string leaf is run
/// through the literal scrubber.
pub fn scrub_secrets_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let scrubbed = if is_secret_key(&key) {
                        serde_json::Value::String("<redacted>".into())
                    } else {
                        scrub_secrets_value(value)
                    };
                    (key, scrubbed)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(scrub_secrets_value).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(scrub_secret_literals(&text)),
        other => other,
    }
}

/// Scrub inline secret literals from free text without touching anything
/// else (no truncation — replay text must stay complete).
pub fn scrub_secret_literals(input: &str) -> String {
    if !looks_suspicious(input) {
        return input.to_string();
    }
    let mut out: Vec<String> = Vec::new();
    let mut redact_next = false;
    for token in input.split_whitespace() {
        if redact_next {
            out.push("<redacted>".to_string());
            redact_next = token.eq_ignore_ascii_case("bearer");
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if lower == "bearer" || lower == "authorization:" {
            out.push(token.to_string());
            redact_next = true;
            continue;
        }
        if (token.starts_with("sk-") && token.len() > 7)
            || lower.contains("api_key=")
            || lower.contains("apikey=")
            || lower.contains("password=")
            || lower.contains("secret=")
        {
            out.push("<redacted>".to_string());
        } else {
            out.push(token.to_string());
        }
    }
    out.join(" ")
}

/// Cheap pre-filter so the common case (ordinary prose / code) avoids the
/// token-splitting rebuild, which would also normalize whitespace.
fn looks_suspicious(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("sk-")
        || lower.contains("bearer")
        || lower.contains("authorization:")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("password=")
        || lower.contains("secret=")
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("api_key")
        || key.contains("apikey")
        || key.contains("authorization")
        || key.contains("password")
        || key.contains("secret")
        || key == "token"
        || key.ends_with("_token")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_secret_keys_and_literals_recursively() {
        let scrubbed = scrub_secrets_value(serde_json::json!({
            "api_key": "sk-live-1234567890",
            "nested": { "Authorization": "Bearer abc", "note": "call with Bearer abcdef now" },
            "list": ["password=hunter2", "plain text"],
            "content": "the quick brown fox",
        }));
        assert_eq!(scrubbed["api_key"], "<redacted>");
        assert_eq!(scrubbed["nested"]["Authorization"], "<redacted>");
        assert!(scrubbed["nested"]["note"]
            .as_str()
            .unwrap()
            .contains("Bearer <redacted>"));
        assert_eq!(scrubbed["list"][0], "<redacted>");
        assert_eq!(scrubbed["list"][1], "plain text");
        // Prose stays byte-identical (no whitespace normalization).
        assert_eq!(scrubbed["content"], "the quick brown fox");
    }

    #[test]
    fn ordinary_text_passes_through_unchanged() {
        let text = "多行\n  缩进 和 空格   保持不变";
        assert_eq!(scrub_secret_literals(text), text);
    }
}
