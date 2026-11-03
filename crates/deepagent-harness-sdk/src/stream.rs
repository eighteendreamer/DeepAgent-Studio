//! Request-owned views over the existing bounded live event channel.
use deepagent_harness_protocol::{HarnessEvent, RpcResponse};
use tokio::sync::{broadcast, watch};

use crate::SdkError;

enum Scope {
    Thread(String),
    Turn { thread: String, turn: String },
}

/// Events for one admitted thread/start or turn/start request.
///
/// The receiver exists before the request is sent, so consuming it after the
/// response cannot lose an early notification. It is bounded by the existing
/// broadcast channel (256 records in a spawned process). Lag/EOF are errors,
/// not successful completion or permission to redispatch. Reconcile using
/// `HarnessClient::thread_read` and durable cursors before considering a retry.
/// Dropping/closing this view only unsubscribes locally; it does not interrupt
/// the server turn. Use `turn_interrupt` for explicit remote cancellation.
pub struct RequestEvents {
    response: RpcResponse,
    receiver: Option<broadcast::Receiver<HarnessEvent>>,
    closed: watch::Receiver<bool>,
    scope: Scope,
}

impl RequestEvents {
    pub(crate) fn thread(
        response: RpcResponse,
        receiver: broadcast::Receiver<HarnessEvent>,
        closed: watch::Receiver<bool>,
    ) -> Result<Self, SdkError> {
        let thread = identity(&response, "thread/start", "threadId")?;
        Ok(Self {
            response,
            receiver: Some(receiver),
            closed,
            scope: Scope::Thread(thread),
        })
    }

    pub(crate) fn turn(
        response: RpcResponse,
        receiver: broadcast::Receiver<HarnessEvent>,
        closed: watch::Receiver<bool>,
        requested_thread: &str,
    ) -> Result<Self, SdkError> {
        let thread = identity(&response, "turn/start", "threadId")?;
        let turn = identity(&response, "turn/start", "turnId")?;
        if thread != requested_thread {
            return Err(SdkError::InvalidStreamIdentity {
                method: "turn/start",
                field: "threadId",
            });
        }
        Ok(Self {
            response,
            receiver: Some(receiver),
            closed,
            scope: Scope::Turn { thread, turn },
        })
    }

    /// The original successful admission response, not a run completion receipt.
    pub fn response(&self) -> &RpcResponse {
        &self.response
    }

    /// The thread identified by this request's response.
    pub fn thread_id(&self) -> &str {
        match &self.scope {
            Scope::Thread(thread) | Scope::Turn { thread, .. } => thread,
        }
    }

    /// The admitted turn, or `None` for a thread/start view.
    pub fn turn_id(&self) -> Option<&str> {
        match &self.scope {
            Scope::Thread(_) => None,
            Scope::Turn { turn, .. } => Some(turn),
        }
    }

    /// Release the local subscription without changing remote execution.
    pub fn close(&mut self) {
        self.receiver.take();
    }

    /// Receive the next matching event, ending after the first matching terminal.
    ///
    /// Thread/start ends at its `thread.started`. Turn/start includes only its
    /// own turn and ends at completed/failed/interrupted. v1 permits an absent
    /// threadId on turn events; a conflicting explicit thread never matches.
    /// Cancelling this receive future does not consume an event or drop the view.
    pub async fn recv(&mut self) -> Result<Option<HarnessEvent>, SdkError> {
        loop {
            let Some(receiver) = self.receiver.as_mut() else {
                return Ok(None);
            };
            // Drain already delivered events first: a terminal before EOF wins.
            let event = match receiver.try_recv() {
                Ok(event) => Ok(event),
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    Err(SdkError::EventLagged { skipped })
                }
                Err(broadcast::error::TryRecvError::Closed) => Err(SdkError::EventStreamClosed),
                Err(broadcast::error::TryRecvError::Empty) => {
                    if *self.closed.borrow() {
                        return self.closed_error();
                    }
                    tokio::select! {
                        event = receiver.recv() => event.map_err(|error| match error {
                            broadcast::error::RecvError::Lagged(skipped) => SdkError::EventLagged { skipped },
                            broadcast::error::RecvError::Closed => SdkError::EventStreamClosed,
                        }),
                        changed = self.closed.changed() => {
                            if changed.is_err() { return self.closed_error(); }
                            continue;
                        },
                    }
                }
            };
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    self.close();
                    return Err(error);
                }
            };
            if !self.matches(&event) {
                continue;
            }
            if matches!(
                event,
                HarnessEvent::ThreadStarted { .. }
                    | HarnessEvent::TurnCompleted { .. }
                    | HarnessEvent::TurnFailed { .. }
                    | HarnessEvent::TurnInterrupted { .. }
            ) {
                self.close();
            }
            return Ok(Some(event));
        }
    }

    fn closed_error(&mut self) -> Result<Option<HarnessEvent>, SdkError> {
        self.close();
        Err(SdkError::EventStreamClosed)
    }

    fn matches(&self, event: &HarnessEvent) -> bool {
        match (&self.scope, event) {
            (Scope::Thread(expected), HarnessEvent::ThreadStarted { thread_id, .. }) => {
                thread_id == expected
            }
            (
                Scope::Turn { thread, turn },
                HarnessEvent::TurnStarted {
                    thread_id, turn_id, ..
                },
            ) => !matches!(thread_id, Some(id) if id != thread) && turn_id == turn,
            (
                Scope::Turn { thread, turn },
                HarnessEvent::ItemStarted {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::ItemUpdated {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::ItemCompleted {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::ApprovalRequested {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::TurnCompleted {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::TurnFailed {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::TurnInterrupted {
                    thread_id, turn_id, ..
                }
                | HarnessEvent::McpLifecycle {
                    thread_id, turn_id, ..
                },
            ) => !matches!(thread_id, Some(id) if id != thread) && turn_id.as_ref() == Some(turn),
            _ => false,
        }
    }
}

fn identity(
    response: &RpcResponse,
    method: &'static str,
    field: &'static str,
) -> Result<String, SdkError> {
    response
        .result
        .as_ref()
        .and_then(|result| result.get(field))
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or(SdkError::InvalidStreamIdentity { method, field })
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
