//! Request-owned views over the existing bounded live event channel.
use deepagent_harness_protocol::{HarnessEvent, RpcResponse};
use tokio::sync::{broadcast, watch};

use crate::{HarnessNotification, SdkError, SequenceStatus};

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
    receiver: Option<broadcast::Receiver<HarnessNotification>>,
    closed: watch::Receiver<bool>,
    scope: Scope,
}

impl RequestEvents {
    pub(crate) fn thread(
        response: RpcResponse,
        receiver: broadcast::Receiver<HarnessNotification>,
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
        receiver: broadcast::Receiver<HarnessNotification>,
        closed: watch::Receiver<bool>,
        requested_thread: &str,
    ) -> Result<Self, SdkError> {
        let thread = identity(&response, "turn/start", "threadId")?;
        let turn = identity(&response, "turn/start", "turnId")?;
        deepagent_harness_protocol::control_result_identity(
            response.result.as_ref().unwrap_or(&serde_json::Value::Null),
            "turn/start",
            "threadId",
            Some(requested_thread),
        )
        .map_err(|_| SdkError::InvalidStreamIdentity {
            method: "turn/start",
            field: "threadId",
        })?;
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
        Ok(self
            .recv_notification()
            .await?
            .map(|notification| notification.event))
    }

    /// Receive the same scoped event with its exact wire sequence. Like `recv`,
    /// any connection gap/nonmonotonic sequence fails this view even when the
    /// offending event belongs to another scope. Never hide loss by filtering.
    /// The raw `HarnessClient::subscribe` view retains offending notifications.
    /// Unsequenced legacy events remain usable but cannot be safely ACKed by count.
    pub async fn recv_notification(&mut self) -> Result<Option<HarnessNotification>, SdkError> {
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
            let discontinuity = match event.sequence_status {
                SequenceStatus::Gap { previous, missing } => Some(SdkError::EventSequenceGap {
                    previous,
                    received: event.event_sequence.expect("reader gap has sequence"),
                    missing,
                }),
                SequenceStatus::NonMonotonic { previous } => {
                    Some(SdkError::EventSequenceNonMonotonic {
                        previous,
                        received: event
                            .event_sequence
                            .expect("reader nonmonotonic has sequence"),
                    })
                }
                _ => None,
            };
            if let Some(error) = discontinuity {
                self.close();
                return Err(error);
            }
            if !self.matches(&event.event) {
                continue;
            }
            if matches!(
                event.event,
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

    fn closed_error(&mut self) -> Result<Option<HarnessNotification>, SdkError> {
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
    deepagent_harness_protocol::control_result_identity(
        response.result.as_ref().unwrap_or(&serde_json::Value::Null),
        method,
        field,
        None,
    )
    .map(str::to_owned)
    .map_err(|_| SdkError::InvalidStreamIdentity { method, field })
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
