//! Runtime statistics aggregated from the event stream (开发计划.md Phase 10:
//! Runtime Metrics / Token Metrics / Cache Metrics).
//!
//! [`SessionStats`] is a pure fold over events — a replayable analytics view
//! that complements the live [`deepagent_tracing::metrics::Metrics`] counters.
//! Where the live registry tracks an in-flight process, this reconstructs the
//! same numbers from durable history (for past sessions, the UI, or audits).

use serde::{Deserialize, Serialize};

use deepagent_core::event::{Event, EventPayload};

/// Aggregated statistics for a session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStats {
    /// Total events in the session.
    pub event_count: u64,
    /// Conversation messages.
    pub messages: u64,
    /// Tool calls requested.
    pub tool_calls: u64,
    /// Tool calls that succeeded.
    pub tool_successes: u64,
    /// Tool calls that failed.
    pub tool_failures: u64,
    /// Total tool execution time (ms).
    pub total_tool_ms: u64,
    /// Context compaction passes.
    pub compactions: u64,
    /// Tokens reclaimed by compaction (sum of before-after).
    pub tokens_saved: u64,
    /// Tasks created.
    pub tasks_created: u64,
    /// Span of the session in milliseconds (last - first event timestamp).
    pub duration_ms: i64,
}

impl SessionStats {
    /// Fold a session's events into aggregate statistics.
    pub fn from_events(events: &[Event]) -> Self {
        let mut s = SessionStats {
            event_count: events.len() as u64,
            ..Default::default()
        };
        if let (Some(first), Some(last)) = (events.first(), events.last()) {
            s.duration_ms = last.timestamp.millis_since(first.timestamp);
        }
        for e in events {
            match &e.payload {
                EventPayload::MessageAppended { .. } => s.messages += 1,
                EventPayload::ToolCallRequested { .. } => s.tool_calls += 1,
                EventPayload::ToolCallCompleted {
                    ok, duration_ms, ..
                } => {
                    if *ok {
                        s.tool_successes += 1;
                    } else {
                        s.tool_failures += 1;
                    }
                    s.total_tool_ms += *duration_ms;
                }
                EventPayload::ContextCompacted {
                    tokens_before,
                    tokens_after,
                    ..
                } => {
                    s.compactions += 1;
                    s.tokens_saved += tokens_before.saturating_sub(*tokens_after);
                }
                EventPayload::TaskCreated { .. } => s.tasks_created += 1,
                _ => {}
            }
        }
        s
    }

    /// Tool success rate in `[0,1]`, or `None` if no tools ran.
    pub fn tool_success_rate(&self) -> Option<f64> {
        let total = self.tool_successes + self.tool_failures;
        if total == 0 {
            None
        } else {
            Some(self.tool_successes as f64 / total as f64)
        }
    }

    /// Average tool latency in milliseconds, or `None` if no tools ran.
    pub fn avg_tool_ms(&self) -> Option<f64> {
        let total = self.tool_successes + self.tool_failures;
        if total == 0 {
            None
        } else {
            Some(self.total_tool_ms as f64 / total as f64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::id::{EventId, SessionId};
    use deepagent_core::message::Message;

    fn event(seq: u64, ts: i64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::nil(),
            sequence: seq,
            timestamp: Timestamp::from_millis(ts),
            payload,
        }
    }

    #[test]
    fn aggregates_tool_and_message_counts() {
        let events = vec![
            event(0, 1000, EventPayload::SessionStarted { title: None }),
            event(
                1,
                1100,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            ),
            event(
                2,
                1200,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: true,
                    output: serde_json::json!({}),
                    duration_ms: 30,
                },
            ),
            event(
                3,
                1300,
                EventPayload::ToolCallCompleted {
                    call_id: "c2".into(),
                    ok: false,
                    output: serde_json::json!({}),
                    duration_ms: 10,
                },
            ),
        ];
        let s = SessionStats::from_events(&events);
        assert_eq!(s.event_count, 4);
        assert_eq!(s.messages, 1);
        assert_eq!(s.tool_successes, 1);
        assert_eq!(s.tool_failures, 1);
        assert_eq!(s.total_tool_ms, 40);
        assert_eq!(s.duration_ms, 300);
        assert_eq!(s.tool_success_rate(), Some(0.5));
        assert_eq!(s.avg_tool_ms(), Some(20.0));
    }

    #[test]
    fn compaction_tokens_saved() {
        let events = vec![event(
            0,
            1000,
            EventPayload::ContextCompacted {
                tokens_before: 1000,
                tokens_after: 600,
                strategy: "summary".into(),
            },
        )];
        let s = SessionStats::from_events(&events);
        assert_eq!(s.compactions, 1);
        assert_eq!(s.tokens_saved, 400);
    }

    #[test]
    fn empty_session_has_no_rates() {
        let s = SessionStats::from_events(&[]);
        assert_eq!(s.event_count, 0);
        assert_eq!(s.tool_success_rate(), None);
        assert_eq!(s.avg_tool_ms(), None);
    }
}
