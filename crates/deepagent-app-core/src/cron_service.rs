//! Scheduled-prompt runtime (D2): CronBackend impl + tick-loop launcher.
//!
//! The backend owns the store and provides the fire→run bridge, spawning each
//! fired prompt as a background session via `ChatService::run_in_session`.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use deepagent_builtins::{CronBackend, CronTaskSummary};
use deepagent_core::error::Result;
use deepagent_runtime::schedule::{run_tick_loop, CronScheduler};

use crate::chat_service::ChatService;

/// Production backend: file-backed store + fire→run via ChatService.
pub struct CronService {
    scheduler: CronScheduler,
    chat: Arc<ChatService>,
}

impl CronService {
    pub fn new(workspace: &Path, chat: Arc<ChatService>) -> Self {
        Self {
            scheduler: CronScheduler::for_workspace(workspace),
            chat,
        }
    }

    /// Launch the tick loop. Each fired task spawns a background session (no
    /// continue_session, so every fire is a fresh session). The loop runs
    /// until `shutdown` fires or an error occurs.
    pub async fn run_tick_loop(self: Arc<Self>, shutdown: CancellationToken) -> Result<()> {
        let tick = std::time::Duration::from_secs(60);
        let clock = || {
            let offset = time::OffsetDateTime::now_local()
                .map(|local| local.offset())
                .unwrap_or(time::UtcOffset::UTC);
            let local = time::OffsetDateTime::now_utc().to_offset(offset);
            time::PrimitiveDateTime::new(local.date(), local.time())
        };

        let self_for_fire = self.clone();
        let fire = move |task: &deepagent_runtime::schedule::ScheduledTask| {
            let chat = self_for_fire.chat.clone();
            let prompt = task.prompt.clone();
            let task_id = task.id.clone();
            tokio::spawn(async move {
                tracing::info!(task_id = %task_id, "firing scheduled task");
                let result = chat
                    .run_in_session(
                        &prompt,
                        None, // fresh session every fire
                        None, // default env_mode
                        None, // no connection_id
                        vec![],
                        None,
                        false,
                        None,
                        |_event| {},    // fire-and-forget: no event collection
                        |_approval| {}, // auto-deny (scheduled tasks don't prompt)
                    )
                    .await;
                if let Err(e) = result {
                    tracing::warn!(task_id = %task_id, error = %e, "scheduled task run failed");
                }
            });
        };

        run_tick_loop(self.scheduler.clone(), tick, clock, fire, shutdown).await
    }
}

#[async_trait]
impl CronBackend for CronService {
    async fn create(&self, cron: String, prompt: String, recurring: bool) -> Result<String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let task = self
            .scheduler
            .store()
            .add(cron, prompt, recurring, now_ms)?;
        Ok(task.id)
    }

    async fn delete(&self, ids: Vec<String>) -> Result<usize> {
        self.scheduler.store().remove(&ids)
    }

    async fn list(&self) -> Result<Vec<CronTaskSummary>> {
        let tasks = self.scheduler.store().read()?;
        Ok(tasks
            .into_iter()
            .map(|task| {
                let recurring = task.is_recurring();
                CronTaskSummary {
                    id: task.id,
                    cron: task.cron,
                    prompt: task.prompt,
                    recurring,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    use crate::{ApprovalPolicy, MemorySecretStore, SettingsService};
    use deepagent_builtins::bash_tool::{CommandExecutor, CommandOutcome};
    use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
    use deepagent_persistence::{run_store::RunStore, Database};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    struct NeverExecutes(Arc<AtomicUsize>);
    #[async_trait]
    impl CommandExecutor for NeverExecutes {
        async fn run(&self, _command: &str, _cwd: &str) -> Result<CommandOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(deepagent_core::error::CoreError::other(
                "audit command must not execute",
            ))
        }
    }

    #[tokio::test]
    #[ignore = "audit: unattended cron must not wait for an approval callback that discards requests"]
    async fn audit_unattended_cron_does_not_leave_an_unreachable_approval() {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("sk-audit-placeholder").await.unwrap();
        settings
            .set_approval_policy(ApprovalPolicy::AlwaysAsk)
            .unwrap();
        let item = serde_json::json!({
            "type":"function_call", "id":"audit-cron-item", "call_id":"audit-cron-approval",
            "name":"bash", "arguments":r#"{"command":"npm install audit-no-execution"}"#
        });
        let transport = MockTransport::new([
            serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
            serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
            serde_json::json!({"type":"response.completed","response":{"status":"completed"}})
                .to_string(),
        ]);
        let commands = Arc::new(AtomicUsize::new(0));
        let chat = Arc::new(
            ChatService::new(db.clone(), settings, Arc::new(transport), root.path())
                .with_local_command_executor(Arc::new(NeverExecutes(commands.clone()))),
        );
        let cron = Arc::new(CronService::new(root.path(), chat.clone()));
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        cron.scheduler
            .store()
            .add(
                "* * * * *".into(),
                "Run the audit command.".into(),
                false,
                now_ms - 120_000,
            )
            .unwrap();
        let shutdown = CancellationToken::new();
        let runner = cron.clone();
        let stop = shutdown.clone();
        let tick = tokio::spawn(async move { runner.run_tick_loop(stop).await });
        let pending = chat.pending_approvals();
        tokio::time::timeout(Duration::from_secs(5), async {
            while pending.pending_count() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("scheduled run must reach the formal approval gate");
        shutdown.cancel();
        tick.await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (run_id, finished): (String, bool) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id, finished_at IS NOT NULL FROM runs ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let unreachable_pending = pending.pending_count();
        let task_removed = cron.list().await.unwrap().is_empty();
        assert!(chat.request_cancel(&run_id).unwrap().accepted);
        tokio::time::timeout(Duration::from_secs(3), async {
            while RunStore::new(&db)
                .get(&run_id)
                .unwrap()
                .unwrap()
                .finished_at
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("audit must clean up its scheduled run");
        eprintln!("cron_task_removed={task_removed} scheduler_stopped=true pending={unreachable_pending} run_finished_before_cancel={finished} command_calls={}", commands.load(Ordering::SeqCst));
        assert_eq!(commands.load(Ordering::SeqCst), 0);
        assert_eq!(
            unreachable_pending, 0,
            "unattended work must reject or expose approval; an empty notifier cannot answer it"
        );
    }

    #[tokio::test]
    #[ignore = "audit: scheduler shutdown must account for in-flight scheduled runs"]
    async fn audit_scheduler_shutdown_leaves_started_cron_run_active() {
        #[derive(Debug)]
        struct NotifyHangTransport {
            started: Arc<tokio::sync::Notify>,
            cancelled: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl HttpTransport for NotifyHangTransport {
            async fn stream(
                &self,
                _request: TransportRequest,
                _sink: &mut dyn EventSink,
            ) -> Result<()> {
                Err(deepagent_core::error::CoreError::other(
                    "requires cancellation-aware streaming",
                ))
            }

            async fn stream_cancelled(
                &self,
                _request: TransportRequest,
                _sink: &mut dyn EventSink,
                cancel: Arc<AtomicBool>,
            ) -> Result<()> {
                self.started.notify_one();
                while !cancel.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                self.cancelled.notify_one();
                Err(deepagent_core::error::CoreError::other("request cancelled"))
            }
        }

        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("sk-audit-placeholder").await.unwrap();

        let started = Arc::new(tokio::sync::Notify::new());
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let transport = Arc::new(NotifyHangTransport {
            started: started.clone(),
            cancelled: cancelled.clone(),
        });
        let chat = Arc::new(ChatService::new(
            db.clone(),
            settings,
            transport,
            root.path(),
        ));
        let cron = Arc::new(CronService::new(root.path(), chat.clone()));
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        cron.scheduler
            .store()
            .add(
                "* * * * *".into(),
                "Wait for the scheduled run to be cancelled.".into(),
                false,
                now_ms - 120_000,
            )
            .unwrap();

        let shutdown = CancellationToken::new();
        let runner = cron.clone();
        let stop = shutdown.clone();
        let tick = tokio::spawn(async move { runner.run_tick_loop(stop).await });
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("scheduled ChatService run must reach the model transport");

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), tick)
            .await
            .expect("scheduler loop should stop")
            .unwrap()
            .unwrap();

        let (run_id, finished): (String, bool) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id, finished_at IS NOT NULL FROM runs ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let scheduler_cancelled_run =
            tokio::time::timeout(Duration::from_millis(100), cancelled.notified())
                .await
                .is_ok();
        eprintln!(
            "scheduler_returned=true run_finished={finished} model_cancel_observed={scheduler_cancelled_run}"
        );

        assert!(!finished, "the held model response keeps the run active");
        assert!(
            !scheduler_cancelled_run,
            "scheduler shutdown must propagate to or await its dispatched work"
        );

        assert!(chat.request_cancel(&run_id).unwrap().accepted);
        tokio::time::timeout(Duration::from_secs(2), cancelled.notified())
            .await
            .expect("explicit run cancellation must reach the held model request");
        tokio::time::timeout(Duration::from_secs(3), async {
            while RunStore::new(&db)
                .get(&run_id)
                .unwrap()
                .unwrap()
                .finished_at
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("audit must clean up its scheduled run");
    }
}
