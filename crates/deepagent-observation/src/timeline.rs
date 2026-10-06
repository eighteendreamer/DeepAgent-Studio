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
/// The timeline is the **model-facing** view (§12), not the archive: everything
/// before the last compaction boundary is dropped, because that is what the
/// agent was actually given after compaction. The boundary row itself survives,
/// and it already says where the model view starts. Nothing is deleted — the
/// append-only log and `transcript` export still carry the earlier events.
///
/// A boundary without a recorded cut point (a manual `/compact` on a build that
/// predates the field) cannot be honoured, so the whole stream stays visible
/// rather than guessing a start.
pub fn build_timeline(events: &[Event]) -> Vec<TimelineEntry> {
    let model_view_from = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                summary: Some(_),
                first_kept_sequence: Some(keep_from),
                ..
            } => Some(*keep_from),
            _ => None,
        })
        .max();
    events
        .iter()
        .filter(|event| model_view_from.map_or(true, |from| event.sequence >= from))
        .map(entry_for_event)
        .collect()
}

fn entry_for_event(event: &Event) -> TimelineEntry {
    let (kind, icon, label, detail, duration_ms) = match &event.payload {
        EventPayload::SessionStarted { title, .. } => (
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
            summary,
            first_kept_sequence,
        } => {
            // A `summary` turns this record into the boundary the model-facing
            // view starts from; without one it is only a metrics entry
            // (micro-compaction and pre-boundary builds).
            let (title, detail) = match (summary, first_kept_sequence) {
                (Some(_), Some(keep_from)) => (
                    format!("Context compacted ({strategy})"),
                    Some(format!(
                        "{tokens_before} → {tokens_after} tokens · 模型视图自事件 #{keep_from} 起"
                    )),
                ),
                (Some(_), None) => (
                    format!("Context compacted ({strategy})"),
                    Some(format!(
                        "{tokens_before} → {tokens_after} tokens · 模型视图仅保留摘要之后的内容"
                    )),
                ),
                (None, _) => (
                    format!("Context compacted ({strategy})"),
                    Some(format!("{tokens_before} → {tokens_after} tokens")),
                ),
            };
            ("compact", "🗜", title, detail, None)
        }
        EventPayload::Note { text } => (
            "note",
            "📝",
            "Note".to_string(),
            Some(truncate(text, 120)),
            None,
        ),
        EventPayload::MemoriesInjected {
            ids,
            count,
            trigger,
            ..
        } => (
            "memory",
            "🔖",
            if trigger.is_empty() {
                "Memories injected".to_string()
            } else {
                format!("Memories injected ({trigger})")
            },
            if ids.is_empty() {
                Some(format!("{count} entries"))
            } else {
                Some(truncate(&ids.join(", "), 120))
            },
            None,
        ),
        EventPayload::MemoryEntryWritten {
            id, tier, scope, ..
        } => (
            "memory",
            "✍",
            "Memory written to the vault".to_string(),
            Some(format!("{scope}/{tier}: {}", truncate(id, 80))),
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

    /// §12: the timeline is the model view. Rows the compaction dropped stop
    /// being shown, while the boundary row itself stays so the cut is visible
    /// rather than silent.
    #[test]
    fn the_timeline_starts_at_the_compaction_boundary() {
        let message = |seq: u64, text: &str| {
            event(
                seq,
                EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user(text),
                },
            )
        };
        // Real shape: the transcript grows, then compaction appends a boundary
        // that says the model view now starts at an earlier event.
        let events = [
            message(0, "dropped"),
            message(1, "dropped too"),
            message(2, "kept"),
            message(3, "kept too"),
            event(
                4,
                EventPayload::ContextCompacted {
                    tokens_before: 9_000,
                    tokens_after: 2_000,
                    strategy: "session_memory".to_string(),
                    summary: Some("- kept".to_string()),
                    first_kept_sequence: Some(2),
                },
            ),
        ];

        let shown: Vec<u64> = build_timeline(&events)
            .iter()
            .map(|entry| entry.sequence)
            .collect();
        assert_eq!(shown, vec![2, 3, 4], "pre-boundary rows must not be shown");

        // A boundary with no recorded cut point cannot be honoured, so nothing is
        // hidden rather than guessing where the model view began.
        let unlocated = [
            message(0, "before"),
            event(
                1,
                EventPayload::ContextCompacted {
                    tokens_before: 9_000,
                    tokens_after: 2_000,
                    strategy: "manual".to_string(),
                    summary: Some("- kept".to_string()),
                    first_kept_sequence: None,
                },
            ),
        ];
        assert_eq!(
            build_timeline(&unlocated).len(),
            2,
            "an unlocatable boundary must not hide history"
        );
    }

    /// Memory events must have their own rows: the generic fallback labels them
    /// by raw kind string, so a replay would show `memories_injected` instead of
    /// what the model was actually given.
    #[test]
    fn memory_events_render_as_memory_rows() {
        let timeline = build_timeline(&[
            event(
                0,
                EventPayload::MemoriesInjected {
                    ids: vec!["project:build-fix".into(), "global:cargo-j1".into()],
                    count: 2,
                    latency_ms: 41,
                    trigger: "prefetch".into(),
                },
            ),
            event(
                1,
                EventPayload::MemoryEntryWritten {
                    id: "project-new-vault-entry".into(),
                    tier: "semantic".into(),
                    scope: "project".into(),
                    importance: Some(0.8),
                },
            ),
        ]);
        assert_eq!(timeline[0].kind, "memory");
        assert!(
            timeline[0].label.contains("prefetch"),
            "the injection trigger belongs in the label: {}",
            timeline[0].label
        );
        let detail = timeline[0].detail.clone().unwrap_or_default();
        assert!(
            detail.contains("project:build-fix") && detail.contains("global:cargo-j1"),
            "the injected ids must be named: {detail}"
        );
        assert_eq!(timeline[1].kind, "memory");
        assert!(
            timeline[1]
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("project/semantic"),
            "where the entry landed belongs in the detail: {:?}",
            timeline[1].detail
        );
    }

    #[test]
    fn builds_entry_per_event_in_order() {
        let task = TaskId::new();
        let events = vec![
            event(
                0,
                EventPayload::SessionStarted {
                    title: Some("demo".into()),
                    mode: deepagent_core::session_mode::SessionMode::Normal,
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
