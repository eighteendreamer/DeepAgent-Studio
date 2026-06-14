//! The `todo_write` built-in — the structured task list, aligned with Claude
//! Code's `TodoWrite`.
//!
//! Claude Code keeps a single, full-snapshot todo list per session: every
//! `TodoWrite` call replaces the entire list (it is not an append). Each item
//! carries a `content` (imperative description), a `status`
//! (`pending`/`in_progress`/`completed`), and an `active_form` (present-tense
//! phrasing shown while the item is in progress). Exactly one item should be
//! `in_progress` at a time, mirroring the upstream guidance.
//!
//! State lives in a shared [`TodoStore`] so the runtime can render the current
//! list between turns while the tool stays a normal [`Tool`].

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Lifecycle state of a single todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started yet.
    #[default]
    Pending,
    /// Actively being worked on (should be unique across the list).
    InProgress,
    /// Finished.
    Completed,
}

impl TodoStatus {
    /// Parse from the wire string, tolerating a couple of common spellings.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" | "in-progress" | "inprogress" => Some(Self::InProgress),
            "completed" | "complete" | "done" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// A single todo entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Imperative description of the task (e.g. "Run the test suite").
    pub content: String,
    /// Lifecycle state.
    pub status: TodoStatus,
    /// Present-tense phrasing shown while in progress (e.g. "Running the test
    /// suite"). Defaults to `content` when omitted.
    #[serde(default)]
    pub active_form: String,
}

/// Shared, full-snapshot todo list for a session.
///
/// Beyond the items themselves, the store tracks a `pending_snapshot` flag
/// (Phase 3E) — set whenever `todo_write` updates the list and consumed by
/// the runtime decorator that appends a `<system-reminder>` snapshot to the
/// next non-todo tool result. The flag is one-shot: consecutive `todo_write`s
/// before any other tool result simply leave it set, so only the latest
/// snapshot is shown.
#[derive(Debug, Clone, Default)]
pub struct TodoStore {
    items: Arc<Mutex<Vec<TodoItem>>>,
    /// Set to `true` by [`TodoWriteTool`] after a successful update; consumed
    /// by [`TodoStore::take_pending_snapshot`] from the runtime decorator.
    pending_snapshot: Arc<std::sync::atomic::AtomicBool>,
}

impl TodoStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire list (the only mutation `TodoWrite` performs).
    pub fn replace(&self, items: Vec<TodoItem>) {
        *self.items.lock().expect("todo store poisoned") = items;
    }

    /// Snapshot the current list.
    pub fn snapshot(&self) -> Vec<TodoItem> {
        self.items.lock().expect("todo store poisoned").clone()
    }

    /// Count items in each state: (pending, in_progress, completed).
    pub fn counts(&self) -> (usize, usize, usize) {
        let items = self.items.lock().expect("todo store poisoned");
        let mut counts = (0, 0, 0);
        for item in items.iter() {
            match item.status {
                TodoStatus::Pending => counts.0 += 1,
                TodoStatus::InProgress => counts.1 += 1,
                TodoStatus::Completed => counts.2 += 1,
            }
        }
        counts
    }

    /// Mark the store as having a fresh update that should be reminded to the
    /// model on the next non-todo tool result. Idempotent.
    pub fn mark_snapshot_pending(&self) {
        self.pending_snapshot
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Atomically clear and return whether a snapshot reminder was pending.
    /// Returns `true` exactly once per `mark_snapshot_pending` call.
    pub fn take_pending_snapshot(&self) -> bool {
        self.pending_snapshot
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Whether a snapshot is currently pending (read-only; does not consume).
    pub fn is_snapshot_pending(&self) -> bool {
        self.pending_snapshot
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Render the current list as a Markdown-ish bullet snapshot suitable for
    /// embedding in a `<system-reminder>` block.
    ///
    /// Format:
    /// ```text
    /// Current todo list (pending: P, in_progress: R, completed: C):
    /// - [ ] pending item content
    /// - [~] active form of in_progress item
    /// - [x] completed item content
    /// ```
    /// Empty store returns "Current todo list is empty.".
    pub fn format_snapshot(&self) -> String {
        let items = self.items.lock().expect("todo store poisoned").clone();
        if items.is_empty() {
            return "Current todo list is empty.".to_string();
        }
        let mut pending = 0usize;
        let mut running = 0usize;
        let mut completed = 0usize;
        for item in &items {
            match item.status {
                TodoStatus::Pending => pending += 1,
                TodoStatus::InProgress => running += 1,
                TodoStatus::Completed => completed += 1,
            }
        }
        let mut out = format!(
            "Current todo list (pending: {pending}, in_progress: {running}, completed: {completed}):"
        );
        for item in &items {
            let (mark, body) = match item.status {
                TodoStatus::Pending => ("[ ]", item.content.as_str()),
                // Use active_form for the running item — that's the present-
                // progressive phrasing the field exists for.
                TodoStatus::InProgress => ("[~]", item.active_form.as_str()),
                TodoStatus::Completed => ("[x]", item.content.as_str()),
            };
            out.push('\n');
            out.push_str(&format!("- {mark} {body}"));
        }
        out
    }
}

/// The `todo_write` tool.
pub struct TodoWriteTool {
    store: TodoStore,
}

impl TodoWriteTool {
    /// Build over a shared [`TodoStore`].
    pub fn new(store: TodoStore) -> Self {
        Self { store }
    }

    /// The store this tool writes to (for the runtime to read between turns).
    pub fn store(&self) -> &TodoStore {
        &self.store
    }
}

/// Parse and validate the `todos` argument into items.
///
/// Validation rules (Phase 3D — coding-amplifier spec, Requirement 6):
/// - `content` is required, non-empty after trim.
/// - `active_form` is required, non-empty after trim, accepted under either
///   the snake_case (`active_form`) or camelCase (`activeForm`) name.
/// - `status` is required and must parse to a known [`TodoStatus`].
fn parse_items(todos: &[serde_json::Value]) -> std::result::Result<Vec<TodoItem>, String> {
    let mut items = Vec::with_capacity(todos.len());
    for (i, raw) in todos.iter().enumerate() {
        let content = raw
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("todo[{i}] missing non-empty 'content'"))?
            .to_string();
        let status = match raw.get("status").and_then(|v| v.as_str()) {
            Some(s) => {
                TodoStatus::parse(s).ok_or_else(|| format!("todo[{i}] has invalid status '{s}'"))?
            }
            None => TodoStatus::Pending,
        };
        // active_form is REQUIRED in Phase 3D — a present-tense companion to
        // the imperative `content`, used by the UI for the in-progress entry.
        let active_form = raw
            .get("active_form")
            .or_else(|| raw.get("activeForm"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "todo[{i}] missing non-empty 'active_form' (present-tense phrasing of content)"
                )
            })?;
        items.push(TodoItem {
            content,
            status,
            active_form,
        });
    }
    Ok(items)
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "todo_write".into(),
            description: "Replace the session task list with a full snapshot. Use it to plan and track multi-step work that would otherwise be hard to keep straight in your head.\n\
                \n\
                ## When to use\n\
                - The task has 3+ steps OR is genuinely complex (multi-file refactor, end-to-end feature, bug investigation across modules).\n\
                - You receive a fresh user instruction with multiple distinct asks (capture each as a separate item).\n\
                - You're starting a new task — write the plan first.\n\
                - You finish a step — mark it completed in the SAME response that finished it (don't batch).\n\
                \n\
                ## When NOT to use\n\
                - Single trivial step (read one file, run one command, answer one question).\n\
                - Pure conversation / clarification turns.\n\
                - One-shot edits with no follow-up.\n\
                \n\
                ## Required fields per item\n\
                - `content` — IMPERATIVE form, what to do (e.g. \"Run the test suite\", \"Implement OAuth callback\").\n\
                - `active_form` — PRESENT-PROGRESSIVE form, what's happening right now while in_progress (e.g. \"Running the test suite\", \"Implementing OAuth callback\"). The UI shows this exact wording for the in_progress item.\n\
                - `status` — `pending` | `in_progress` | `completed`.\n\
                \n\
                ## State-machine rules (HARD)\n\
                - At most ONE item may be `in_progress` at any time. Multiple in_progress items are rejected.\n\
                - Mark an item `in_progress` BEFORE starting it; mark it `completed` only after the work is genuinely done (tests pass, file written, command ran clean).\n\
                - DO NOT mark an item `completed` when: tests are failing, the implementation is partial, errors are unresolved, dependencies are missing, or the work was deferred. In those cases keep it `in_progress` (or split it into a new pending item describing the blocker).\n\
                - If progress is blocked, leave the item `in_progress` and add a new pending item describing the blocker."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "Imperative form of the task (e.g. 'Run the test suite')."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "active_form": {
                                    "type": "string",
                                    "description": "Present-progressive form shown when in_progress (e.g. 'Running the test suite')."
                                }
                            },
                            "required": ["content", "status", "active_form"]
                        }
                    }
                },
                "required": ["todos"]
            }),
            // Planning only — no filesystem or shell effects.
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(todos) = args.get("todos").and_then(|v| v.as_array()) else {
            return Ok(ToolOutput::failure("missing 'todos' array"));
        };

        let items = match parse_items(todos) {
            Ok(items) => items,
            Err(e) => return Ok(ToolOutput::failure(e)),
        };

        // Hard rule (Phase 3D): at most one item may be in_progress at any
        // time. Reject the write rather than silently warning so the model
        // self-corrects in the next turn.
        let in_progress_count = items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .count();
        if in_progress_count > 1 {
            return Ok(ToolOutput::failure(format!(
                "exactly one in_progress at a time: got {in_progress_count}. Demote {} of them to pending or finish them first.",
                in_progress_count - 1
            )));
        }

        // Soft warning: the model frequently marks something completed when
        // the underlying work actually failed. Surface a hint when an item
        // moved to completed but its content describes a failure.
        let mut completed_with_failure_keyword: Vec<String> = Vec::new();
        for item in &items {
            if item.status == TodoStatus::Completed && content_smells_like_failure(&item.content) {
                completed_with_failure_keyword.push(item.content.clone());
            }
        }

        // Soft warning: active_form should differ from content (imperative vs
        // present-progressive). Same wording defeats the UI's purpose.
        let mut same_form: Vec<String> = Vec::new();
        for item in &items {
            if item.active_form.trim() == item.content.trim() {
                same_form.push(item.content.clone());
            }
        }

        self.store.replace(items.clone());
        let (pending, running, completed) = self.store.counts();
        // Phase 3E: arm the snapshot reminder for the next non-todo tool
        // result so the model is reminded of its plan exactly once.
        self.store.mark_snapshot_pending();

        let mut value = serde_json::json!({
            "todos": items,
            "summary": {
                "total": items.len(),
                "pending": pending,
                "in_progress": running,
                "completed": completed,
            }
        });
        let mut warnings: Vec<String> = Vec::new();
        if !completed_with_failure_keyword.is_empty() {
            warnings.push(format!(
                "completed items contain failure keywords (failed/error/blocked) — verify they actually succeeded: {}",
                completed_with_failure_keyword.join("; ")
            ));
        }
        if !same_form.is_empty() {
            warnings.push(format!(
                "active_form should be present-progressive (e.g. 'Running tests'), not identical to content: {}",
                same_form.join("; ")
            ));
        }
        if !warnings.is_empty() {
            value["warnings"] = serde_json::Value::Array(
                warnings
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            );
        }
        Ok(ToolOutput::success(value))
    }
}

/// Whether `content` reads like the work is incomplete or broken — used to
/// warn (not reject) when an item is marked `completed` with such wording.
fn content_smells_like_failure(content: &str) -> bool {
    let lc = content.to_lowercase();
    lc.contains("failed") || lc.contains("error") || lc.contains("blocked")
}

/// The `task_list` tool — read the current session task list (read-only).
///
/// Complements [`TodoWriteTool`]: where `todo_write` replaces the list,
/// `task_list` returns the current items in stable order (by their position in
/// the snapshot) plus a status summary. Mirrors Claude Code's `TaskList`, which
/// must return tasks in a deterministic order rather than arbitrary order.
pub struct TaskListTool {
    store: TodoStore,
}

impl TaskListTool {
    /// Build over a shared [`TodoStore`].
    pub fn new(store: TodoStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "task_list".into(),
            description: "Read the current session task list (items + status summary), in stable \
                order. Takes no arguments."
                .into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        let items = self.store.snapshot();
        let (pending, in_progress, completed) = self.store.counts();
        Ok(ToolOutput::success(serde_json::json!({
            "todos": items,
            "summary": {
                "total": items.len(),
                "pending": pending,
                "in_progress": in_progress,
                "completed": completed,
            }
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_and_snapshots() {
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [
                    { "content": "Read the code", "status": "completed", "active_form": "Reading the code" },
                    { "content": "Write the fix", "status": "in_progress", "active_form": "Writing the fix" },
                    { "content": "Run tests", "status": "pending", "active_form": "Running tests" }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["summary"]["total"], 3);
        assert_eq!(out.value["summary"]["completed"], 1);
        assert_eq!(out.value["summary"]["in_progress"], 1);
        assert_eq!(out.value["summary"]["pending"], 1);

        let snap = store.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[2].active_form, "Running tests");
    }

    #[tokio::test]
    async fn replace_is_full_snapshot_not_append() {
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "A", "status": "pending", "active_form": "Doing A" }]
        }))
        .await
        .unwrap();
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "B", "status": "pending", "active_form": "Doing B" }]
        }))
        .await
        .unwrap();
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].content, "B");
    }

    #[tokio::test]
    async fn rejects_invalid_status_and_empty_content() {
        let tool = TodoWriteTool::new(TodoStore::new());
        let bad_status = tool
            .invoke(
                serde_json::json!({"todos": [{ "content": "x", "status": "nope", "active_form": "Xing" }]}),
            )
            .await
            .unwrap();
        assert!(!bad_status.ok);

        let empty = tool
            .invoke(
                serde_json::json!({"todos": [{ "content": "  ", "status": "pending", "active_form": "Doing" }]}),
            )
            .await
            .unwrap();
        assert!(!empty.ok);
    }

    #[tokio::test]
    async fn rejects_missing_active_form() {
        // Phase 3D: active_form is now REQUIRED. Items without it (or with
        // only whitespace) must be rejected at parse time.
        let tool = TodoWriteTool::new(TodoStore::new());
        let missing = tool
            .invoke(serde_json::json!({"todos": [{ "content": "Run tests", "status": "pending" }]}))
            .await
            .unwrap();
        assert!(!missing.ok);
        let err = missing.value["error"].as_str().unwrap();
        assert!(err.contains("active_form"), "got: {err}");

        let empty_active = tool
            .invoke(serde_json::json!({"todos": [{
                "content": "Run tests",
                "status": "pending",
                "active_form": "  ",
            }]}))
            .await
            .unwrap();
        assert!(!empty_active.ok);
    }

    #[tokio::test]
    async fn accepts_camelcase_active_form_alias() {
        // Backward-compat: callers using camelCase (`activeForm`) are still
        // accepted — the schema documents `active_form` but Claude Code
        // historically used `activeForm`.
        let tool = TodoWriteTool::new(TodoStore::new());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [{
                    "content": "Run tests",
                    "status": "pending",
                    "activeForm": "Running tests",
                }]
            }))
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn rejects_multiple_in_progress() {
        // Phase 3D: was a soft warning; now a hard rejection. Multiple
        // in_progress items violate the state machine and a successful write
        // would let the model lose track of "what am I doing right now?".
        let tool = TodoWriteTool::new(TodoStore::new());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [
                    { "content": "A", "status": "in_progress", "active_form": "Doing A" },
                    { "content": "B", "status": "in_progress", "active_form": "Doing B" }
                ]
            }))
            .await
            .unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        assert!(err.contains("in_progress"));
        assert!(err.contains("2"));
    }

    #[tokio::test]
    async fn warns_on_completed_with_failure_keyword() {
        // Soft warning: marking an item completed when its description names
        // a failure mode is suspicious. We DO NOT reject — the model may
        // legitimately be tracking a meta task like "Document the error
        // recovery path" — but we surface a warning so review is invited.
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [
                    {
                        "content": "Fix the failed migration",
                        "status": "completed",
                        "active_form": "Fixing the failed migration"
                    }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        let warnings = out.value["warnings"].as_array().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("failure keywords")));
    }

    #[tokio::test]
    async fn warns_on_active_form_same_as_content() {
        // Soft warning: identical content/active_form defeats the UI purpose.
        let tool = TodoWriteTool::new(TodoStore::new());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [
                    { "content": "Run tests", "status": "pending", "active_form": "Run tests" }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        let warnings = out.value["warnings"].as_array().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("present-progressive")));
    }

    #[test]
    fn status_parse_tolerant() {
        assert_eq!(
            TodoStatus::parse("in-progress"),
            Some(TodoStatus::InProgress)
        );
        assert_eq!(TodoStatus::parse("done"), Some(TodoStatus::Completed));
        assert_eq!(TodoStatus::parse("bogus"), None);
    }

    #[tokio::test]
    async fn task_list_reads_current_store() {
        let store = TodoStore::new();
        // Seed via the writer.
        TodoWriteTool::new(store.clone())
            .invoke(serde_json::json!({
                "todos": [
                    { "content": "A", "status": "completed", "active_form": "Doing A" },
                    { "content": "B", "status": "in_progress", "active_form": "Doing B" },
                    { "content": "C", "status": "pending", "active_form": "Doing C" }
                ]
            }))
            .await
            .unwrap();

        let list = TaskListTool::new(store.clone());
        let out = list.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["summary"]["total"], 3);
        assert_eq!(out.value["summary"]["completed"], 1);
        assert_eq!(out.value["summary"]["in_progress"], 1);
        assert_eq!(out.value["summary"]["pending"], 1);
        // Stable order: same as written.
        let todos = out.value["todos"].as_array().unwrap();
        assert_eq!(todos[0]["content"], "A");
        assert_eq!(todos[2]["content"], "C");
    }

    #[tokio::test]
    async fn task_list_empty_store() {
        let list = TaskListTool::new(TodoStore::new());
        let out = list.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["summary"]["total"], 0);
    }

    // ----- Phase 3E: snapshot formatting + pending-flag state machine -----

    #[test]
    fn format_snapshot_empty() {
        let store = TodoStore::new();
        assert_eq!(store.format_snapshot(), "Current todo list is empty.");
    }

    #[test]
    fn format_snapshot_uses_active_form_for_in_progress_only() {
        let store = TodoStore::new();
        store.replace(vec![
            TodoItem {
                content: "Run tests".into(),
                status: TodoStatus::Pending,
                active_form: "Running tests".into(),
            },
            TodoItem {
                content: "Implement OAuth".into(),
                status: TodoStatus::InProgress,
                active_form: "Implementing OAuth".into(),
            },
            TodoItem {
                content: "Read the docs".into(),
                status: TodoStatus::Completed,
                active_form: "Reading the docs".into(),
            },
        ]);
        let snap = store.format_snapshot();
        // Header has all three counts.
        assert!(snap.contains("pending: 1"));
        assert!(snap.contains("in_progress: 1"));
        assert!(snap.contains("completed: 1"));
        // Pending uses content (imperative).
        assert!(snap.contains("[ ] Run tests"));
        // In-progress uses active_form (present-progressive).
        assert!(snap.contains("[~] Implementing OAuth"));
        assert!(!snap.contains("[~] Implement OAuth"));
        // Completed uses content (the imperative is what was checked off).
        assert!(snap.contains("[x] Read the docs"));
    }

    #[tokio::test]
    async fn todo_write_arms_pending_snapshot_flag() {
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        assert!(!store.is_snapshot_pending());
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "A", "status": "pending", "active_form": "Doing A" }]
        }))
        .await
        .unwrap();
        assert!(store.is_snapshot_pending());
    }

    #[test]
    fn take_pending_snapshot_is_one_shot() {
        let store = TodoStore::new();
        store.mark_snapshot_pending();
        assert!(store.take_pending_snapshot());
        // Second call returns false — the runtime decorator only injects once.
        assert!(!store.take_pending_snapshot());
    }

    #[test]
    fn consecutive_marks_then_one_take_only_yields_true_once() {
        // Phase 3E acceptance: consecutive todo_write calls before any
        // intervening tool only inject ONE snapshot reminder, with the
        // latest list. The flag is idempotent under repeated `mark`s, and
        // `take` consumes it exactly once.
        let store = TodoStore::new();
        store.mark_snapshot_pending();
        store.mark_snapshot_pending();
        store.mark_snapshot_pending();
        assert!(store.take_pending_snapshot());
        assert!(!store.take_pending_snapshot());
    }

    #[tokio::test]
    async fn snapshot_after_todo_write_reflects_latest_list() {
        // The snapshot rendered AFTER a series of todo_writes shows only the
        // last write — older lists are gone (replace is full snapshot).
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "Old", "status": "pending", "active_form": "Doing old" }]
        }))
        .await
        .unwrap();
        tool.invoke(serde_json::json!({
            "todos": [
                { "content": "New A", "status": "in_progress", "active_form": "Doing new A" },
                { "content": "New B", "status": "pending", "active_form": "Doing new B" },
            ]
        }))
        .await
        .unwrap();
        // Pending flag stays set across both calls.
        assert!(store.is_snapshot_pending());
        let snap = store.format_snapshot();
        // Old list is gone.
        assert!(!snap.contains("Old"));
        assert!(!snap.contains("Doing old"));
        // In-progress item renders via active_form (present-progressive).
        assert!(snap.contains("[~] Doing new A"));
        // Pending item renders via content (imperative).
        assert!(snap.contains("[ ] New B"));
    }
}
