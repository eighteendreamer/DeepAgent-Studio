//! Empty-output stub (Phase 2E of the coding-amplifier spec).
//!
//! When a tool returns a value that is essentially empty (`null`,
//! whitespace-only string, empty array, or empty object), the runtime replaces
//! the model-visible payload with a sentinel object:
//!
//! ```json
//! {"stub": "(toolName completed with no output)"}
//! ```
//!
//! ## Why
//!
//! DeepSeek's streaming inference treats a tool result whose serialized content
//! is empty/whitespace as a stop signal — the model often ends the turn the
//! moment it sees one. That breaks legitimate silent-success flows (e.g. `mkdir
//! -p` returning no stdout, `grep` finding zero hits, a write that returns no
//! body). By substituting a tiny self-describing payload, we preserve the
//! `ok = true` semantics while still giving the model something to read.
//!
//! ## What counts as "empty"
//!
//! Strict, conservative semantics — only the value's *immediate shape* is
//! inspected, not its contents recursively:
//!
//! - `Value::Null`
//! - `Value::String(s)` where `s.trim().is_empty()`
//! - `Value::Array(a)` where `a.is_empty()`
//! - `Value::Object(o)` where `o.is_empty()`
//!
//! Numbers, booleans, and any non-empty container are left alone — they carry
//! information (`exit_code: 0`, `count: 0`, structured bash records, ...) and
//! the model should see them verbatim.
//!
//! ## What stays the same
//!
//! `ok` and `truncated` are never touched. An empty success stays a success.
//! An empty failure (rare, but possible if a tool returns `ToolOutput { ok:
//! false, value: Null, .. }`) stays a failure but gets a more readable payload.

use deepagent_tools::ToolOutput;
use serde_json::{json, Value};

/// Replace the tool output's `value` in place with a self-describing stub when
/// the original payload would be semantically empty. Leaves `ok` and
/// `truncated` untouched.
pub fn ensure_non_empty_output(output: &mut ToolOutput, tool_name: &str) {
    if is_semantically_empty(&output.value) {
        output.value = json!({
            "stub": format!("({} completed with no output)", tool_name),
        });
    }
}

fn is_semantically_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_with(value: Value) -> ToolOutput {
        ToolOutput {
            ok: true,
            value,
            truncated: false,
        }
    }

    #[test]
    fn replaces_null_value() {
        let mut out = ok_with(Value::Null);
        ensure_non_empty_output(&mut out, "bash");
        assert!(out.ok);
        assert_eq!(out.value["stub"], "(bash completed with no output)");
    }

    #[test]
    fn replaces_empty_string() {
        let mut out = ok_with(Value::String(String::new()));
        ensure_non_empty_output(&mut out, "grep");
        assert_eq!(out.value["stub"], "(grep completed with no output)");
    }

    #[test]
    fn replaces_whitespace_only_string() {
        let mut out = ok_with(Value::String("   \n\t  ".to_string()));
        ensure_non_empty_output(&mut out, "shell");
        assert_eq!(out.value["stub"], "(shell completed with no output)");
    }

    #[test]
    fn replaces_empty_array() {
        let mut out = ok_with(json!([]));
        ensure_non_empty_output(&mut out, "grep");
        assert_eq!(out.value["stub"], "(grep completed with no output)");
    }

    #[test]
    fn replaces_empty_object() {
        let mut out = ok_with(json!({}));
        ensure_non_empty_output(&mut out, "noop");
        assert_eq!(out.value["stub"], "(noop completed with no output)");
    }

    #[test]
    fn preserves_ok_status_when_replacing() {
        let mut out = ok_with(Value::Null);
        ensure_non_empty_output(&mut out, "tool");
        // ok stays true — silent success is still success.
        assert!(out.ok);
    }

    #[test]
    fn preserves_failure_status_when_replacing() {
        let mut out = ToolOutput {
            ok: false,
            value: Value::Null,
            truncated: false,
        };
        ensure_non_empty_output(&mut out, "tool");
        // ok stays false — but the payload is now self-describing.
        assert!(!out.ok);
        assert_eq!(out.value["stub"], "(tool completed with no output)");
    }

    #[test]
    fn preserves_truncated_flag() {
        let mut out = ToolOutput {
            ok: true,
            value: Value::Null,
            truncated: true,
        };
        ensure_non_empty_output(&mut out, "tool");
        assert!(out.truncated);
    }

    #[test]
    fn does_not_replace_non_empty_string() {
        let mut out = ok_with(Value::String("hello".to_string()));
        ensure_non_empty_output(&mut out, "tool");
        assert_eq!(out.value, Value::String("hello".to_string()));
    }

    #[test]
    fn does_not_replace_non_empty_array() {
        let value = json!([1, 2, 3]);
        let mut out = ok_with(value.clone());
        ensure_non_empty_output(&mut out, "tool");
        assert_eq!(out.value, value);
    }

    #[test]
    fn does_not_replace_non_empty_object() {
        let value = json!({"a": 1});
        let mut out = ok_with(value.clone());
        ensure_non_empty_output(&mut out, "tool");
        assert_eq!(out.value, value);
    }

    #[test]
    fn does_not_replace_structured_bash_silent_success() {
        // Bash returns a structured record with exit_code/stdout/stderr — even
        // if stdout/stderr are empty strings, the object as a whole is not
        // empty and the model should see exit_code: 0 verbatim.
        let value = json!({
            "command": "true",
            "exit_code": 0,
            "stdout": "",
            "stderr": "",
        });
        let mut out = ok_with(value.clone());
        ensure_non_empty_output(&mut out, "bash");
        assert_eq!(out.value, value);
    }

    #[test]
    fn does_not_replace_zero_number() {
        let mut out = ok_with(json!(0));
        ensure_non_empty_output(&mut out, "tool");
        assert_eq!(out.value, json!(0));
    }

    #[test]
    fn does_not_replace_false_bool() {
        let mut out = ok_with(json!(false));
        ensure_non_empty_output(&mut out, "tool");
        assert_eq!(out.value, json!(false));
    }

    #[test]
    fn stub_uses_provided_tool_name() {
        let mut out = ok_with(Value::Null);
        ensure_non_empty_output(&mut out, "code_map_search");
        assert_eq!(
            out.value["stub"],
            "(code_map_search completed with no output)"
        );
    }
}
