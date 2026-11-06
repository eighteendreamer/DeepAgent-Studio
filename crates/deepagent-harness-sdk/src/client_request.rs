//! Local ownership of one pending RPC and its frame. Never retries or cancels server work.
use super::Pending;
use crate::SdkError;
use deepagent_harness_protocol::RpcResponse;
use std::time::Duration;
use tokio::{
    io::AsyncWriteExt,
    process::ChildStdin,
    sync::{oneshot, watch, MutexGuard},
    time::Instant,
};

/// Finite local RPC wait limits, including writer-lock acquisition and frame writing.
/// These do not limit a running turn/event stream or cancel admitted server work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTimeouts {
    /// Handshake deadline; defaults to 30 seconds.
    pub initialize: Duration,
    /// All other request deadlines; defaults to 60 seconds.
    pub request: Duration,
}

impl Default for RequestTimeouts {
    fn default() -> Self {
        Self {
            initialize: Duration::from_secs(30),
            request: Duration::from_secs(60),
        }
    }
}

impl RequestTimeouts {
    pub(super) fn validate(self) -> Result<Self, SdkError> {
        Self::deadline(self.initialize)?;
        Self::deadline(self.request)?;
        Ok(self)
    }

    pub(super) fn deadline(duration: Duration) -> Result<Instant, SdkError> {
        if duration.is_zero() {
            return Err(SdkError::InvalidRequestTimeout);
        }
        Instant::now()
            .checked_add(duration)
            .ok_or(SdkError::InvalidRequestTimeout)
    }
}

/// The only local pending owner. Reader completion/EOF may already have removed
/// the sender; every caller exit (including drop while awaiting a lock) is idempotent.
pub(super) struct Registration<'a> {
    table: &'a Pending,
    id: i64,
}

impl<'a> Registration<'a> {
    pub fn new(table: &'a Pending, id: i64) -> (Self, oneshot::Receiver<RpcResponse>) {
        let (tx, rx) = oneshot::channel();
        table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, tx);
        (Self { table, id }, rx)
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id);
    }
}

/// write_all is not cancellation safe (Tokio 1.52.3). If a caller abandons a
/// partially written frame, retire this pipe instead of appending a later RPC
/// to its prefix. A queued caller owns no frame and never retires another writer.
pub(super) struct FrameWriter<'a> {
    stdin: MutexGuard<'a, Option<ChildStdin>>,
    table: &'a Pending,
    lifecycle: &'a watch::Sender<bool>,
    completed: bool,
}

impl<'a> FrameWriter<'a> {
    pub fn new(
        stdin: MutexGuard<'a, Option<ChildStdin>>,
        table: &'a Pending,
        lifecycle: &'a watch::Sender<bool>,
    ) -> Self {
        Self {
            stdin,
            table,
            lifecycle,
            completed: false,
        }
    }

    pub async fn send(mut self, line: &[u8]) -> Result<(), SdkError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| SdkError::Write(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))?;
        stdin.write_all(line).await.map_err(SdkError::Write)?;
        stdin.flush().await.map_err(SdkError::Write)?;
        self.completed = true;
        Ok(())
    }
}

impl Drop for FrameWriter<'_> {
    fn drop(&mut self) {
        if !self.completed {
            drop(self.stdin.take());
            self.lifecycle.send_replace(true);
            self.table
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
    }
}
