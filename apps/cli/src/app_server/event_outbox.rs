//! Bounded delivery for one stdio connection. Sync runtime callbacks cannot
//! await channel capacity: overflow poisons the connection instead of hiding loss.
use super::{HarnessEvent, RpcNotification, JSONRPC_VERSION, NOTIFICATION_METHOD};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch, Mutex};

pub(super) struct QueuedEvent {
    pub(super) sequence: u64,
    pub(super) event: HarnessEvent,
}

pub(super) struct OutputState {
    failure: watch::Sender<Option<String>>,
    pub(super) delivered_sequence: AtomicU64,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            failure: watch::channel(None).0,
            delivered_sequence: AtomicU64::new(0),
        }
    }
}

impl OutputState {
    pub(super) fn failure_reason(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    pub(super) fn fail(&self, reason: String) {
        if self.failure.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(reason.clone());
            true
        }) {
            // Never log the dropped event payload (which can contain user data).
            eprintln!("app-server connection failed: {reason}; reconnect and use thread/read with durable cursors");
        }
    }

    pub(super) async fn failed(&self) -> String {
        let mut receiver = self.failure.subscribe();
        let reason = receiver
            .wait_for(Option::is_some)
            .await
            .expect("OutputState owns the failure sender");
        reason.clone().expect("wait_for requires a failure")
    }

    pub(super) async fn while_healthy<T>(
        &self,
        operation: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        tokio::select! {
            biased;
            reason = self.failed() => Err(reason),
            result = operation => result,
        }
    }
}

pub(super) struct EventOutbox {
    tx: mpsc::Sender<QueuedEvent>,
    next_sequence: AtomicU64,
    pub(super) output: Arc<OutputState>,
}

impl EventOutbox {
    // Also used by deterministic tests without a writer, not a second queue.
    pub(super) fn channel(capacity: usize) -> (Arc<Self>, mpsc::Receiver<QueuedEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Arc::new(Self {
                tx,
                next_sequence: AtomicU64::new(1),
                output: Arc::new(OutputState::default()),
            }),
            rx,
        )
    }

    pub(super) fn new<W: AsyncWrite + Unpin + Send + 'static>(
        stdout: Arc<Mutex<W>>,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (outbox, rx) = Self::channel(256);
        let output = outbox.output.clone();
        let writer = tokio::spawn(async move {
            // The writer owns no queue Sender, so dropping all emitters closes
            // it naturally. A fault also interrupts a stalled lock/write/flush.
            if let Err(error) = output
                .while_healthy(Self::write_events(stdout, rx, &output))
                .await
            {
                output.fail(error);
            }
        });
        (outbox, writer)
    }

    async fn write_events<W: AsyncWrite + Unpin>(
        stdout: Arc<Mutex<W>>,
        mut rx: mpsc::Receiver<QueuedEvent>,
        output: &OutputState,
    ) -> Result<(), String> {
        while let Some(queued) = rx.recv().await {
            let notification = RpcNotification {
                jsonrpc: JSONRPC_VERSION.into(),
                method: NOTIFICATION_METHOD.into(),
                event_sequence: Some(queued.sequence),
                params: serde_json::to_value(queued.event)
                    .map_err(|error| format!("serialize app-server event: {error}"))?,
            };
            let line = serde_json::to_string(&notification)
                .map_err(|error| format!("serialize app-server notification: {error}"))?;
            let mut writer = stdout.lock().await;
            writer
                .write_all(format!("{line}\n").as_bytes())
                .await
                .map_err(|error| format!("write app-server notification: {error}"))?;
            writer
                .flush()
                .await
                .map_err(|error| format!("flush app-server notification: {error}"))?;
            output
                .delivered_sequence
                .fetch_max(queued.sequence, Ordering::Release);
        }
        Ok(())
    }

    pub(super) fn emit(&self, event: HarnessEvent) {
        if self.output.failure_reason().is_none() {
            self.enqueue(self.allocate_sequence(), event);
        }
    }

    // H-91 (concurrent allocation/enqueue order) remains a separate defect.
    pub(super) fn allocate_sequence(&self) -> u64 {
        self.next_sequence.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn enqueue(&self, sequence: u64, event: HarnessEvent) {
        if self.output.failure_reason().is_some() {
            return;
        }
        if let Err(error) = self.tx.try_send(QueuedEvent { sequence, event }) {
            let reason = match error {
                mpsc::error::TrySendError::Full(_) => "event outbox capacity exceeded",
                mpsc::error::TrySendError::Closed(_) => "event outbox writer closed",
            };
            self.output.fail(reason.into());
        }
    }
}
