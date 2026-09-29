//! System-Reminder meta-channel envelope (shared).
//!
//! The runtime injects out-of-band hints to the model — verification results,
//! Plan-mode reminders, todo snapshots, knowledge nudges, stall/doom-loop
//! nudges — through one XML-like envelope:
//!
//! ```text
//! <system-reminder>
//! free-form text the model should treat as a system message
//! </system-reminder>
//! ```
//!
//! The envelope lives in `deepagent-context` (rather than `deepagent-app-core`)
//! so both the app layer and `deepagent-runtime` share one implementation; the
//! runtime must not depend on `deepagent-app-core`, and previously hand-wrote
//! the same tags in several places. The system-prompt section
//! `SECTION_SYSTEM_REMINDERS_INTRO` documents the contract so the model
//! discounts these hints appropriately (it must not echo them or treat them as
//! user instructions).

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
/// - If `value` is a JSON **object**, a `_system_reminder` string field is added
///   (or appended to with a newline separator if it already exists), leaving
///   every other field intact so tool-result parsers are undisturbed.
/// - Otherwise the value is wrapped as `{"value": <original>,
///   "_system_reminder": <reminder>}`.
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
        assert_eq!(
            wrap("hello world"),
            "<system-reminder>\nhello world\n</system-reminder>"
        );
    }

    #[test]
    fn wrap_trims_surrounding_whitespace() {
        assert_eq!(
            wrap("\n  payload  \n\n"),
            "<system-reminder>\npayload\n</system-reminder>"
        );
    }

    #[test]
    fn append_adds_reminder_field_to_object() {
        let mut v = json!({"stdout": "ok", "exit_code": 0});
        append_to_tool_result(&mut v, &wrap("verified: src/lib.rs"));
        assert_eq!(v["stdout"], "ok");
        assert_eq!(v["exit_code"], 0);
        assert!(v["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("verified: src/lib.rs"));
    }

    #[test]
    fn append_wraps_non_object_values() {
        let mut v = Value::String("plain text result".into());
        append_to_tool_result(&mut v, &wrap("hint"));
        assert_eq!(v["value"], "plain text result");
        assert!(v["_system_reminder"].as_str().unwrap().contains("hint"));
    }
}
