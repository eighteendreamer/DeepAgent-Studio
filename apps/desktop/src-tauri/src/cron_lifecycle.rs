//! Desktop's owner of the existing CronService loop, not another executor.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deepagent_app_core::CronService;
use deepagent_core::error::Result;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

pub(crate) fn on_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if let tauri::RunEvent::ExitRequested { .. } = event {
        if let Some(cron) = app.try_state::<CronLifecycle>() {
            if let Err(error) = cron.shutdown() {
                eprintln!("desktop exit has unresolved scheduled work: {error}");
            }
        }
    }
}

pub(crate) struct CronLifecycle {
    shutdown: CancellationToken,
    worker: Mutex<Worker>,
}

enum Worker {
    Running(tauri::async_runtime::JoinHandle<Result<()>>),
    Finished(std::result::Result<(), String>),
}

impl CronLifecycle {
    pub(crate) fn start(cron: Arc<CronService>) -> Self {
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let worker = tauri::async_runtime::spawn(async move {
            let result = cron.run_tick_loop(stop).await;
            if let Err(error) = &result {
                eprintln!("cron lifecycle failed: {error}");
            }
            result
        });
        Self {
            shutdown,
            worker: Mutex::new(Worker::Running(worker)),
        }
    }

    pub(crate) fn shutdown(&self) -> std::result::Result<(), String> {
        self.shutdown.cancel();
        let mut state = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        let worker = match &mut *state {
            Worker::Running(worker) => worker,
            Worker::Finished(result) => return result.clone(),
        };
        // ExitRequested is synchronous (including framework restart requests).
        // Keep its event loop alive until the existing 10s kernel drain returns,
        // plus one second for the loop's completion handoff. Never wait forever.
        let result = tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(11), worker)
                .await
                .map_err(|_| {
                    "desktop cron shutdown unresolved after 11s; worker was not aborted".to_string()
                })?
                .map_err(|error| format!("desktop cron worker failed: {error}"))?
                .map_err(|error| error.to_string())
        });
        // Repeated exit notifications must not turn an unresolved drain into
        // success merely because its JoinHandle has already been consumed.
        *state = Worker::Finished(result.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::error::CoreError;

    #[test]
    fn h97_repeated_exit_retains_the_original_drain_failure() {
        let lifecycle = CronLifecycle {
            shutdown: CancellationToken::new(),
            worker: Mutex::new(Worker::Running(tauri::async_runtime::spawn(async {
                Err(CoreError::other("H97 synthetic unresolved terminal"))
            }))),
        };
        let first = lifecycle.shutdown().unwrap_err();
        assert!(first.contains("H97 synthetic unresolved terminal"));
        assert!(lifecycle.shutdown.is_cancelled());
        assert_eq!(lifecycle.shutdown().unwrap_err(), first);
    }

    #[test]
    fn h97_panicked_loop_is_not_reported_as_clean_exit() {
        let lifecycle = CronLifecycle {
            shutdown: CancellationToken::new(),
            worker: Mutex::new(Worker::Running(tauri::async_runtime::spawn(async {
                panic!("H97 synthetic loop panic");
            }))),
        };
        let first = lifecycle.shutdown().unwrap_err();
        assert!(first.contains("desktop cron worker failed"));
        assert_eq!(lifecycle.shutdown().unwrap_err(), first);
    }
}

impl Drop for CronLifecycle {
    fn drop(&mut self) {
        // Startup failure/forced owner drop must at least route cancellation;
        // the ordinary app exit path above also waits for durable finalization.
        self.shutdown.cancel();
    }
}
