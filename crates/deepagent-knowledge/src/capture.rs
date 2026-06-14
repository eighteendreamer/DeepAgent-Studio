//! Session auto-capture detection (Requirement 9, 方案 A).
//!
//! After a chat run completes, we want to capture *worthwhile* experience as a
//! knowledge entry. Two complementary paths live here, both **pure and
//! offline-testable** (the model call lives in `app-core`):
//!
//! - **Recovery arc** ([`detect_recovery`] + [`is_worth_capturing`]): the
//!   "hit a wall → investigated → solved it" pattern. A strong signal — the
//!   summarization call has a deterministic fallback so the operational lesson
//!   is preserved even if the model declines.
//! - **Session digest** ([`detect_session_digest`] +
//!   [`is_session_substantive`]): a generic "did anything useful happen here?"
//!   path that runs on **every** completed, non-trivial session. The local
//!   substantiveness gate filters out single-turn chatter before any model
//!   call; the model's `worth_saving` reply is then the final quality gate, so
//!   nothing is saved unless the model affirmatively says it is reusable.
//!
//! Together they keep auto-capture ambitious (every session is considered)
//! without polluting the knowledge base (only meaningful runs survive).

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

// ---------------------------------------------------------------------------
// Session digest: the generic "did anything useful happen?" path.
// ---------------------------------------------------------------------------

/// Total user+assistant character count below which a session is treated as
/// trivial chatter and never sent to the summarizer (avoids API spend on
/// "hi"/"thanks" turns). Sessions with at least one tool call bypass this gate.
pub const SUBSTANTIVE_CHAR_THRESHOLD: usize = 200;

/// A compact, length-bounded summary of any completed session. Independent of
/// whether failures happened — used by the generic auto-capture path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionDigest {
    /// Task reached [`TaskState::Completed`].
    pub completed: bool,
    /// The session contains at least one tool call (any outcome).
    pub had_tool_call: bool,
    /// Tools used (deduped, in first-seen order).
    pub used_tools: Vec<String>,
    /// First user message — the task goal.
    pub user_goal: String,
    /// Last non-empty assistant message — the final answer.
    pub final_answer: String,
    /// Total characters across user + assistant content (proxy for "real chat").
    pub content_chars: usize,
    /// A bounded transcript digest the summarizer reads.
    pub transcript_digest: String,
}

/// Whether a session is substantive enough to spend a model call on. Local,
/// cheap gate that runs before any network I/O.
pub fn is_session_substantive(d: &SessionDigest) -> bool {
    if !d.completed || d.user_goal.trim().is_empty() {
        return false;
    }
    // Either real action happened (tool call) or the conversation has real
    // content (not a one-line greeting).
    d.had_tool_call || d.content_chars >= SUBSTANTIVE_CHAR_THRESHOLD
}

/// Build a [`SessionDigest`] over a session's event stream.
pub fn detect_session_digest(events: &[Event]) -> SessionDigest {
    let mut d = SessionDigest::default();
    let mut tools_seen: Vec<String> = Vec::new();
    let mut transcript_lines: Vec<String> = Vec::new();
    let mut call_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for ev in events {
        match &ev.payload {
            EventPayload::ToolCallRequested { call } => {
                d.had_tool_call = true;
                if !tools_seen.contains(&call.name) {
                    tools_seen.push(call.name.clone());
                }
                call_names.insert(call.id.clone(), call.name.clone());
                transcript_lines.push(format!("→ tool {}", call.name));
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } => {
                let name = call_names
                    .get(call_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let detail = summarize_output(output);
                let status = if *ok { "ok" } else { "error" };
                transcript_lines.push(format!("← {name} {status}: {detail}"));
            }
            EventPayload::TaskStateChanged { to, .. } if *to == TaskState::Completed => {
                d.completed = true;
            }
            EventPayload::MessageAppended { message } => match message.role {
                Role::User => {
                    let s = message.content.trim();
                    if !s.is_empty() {
                        if d.user_goal.is_empty() {
                            d.user_goal = s.to_string();
                        }
                        d.content_chars += s.chars().count();
                        transcript_lines.push(format!("user: {}", truncate_chars(s, 400)));
                    }
                }
                Role::Assistant => {
                    let s = message.content.trim();
                    if !s.is_empty() {
                        d.final_answer = s.to_string();
                        d.content_chars += s.chars().count();
                        transcript_lines.push(format!("assistant: {}", truncate_chars(s, 400)));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    d.used_tools = tools_seen;
    d.transcript_digest = build_session_digest_text(&d, &transcript_lines);
    d
}

fn build_session_digest_text(d: &SessionDigest, lines: &[String]) -> String {
    let mut out = String::new();
    out.push_str("## User goal\n");
    out.push_str(d.user_goal.trim());
    out.push_str("\n\n## Tools used\n");
    if d.used_tools.is_empty() {
        out.push_str("(none)\n");
    } else {
        out.push_str(&d.used_tools.join(", "));
        out.push('\n');
    }
    out.push_str("\n## Transcript (abridged)\n");
    // Keep the digest bounded: at most 40 transcript lines.
    for line in lines.iter().take(40) {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n## Final assistant answer\n");
    out.push_str(d.final_answer.trim());
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

    // ---- session-digest path ------------------------------------------------

    #[test]
    fn substantive_when_completed_with_tool_call() {
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("read main.rs"),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallRequested {
                    call: tool_call("c1", "read_file"),
                },
            ),
            ev(
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: true,
                    output: serde_json::json!({"bytes": 1234}),
                    duration_ms: 4,
                },
            ),
            ev(
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant("Read it. Looks like a Rust binary."),
                },
            ),
            ev(
                4,
                EventPayload::TaskStateChanged {
                    task_id: TaskId::new(),
                    from: TaskState::Running,
                    to: TaskState::Completed,
                },
            ),
        ];
        let d = detect_session_digest(&events);
        assert!(d.completed);
        assert!(d.had_tool_call);
        assert_eq!(d.used_tools, vec!["read_file".to_string()]);
        assert!(is_session_substantive(&d));
        assert!(d.transcript_digest.contains("Read it"));
    }

    #[test]
    fn trivial_greeting_is_not_substantive() {
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant("hello"),
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
        let d = detect_session_digest(&events);
        assert!(d.completed);
        assert!(!d.had_tool_call);
        assert!(d.content_chars < SUBSTANTIVE_CHAR_THRESHOLD);
        assert!(!is_session_substantive(&d));
    }

    #[test]
    fn long_chat_without_tools_still_substantive() {
        // No tool calls but the conversation crosses the char threshold.
        let long_user = "explain in detail how async runtimes schedule tasks ".repeat(6);
        let long_assistant =
            "An async runtime maintains an executor that polls futures. ".repeat(4);
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user(long_user.trim()),
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant(long_assistant.trim()),
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
        let d = detect_session_digest(&events);
        assert!(!d.had_tool_call);
        assert!(d.content_chars >= SUBSTANTIVE_CHAR_THRESHOLD);
        assert!(is_session_substantive(&d));
    }

    #[test]
    fn unfinished_session_is_not_substantive() {
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
                    ok: true,
                    output: serde_json::json!({}),
                    duration_ms: 2,
                },
            ),
            // No completion event.
        ];
        let d = detect_session_digest(&events);
        assert!(!d.completed);
        assert!(!is_session_substantive(&d));
    }
}
