//! Live runtime events for streaming a run to a UI (P1-C event stream).
//!
//! The runtime loop persists everything to the append-only event store for
//! replay; **this** is the complementary *push* path: a [`RuntimeEventSink`]
//! receives [`RuntimeEvent`]s as they happen so a desktop/web frontend can
//! render tokens, tool status, and thinking progress live — without polling the
//! database.
//!
//! Design notes:
//! - Events are a projection, not the source of truth. Dropping or missing an
//!   event never corrupts state (the event log remains authoritative).
//! - The sink is cheap and non-blocking. The default no-op sink means a run
//!   without a UI pays nothing.
//! - Channel-based sinks ([`ChannelSink`]) bridge to Tauri events / SSE / WS at
//!   the app layer without the runtime depending on any transport.

use serde::{Deserialize, Serialize};

/// A live event emitted during a run, mirroring the loop's phases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// The run started for a task.
    RunStarted {
        /// The task being run.
        task_id: String,
    },
    /// A new model turn began (step index).
    TurnStarted {
        /// 0-based step index.
        step: usize,
    },
    /// A Thinking Mode reasoning fragment (DeepSeek `reasoning_content`).
    ReasoningDelta {
        /// Incremental reasoning text.
        text: String,
    },
    /// A visible assistant content fragment (token stream).
    ContentDelta {
        /// Incremental visible text.
        text: String,
    },
    /// A tool call is about to run (after the BeforeToolUse gate allowed it).
    ToolStarted {
        /// Tool name.
        name: String,
        /// Correlation id for matching the completion.
        call_id: String,
        /// JSON arguments.
        arguments: serde_json::Value,
    },
    /// A tool finished (success or failure).
    ToolCompleted {
        /// Tool name.
        name: String,
        /// Correlation id.
        call_id: String,
        /// Whether it succeeded.
        ok: bool,
        /// JSON output (or error detail).
        output: serde_json::Value,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// A tool call was blocked by a hook (deny) or needs approval (ask).
    ToolBlocked {
        /// Tool name.
        name: String,
        /// Why it was blocked.
        reason: String,
        /// Whether this is an approval request (`true`) vs a hard deny.
        needs_approval: bool,
    },
    /// A post-completion verification step result.
    Verification {
        /// Whether verification passed.
        passed: bool,
        /// Short diagnosis / detail.
        detail: String,
    },
    /// The run finished.
    RunCompleted {
        /// Final assistant message.
        message: String,
    },
    /// The run is waiting for human approval and yielded.
    RunAwaitingApproval {
        /// What needs approval.
        message: String,
    },
    /// The run failed / hit the step limit.
    RunFailed {
        /// Failure reason.
        reason: String,
    },
}

impl RuntimeEvent {
    /// A stable label for tracing / filtering.
    pub fn label(&self) -> &'static str {
        match self {
            RuntimeEvent::RunStarted { .. } => "run_started",
            RuntimeEvent::TurnStarted { .. } => "turn_started",
            RuntimeEvent::ReasoningDelta { .. } => "reasoning_delta",
            RuntimeEvent::ContentDelta { .. } => "content_delta",
            RuntimeEvent::ToolStarted { .. } => "tool_started",
            RuntimeEvent::ToolCompleted { .. } => "tool_completed",
            RuntimeEvent::ToolBlocked { .. } => "tool_blocked",
            RuntimeEvent::Verification { .. } => "verification",
            RuntimeEvent::RunCompleted { .. } => "run_completed",
            RuntimeEvent::RunAwaitingApproval { .. } => "run_awaiting_approval",
            RuntimeEvent::RunFailed { .. } => "run_failed",
        }
    }

    /// Whether this event is terminal (the run will emit no more events).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RuntimeEvent::RunCompleted { .. }
                | RuntimeEvent::RunAwaitingApproval { .. }
                | RuntimeEvent::RunFailed { .. }
        )
    }
}

/// Receives [`RuntimeEvent`]s as a run progresses.
///
/// Implementations must be cheap and non-blocking (e.g. push onto a channel).
/// `emit` takes `&self` so the sink can be shared behind an `Arc` across the
/// async loop without `&mut` plumbing.
pub trait RuntimeEventSink: Send + Sync {
    /// Handle one event. Must not block.
    fn emit(&self, event: RuntimeEvent);
}

/// A sink that discards events (the default when no UI is attached).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEventSink;

impl RuntimeEventSink for NullEventSink {
    fn emit(&self, _event: RuntimeEvent) {}
}

/// A sink that forwards events onto an unbounded channel. Pair the returned
/// receiver with a Tauri-event / SSE / WS bridge at the app layer.
pub struct ChannelSink {
    tx: tokio::sync::mpsc::UnboundedSender<RuntimeEvent>,
}

impl ChannelSink {
    /// Create a sink + its receiver.
    pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl RuntimeEventSink for ChannelSink {
    fn emit(&self, event: RuntimeEvent) {
        // A closed receiver just means the UI went away; dropping is fine since
        // the event log remains the source of truth.
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_terminality() {
        assert_eq!(
            RuntimeEvent::ContentDelta { text: "x".into() }.label(),
            "content_delta"
        );
        assert!(RuntimeEvent::RunCompleted {
            message: "m".into()
        }
        .is_terminal());
        assert!(!RuntimeEvent::TurnStarted { step: 0 }.is_terminal());
    }

    #[test]
    fn null_sink_is_noop() {
        let sink = NullEventSink;
        sink.emit(RuntimeEvent::TurnStarted { step: 0 }); // must not panic
    }

    #[tokio::test]
    async fn channel_sink_forwards_events() {
        let (sink, mut rx) = ChannelSink::new();
        sink.emit(RuntimeEvent::RunStarted {
            task_id: "t1".into(),
        });
        sink.emit(RuntimeEvent::ContentDelta { text: "Hi".into() });
        sink.emit(RuntimeEvent::RunCompleted {
            message: "done".into(),
        });
        drop(sink);

        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev.label().to_string());
        }
        assert_eq!(got, vec!["run_started", "content_delta", "run_completed"]);
    }

    #[test]
    fn serde_roundtrip_tagged() {
        let ev = RuntimeEvent::ToolStarted {
            name: "read_file".into(),
            call_id: "c1".into(),
            arguments: serde_json::json!({"path": "a.rs"}),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "tool_started");
        assert_eq!(json["name"], "read_file");
        let back: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }
}
