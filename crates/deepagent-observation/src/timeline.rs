//! The Agent Timeline (开发提示词.md §18; 开发计划.md Phase 10).
//!
//! The timeline is a replayable, time-ordered view of everything an agent did,
//! built purely by folding the append-only [`Event`] stream. Because it is a
//! deterministic projection of durable events, it is inherently *replayable*
//! (the Phase 10 "Timeline 可回放" criterion): given the same events you always
//! reconstruct the same timeline.
//!
//! Each [`TimelineEntry`] carries an icon, a human label, and optional detail —
//! ready for the Codex-style UI's timeline panel (Phase 8).

use serde::{Deserialize, Serialize};

use deepagent_core::clock::Timestamp;
use deepagent_core::event::{Event, EventPayload};

/// A single, display-ready entry on the agent timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// Sequence number from the source event (stable ordering key).
    pub sequence: u64,
    /// When it happened.
    pub timestamp: Timestamp,
    /// A short kind tag (e.g. "tool", "message", "verify").
    pub kind: String,
    /// An emoji/icon hint for the UI.
    pub icon: &'static str,
    /// One-line human label.
    pub label: String,
    /// Optional secondary detail.
    pub detail: Option<String>,
    /// Duration in milliseconds, when known (e.g. tool calls).
    pub duration_ms: Option<u64>,
}

/// Build a timeline from a session's events. Events that do not map to a
/// user-meaningful entry (internal notes can be included or filtered by the
/// caller) are still represented so the timeline is a faithful replay.
pub fn build_timeline(events: &[Event]) -> Vec<TimelineEntry> {
    events.iter().map(entry_for_event).collect()
}

fn entry_for_event(event: &Event) -> TimelineEntry {
    let (kind, icon, label, detail, duration_ms) = match &event.payload {
        EventPayload::SessionStarted { title } => (
            "session",
            "🟢",
            "Session started".to_string(),
            title.clone(),
            None,
        ),
        EventPayload::SessionEnded { reason } => (
            "session",
            "🔴",
            "Session ended".to_string(),
            reason.clone(),
            None,
        ),
        EventPayload::TaskCreated { task_id, goal } => (
            "task",
            "📋",
            format!("Task created: {goal}"),
            Some(task_id.to_string()),
            None,
        ),
        EventPayload::TaskStateChanged { from, to, .. } => {
            ("task", "🔄", format!("Task {from:?} → {to:?}"), None, None)
        }
        EventPayload::MessageAppended { message } => {
            let preview: String = message.content.chars().take(80).collect();
            (
                "message",
                "💬",
                format!("{:?} message", message.role),
                if preview.is_empty() {
                    None
                } else {
                    Some(preview)
                },
                None,
            )
        }
        EventPayload::ToolCallRequested { call } => (
            "tool",
            "🔧",
            format!("Tool requested: {}", call.name),
            Some(call.arguments.to_string()),
            None,
        ),
        EventPayload::ToolCallCompleted {
            ok,
            output,
            duration_ms,
            ..
        } => (
            "tool",
            if *ok { "✅" } else { "❌" },
            format!("Tool {}", if *ok { "completed" } else { "failed" }),
            Some(truncate(&output.to_string(), 120)),
            Some(*duration_ms),
        ),
        EventPayload::ContextCompacted {
            tokens_before,
            tokens_after,
            strategy,
        } => (
            "compact",
            "🗜",
            format!("Context compacted ({strategy})"),
            Some(format!("{tokens_before} → {tokens_after} tokens")),
            None,
        ),
        EventPayload::Note { text } => (
            "note",
            "📝",
            "Note".to_string(),
            Some(truncate(text, 120)),
            None,
        ),
        _ => ("event", "•", event.kind().to_string(), None, None),
    };

    TimelineEntry {
        sequence: event.sequence,
        timestamp: event.timestamp,
        kind: kind.to_string(),
        icon,
        label,
        detail,
        duration_ms,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::event::EventPayload;
    use deepagent_core::id::{EventId, SessionId, TaskId};
    use deepagent_core::message::Message;
    use deepagent_core::task::TaskState;

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::nil(),
            sequence: seq,
            timestamp: Timestamp::from_millis(1000 + seq as i64),
            payload,
        }
    }

    #[test]
    fn builds_entry_per_event_in_order() {
        let task = TaskId::new();
        let events = vec![
            event(
                0,
                EventPayload::SessionStarted {
                    title: Some("demo".into()),
                },
            ),
            event(
                1,
                EventPayload::TaskCreated {
                    task_id: task,
                    goal: "do it".into(),
                },
            ),
            event(
                2,
                EventPayload::TaskStateChanged {
                    task_id: task,
                    from: TaskState::Queued,
                    to: TaskState::Running,
                },
            ),
        ];
        let timeline = build_timeline(&events);
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline[0].sequence, 0);
        assert_eq!(timeline[0].icon, "🟢");
        assert_eq!(timeline[1].kind, "task");
        assert!(timeline[2].label.contains("Running"));
    }

    #[test]
    fn tool_completed_carries_duration_and_status() {
        let ok = event(
            0,
            EventPayload::ToolCallCompleted {
                call_id: "c1".into(),
                ok: true,
                output: serde_json::json!({"sum": 5}),
                duration_ms: 42,
            },
        );
        let entry = &build_timeline(&[ok])[0];
        assert_eq!(entry.icon, "✅");
        assert_eq!(entry.duration_ms, Some(42));

        let fail = event(
            0,
            EventPayload::ToolCallCompleted {
                call_id: "c2".into(),
                ok: false,
                output: serde_json::json!({"error": "boom"}),
                duration_ms: 7,
            },
        );
        assert_eq!(build_timeline(&[fail])[0].icon, "❌");
    }

    #[test]
    fn message_preview_is_truncated() {
        let long = "x".repeat(200);
        let e = event(
            0,
            EventPayload::MessageAppended {
                message: Message::user(long),
            },
        );
        let entry = &build_timeline(&[e])[0];
        assert!(entry.detail.as_ref().unwrap().chars().count() <= 80);
    }

    #[test]
    fn replay_is_deterministic() {
        let events = vec![
            event(0, EventPayload::Note { text: "a".into() }),
            event(1, EventPayload::Note { text: "b".into() }),
        ];
        assert_eq!(build_timeline(&events), build_timeline(&events));
    }
}
