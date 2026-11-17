//! Messages usage is a sequence of snapshots; omitted counters retain their value.
use deepagent_core::error::{CoreError, Result};
use serde_json::{Map, Value};

use crate::Usage;

#[derive(Debug, Default)]
pub(super) struct UsageState {
    raw: Map<String, Value>,
    usage: Option<Usage>,
}

impl UsageState {
    pub(super) fn apply(&mut self, value: Value) -> Result<Usage> {
        let Value::Object(update) = value else {
            return Err(CoreError::invalid("Messages usage must be an object"));
        };
        // Only the shared numeric allowlist reaches this state. Validate a
        // candidate before publishing so a bad snapshot cannot corrupt billing.
        let mut candidate = self.raw.clone();
        for (key, value) in update {
            if let Value::Object(nested) = value {
                for count in nested.values() {
                    counter(Some(count))?;
                }
                let target = candidate
                    .entry(key)
                    .or_insert_with(|| Value::Object(Map::new()));
                let Value::Object(target) = target else {
                    return Err(CoreError::invalid("Messages usage detail is not an object"));
                };
                target.extend(nested);
            } else {
                counter(Some(&value))?;
                candidate.insert(key, value);
            }
        }
        let input = counter(candidate.get("input_tokens"))?;
        let output = counter(candidate.get("output_tokens"))?;
        let cached = counter(candidate.get("cache_read_input_tokens"))?;
        let created = counter(candidate.get("cache_creation_input_tokens"))?;
        let reasoning = counter(
            candidate
                .get("output_tokens_details")
                .and_then(|v| v.get("thinking_tokens")),
        )?;
        if reasoning > output {
            return Err(CoreError::invalid(
                "Messages thinking tokens exceed output tokens",
            ));
        }
        let miss = checked_add(input, created)?;
        let prompt = checked_add(miss, cached)?;
        let usage = Usage {
            prompt_tokens: prompt,
            completion_tokens: output,
            reasoning_tokens: reasoning,
            total_tokens: checked_add(prompt, output)?,
            prompt_cache_hit_tokens: cached,
            prompt_cache_miss_tokens: miss,
        };
        self.raw = candidate;
        self.usage = Some(usage);
        Ok(usage)
    }

    pub(super) fn raw(&self) -> Value {
        Value::Object(self.raw.clone())
    }

    pub(super) fn into_parts(self) -> (Option<Usage>, Option<Value>) {
        (self.usage, self.usage.map(|_| Value::Object(self.raw)))
    }
}

fn counter(value: Option<&Value>) -> Result<u32> {
    match value {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                CoreError::invalid("Messages token counter must be an unsigned 32-bit integer")
            }),
    }
}

fn checked_add(left: u32, right: u32) -> Result<u32> {
    left.checked_add(right)
        .ok_or_else(|| CoreError::invalid("Messages token total exceeds the host counter range"))
}
