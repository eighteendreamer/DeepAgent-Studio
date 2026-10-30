//! Physical-request usage observation and bounded numeric diagnostics. Never
//! retain arbitrary fields: a provider may put prompt text or secrets in usage.

use serde_json::{Map, Value};
use std::sync::Arc;

use crate::stream::DeltaObserver;
use crate::Usage;

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
        ("input_tokens_details", &["cached_tokens"][..]),
        ("output_tokens_details", &["reasoning_tokens"][..]),
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

/// Receipt for one physical provider request, before output validation.
#[derive(Debug, Clone)]
pub struct CallUsageReport {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub usage: Option<Usage>,
    pub raw_usage: Option<Value>,
}

/// One host-owned accounting/admission boundary for each physical request.
/// Admission does not reserve or predict the cost of requests already in flight.
pub trait CallUsageSink: Send + Sync {
    fn record(&self, report: CallUsageReport);

    /// Called immediately before a new provider request, including retries.
    fn before_call(&self) -> deepagent_core::error::Result<()> {
        self.checkpoint()
    }

    /// Surface accounting failure without rejecting an already-paid answer
    /// just because that answer itself reached a spending limit.
    fn checkpoint(&self) -> deepagent_core::error::Result<()> {
        Ok(())
    }
}

/// Retain the latest SSE usage snapshot, then settle exactly once, including
/// when a provider error or cancellation drops the waiting future. The request
/// route is captured before dispatch, never read from a later fallback model.
pub struct CallUsageObserver {
    sink: Arc<dyn CallUsageSink>,
    model_id: String,
    usage: Option<Usage>,
    raw: Option<Value>,
    record_unreported: bool,
}

impl CallUsageObserver {
    pub fn new(sink: Arc<dyn CallUsageSink>, model_id: String, record_unreported: bool) -> Self {
        Self {
            sink,
            model_id,
            usage: None,
            raw: None,
            record_unreported,
        }
    }

    pub fn completed(&mut self, usage: Option<Usage>, raw: Option<&Value>) {
        self.record_unreported = true;
        if usage.is_some() {
            self.usage = usage;
        }
        if let Some(raw) = raw {
            self.raw = Some(raw.clone());
        }
    }

    /// Latest reported snapshot, not a sum of repeated SSE usage events.
    pub fn measured_usage(&self) -> Option<Usage> {
        self.usage
    }
}

impl DeltaObserver for CallUsageObserver {
    fn on_usage(&mut self, usage: Usage, raw: &Value) {
        self.usage = Some(usage);
        self.raw = Some(raw.clone());
    }

    fn on_event(&mut self, event: crate::stream::ModelStreamEvent) {
        if let crate::stream::ModelStreamEvent::Usage { usage } = event {
            self.usage = Some(usage);
        }
    }
}

impl Drop for CallUsageObserver {
    fn drop(&mut self) {
        // No fabricated charge or unknown-cost row for an unmeasured failed
        // request. A successful unmeasured response is explicitly incomplete.
        if self.usage.is_some() || self.record_unreported {
            self.sink.record(CallUsageReport {
                provider_id: None,
                model_id: self.model_id.clone(),
                usage: self.usage,
                raw_usage: self.raw.take(),
            });
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

    #[test]
    fn responses_nested_cache_and_reasoning_counters_remain_numeric_only() {
        assert_eq!(
            sanitized_provider_usage(&json!({
                "input_tokens":1000,"output_tokens":100,
                "input_tokens_details":{"cached_tokens":800,"prompt":"secret-canary"},
                "output_tokens_details":{"reasoning_tokens":20,"text":"secret-canary"},
                "key":"secret-canary",
            })),
            json!({"input_tokens":1000,"output_tokens":100,
            "input_tokens_details":{"cached_tokens":800},
            "output_tokens_details":{"reasoning_tokens":20}})
        );
    }
}
