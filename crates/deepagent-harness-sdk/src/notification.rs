//! Metadata on the existing bounded notification bus, not another event store.
use deepagent_harness_protocol::HarnessEvent;
use serde::{Deserialize, Serialize};

/// One validated `harness/event` with its outer connection-local wire sequence.
/// `event_sequence` is never a `thread/read` durable cursor or a provider sequence.
/// No automatic ACK/replay occurs: acknowledge only after consuming/reconciling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessNotification {
    /// Original typed business payload; serialization retains its existing fields.
    #[serde(flatten)]
    pub event: HarnessEvent,
    /// Sequence exactly as received, absent for legacy/unsequenced producers.
    #[serde(
        rename = "eventSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub event_sequence: Option<u64>,
    /// Local reader observation before subscriber scope filtering. This is not
    /// proof of persistence, a subscriber ACK or lossless receipt before subscription.
    #[serde(rename = "sequenceStatus", default)]
    pub sequence_status: SequenceStatus,
}

/// Connection-wide sequence progression among validated notifications only.
/// Missing sequence resets the baseline; no continuity can be proved across it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SequenceStatus {
    /// No sequence supplied; never synthesize an ACK cursor from a count.
    #[default]
    Unavailable,
    /// First numbered notification observed (not necessarily sequence 1).
    First,
    /// Immediately follows the previously observed high-water sequence.
    Continuous,
    /// Forward discontinuity; reconcile durable state before advancing ACK.
    Gap {
        /// Previous accepted high-water value on this physical connection.
        previous: u64,
        /// Number of missing wire sequence values, not missing durable rows.
        missing: u64,
    },
    /// Duplicate or descending value. Delivery order is preserved, never sorted.
    NonMonotonic {
        /// High-water value is not lowered by a duplicate/descending notification.
        previous: u64,
    },
}

impl From<HarnessEvent> for HarnessNotification {
    fn from(event: HarnessEvent) -> Self {
        Self {
            event,
            event_sequence: None,
            sequence_status: SequenceStatus::Unavailable,
        }
    }
}

#[derive(Default)]
pub(crate) struct SequenceTracker(Option<u64>);

impl SequenceTracker {
    pub fn observe(&mut self, sequence: Option<u64>) -> SequenceStatus {
        let Some(received) = sequence else {
            self.0 = None;
            return SequenceStatus::Unavailable;
        };
        let status = match self.0 {
            None => SequenceStatus::First,
            Some(previous) if received <= previous => {
                return SequenceStatus::NonMonotonic { previous }
            }
            Some(previous) if received - previous == 1 => SequenceStatus::Continuous,
            Some(previous) => SequenceStatus::Gap {
                previous,
                missing: received - previous - 1,
            },
        };
        self.0 = Some(received);
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn h205_old_payloads_remain_readable_and_notifications_roundtrip_full_u64() {
        let old = json!({"type":"thread.started","threadId":"synthetic","protocolVersion":1});
        let mut notification: HarnessNotification = serde_json::from_value(old).unwrap();
        assert_eq!(notification.event_sequence, None);
        assert_eq!(notification.sequence_status, SequenceStatus::Unavailable);
        notification.event_sequence = Some(u64::MAX);
        notification.sequence_status = SequenceStatus::Gap {
            previous: 7,
            missing: u64::MAX - 8,
        };
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["eventSequence"].as_u64(), Some(u64::MAX));
        assert_eq!(
            serde_json::from_value::<HarnessNotification>(value.clone()).unwrap(),
            notification
        );
        // Legacy business DTO readers ignore the additive diagnostic fields.
        assert_eq!(
            serde_json::from_value::<HarnessEvent>(value).unwrap(),
            notification.event
        );
    }

    #[test]
    fn h205_tracker_is_per_connection_and_missing_metadata_breaks_continuity_proof() {
        let mut first = SequenceTracker::default();
        assert_eq!(first.observe(Some(u64::MAX)), SequenceStatus::First);
        assert_eq!(
            first.observe(Some(0)),
            SequenceStatus::NonMonotonic { previous: u64::MAX }
        );
        assert_eq!(first.observe(None), SequenceStatus::Unavailable);
        assert_eq!(first.observe(Some(0)), SequenceStatus::First);
        assert_eq!(first.observe(Some(1)), SequenceStatus::Continuous);
        let mut second = SequenceTracker::default();
        assert_eq!(second.observe(Some(9)), SequenceStatus::First);
    }
}
