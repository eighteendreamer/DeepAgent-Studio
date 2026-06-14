//! System-Reminder meta-channel (Phase 3A of the coding-amplifier spec).
//!
//! The runtime injects out-of-band hints to the model — verification results,
//! Plan-mode reminders, todo-list snapshots, knowledge-base nudges, etc. —
//! through a small XML-like envelope:
//!
//! ```text
//! <system-reminder>
//! free-form text the model should treat as a system message
//! </system-reminder>
//! ```
//!
//! These reminders ride along on tool results (or, in some cases, are prepended
//! to the next user turn). They are **not** part of the tool's authentic output
//! and **not** part of the user's message — the system-prompt section
//! `SECTION_SYSTEM_REMINDERS_INTRO` declares this contract so the model
//! discounts them appropriately.
//!
//! ## Why a separate channel
//!
//! Phase 4's post-edit verifier needs a way to nudge the model ("verified: …",
//! "verification failed: …") without conflating the message with real tool
//! output (which would lie about what the tool returned). Phase 3's plan-mode
//! and todo-snapshot features have the same need. Putting all of them under
//! one well-known wrapper means the model only has to learn one rule:
//! anything inside `<system-reminder>` is meta-commentary it should
//! incorporate, not echo or trust like a user instruction.
//!
//! ## API
//!
//! - [`wrap`] formats a string into the envelope.
//! - [`append_to_tool_result`] attaches a reminder to a [`serde_json::Value`]
//!   that's about to be serialized as a tool result.

use serde_json::{json, Value};

/// Format `content` into the `<system-reminder>...</system-reminder>` envelope.
/// Newlines around the body are inserted so the model sees the open/close tags
/// on their own lines.
pub fn wrap(content: &str) -> String {
    format!("<system-reminder>\n{}\n</system-reminder>", content.trim())
}

/// Attach `reminder` (already wrapped or raw text — both accepted) to a tool
/// result `value` so the model sees it alongside the rest of the result.
///
/// Behavior:
/// - If `value` is a JSON **object**, a `_system_reminder` string field is
///   added (or appended to with a newline separator if it already exists).
///   This keeps every other field of the tool result intact, so model output
///   parsers that look at e.g. `value.exit_code` or `value.content` are
///   undisturbed.
/// - If `value` is anything else (string, array, number, null), it is wrapped
///   as `{"value": <original>, "_system_reminder": <reminder>}`. This is rare
///   for production tool outputs (which are almost always objects) but is
///   handled defensively so the function is safe to call unconditionally.
///
/// Passing `reminder` raw (no `<system-reminder>` tags) is fine — most callers
/// are expected to pass [`wrap`]'s output, but the helper accepts either form
/// because the destination field carries a clear "this is a reminder" name.
pub fn append_to_tool_result(value: &mut Value, reminder: &str) {
    if let Value::Object(map) = value {
        match map.get_mut("_system_reminder") {
            Some(existing @ Value::String(_)) => {
                if let Value::String(s) = existing {
                    s.push('\n');
                    s.push_str(reminder);
                }
            }
            _ => {
                map.insert(
                    "_system_reminder".to_string(),
                    Value::String(reminder.into()),
                );
            }
        }
    } else {
        let original = std::mem::take(value);
        *value = json!({
            "value": original,
            "_system_reminder": reminder,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_uses_open_close_tags_on_their_own_lines() {
        let out = wrap("hello world");
        assert_eq!(out, "<system-reminder>\nhello world\n</system-reminder>");
    }

    #[test]
    fn wrap_trims_surrounding_whitespace() {
        // Callers occasionally hand us a body with stray newlines (e.g. from
        // formatting templates). The wrapper normalizes so we never emit
        // doubled blank lines around the body.
        let out = wrap("\n  payload  \n\n");
        assert_eq!(out, "<system-reminder>\npayload\n</system-reminder>");
    }

    #[test]
    fn wrap_preserves_internal_newlines() {
        let body = "line one\nline two\nline three";
        let out = wrap(body);
        assert_eq!(
            out,
            "<system-reminder>\nline one\nline two\nline three\n</system-reminder>"
        );
    }

    #[test]
    fn append_adds_reminder_field_to_object() {
        let mut v = json!({"stdout": "ok", "exit_code": 0});
        let reminder = wrap("verified: src/lib.rs");
        append_to_tool_result(&mut v, &reminder);

        // Original fields untouched.
        assert_eq!(v["stdout"], "ok");
        assert_eq!(v["exit_code"], 0);
        // Reminder injected.
        let injected = v["_system_reminder"].as_str().unwrap();
        assert!(injected.starts_with("<system-reminder>"));
        assert!(injected.contains("verified: src/lib.rs"));
    }

    #[test]
    fn append_concatenates_multiple_reminders() {
        let mut v = json!({"hits": []});
        append_to_tool_result(&mut v, &wrap("first"));
        append_to_tool_result(&mut v, &wrap("second"));

        let combined = v["_system_reminder"].as_str().unwrap();
        assert!(combined.contains("first"));
        assert!(combined.contains("second"));
        // Two distinct envelopes separated by a newline; preserves provenance.
        assert!(combined.matches("<system-reminder>").count() >= 2);
    }

    #[test]
    fn append_wraps_non_object_values() {
        let mut v = Value::String("plain text result".into());
        append_to_tool_result(&mut v, &wrap("hint"));

        // Scalar promoted to {value, _system_reminder}.
        assert_eq!(v["value"], "plain text result");
        assert!(v["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("<system-reminder>"));
    }

    #[test]
    fn append_handles_array_values() {
        let mut v = json!([1, 2, 3]);
        append_to_tool_result(&mut v, "raw text reminder");
        assert_eq!(v["value"], json!([1, 2, 3]));
        assert_eq!(v["_system_reminder"], "raw text reminder");
    }

    #[test]
    fn append_handles_null_value() {
        let mut v = Value::Null;
        append_to_tool_result(&mut v, "note");
        assert_eq!(v["value"], Value::Null);
        assert_eq!(v["_system_reminder"], "note");
    }

    #[test]
    fn append_accepts_unwrapped_text() {
        // Callers can pass a raw string — the destination field name carries
        // the "this is a reminder" semantic.
        let mut v = json!({"x": 1});
        append_to_tool_result(&mut v, "raw note");
        assert_eq!(v["_system_reminder"], "raw note");
    }
}
