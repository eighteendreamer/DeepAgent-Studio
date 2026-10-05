//! The append-only event model.
//!
//! Per 开发提示词.md §3 ("Append-Only Event Store") and §22 ("SAVE EVENTS"),
//! every meaningful action in the runtime is recorded as an immutable [`Event`].
//! The event stream is the source of truth: sessions are *replayed* from it,
//! which is what gives the runtime its **Replayability** and crash-recovery
//! properties.
//!
//! An [`Event`] is an envelope (id, session, monotonically increasing sequence
//! number, timestamp) wrapping a typed [`EventPayload`].

use serde::{Deserialize, Serialize};

use crate::clock::Timestamp;
use crate::id::{EventId, SessionId, TaskId};
use crate::message::{Message, ToolCall};
use crate::response_item::ResponseItem;
use crate::session_mode::SessionMode;
use crate::task::TaskState;

/// Monotonic per-session sequence number. The first event in a session has
/// sequence `0`; the store enforces no gaps and no duplicates.
pub type Sequence = u64;

/// An immutable, append-only event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Globally unique id.
    pub id: EventId,
    /// The session this event belongs to.
    pub session_id: SessionId,
    /// Position within the session's stream (0-based, gapless).
    pub sequence: Sequence,
    /// When the event was recorded.
    pub timestamp: Timestamp,
    /// The typed payload.
    pub payload: EventPayload,
}

impl Event {
    /// A short, stable discriminant string for the payload (used for
    /// indexing / filtering without deserializing the whole payload).
    pub fn kind(&self) -> &'static str {
        self.payload.kind()
    }
}

/// The typed content of an [`Event`].
///
/// New variants may be added over time; persisted events are forward-compatible
/// because they are stored as tagged JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventPayload {
    /// A new session was created.
    SessionStarted {
        /// Optional human-friendly title.
        title: Option<String>,
        /// The run mode of the session (defaults to Normal for old events).
        #[serde(default)]
        mode: SessionMode,
    },

    /// A session was closed (graceful end).
    SessionEnded {
        /// Optional reason / summary.
        reason: Option<String>,
    },

    /// A task was created within the session.
    TaskCreated {
        /// The new task.
        task_id: TaskId,
        /// The user-provided goal for the task.
        goal: String,
    },

    /// A task changed state.
    TaskStateChanged {
        /// Which task.
        task_id: TaskId,
        /// Previous state.
        from: TaskState,
        /// New state.
        to: TaskState,
    },

    /// A conversation message was appended.
    MessageAppended {
        /// The message.
        message: Message,
    },

    /// Provider-neutral Responses item persisted for exact model-context
    /// recovery. UI projections continue to use `MessageAppended`.
    ResponseItemAppended { item: ResponseItem },

    /// The model requested a tool invocation.
    ToolCallRequested {
        /// The requested call.
        call: ToolCall,
    },

    /// A tool finished executing.
    ToolCallCompleted {
        /// Correlates to [`ToolCall::id`].
        call_id: String,
        /// Whether the tool succeeded.
        ok: bool,
        /// JSON-encoded output or error detail.
        output: serde_json::Value,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },

    /// The context pipeline performed a compaction step (开发提示词.md §4).
    ///
    /// Two meanings share this variant, distinguished by `summary`:
    /// - `summary: None` — a metrics-only record (what older builds wrote, and
    ///   what micro-compaction still writes). Replay is unaffected.
    /// - `summary: Some` — a **durable boundary**: the model-facing view starts
    ///   here, with `summary` standing in for everything before
    ///   `first_kept_sequence`. Raw events are never rewritten or deleted, so
    ///   the full transcript stays replayable and exportable; only the view the
    ///   model sees is cut. This is how Claude Code does it
    ///   (`compact_boundary` record + `getMessagesAfterCompactBoundary`).
    ContextCompacted {
        /// Tokens before compaction.
        tokens_before: u64,
        /// Tokens after compaction.
        tokens_after: u64,
        /// Strategy label, e.g. "task_summary" / "memory_unload".
        strategy: String,
        /// Replacement text for the compacted prefix; `Some` marks a boundary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// First session-event sequence still visible to the model. Absent on a
        /// boundary means "keep nothing before the summary".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_kept_sequence: Option<Sequence>,
    },

    /// Earlier conversation segments were snipped (model-invoked history-snip).
    /// Records the `[id:uN]` tags of the removed segments so history rebuild
    /// after a restart re-applies the snip (Claude Code `removedUuids` parity).
    ContextSnipped {
        /// The snipped segment tags (e.g. `["u3", "u5"]`).
        #[serde(default)]
        tags: Vec<String>,
    },

    /// The kernel run reached a terminal state. Projected into the session
    /// stream so replaying the session log alone reconstructs the run terminal
    /// (the `runs`/`run_events` tables remain the run-local authority; this
    /// closes the two-log gap).
    RunTerminal {
        /// Terminal kind label (e.g. `succeeded` / `cancelled` / `max_turns`).
        kind: String,
        /// Optional reason / message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// A free-form note / diagnostic recorded into the stream.
    Note {
        /// The note text.
        text: String,
    },

    /// Token usage + wall-clock duration for a completed run, persisted so the
    /// UI can show per-turn metrics when a session is reopened.
    UsageRecorded {
        /// Prompt (input) tokens.
        prompt_tokens: u32,
        /// Completion (output) tokens.
        completion_tokens: u32,
        /// Reasoning output tokens (already included in completion tokens).
        #[serde(default)]
        reasoning_tokens: u32,
        /// Total tokens.
        total_tokens: u32,
        /// Prompt tokens served from the context cache (a "hit").
        prompt_cache_hit_tokens: u32,
        /// Prompt tokens NOT served from cache (a "miss").
        prompt_cache_miss_tokens: u32,
        /// Wall-clock duration of the run in milliseconds.
        duration_ms: u64,
        /// Raw provider Responses usage object(s), one per model call that
        /// returned usage. Persisted for diagnostics/future billing without
        /// exposing prompt text or credentials.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_responses_usage: Option<serde_json::Value>,
    },

    /// One or more deferred tools were discovered via the `tool_search`
    /// built-in (tool-search spec, Phase 3C). Persisted append-only so a
    /// session that's resumed can rebuild its active tool set without the
    /// model re-issuing `tool_search` for tools it already has access to.
    ///
    /// The payload carries only the **delta** (names newly added in this
    /// turn), not the full set; the resume path accumulates across all
    /// `ToolsDiscovered` events to reconstruct the cumulative state.
    ToolsDiscovered {
        /// Tool names added to the active set in this turn.
        names: Vec<String>,
    },

    /// MCP server `instructions` were (re)announced to the model. Persisted
    /// as a delta so a resumed session rebuilds which servers have already had
    /// their instructions surfaced — the MCP-instructions counterpart to
    /// `mcpInstructionsDelta.ts` in Claude Code, which announces each server's
    /// instructions once and only once per session instead of re-sending all
    /// of them every turn.
    ///
    /// The payload carries only the **delta**: `added` names whose instructions
    /// were newly surfaced this turn, plus `removed` names that were previously
    /// announced but are no longer connected. The resume path replays both to
    /// reconstruct the cumulative announced set.
    McpInstructionsDelta {
        /// Server names whose instructions were newly surfaced this turn.
        #[serde(default)]
        added: Vec<String>,
        /// Server names previously announced that are no longer connected.
        #[serde(default)]
        removed: Vec<String>,
    },

    /// Long-term memory entries were injected into the model context. The
    /// `relevant_memories` counterpart of [`EventPayload::ToolsDiscovered`]:
    /// persisting the **ids** makes the already-surfaced de-dup set derivable
    /// from the event stream, so a resumed or reopened session does not
    /// re-inject the same memories, and the UI can show what was added.
    MemoriesInjected {
        /// Ids of the entries newly surfaced (empty when only counts are known).
        #[serde(default)]
        ids: Vec<String>,
        /// How many distinct entries this injection added.
        count: usize,
        /// Prefetch-to-injection wall-clock latency in milliseconds.
        #[serde(default)]
        latency_ms: u64,
        /// What caused the injection (e.g. `prefetch`, `model_request`).
        #[serde(default)]
        trigger: String,
    },

    /// A cross-session memory entry was written to the knowledge vault.
    MemoryEntryWritten {
        /// Vault entry id (also the tool's returned handle).
        id: String,
        /// Memory tier label (`semantic` / `episodic` / ...).
        tier: String,
        /// Vault scope label (`global` / `project`).
        scope: String,
        /// Importance weight when the writer supplied one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        importance: Option<f32>,
    },
}

impl EventPayload {
    /// Stable discriminant string. Keep these in sync with the variant names;
    /// they are used for DB indexing and analytics.
    pub fn kind(&self) -> &'static str {
        match self {
            EventPayload::SessionStarted { .. } => "session_started",
            EventPayload::SessionEnded { .. } => "session_ended",
            EventPayload::TaskCreated { .. } => "task_created",
            EventPayload::TaskStateChanged { .. } => "task_state_changed",
            EventPayload::MessageAppended { .. } => "message_appended",
            EventPayload::ResponseItemAppended { .. } => "response_item_appended",
            EventPayload::ToolCallRequested { .. } => "tool_call_requested",
            EventPayload::ToolCallCompleted { .. } => "tool_call_completed",
            EventPayload::ContextCompacted { .. } => "context_compacted",
            EventPayload::ContextSnipped { .. } => "context_snipped",
            EventPayload::RunTerminal { .. } => "run_terminal",
            EventPayload::Note { .. } => "note",
            EventPayload::UsageRecorded { .. } => "usage_recorded",
            EventPayload::ToolsDiscovered { .. } => "tools_discovered",
            EventPayload::McpInstructionsDelta { .. } => "mcp_instructions_delta",
            EventPayload::MemoriesInjected { .. } => "memories_injected",
            EventPayload::MemoryEntryWritten { .. } => "memory_entry_written",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    #[test]
    fn memories_injected_round_trips_and_reports_its_kind() {
        let payload = EventPayload::MemoriesInjected {
            ids: vec![
                "project:build-fix".to_string(),
                "global:cargo-j1".to_string(),
            ],
            count: 2,
            latency_ms: 41,
            trigger: "prefetch".to_string(),
        };
        assert_eq!(payload.kind(), "memories_injected");
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "memories_injected");
        assert_eq!(json["count"], 2);
        let back: EventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn memory_entry_written_round_trips_and_omits_absent_importance() {
        let payload = EventPayload::MemoryEntryWritten {
            id: "m-1".to_string(),
            tier: "failure".to_string(),
            scope: "global".to_string(),
            importance: None,
        };
        assert_eq!(payload.kind(), "memory_entry_written");
        let json = serde_json::to_value(&payload).unwrap();
        assert!(
            json.get("importance").is_none(),
            "absent importance must not be serialized"
        );
        let back: EventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn memories_injected_deserializes_without_the_new_defaults() {
        // Forward compatibility: a payload written before `ids`/`trigger`
        // existed must still parse.
        let legacy = serde_json::json!({
            "type": "memories_injected",
            "count": 1,
            "latency_ms": 7,
        });
        let parsed: EventPayload = serde_json::from_value(legacy).unwrap();
        match parsed {
            EventPayload::MemoriesInjected {
                ids,
                count,
                latency_ms,
                trigger,
            } => {
                assert!(ids.is_empty());
                assert_eq!(count, 1);
                assert_eq!(latency_ms, 7);
                assert_eq!(trigger, "");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    fn sample_event(seq: Sequence) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::nil(),
            sequence: seq,
            timestamp: Timestamp::from_millis(1_748_492_921_000),
            payload: EventPayload::MessageAppended {
                message: Message::user("hello"),
            },
        }
    }

    #[test]
    fn kind_matches_variant() {
        let e = sample_event(0);
        assert_eq!(e.kind(), "message_appended");
    }

    #[test]
    fn payload_is_tagged_json() {
        let p = EventPayload::Note { text: "hi".into() };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"type\":\"note\""));
        let back: EventPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn event_roundtrips() {
        let e = sample_event(3);
        let json = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn tool_completed_carries_duration() {
        let p = EventPayload::ToolCallCompleted {
            call_id: "c1".into(),
            ok: true,
            output: serde_json::json!({"bytes": 10}),
            duration_ms: 42,
        };
        assert_eq!(p.kind(), "tool_call_completed");
    }
}
