//! Session auto-capture detection (Requirement 9, 方案 A).
//!
//! After a chat run completes, we want to capture *worthwhile* experience —
//! specifically the "hit a wall → investigated → solved it" arc — as a pending
//! knowledge draft. This module is the **pure, offline-testable** half: given a
//! session's event sequence it decides whether the run is worth summarizing and
//! distills a compact digest for the (separate, app-core) summarization call.
//!
//! Deliberately conservative (Property 11): we only flag a run when at least one
//! tool call FAILED and the task nonetheless reached completion — i.e. a real
//! recovery happened. Trivial runs with no failures never produce an auto-capture
//! signal, so the knowledge base is not polluted with noise.

use deepagent_core::event::{Event, EventPayload};
use deepagent_core::message::Role;
use deepagent_core::task::TaskState;

/// The maximum number of characters in the distilled transcript handed to the
/// summarization model (keeps the prompt small / cheap).
const MAX_DIGEST_CHARS: usize = 4000;

/// The outcome of scanning a run for a recovery arc.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoverySignal {
    /// At least one tool call completed with `ok = false`.
    pub had_failure: bool,
    /// The task reached [`TaskState::Completed`].
    pub completed: bool,
    /// Names of tools that failed at least once (deduped, in first-seen order).
    pub failed_tools: Vec<String>,
    /// The first user message (the task goal).
    pub user_goal: String,
    /// The final assistant message text.
    pub final_answer: String,
    /// A compact, length-bounded digest for the summarization model.
    pub transcript_digest: String,
}

/// Whether a run is worth auto-capturing: a genuine recovery (a failure that was
/// ultimately overcome). This is the single gate (Property 11).
pub fn is_worth_capturing(sig: &RecoverySignal) -> bool {
    sig.had_failure && sig.completed && !sig.user_goal.trim().is_empty()
}

/// Scan a run's events for the recovery arc and build a [`RecoverySignal`].
pub fn detect_recovery(events: &[Event]) -> RecoverySignal {
    let mut sig = RecoverySignal::default();

    // Map tool call_id -> tool name (from the requests) so we can name failures.
    let mut call_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut failed_lines: Vec<String> = Vec::new();

    for ev in events {
        match &ev.payload {
            EventPayload::ToolCallRequested { call } => {
                call_names.insert(call.id.clone(), call.name.clone());
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } if !*ok => {
                sig.had_failure = true;
                let name = call_names
                    .get(call_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                if !sig.failed_tools.contains(&name) {
                    sig.failed_tools.push(name.clone());
                }
                let detail = summarize_output(output);
                failed_lines.push(format!("- {name} failed: {detail}"));
            }
            EventPayload::TaskStateChanged { to, .. } if *to == TaskState::Completed => {
                sig.completed = true;
            }
            EventPayload::MessageAppended { message } => match message.role {
                Role::User if sig.user_goal.is_empty() && !message.content.trim().is_empty() => {
                    sig.user_goal = message.content.trim().to_string();
                }
                Role::Assistant if !message.content.trim().is_empty() => {
                    sig.final_answer = message.content.trim().to_string();
                }
                _ => {}
            },
            _ => {}
        }
    }

    sig.transcript_digest = build_digest(&sig, &failed_lines);
    sig
}

/// Build the compact digest fed to the summarization model.
fn build_digest(sig: &RecoverySignal, failed_lines: &[String]) -> String {
    let mut out = String::new();
    out.push_str("## User goal\n");
    out.push_str(sig.user_goal.trim());
    out.push_str("\n\n## Failures encountered\n");
    if failed_lines.is_empty() {
        out.push_str("(none)\n");
    } else {
        for line in failed_lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\n## Final resolution\n");
    out.push_str(sig.final_answer.trim());

    truncate_chars(&out, MAX_DIGEST_CHARS)
}

/// Render a short, single-line summary of a tool's error output.
fn summarize_output(output: &serde_json::Value) -> String {
    // Prefer an explicit error/message field; else stringify the value.
    let raw = output
        .get("error")
        .or_else(|| output.get("message"))
        .or_else(|| output.get("recovery_hint"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| output.to_string());
    let one_line = raw.replace('\n', " ");
    truncate_chars(one_line.trim(), 200)
}

/// Truncate to at most `max` characters (char-aware, not byte-aware).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::event::{Event, EventPayload};
    use deepagent_core::id::{EventId, SessionId, TaskId};
    use deepagent_core::message::{Message, ToolCall};

    fn ev(seq: u64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence: seq,
            timestamp: Timestamp::from_millis(seq as i64),
            payload,
        }
    }

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    #[test]
    fn detects_failure_then_completion() {
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("get me today's weather in Changsha"),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallRequested {
                    call: tool_call("c1", "web_search"),
                },
            ),
            ev(
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: false,
                    output: serde_json::json!({"error": "network unreachable"}),
                    duration_ms: 5,
                },
            ),
            ev(
                3,
                EventPayload::ToolCallRequested {
                    call: tool_call("c2", "web_search"),
                },
            ),
            ev(
                4,
                EventPayload::ToolCallCompleted {
                    call_id: "c2".into(),
                    ok: true,
                    output: serde_json::json!({"results": []}),
                    duration_ms: 7,
                },
            ),
            ev(
                5,
                EventPayload::MessageAppended {
                    message: Message::assistant("It is 18°C and clear in Changsha."),
                },
            ),
            ev(
                6,
                EventPayload::TaskStateChanged {
                    task_id: TaskId::new(),
                    from: TaskState::Running,
                    to: TaskState::Completed,
                },
            ),
        ];
        let sig = detect_recovery(&events);
        assert!(sig.had_failure);
        assert!(sig.completed);
        assert!(is_worth_capturing(&sig));
        assert_eq!(sig.failed_tools, vec!["web_search".to_string()]);
        assert!(sig.user_goal.contains("Changsha"));
        assert!(sig.final_answer.contains("18°C"));
        assert!(sig.transcript_digest.contains("network unreachable"));
    }

    #[test]
    fn trivial_run_without_failure_is_not_worth() {
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("say hi"),
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant("hi"),
                },
            ),
            ev(
                2,
                EventPayload::TaskStateChanged {
                    task_id: TaskId::new(),
                    from: TaskState::Running,
                    to: TaskState::Completed,
                },
            ),
        ];
        let sig = detect_recovery(&events);
        assert!(!sig.had_failure);
        assert!(!is_worth_capturing(&sig));
    }

    #[test]
    fn failure_without_completion_is_not_worth() {
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("do the thing"),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallRequested {
                    call: tool_call("c1", "bash"),
                },
            ),
            ev(
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: false,
                    output: serde_json::json!({"error": "boom"}),
                    duration_ms: 1,
                },
            ),
            // No completion event.
        ];
        let sig = detect_recovery(&events);
        assert!(sig.had_failure);
        assert!(!sig.completed);
        assert!(!is_worth_capturing(&sig));
    }

    #[test]
    fn empty_events_are_safe() {
        let sig = detect_recovery(&[]);
        assert!(!is_worth_capturing(&sig));
        assert!(sig.failed_tools.is_empty());
    }
}
