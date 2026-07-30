//! Todo snapshot `<system-reminder>` decorator (Phase 3E of coding-amplifier).
//!
//! When the model calls `todo_write`, the [`deepagent_builtins::TodoStore`]
//! arms a one-shot "snapshot pending" flag. This decorator consumes that flag
//! on the **next** non-todo tool result and appends a `<system-reminder>` body
//! containing [`TodoStore::format_snapshot`]. The rationale:
//!
//! - On `todo_write`'s OWN result, the model already sees the full updated
//!   list — re-injecting it would be redundant.
//! - On the very next tool call (e.g. `read_file`, `bash`), the model has
//!   moved on. Re-attaching the latest plan keeps the model from
//!   "forgetting what it's doing" several turns into a long task.
//! - Consecutive `todo_write` calls without an intervening tool collapse to
//!   one reminder (the `mark`/`take` pair is idempotent + one-shot).
//!
//! This decorator runs alongside [`crate::plan_mode_reminder`] via
//! [`deepagent_runtime::ChainDecorator`].

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_builtins::TodoStore;
use deepagent_runtime::ToolResultDecorator;
use deepagent_tools::ToolOutput;

use crate::system_reminder::{append_to_tool_result, wrap};

/// Tool names the decorator skips: `todo_write`'s own result already carries
/// the list, and `task_list` exists specifically to render it on demand.
const SKIP_TOOLS: &[&str] = &["todo_write", "task_list"];

/// Decorator that injects a one-shot todo-list reminder after the next
/// non-todo tool call following a successful `todo_write`.
#[derive(Debug, Clone)]
pub struct TodoSnapshotReminderDecorator {
    store: TodoStore,
}

impl TodoSnapshotReminderDecorator {
    /// Build over the session's shared todo store.
    pub fn new(store: TodoStore) -> Self {
        Self { store }
    }

    /// Erase to a runtime-friendly trait object.
    pub fn into_arc(self) -> Arc<dyn ToolResultDecorator> {
        Arc::new(self)
    }
}

#[async_trait]
impl ToolResultDecorator for TodoSnapshotReminderDecorator {
    async fn decorate(&self, tool_name: &str, output: &mut ToolOutput) {
        // Skip the todo tools themselves — re-emitting the list there is
        // pure noise and would also stop the flag from carrying over to a
        // genuinely informative subsequent tool call.
        if SKIP_TOOLS.contains(&tool_name) {
            return;
        }
        if !self.store.take_pending_snapshot() {
            return;
        }
        let body = format!("Todo list snapshot:\n{}", self.store.format_snapshot());
        append_to_tool_result(&mut output.value, &wrap(&body));
    }
}

/// Adapts the session [`TodoStore`] to the runtime's periodic todo-reminder
/// source (§3.1, Claude Code `getTodoReminderAttachments`). Distinct from the
/// one-shot [`TodoSnapshotReminderDecorator`] above: this feeds the
/// turn-paced reminder the `ModelAgent` injects when the model hasn't tracked
/// its plan for a while, so a long run can't silently drift off-plan.
pub struct TodoReminderAdapter {
    store: TodoStore,
}

impl TodoReminderAdapter {
    /// Wrap the session's shared todo store.
    pub fn new(store: TodoStore) -> Self {
        Self { store }
    }
}

impl deepagent_runtime::TodoReminderSource for TodoReminderAdapter {
    fn todo_snapshot(&self) -> deepagent_runtime::TodoReminderSnapshot {
        let (pending, running, completed) = self.store.counts();
        deepagent_runtime::TodoReminderSnapshot {
            has_items: pending + running + completed > 0,
            rendered: self.store.format_snapshot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_builtins::{TodoItem, TodoStatus};
    use serde_json::json;

    fn ok_with(value: serde_json::Value) -> ToolOutput {
        ToolOutput {
            ok: true,
            value,
            truncated: false,
        }
    }

    fn store_with_one_in_progress() -> TodoStore {
        let store = TodoStore::new();
        store.replace(vec![TodoItem {
            content: "Implement OAuth".into(),
            status: TodoStatus::InProgress,
            active_form: "Implementing OAuth".into(),
        }]);
        store.mark_snapshot_pending();
        store
    }

    #[tokio::test]
    async fn no_reminder_when_no_pending_snapshot() {
        let store = TodoStore::new();
        // Never marked pending → no reminder.
        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut out = ok_with(json!({"path": "src/lib.rs"}));
        dec.decorate("read_file", &mut out).await;
        assert!(out.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn reminder_injected_on_next_tool_after_todo_write() {
        let store = store_with_one_in_progress();
        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut out = ok_with(json!({"path": "src/lib.rs", "content": "..."}));
        dec.decorate("read_file", &mut out).await;

        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.starts_with("<system-reminder>"));
        assert!(reminder.contains("Todo list snapshot"));
        assert!(reminder.contains("[~] Implementing OAuth"));
        // Original output preserved.
        assert_eq!(out.value["path"], "src/lib.rs");
    }

    #[tokio::test]
    async fn reminder_only_injected_once() {
        // Phase 3E one-shot: after one consume, the flag is cleared and a
        // subsequent tool result (with no intervening todo_write) gets nothing.
        let store = store_with_one_in_progress();
        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut first = ok_with(json!({}));
        dec.decorate("read_file", &mut first).await;
        assert!(first.value.get("_system_reminder").is_some());

        let mut second = ok_with(json!({}));
        dec.decorate("read_file", &mut second).await;
        assert!(second.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn reminder_skipped_for_todo_write_itself() {
        let store = store_with_one_in_progress();
        let dec = TodoSnapshotReminderDecorator::new(store.clone());
        let mut out = ok_with(json!({"todos": [], "summary": {}}));
        dec.decorate("todo_write", &mut out).await;
        // No reminder injected, AND the flag is preserved so the NEXT
        // non-todo tool still gets the snapshot.
        assert!(out.value.get("_system_reminder").is_none());
        assert!(store.is_snapshot_pending());
    }

    #[tokio::test]
    async fn reminder_skipped_for_task_list_too() {
        // task_list already returns the full list on demand — re-injecting
        // a reminder there would just duplicate the body.
        let store = store_with_one_in_progress();
        let dec = TodoSnapshotReminderDecorator::new(store.clone());
        let mut out = ok_with(json!({"todos": [], "summary": {}}));
        dec.decorate("task_list", &mut out).await;
        assert!(out.value.get("_system_reminder").is_none());
        assert!(store.is_snapshot_pending());
    }

    #[tokio::test]
    async fn consecutive_todo_writes_collapse_to_one_reminder() {
        let store = TodoStore::new();
        // Three consecutive marks (e.g. three todo_write calls in a row).
        store.replace(vec![TodoItem {
            content: "First".into(),
            status: TodoStatus::Pending,
            active_form: "Doing first".into(),
        }]);
        store.mark_snapshot_pending();
        store.replace(vec![TodoItem {
            content: "Second".into(),
            status: TodoStatus::Pending,
            active_form: "Doing second".into(),
        }]);
        store.mark_snapshot_pending();
        store.replace(vec![TodoItem {
            content: "Final".into(),
            status: TodoStatus::Pending,
            active_form: "Doing final".into(),
        }]);
        store.mark_snapshot_pending();

        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut out = ok_with(json!({}));
        dec.decorate("read_file", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        // Only the latest snapshot is shown.
        assert!(reminder.contains("Final"));
        assert!(!reminder.contains("First"));
        assert!(!reminder.contains("Second"));

        // And the flag is cleared after the single consume.
        let mut next = ok_with(json!({}));
        dec.decorate("read_file", &mut next).await;
        assert!(next.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn empty_list_snapshot_still_renders_when_pending() {
        let store = TodoStore::new();
        // Mark pending without any items (rare but valid: model cleared list).
        store.mark_snapshot_pending();
        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut out = ok_with(json!({}));
        dec.decorate("bash", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("Current todo list is empty"));
    }

    #[tokio::test]
    async fn decorator_does_not_change_ok_status() {
        let store = store_with_one_in_progress();
        let dec = TodoSnapshotReminderDecorator::new(store);
        let mut out = ok_with(json!({}));
        dec.decorate("read_file", &mut out).await;
        assert!(out.ok);
    }
}
