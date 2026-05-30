//! Transcript export (会话导出).
//!
//! Renders a session's append-only [`Event`] stream into a portable transcript
//! the user can save or share — either human-readable **Markdown** or a
//! structured **JSON** array of events. Like the timeline, this is a pure
//! projection of durable events, so an export always reflects the exact
//! recorded history.

use deepagent_core::event::{Event, EventPayload};
use deepagent_core::message::Role;

/// The export format requested by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptFormat {
    /// Human-readable Markdown transcript.
    Markdown,
    /// Structured JSON (the raw event array, pretty-printed).
    Json,
}

impl TranscriptFormat {
    /// Parse a format from a UI label ("markdown"/"md" or "json").
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "markdown" | "md" => Some(Self::Markdown),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// The conventional file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
        }
    }
}

/// Render `events` to a transcript string in `format`. `title` is used as the
/// document heading for Markdown (ignored for JSON).
pub fn export_transcript(
    title: Option<&str>,
    events: &[Event],
    format: TranscriptFormat,
) -> Result<String, serde_json::Error> {
    match format {
        TranscriptFormat::Json => serde_json::to_string_pretty(events),
        TranscriptFormat::Markdown => Ok(render_markdown(title, events)),
    }
}

fn render_markdown(title: Option<&str>, events: &[Event]) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(title.unwrap_or("Session transcript"));
    out.push_str("\n\n");

    for event in events {
        match &event.payload {
            EventPayload::SessionStarted { title, mode } => {
                out.push_str(&format!("_Session started_ (mode: {})", mode.label()));
                if let Some(t) = title {
                    out.push_str(&format!(" — {t}"));
                }
                out.push_str("\n\n");
            }
            EventPayload::SessionEnded { reason } => {
                out.push_str("_Session ended_");
                if let Some(r) = reason {
                    out.push_str(&format!(": {r}"));
                }
                out.push_str("\n\n");
            }
            EventPayload::TaskCreated { goal, .. } => {
                out.push_str(&format!("### Task: {goal}\n\n"));
            }
            EventPayload::TaskStateChanged { from, to, .. } => {
                out.push_str(&format!("> task {from:?} → {to:?}\n\n"));
            }
            EventPayload::MessageAppended { message } => {
                let who = match message.role {
                    Role::System => "System",
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::Tool => "Tool",
                };
                out.push_str(&format!("**{who}:**\n\n"));
                if let Some(reasoning) = &message.reasoning_content {
                    if !reasoning.is_empty() {
                        out.push_str("<details><summary>reasoning</summary>\n\n");
                        out.push_str(reasoning);
                        out.push_str("\n\n</details>\n\n");
                    }
                }
                if !message.content.is_empty() {
                    out.push_str(&message.content);
                    out.push_str("\n\n");
                }
                for call in &message.tool_calls {
                    out.push_str(&format!("- 🔧 calls `{}`({})\n", call.name, call.arguments));
                }
                if !message.tool_calls.is_empty() {
                    out.push('\n');
                }
            }
            EventPayload::ToolCallRequested { call } => {
                out.push_str(&format!(
                    "🔧 **Tool request** `{}`\n\n```json\n{}\n```\n\n",
                    call.name,
                    serde_json::to_string_pretty(&call.arguments).unwrap_or_default()
                ));
            }
            EventPayload::ToolCallCompleted {
                ok,
                output,
                duration_ms,
                ..
            } => {
                let status = if *ok { "✅ ok" } else { "❌ failed" };
                out.push_str(&format!(
                    "{status} ({duration_ms} ms)\n\n```json\n{}\n```\n\n",
                    serde_json::to_string_pretty(output).unwrap_or_default()
                ));
            }
            EventPayload::ContextCompacted {
                tokens_before,
                tokens_after,
                strategy,
            } => {
                out.push_str(&format!(
                    "> 🗜 context compacted ({strategy}): {tokens_before} → {tokens_after} tokens\n\n"
                ));
            }
            EventPayload::Note { text } => {
                out.push_str(&format!("> 📝 {text}\n\n"));
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::id::{EventId, SessionId};
    use deepagent_core::message::Message;

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::nil(),
            sequence: seq,
            timestamp: Timestamp::from_millis(1000 + seq as i64),
            payload,
        }
    }

    fn sample() -> Vec<Event> {
        vec![
            event(
                0,
                EventPayload::SessionStarted {
                    title: Some("demo".into()),
                    mode: deepagent_core::session_mode::SessionMode::Normal,
                },
            ),
            event(
                1,
                EventPayload::MessageAppended {
                    message: Message::user("hello there"),
                },
            ),
            event(
                2,
                EventPayload::MessageAppended {
                    message: Message::assistant("hi, how can I help?"),
                },
            ),
        ]
    }

    #[test]
    fn format_parse_roundtrip() {
        assert_eq!(
            TranscriptFormat::parse("md"),
            Some(TranscriptFormat::Markdown)
        );
        assert_eq!(
            TranscriptFormat::parse("Markdown"),
            Some(TranscriptFormat::Markdown)
        );
        assert_eq!(
            TranscriptFormat::parse("JSON"),
            Some(TranscriptFormat::Json)
        );
        assert_eq!(TranscriptFormat::parse("xml"), None);
    }

    #[test]
    fn markdown_has_heading_and_messages() {
        let md = export_transcript(Some("My chat"), &sample(), TranscriptFormat::Markdown).unwrap();
        assert!(md.starts_with("# My chat"));
        assert!(md.contains("**User:**"));
        assert!(md.contains("hello there"));
        assert!(md.contains("**Assistant:**"));
    }

    #[test]
    fn json_is_valid_event_array() {
        let json = export_transcript(None, &sample(), TranscriptFormat::Json).unwrap();
        let parsed: Vec<Event> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].sequence, 1);
    }

    #[test]
    fn extension_matches_format() {
        assert_eq!(TranscriptFormat::Markdown.extension(), "md");
        assert_eq!(TranscriptFormat::Json.extension(), "json");
    }
}
