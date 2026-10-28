//! Explicit usage mappings for the canvas provider adapters. This module does
//! not accumulate calls or write costs; those remain runtime/finalizer duties.

use deepagent_models::chat_completions::parse_chat_usage;
use deepagent_models::usage::sanitized_provider_usage;
use deepagent_models::Usage;
use serde_json::Value;

use crate::canvas_provider_service::{CanvasProtocol, CanvasRequestKind};

pub(crate) fn parse_usage(
    protocol: CanvasProtocol,
    kind: CanvasRequestKind,
    response: &Value,
) -> (Option<Usage>, Option<Value>) {
    let key = if protocol == CanvasProtocol::Gemini {
        "usageMetadata"
    } else {
        "usage"
    };
    let Some(raw) = response.get(key).filter(|v| v.is_object()) else {
        return (None, None);
    };
    let raw = sanitized_provider_usage(raw);
    let count =
        |key: &str| -> Option<u32> { raw.get(key).and_then(Value::as_u64)?.try_into().ok() };
    let optional = |key: &str| -> Option<u32> {
        if raw.get(key).is_none() {
            Some(0)
        } else {
            count(key)
        }
    };
    let usage = (|| {
        for key in [
            "prompt_tokens_details",
            "completion_tokens_details",
            "cache_creation",
        ] {
            if raw.get(key).is_some_and(|value| !value.is_object()) {
                return None;
            }
        }
        match protocol {
            CanvasProtocol::OpenAi => {
                // The shared Chat parser owns DeepSeek cache/reasoning semantics.
                // Embeddings reports input/total only, not completion tokens.
                let prompt = count("prompt_tokens")?;
                let completion = if kind == CanvasRequestKind::Embeddings {
                    optional("completion_tokens")?
                } else {
                    count("completion_tokens")?
                };
                prompt.checked_add(completion)?;
                for key in [
                    "total_tokens",
                    "prompt_cache_hit_tokens",
                    "prompt_cache_miss_tokens",
                ] {
                    if raw.get(key).is_some() {
                        count(key)?;
                    }
                }
                for path in [
                    "/prompt_tokens_details/cached_tokens",
                    "/completion_tokens_details/reasoning_tokens",
                ] {
                    if let Some(value) = raw.pointer(path) {
                        u32::try_from(value.as_u64()?).ok()?;
                    }
                }
                let usage = parse_chat_usage(&raw);
                (usage.total_tokens == prompt.checked_add(completion)?
                    && usage.prompt_cache_hit_tokens <= prompt
                    && usage.reasoning_tokens <= completion
                    && usage
                        .prompt_cache_hit_tokens
                        .checked_add(usage.prompt_cache_miss_tokens)?
                        == prompt)
                    .then_some(usage)
            }
            CanvasProtocol::Anthropic => {
                // Messages input_tokens excludes both cache reads and writes.
                // https://platform.claude.com/docs/en/build-with-claude/prompt-caching
                let plain = count("input_tokens")?;
                let hit = optional("cache_read_input_tokens")?;
                let creation = optional("cache_creation_input_tokens")?;
                let miss = plain.checked_add(creation)?;
                let prompt = miss.checked_add(hit)?;
                let completion = count("output_tokens")?;
                Some(Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    reasoning_tokens: 0, // not separately reported by Messages
                    total_tokens: prompt.checked_add(completion)?,
                    prompt_cache_hit_tokens: hit,
                    prompt_cache_miss_tokens: miss,
                })
            }
            CanvasProtocol::Gemini => {
                // generateContent total includes tool-use input and thoughts;
                // thoughts are output, but are NOT in candidatesTokenCount.
                // https://ai.google.dev/api/generate-content#UsageMetadata
                let prompt =
                    count("promptTokenCount")?.checked_add(optional("toolUsePromptTokenCount")?)?;
                let hit = optional("cachedContentTokenCount")?;
                let reasoning = optional("thoughtsTokenCount")?;
                let completion = count("candidatesTokenCount")?.checked_add(reasoning)?;
                let total = match raw.get("totalTokenCount") {
                    Some(_) => count("totalTokenCount")?,
                    None => prompt.checked_add(completion)?,
                };
                if total != prompt.checked_add(completion)? {
                    return None;
                }
                Some(Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    reasoning_tokens: reasoning,
                    total_tokens: total,
                    prompt_cache_hit_tokens: hit,
                    prompt_cache_miss_tokens: prompt.checked_sub(hit)?,
                })
            }
        }
    })();
    (usage, Some(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_specific_buckets_are_not_interchangeable() {
        let cases = [
            (
                CanvasProtocol::OpenAi,
                json!({"usage": {
                    "prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120,
                    "prompt_cache_hit_tokens": 80, "prompt_cache_miss_tokens": 20,
                    "completion_tokens_details": {"reasoning_tokens": 12}, "secret": "canary"
                }}),
                100,
                20,
                80,
                20,
                12,
            ),
            (
                CanvasProtocol::Anthropic,
                json!({"usage": {
                    "input_tokens": 10, "cache_read_input_tokens": 80,
                    "cache_creation_input_tokens": 10, "output_tokens": 20
                }}),
                100,
                20,
                80,
                20,
                0,
            ),
            (
                CanvasProtocol::Gemini,
                json!({"usageMetadata": {
                    "promptTokenCount": 90, "toolUsePromptTokenCount": 10,
                    "cachedContentTokenCount": 80, "candidatesTokenCount": 8,
                    "thoughtsTokenCount": 12, "totalTokenCount": 120
                }}),
                100,
                20,
                80,
                20,
                12,
            ),
        ];
        for (protocol, response, prompt, completion, hit, miss, reasoning) in cases {
            let (usage, raw) = parse_usage(protocol, CanvasRequestKind::Chat, &response);
            assert_eq!(
                usage,
                Some(Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    total_tokens: 120,
                    prompt_cache_hit_tokens: hit,
                    prompt_cache_miss_tokens: miss,
                    reasoning_tokens: reasoning
                })
            );
            assert!(!raw.unwrap().to_string().contains("canary"));
        }
    }

    #[test]
    fn absent_usage_is_unknown_and_embedding_input_is_not_output() {
        assert_eq!(
            parse_usage(CanvasProtocol::OpenAi, CanvasRequestKind::Chat, &json!({})),
            (None, None)
        );
        let (usage, _) = parse_usage(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Embeddings,
            &json!({"usage": {"prompt_tokens": 11, "total_tokens": 11}}),
        );
        let usage = usage.unwrap();
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (11, 0, 11)
        );
        let (usage, raw) = parse_usage(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Chat,
            &json!({"usage": {"prompt_tokens": 100}}),
        );
        assert!(usage.is_none(), "missing output is not a measured zero");
        assert_eq!(raw, Some(json!({"prompt_tokens": 100})));
    }

    #[test]
    fn invalid_or_inconsistent_usage_is_not_a_measured_zero() {
        for usage in [
            json!({"prompt_tokens": 100, "completion_tokens": "secret-canary"}),
            json!({"prompt_tokens": u64::MAX, "completion_tokens": 1}),
            json!({"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 119}),
            json!({"prompt_tokens": 100, "completion_tokens": 20,
                "completion_tokens_details": {"reasoning_tokens": 21}}),
            json!({"prompt_tokens": 100, "completion_tokens": 20,
                "prompt_cache_hit_tokens": 101}),
            json!({"prompt_tokens": 100, "completion_tokens": 20,
                "prompt_tokens_details": "secret-canary"}),
        ] {
            let (canonical, raw) = parse_usage(
                CanvasProtocol::OpenAi,
                CanvasRequestKind::Chat,
                &json!({"usage": usage}),
            );
            assert!(canonical.is_none());
            assert!(!raw.unwrap().to_string().contains("secret-canary"));
        }
    }
}
