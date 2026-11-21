use deepagent_ssh::SshService;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub(crate) struct SshLifecycle {
    service: Arc<SshService>,
    stop: CancellationToken,
    result: Mutex<Option<Result<usize, String>>>,
}

impl SshLifecycle {
    pub(crate) fn new(service: Arc<SshService>) -> Self {
        Self {
            service,
            stop: CancellationToken::new(),
            result: Mutex::new(None),
        }
    }

    pub(crate) fn stop_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub(crate) fn shutdown(&self) -> Result<usize, String> {
        self.stop.cancel();
        let mut result = self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(result) = &*result {
            return result.clone();
        }
        let outcome = tauri::async_runtime::block_on(self.service.shutdown())
            .map_err(|error| error.to_string());
        *result = Some(outcome.clone());
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_is_idempotent_and_stops_the_monitor_token() {
        let data = tempfile::tempdir().unwrap();
        let lifecycle = SshLifecycle::new(Arc::new(SshService::new(data.path().into())));
        let stop = lifecycle.stop_token();

        assert_eq!(lifecycle.shutdown().unwrap(), 0);
        assert!(stop.is_cancelled());
        assert_eq!(lifecycle.shutdown().unwrap(), 0);
    }
}
