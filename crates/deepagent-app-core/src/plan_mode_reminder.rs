//! Plan-mode `<system-reminder>` decorator (Phase 3B of coding-amplifier).
//!
//! Wraps the runtime's [`ToolResultDecorator`] extension point so that, while
//! a session is in [Plan mode](deepagent_builtins::PlanMode), every tool
//! result carries a short reminder explaining which tools are blocked and how
//! to leave the mode.
//!
//! ## Why a reminder
//!
//! Plan-mode is enforced as a hard gate (`PlanModeHook` denies write tools at
//! permission time). But the model still benefits from being told, on every
//! turn, that the gate is active — without that, it can repeatedly try
//! disallowed tools and then narrate confusion. The reminder is **out of
//! band**, attached via [`crate::system_reminder::append_to_tool_result`], so
//! it doesn't lie about what the tool itself returned.
//!
//! ## When the reminder is emitted
//!
//! - Plan-mode flag is `active` at the moment the tool result is being
//!   decorated: yes.
//! - Plan-mode flag is `inactive`: no — zero overhead beyond an atomic load.
//!
//! The reminder is the same regardless of which tool was called; the model
//! only needs to know "you're in plan mode" once per result, not "you just
//! tried bash, which is blocked".

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_builtins::PlanMode;
use deepagent_runtime::ToolResultDecorator;
use deepagent_tools::ToolOutput;

use crate::system_reminder::{append_to_tool_result, wrap};

/// Reminder text. Aligned with the `enter_plan_mode` / `exit_plan_mode` tool
/// descriptions and with the user-facing slash-command hint so the model
/// never sees conflicting wording.
const PLAN_MODE_REMINDER_BODY: &str = "Plan mode is active. Read-only tools are allowed (read_file, glob, grep, code_map_*, list_dir, web_search, web_fetch, knowledge_search, todo_write, task_list). Write/execute tools (write_file, edit_file, multi_edit, bash, git_commit, ...) are disabled. Call exit_plan_mode (or run /execute) when the plan is ready.";

/// Decorator that appends the plan-mode reminder to every tool result while a
/// session's plan-mode flag is set.
#[derive(Debug, Clone)]
pub struct PlanModeReminderDecorator {
    plan: PlanMode,
}

impl PlanModeReminderDecorator {
    /// Build over a session's shared plan-mode flag. The decorator clones the
    /// flag (cheap — `PlanMode` is `Arc<AtomicBool>` under the hood) so it
    /// continues to observe live toggles as the session runs.
    pub fn new(plan: PlanMode) -> Self {
        Self { plan }
    }

    /// Convenience: erase to a runtime-friendly trait object.
    pub fn into_arc(self) -> Arc<dyn ToolResultDecorator> {
        Arc::new(self)
    }
}

#[async_trait]
impl ToolResultDecorator for PlanModeReminderDecorator {
    async fn decorate(&self, _tool_name: &str, output: &mut ToolOutput) {
        if !self.plan.is_active() {
            return;
        }
        append_to_tool_result(&mut output.value, &wrap(PLAN_MODE_REMINDER_BODY));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok_with(value: serde_json::Value) -> ToolOutput {
        ToolOutput {
            ok: true,
            value,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn no_reminder_when_plan_mode_inactive() {
        let plan = PlanMode::new();
        // Default state is inactive.
        assert!(!plan.is_active());
        let dec = PlanModeReminderDecorator::new(plan);
        let mut out = ok_with(json!({"path": "src/main.rs", "content": "..."}));
        dec.decorate("read_file", &mut out).await;
        // Output untouched.
        assert!(out.value.get("_system_reminder").is_none());
        assert!(out.ok);
    }

    #[tokio::test]
    async fn reminder_appended_when_plan_mode_active() {
        let plan = PlanMode::new();
        plan.set(true);
        let dec = PlanModeReminderDecorator::new(plan);
        let mut out = ok_with(json!({"matches": ["src/main.rs"]}));
        dec.decorate("glob", &mut out).await;

        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.starts_with("<system-reminder>"));
        assert!(reminder.contains("Plan mode is active"));
        assert!(reminder.contains("exit_plan_mode"));
        // Original fields preserved.
        assert!(out.value["matches"].is_array());
    }

    #[tokio::test]
    async fn reminder_lists_disabled_tools_so_model_does_not_retry_them() {
        let plan = PlanMode::new();
        plan.set(true);
        let dec = PlanModeReminderDecorator::new(plan);
        let mut out = ok_with(json!({}));
        dec.decorate("read_file", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("write_file"));
        assert!(reminder.contains("edit_file"));
        assert!(reminder.contains("bash"));
    }

    #[tokio::test]
    async fn reminder_does_not_change_ok_status() {
        let plan = PlanMode::new();
        plan.set(true);
        let dec = PlanModeReminderDecorator::new(plan);
        let mut out = ok_with(json!({"x": 1}));
        dec.decorate("any", &mut out).await;
        assert!(out.ok);

        let mut err = ToolOutput {
            ok: false,
            value: json!({"error": "denied"}),
            truncated: false,
        };
        dec.decorate("any", &mut err).await;
        assert!(!err.ok);
    }

    #[tokio::test]
    async fn toggling_plan_mode_changes_decorator_behavior_live() {
        // Same decorator instance must observe live toggles via the shared
        // Arc<AtomicBool> inside PlanMode.
        let plan = PlanMode::new();
        let dec = PlanModeReminderDecorator::new(plan.clone());

        let mut off = ok_with(json!({}));
        dec.decorate("read_file", &mut off).await;
        assert!(off.value.get("_system_reminder").is_none());

        plan.set(true);
        let mut on = ok_with(json!({}));
        dec.decorate("read_file", &mut on).await;
        assert!(on.value["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("Plan mode is active"));

        plan.set(false);
        let mut off_again = ok_with(json!({}));
        dec.decorate("read_file", &mut off_again).await;
        assert!(off_again.value.get("_system_reminder").is_none());
    }
}
