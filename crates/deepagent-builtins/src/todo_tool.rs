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
#[derive(Debug, Clone, Default)]
pub struct TodoStore {
    items: Arc<Mutex<Vec<TodoItem>>>,
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
        let active_form = raw
            .get("active_form")
            .or_else(|| raw.get("activeForm"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| content.clone());
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
            description: "Replace the session task list with a full snapshot. Use to plan and \
                track multi-step work; keep exactly one item in_progress. Args: { todos: \
                [{ content, status, active_form }] }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "active_form": { "type": "string" }
                            },
                            "required": ["content", "status"]
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

        // Soft guard: warn (but don't fail) if more than one item is in_progress,
        // mirroring Claude Code's "exactly one in_progress" guidance.
        let in_progress = items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .count();

        self.store.replace(items.clone());
        let (pending, running, completed) = self.store.counts();

        let mut value = serde_json::json!({
            "todos": items,
            "summary": {
                "total": items.len(),
                "pending": pending,
                "in_progress": running,
                "completed": completed,
            }
        });
        if in_progress > 1 {
            value["warning"] = serde_json::Value::String(format!(
                "{in_progress} items are in_progress; prefer exactly one at a time"
            ));
        }
        Ok(ToolOutput::success(value))
    }
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
                    { "content": "Run tests", "status": "pending" }
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
        assert_eq!(snap[2].active_form, "Run tests"); // defaulted from content
    }

    #[tokio::test]
    async fn replace_is_full_snapshot_not_append() {
        let store = TodoStore::new();
        let tool = TodoWriteTool::new(store.clone());
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "A", "status": "pending" }]
        }))
        .await
        .unwrap();
        tool.invoke(serde_json::json!({
            "todos": [{ "content": "B", "status": "pending" }]
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
            .invoke(serde_json::json!({"todos": [{ "content": "x", "status": "nope" }]}))
            .await
            .unwrap();
        assert!(!bad_status.ok);

        let empty = tool
            .invoke(serde_json::json!({"todos": [{ "content": "  ", "status": "pending" }]}))
            .await
            .unwrap();
        assert!(!empty.ok);
    }

    #[tokio::test]
    async fn warns_on_multiple_in_progress() {
        let tool = TodoWriteTool::new(TodoStore::new());
        let out = tool
            .invoke(serde_json::json!({
                "todos": [
                    { "content": "A", "status": "in_progress" },
                    { "content": "B", "status": "in_progress" }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.value.get("warning").is_some());
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
                    { "content": "A", "status": "completed" },
                    { "content": "B", "status": "in_progress" },
                    { "content": "C", "status": "pending" }
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
}
