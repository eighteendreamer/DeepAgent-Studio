//! `cron_create` / `cron_delete` — scheduled prompts (D2).
//!
//! A scheduled task is a prompt plus a 5-field cron expression, persisted by
//! `deepagent-runtime::schedule` and fired by its tick loop. These tools are the
//! model-facing surface; the store/scheduler live behind a [`CronBackend`] the
//! host wires up, keeping `deepagent-builtins` runtime-agnostic (same pattern as
//! the knowledge/memory/task tools).

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// The `cron_create` tool name.
pub const CRON_CREATE_TOOL_NAME: &str = "cron_create";
/// The `cron_delete` tool name.
pub const CRON_DELETE_TOOL_NAME: &str = "cron_delete";

/// A scheduled task as reported back to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronTaskSummary {
    /// Short task id.
    pub id: String,
    /// The 5-field cron expression.
    pub cron: String,
    /// The prompt that will run.
    pub prompt: String,
    /// Whether the task repeats.
    pub recurring: bool,
}

/// Durable store + fire bridge behind the cron tools.
#[async_trait]
pub trait CronBackend: Send + Sync {
    /// Persist a scheduled task, returning its id.
    async fn create(&self, cron: String, prompt: String, recurring: bool) -> Result<String>;

    /// Remove scheduled tasks by id, returning how many were removed.
    async fn delete(&self, ids: Vec<String>) -> Result<usize>;

    /// List the currently scheduled tasks.
    async fn list(&self) -> Result<Vec<CronTaskSummary>>;
}

/// Delegate through a shared handle so `Arc<dyn CronBackend>` is itself a
/// [`CronBackend`] and can back the tools.
#[async_trait]
impl<T: CronBackend + ?Sized> CronBackend for std::sync::Arc<T> {
    async fn create(&self, cron: String, prompt: String, recurring: bool) -> Result<String> {
        (**self).create(cron, prompt, recurring).await
    }

    async fn delete(&self, ids: Vec<String>) -> Result<usize> {
        (**self).delete(ids).await
    }

    async fn list(&self) -> Result<Vec<CronTaskSummary>> {
        (**self).list().await
    }
}

/// A backend reporting scheduling is unavailable (headless default).
pub struct UnavailableCronBackend;

#[async_trait]
impl CronBackend for UnavailableCronBackend {
    async fn create(&self, _cron: String, _prompt: String, _recurring: bool) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "scheduled tasks are not configured in this environment",
        ))
    }

    async fn delete(&self, _ids: Vec<String>) -> Result<usize> {
        Err(deepagent_core::error::CoreError::other(
            "scheduled tasks are not configured in this environment",
        ))
    }

    async fn list(&self) -> Result<Vec<CronTaskSummary>> {
        Err(deepagent_core::error::CoreError::other(
            "scheduled tasks are not configured in this environment",
        ))
    }
}

/// The `cron_create` tool over a [`CronBackend`].
pub struct CronCreateTool<B: CronBackend> {
    backend: B,
}

impl<B: CronBackend> CronCreateTool<B> {
    /// Build the tool over `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CronBackend> Tool for CronCreateTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CRON_CREATE_TOOL_NAME.into(),
            description: "Schedule a prompt to run at a future time, using a standard 5-field \
                cron expression in local time (minute hour day-of-month month day-of-week). \
                Use recurring=false for a one-shot reminder (\"at 2:30pm today, check the \
                deploy\"), recurring=true for a repeating job (\"weekdays at 9am\"). A \
                recurring task auto-expires after 7 days. The prompt must be self-contained: \
                it runs later in a fresh session with none of this conversation's context."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "cron": {
                        "type": "string",
                        "description": "5-field cron in local time: \"M H DoM Mon DoW\" (e.g. \"0 9 * * 1-5\" = weekdays 9am, \"*/5 * * * *\" = every 5 minutes)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The self-contained prompt to run at each fire time."
                    },
                    "recurring": {
                        "type": "boolean",
                        "description": "true = fire on every cron match until deleted (auto-expires after 7 days); false = fire once then auto-delete. Default true."
                    }
                },
                "required": ["cron", "prompt"]
            }),
            // Schedules future autonomous work: same posture as spawning a
            // sub-agent.
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([
                Permission::ReadOnly,
                Permission::Subagent,
            ]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(cron) = args
            .get("cron")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|cron| !cron.is_empty())
        else {
            return Ok(ToolOutput::failure("missing non-empty 'cron'"));
        };
        let Some(prompt) = args
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
        else {
            return Ok(ToolOutput::failure("missing non-empty 'prompt'"));
        };
        let recurring = args
            .get("recurring")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        match self
            .backend
            .create(cron.to_string(), prompt.to_string(), recurring)
            .await
        {
            Ok(id) => Ok(ToolOutput::success(serde_json::json!({
                "id": id,
                "cron": cron,
                "recurring": recurring,
                "scheduled": true,
            }))),
            Err(error) => Ok(ToolOutput::failure(format!("cron_create failed: {error}"))),
        }
    }
}

/// The `cron_delete` tool over a [`CronBackend`]. Also lists tasks, so the model
/// can find an id without a third tool.
pub struct CronDeleteTool<B: CronBackend> {
    backend: B,
}

impl<B: CronBackend> CronDeleteTool<B> {
    /// Build the tool over `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CronBackend> Tool for CronDeleteTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CRON_DELETE_TOOL_NAME.into(),
            description: "Cancel scheduled tasks created with cron_create, or list them. \
                Pass operation=\"list\" to see every scheduled task with its id, or \
                operation=\"delete\" with task_ids to cancel specific ones."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["delete", "list"],
                        "description": "\"delete\" cancels the given task_ids; \"list\" returns all scheduled tasks. Default \"delete\"."
                    },
                    "task_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Ids to cancel (required for operation=delete)."
                    }
                }
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let operation = args
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|operation| !operation.is_empty())
            .unwrap_or("delete");
        match operation {
            "list" => match self.backend.list().await {
                Ok(tasks) => Ok(ToolOutput::success(serde_json::json!({
                    "count": tasks.len(),
                    "tasks": tasks
                        .into_iter()
                        .map(|task| serde_json::json!({
                            "id": task.id,
                            "cron": task.cron,
                            "prompt": task.prompt,
                            "recurring": task.recurring,
                        }))
                        .collect::<Vec<_>>(),
                }))),
                Err(error) => Ok(ToolOutput::failure(format!("cron list failed: {error}"))),
            },
            "delete" => {
                let ids: Vec<String> = args
                    .get("task_ids")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|id| !id.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if ids.is_empty() {
                    return Ok(ToolOutput::failure(
                        "missing non-empty 'task_ids' for operation=delete",
                    ));
                }
                match self.backend.delete(ids).await {
                    Ok(removed) => Ok(ToolOutput::success(serde_json::json!({
                        "removed": removed,
                    }))),
                    Err(error) => Ok(ToolOutput::failure(format!("cron_delete failed: {error}"))),
                }
            }
            other => Ok(ToolOutput::failure(format!(
                "unknown operation '{other}'; expected \"delete\" or \"list\""
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct StubBackend {
        created: Mutex<Vec<(String, String, bool)>>,
        deleted: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CronBackend for StubBackend {
        async fn create(&self, cron: String, prompt: String, recurring: bool) -> Result<String> {
            self.created.lock().unwrap().push((cron, prompt, recurring));
            Ok("abcd1234".to_string())
        }

        async fn delete(&self, ids: Vec<String>) -> Result<usize> {
            let n = ids.len();
            self.deleted.lock().unwrap().extend(ids);
            Ok(n)
        }

        async fn list(&self) -> Result<Vec<CronTaskSummary>> {
            Ok(vec![CronTaskSummary {
                id: "abcd1234".into(),
                cron: "0 9 * * *".into(),
                prompt: "standup".into(),
                recurring: true,
            }])
        }
    }

    #[tokio::test]
    async fn create_schedules_and_defaults_to_recurring() {
        let tool = CronCreateTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({ "cron": "0 9 * * 1-5", "prompt": "run the smoke test" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["id"], "abcd1234");
        assert_eq!(out.value["recurring"], true);
    }

    #[tokio::test]
    async fn create_honors_one_shot() {
        let backend = std::sync::Arc::new(StubBackend::default());
        let tool = CronCreateTool::new(backend.clone());
        let out = tool
            .invoke(serde_json::json!({
                "cron": "30 14 28 2 *",
                "prompt": "check the deploy",
                "recurring": false
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["recurring"], false);
        assert_eq!(backend.created.lock().unwrap()[0].2, false);
    }

    #[tokio::test]
    async fn create_requires_cron_and_prompt() {
        let tool = CronCreateTool::new(StubBackend::default());
        assert!(
            !tool
                .invoke(serde_json::json!({ "prompt": "x" }))
                .await
                .unwrap()
                .ok
        );
        assert!(
            !tool
                .invoke(serde_json::json!({ "cron": "0 9 * * *", "prompt": "  " }))
                .await
                .unwrap()
                .ok
        );
    }

    #[tokio::test]
    async fn delete_removes_given_ids() {
        let backend = std::sync::Arc::new(StubBackend::default());
        let tool = CronDeleteTool::new(backend.clone());
        let out = tool
            .invoke(serde_json::json!({ "task_ids": ["abcd1234"] }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["removed"], 1);
        assert_eq!(backend.deleted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_requires_ids() {
        let tool = CronDeleteTool::new(StubBackend::default());
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn list_returns_tasks() {
        let tool = CronDeleteTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({ "operation": "list" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["count"], 1);
        assert_eq!(out.value["tasks"][0]["id"], "abcd1234");
    }

    #[tokio::test]
    async fn unavailable_backend_reports_failure() {
        let tool = CronCreateTool::new(UnavailableCronBackend);
        let out = tool
            .invoke(serde_json::json!({ "cron": "0 9 * * *", "prompt": "x" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
