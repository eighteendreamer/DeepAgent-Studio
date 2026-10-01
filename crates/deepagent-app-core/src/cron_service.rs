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
