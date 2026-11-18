//! One bounded admission state per physical Responses request.
use deepagent_core::error::{CoreError, Result};
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct Sequence {
    last: Option<u64>,
    invalid: bool,
    progressed: bool,
}

impl Sequence {
    pub(super) fn accept(&mut self, event: &Value) -> Result<(u64, Option<u64>)> {
        if self.invalid {
            return Err(invalid_sequence(self.last, None));
        }
        let Some(current) = event.get("sequence_number").and_then(Value::as_u64) else {
            self.invalid = true;
            return Err(invalid_sequence(self.last, None));
        };
        // Creation only supplies provisional response metadata. Some compatible
        // transports emit it before forwarding the upstream creation event.
        // Once any other Responses event arrives, a restart is no longer safe.
        if event.get("type").and_then(Value::as_str) == Some("response.created") && !self.progressed
        {
            self.last = None;
        } else {
            self.progressed = true;
        }
        let gap = if let Some(last) = self.last {
            if current <= last {
                self.invalid = true;
                return Err(invalid_sequence(self.last, Some(current)));
            }
            // The official contract guarantees monotonicity, not contiguity.
            // Preserve the number of unseen positions as a diagnostic, without
            // inventing a provider replay/resume or deduplication protocol.
            Some(current - last - 1).filter(|gap| *gap > 0)
        } else {
            None
        };
        self.last = Some(current);
        Ok((current, gap))
    }

    pub(super) fn ensure_valid(&self) -> Result<()> {
        if self.invalid {
            Err(invalid_sequence(self.last, None))
        } else {
            Ok(())
        }
    }
}

fn invalid_sequence(previous: Option<u64>, received: Option<u64>) -> CoreError {
    CoreError::provider(
        None,
        Some("invalid_response_sequence".into()),
        format!("Responses sequence_number must be an unsigned integer and strictly increase within each physical request; previous={previous:?}, received={received:?}"),
    )
}
