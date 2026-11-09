//! One live catalog per registry connection; runs borrow immutable revisions.
//!
//! A list_changed message is only an invalidation hint. Discovery reuses the
//! client's bounded pagination and the registry's schema/name admission. The
//! reader/relay never performs I/O, and notifications cannot grant permissions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use deepagent_core::id::EventId;
use deepagent_core::redaction::scrub_secret_literals;

use crate::client::McpClient;
use crate::registry::{validated_tools, RemoteTool};

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(100);
// A discovery deadline, independent of business-call and liveness policies.
// Matches the host's current startup discovery budget, not an MCP wire mandate.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct CatalogSnapshot {
    pub(crate) tools: Vec<RemoteTool>,
    pub(crate) generation: String,
    pub(crate) error: Option<String>,
}

pub(crate) struct ToolCatalog {
    current: Arc<RwLock<Arc<CatalogSnapshot>>>,
    stopped: Arc<AtomicBool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ToolCatalog {
    pub(crate) fn new(name: String, client: Arc<McpClient>, tools: Vec<RemoteTool>) -> Self {
        Self::start(name, client, tools, REFRESH_DEBOUNCE, REFRESH_TIMEOUT)
    }

    fn start(
        name: String,
        client: Arc<McpClient>,
        tools: Vec<RemoteTool>,
        debounce: Duration,
        timeout: Duration,
    ) -> Self {
        let current = Arc::new(RwLock::new(Arc::new(CatalogSnapshot {
            tools,
            generation: EventId::new().to_string(),
            error: None,
        })));
        let stopped = Arc::new(AtomicBool::new(false));
        let task = client.tool_list_changes().map(|changes| {
            tokio::spawn(refresh_catalog(
                name,
                client,
                changes,
                current.clone(),
                stopped.clone(),
                debounce,
                timeout,
            ))
        });
        Self {
            current,
            stopped,
            task: Mutex::new(task),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<CatalogSnapshot> {
        self.current
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(crate) fn stop(&self) {
        // Serialize invalidation with publication; even a completed stale
        // response cannot publish after this connection owner retires.
        let _publication = self.current.write().unwrap_or_else(|p| p.into_inner());
        self.stopped.store(true, Ordering::Release);
        if let Some(task) = self.task.lock().unwrap_or_else(|p| p.into_inner()).take() {
            task.abort();
        }
    }
}

impl Drop for ToolCatalog {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn refresh_catalog(
    name: String,
    client: Arc<McpClient>,
    mut changes: tokio::sync::watch::Receiver<u64>,
    current: Arc<RwLock<Arc<CatalogSnapshot>>>,
    stopped: Arc<AtomicBool>,
    debounce: Duration,
    timeout: Duration,
) {
    let mut seen = 0;
    loop {
        let observed = *changes.borrow_and_update();
        if observed == seen && changes.changed().await.is_err() {
            return;
        }
        // Constant-size watch storage coalesces bursts. A change during this
        // single in-flight list remains unseen and gets one trailing refresh.
        tokio::time::sleep(debounce).await;
        seen = *changes.borrow_and_update();
        let result = match tokio::time::timeout(timeout, client.list_tools()).await {
            Ok(result) => result.and_then(|defs| validated_tools(&name, defs)),
            Err(_) => Err(deepagent_core::error::CoreError::other(format!(
                "MCP catalog refresh timed out after {} seconds",
                timeout.as_secs()
            ))),
        };
        let (tools, error) = match result {
            Ok(tools) => (tools, None),
            Err(error) => {
                let error = scrub_secret_literals(&error.to_string());
                tracing::warn!(server = name, %error, "MCP catalog refresh failed; tools withdrawn until a later change or explicit retry");
                (Vec::new(), Some(error))
            }
        };
        let mut published = current.write().unwrap_or_else(|p| p.into_inner());
        if stopped.load(Ordering::Acquire) {
            return;
        }
        if published.tools != tools || published.error != error {
            tracing::info!(
                server = name,
                tool_count = tools.len(),
                available = error.is_none(),
                "MCP catalog revision published"
            );
            *published = Arc::new(CatalogSnapshot {
                tools,
                generation: EventId::new().to_string(),
                error,
            });
        }
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
