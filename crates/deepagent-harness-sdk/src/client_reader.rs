//! The sole notification producer. Client handles cannot prolong its lifetime.
use super::Pending;
use crate::connection::{ConnectionState, DecodeFailure};
use crate::transport::{decode_server_line, notification_event, ServerLineError, WireMessage};
use crate::HarnessNotification;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStderr, ChildStdout},
    sync::{broadcast, watch},
    task::JoinHandle,
};
use tracing::debug;

/// Dropping the reader, including before its first poll or during unwinding,
/// closes the event producer and settles pending RPCs. It never invents a run
/// terminal or kills/restarts the server. The stderr drainer has the same owner.
struct ReaderOwner {
    events: Option<broadcast::Sender<HarnessNotification>>,
    pending: Pending,
    lifecycle: watch::Sender<ConnectionState>,
    stderr: JoinHandle<()>,
}

impl Drop for ReaderOwner {
    fn drop(&mut self) {
        drop(self.events.take());
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        ConnectionState::retire(&self.lifecycle, ConnectionState::Closed);
        self.stderr.abort();
    }
}

pub(super) fn spawn_reader(
    stdout: ChildStdout,
    stderr: ChildStderr,
    pending: Pending,
    events: broadcast::Sender<HarnessNotification>,
) -> watch::Sender<ConnectionState> {
    let (lifecycle, _) = watch::channel(ConnectionState::Open);
    let stderr = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    debug!(target: "harness_sdk::stderr", line = %line.trim(), "server stderr")
                }
            }
        }
    });
    let owner = ReaderOwner {
        events: Some(events),
        pending,
        lifecycle: lifecycle.clone(),
        stderr,
    };
    let mut closed = lifecycle.subscribe();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut sequences = crate::notification::SequenceTracker::default();
        loop {
            let line = tokio::select! {
                biased;
                // Do not retain a watch read lock when dropping the owner:
                // its teardown also publishes the final closed state.
                _ = async { drop(closed.wait_for(|closed| closed.is_closed()).await); } => break,
                line = lines.next_line() => line,
            };
            let line = match line {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(error) => {
                    debug!(target: "harness_sdk::read", kind = ?error.kind(), "server stdout read failed");
                    break;
                }
            };
            match decode_server_line(&line) {
                Ok(WireMessage::Response(response)) => {
                    let id = response.id.as_i64().unwrap_or_default();
                    let reply = owner
                        .pending
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&id);
                    if let Some(reply) = reply {
                        let _ = reply.send(response);
                    }
                }
                Ok(WireMessage::Notification(notification)) => {
                    match notification_event(notification) {
                        Ok(mut event) => {
                            event.sequence_status = sequences.observe(event.event_sequence);
                            let _ = owner
                                .events
                                .as_ref()
                                .expect("reader owns sender")
                                .send(event);
                        }
                        Err(reason) => {
                            debug!(target: "harness_sdk::notification", reason, "ignored server notification")
                        }
                    }
                }
                Err(ServerLineError::Version(reason)) => {
                    debug!(target: "harness_sdk::decode", reason, "ignored unsupported protocol version");
                }
                Err(ServerLineError::Decode(error)) => {
                    let failure = DecodeFailure::from_error(&error);
                    ConnectionState::retire(
                        &owner.lifecycle,
                        ConnectionState::DecodeFailed(failure),
                    );
                    debug!(target: "harness_sdk::decode", ?failure, "invalid stdout retired connection");
                    break;
                }
            }
        }
    });
    lifecycle
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[tokio::test]
    async fn h256_reader_owner_drop_closes_events_pending_and_stderr_without_a_client_drop() {
        let (sender, mut events) = broadcast::channel(2);
        let weak = sender.downgrade();
        let (lifecycle, _) = watch::channel(crate::connection::ConnectionState::Open);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (reply, received) = tokio::sync::oneshot::channel();
        pending.lock().unwrap().insert(1, reply);
        let stderr = tokio::spawn(std::future::pending());
        let stderr_abort = stderr.abort_handle();
        let owner = ReaderOwner {
            events: Some(sender),
            pending: pending.clone(),
            lifecycle: lifecycle.clone(),
            stderr,
        };
        // An unpolled cancelled task still drops the captured owner.
        let task = tokio::spawn(async move {
            let _owner = owner;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(received.await.is_err());
        assert!(pending.lock().unwrap().is_empty());
        assert!(lifecycle.borrow().is_closed());
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
        assert!(weak.upgrade().is_none());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !stderr_abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
