//! One owned startup per connection identity; view waiters do not own its lifetime.
use super::*;
use tokio::sync::watch;

#[derive(Clone)]
enum StartupState {
    Pending,
    Ready(Arc<McpRegistry>),
    Failed(String),
}

pub(super) struct ServerSlot {
    fingerprint: String,
    cancellation: deepagent_runtime::CancellationTree,
    state: watch::Sender<StartupState>,
}

impl ServerSlot {
    pub(super) fn start(
        name: String,
        cfg: McpServerConfig,
        fingerprint: String,
        semaphore: Arc<Semaphore>,
        #[cfg(test)] attempts: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Arc<Self> {
        let (state, _) = watch::channel(StartupState::Pending);
        let slot = Arc::new(Self {
            fingerprint,
            cancellation: Default::default(),
            state,
        });
        let cancellation = slot.cancellation.clone();
        let state = slot.state.clone();
        // Capture neither the slot nor the service: their last owner can drop
        // and cancel startup even while this task is queued or handshaking.
        tokio::spawn(async move {
            let startup = async {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|error| CoreError::other(error.to_string()))?;
                #[cfg(test)]
                attempts.fetch_add(1, Ordering::AcqRel);
                tokio::time::timeout(MCP_STARTUP_TIMEOUT, connect(&name, &cfg))
                    .await
                    .unwrap_or_else(|_| {
                        Err(CoreError::other(format!(
                            "MCP startup timed out after {} seconds",
                            MCP_STARTUP_TIMEOUT.as_secs()
                        )))
                    })
            };
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                result = startup => result,
            };
            if cancellation.is_cancelled() {
                return;
            }
            state.send_replace(match result {
                Ok(registry) => StartupState::Ready(registry),
                Err(error) => StartupState::Failed(error.to_string()),
            });
        });
        slot
    }

    pub(super) fn matches(&self, fingerprint: &str) -> bool {
        self.fingerprint == fingerprint && !self.cancellation.is_cancelled()
    }

    pub(super) fn reusable(&self, fingerprint: &str, retry_failed: bool) -> bool {
        self.matches(fingerprint)
            && !(retry_failed && matches!(*self.state.borrow(), StartupState::Failed(_)))
    }

    pub(super) fn result(&self) -> Option<Result<Arc<McpRegistry>>> {
        match self.state.borrow().clone() {
            StartupState::Pending => None,
            StartupState::Ready(registry) => Some(Ok(registry)),
            StartupState::Failed(error) => Some(Err(CoreError::other(error))),
        }
    }

    pub(super) fn fail(&self, error: String) {
        if !self.cancellation.is_cancelled() {
            self.state.send_replace(StartupState::Failed(error));
        }
    }

    pub(super) fn retire(&self) {
        self.cancellation.cancel();
    }

    pub(super) async fn wait(
        &self,
        caller: Option<deepagent_runtime::CancellationTree>,
    ) -> Result<Arc<McpRegistry>> {
        let caller = caller.unwrap_or_default();
        let mut changed = self.state.subscribe();
        loop {
            if caller.is_cancelled() || self.cancellation.is_cancelled() {
                return Err(CoreError::other("MCP preparation cancelled"));
            }
            if let Some(result) = self.result() {
                return result;
            }
            tokio::select! {
                biased;
                _ = caller.cancelled() => return Err(CoreError::other("MCP preparation cancelled")),
                _ = self.cancellation.cancelled() => return Err(CoreError::other("MCP preparation cancelled")),
                result = changed.changed() => result.map_err(|error| CoreError::other(format!("MCP startup state closed: {error}")))?,
            }
        }
    }
}

impl Drop for ServerSlot {
    fn drop(&mut self) {
        self.retire();
    }
}

async fn connect(name: &str, cfg: &McpServerConfig) -> Result<Arc<McpRegistry>> {
    let (client, instructions) = McpService::connect_one_resilient(name, cfg).await?;
    let mut registry = McpRegistry::new();
    if let Err(error) = registry
        .register_with_instructions(name, client.clone(), instructions)
        .await
    {
        let _ = client.close().await;
        return Err(error);
    }
    registry.enable_liveness(deepagent_mcp::LivenessConfig::default());
    Ok(Arc::new(registry))
}
