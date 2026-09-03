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

/// UI-oriented metadata for rendering a tool call as a lightweight timeline
/// row. This is a derived projection: the persisted event log remains the
/// source of truth, while live/replayed UI surfaces can use these fields
/// instead of re-parsing arbitrary JSON in the frontend.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolUiMetadata {
    /// Coarse presentation category, e.g. `file_read`, `file_change`,
    /// `search`, `command_execution`, or `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<String>,
    /// Primary path/URL/command target when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Compact one-line row text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Small structured hints for the frontend. Raw args/output are still
    /// carried separately; this is intentionally lightweight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// A live event emitted during a run, mirroring the loop's phases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// The run started for a task.
    RunStarted {
        /// The task being run.
        task_id: String,
    },
    /// The session backing this run is known (emitted early, before the model
    /// loop starts) so a UI can register and navigate to the session while it
    /// is still running — not only after it completes.
    SessionRegistered {
        /// The session id this run is recorded under.
        session_id: String,
        /// The session title (the originating prompt), if any.
        title: Option<String>,
    },
    /// A new model turn began (step index).
    TurnStarted {
        /// 0-based step index.
        step: usize,
    },
    /// A provider request is about to be sent. This is intentionally emitted
    /// immediately before the streaming call so UI/debug logs can separate
    /// local prompt assembly from provider first-token latency.
    ModelRequestStarted {
        /// 0-based turn/step index.
        step: usize,
        /// Target model id.
        model: String,
        /// User-facing thinking depth label (`simple` / `medium` / `deep`).
        thinking_depth: String,
        /// Number of messages in the request payload.
        message_count: usize,
        /// Number of tool schemas advertised in this request.
        tool_count: usize,
    },
    /// The first streamed model fragment arrived. `kind` is either
    /// `reasoning` or `content`, matching the first visible stream channel.
    ModelFirstToken {
        /// 0-based turn/step index.
        step: usize,
        /// `reasoning` or `content`.
        kind: String,
        /// Milliseconds from `ModelRequestStarted` to this first fragment.
        elapsed_ms: u64,
    },
    /// The provider stream finished for this model turn.
    ModelRequestCompleted {
        /// 0-based turn/step index.
        step: usize,
        /// Total milliseconds spent inside the provider streaming request.
        elapsed_ms: u64,
    },
    /// A streaming attempt failed after publishing visible deltas. Consumers
    /// must discard those deltas before rendering the retry, otherwise users
    /// see duplicated/contradictory partial answers.
    ModelAttemptReset {
        /// 0-based turn/step index.
        step: usize,
        /// Failed provider attempt, starting at 1.
        attempt: usize,
        /// Sanitized provider error used to explain the retry in logs.
        reason: String,
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
    /// Sanitized metadata for one DeepSeek Responses SSE event.
    ResponsesStreamEvent {
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta_chars: Option<usize>,
    },
    /// Provider-owned native web-search lifecycle, projected separately from
    /// local tools so it cannot accidentally enter approval/execution.
    ResponsesWebSearchCall {
        call_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action_type: Option<String>,
        #[serde(default)]
        queries_count: usize,
    },
    /// MCP server resolution/start lifecycle. Failures are explicit
    /// degradations so clients can distinguish unavailable remote tools from
    /// an empty registry.
    McpLifecycle {
        server_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        degradation_code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default)]
        tool_count: usize,
    },
    /// A tool call is about to run (after the BeforeToolUse gate allowed it).
    ToolStarted {
        /// Tool name.
        name: String,
        /// Correlation id for matching the completion.
        call_id: String,
        /// JSON arguments.
        arguments: serde_json::Value,
        /// Coarse UI category for row-based rendering.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_kind: Option<String>,
        /// Primary file path / URL / target, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        /// Compact one-line row summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Lightweight structured UI hints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
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
        /// Coarse UI category for row-based rendering.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_kind: Option<String>,
        /// Primary file path / URL / target, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        /// Compact one-line row summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Lightweight structured UI hints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
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
    /// An external hook command is about to run.
    HookStarted {
        /// Correlation id for matching the completion.
        id: String,
        /// Claude-Code-style hook event name.
        event: String,
        /// Shell used to execute this hook command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        /// Command line being executed.
        command: String,
    },
    /// An external hook command finished.
    HookCompleted {
        /// Correlation id.
        id: String,
        /// Claude-Code-style hook event name.
        event: String,
        /// Shell used to execute this hook command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        /// Command line that ran.
        command: String,
        /// Process exit code.
        exit_code: i32,
        /// Captured stdout.
        stdout: String,
        /// Captured stderr.
        stderr: String,
        /// Lightweight interpreted outcome (`continued`, `blocked`, or `error`).
        outcome: String,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// A child agent was accepted and began running.
    SubagentStarted {
        /// Durable child run id.
        id: String,
        /// Parent kernel run id.
        parent_run_id: String,
        /// Selected agent profile, or `general`.
        agent_type: String,
        /// Short task description supplied by the parent.
        description: String,
        /// Whether the parent returned immediately after launching the child.
        background: bool,
    },
    /// A child agent reached a non-cancelled terminal state.
    SubagentCompleted {
        /// Durable child run id.
        id: String,
        /// Parent kernel run id.
        parent_run_id: String,
        /// `succeeded` or `failed`.
        state: String,
        /// Bounded final summary persisted for the parent.
        summary: String,
        /// Child wall-clock duration.
        duration_ms: u64,
        /// Whether this was a detached background child.
        background: bool,
    },
    /// A child agent stopped because its cancellation flag was raised.
    SubagentCancelled {
        /// Durable child run id.
        id: String,
        /// Parent kernel run id.
        parent_run_id: String,
        /// Child wall-clock duration.
        duration_ms: u64,
        /// Whether this was a detached background child.
        background: bool,
    },
    /// Durable notification that a background result is ready for its parent.
    SubagentNotification {
        /// Durable child run id.
        id: String,
        /// Parent kernel run id.
        parent_run_id: String,
        /// Child terminal state.
        state: String,
        /// Bounded result or failure summary.
        summary: String,
    },
    WorktreeCreated {
        subagent_id: String,
        path: String,
    },
    WorktreeRemoved {
        subagent_id: String,
        path: String,
    },
    /// A post-completion verification step result.
    Verification {
        /// Whether verification passed.
        passed: bool,
        /// Short diagnosis / detail.
        detail: String,
    },
    /// Final on-disk comparison for every path captured by the run checkpoint.
    /// This is emitted before completion is accepted.
    CompletionEvidence {
        mutations: Vec<crate::checkpoint::MutationEvidence>,
    },
    /// Estimated context usage for the next model call. Emitted before the
    /// request so the UI can show capacity even before provider usage arrives.
    ContextUsage {
        snapshot: deepagent_context::ContextUsageSnapshot,
    },
    /// A proactive or reactive compaction replaced older conversation turns
    /// with a bounded structured summary.
    ContextCompacted {
        tokens_before: u64,
        tokens_after: u64,
        strategy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Background-prefetched relevant memories were injected into the context
    /// (§3.2, Claude Code `relevant_memories`). Non-blocking: the prefetch ran
    /// off the critical path; this fires only when fresh entries surfaced.
    RelevantMemoriesInjected {
        /// Number of distinct memory entries injected this round.
        count: usize,
        /// Milliseconds from prefetch start to injection.
        latency_ms: u64,
    },
    /// The stall/laziness detector (§2.3, Grok laziness classifier) flagged a
    /// final answer as stalled and injected one advisory nudge before
    /// re-entering the model turn. Advisory: capped per run and never blocks.
    StallNudgeInjected {
        /// Loop step at which the nudge fired.
        step: usize,
        /// Stalled category label (snake_case, Grok category set).
        category: String,
        /// Classifier confidence in `[0.0, 1.0]`.
        confidence: f32,
        /// Classifier evidence sentence.
        evidence: String,
    },
    /// Token accounting for one model call (accumulates across a multi-turn run
    /// on the UI side). Carries DeepSeek's cache hit/miss breakdown when present.
    Usage {
        /// Prompt (input) tokens for this call.
        prompt_tokens: u32,
        /// Completion (output) tokens for this call.
        completion_tokens: u32,
        /// Reasoning tokens already included in `completion_tokens`.
        reasoning_tokens: u32,
        /// Total tokens for this call.
        total_tokens: u32,
        /// Prompt tokens served from the context cache (a "hit").
        prompt_cache_hit_tokens: u32,
        /// Prompt tokens NOT served from cache (a "miss").
        prompt_cache_miss_tokens: u32,
        /// Backend-computed RMB cost for the completed run, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_yuan: Option<f64>,
        /// Raw provider Responses usage object for this call, preserved as
        /// returned by DeepSeek / OpenAI-compatible Responses endpoints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_responses_usage: Option<serde_json::Value>,
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
    /// The run was cancelled by the user (manual stop).
    RunCancelled,
}

/// Derive row-rendering metadata from a tool name, arguments and optional
/// output. Keep this helper deterministic and side-effect free so both live
/// events and historical session replay can share it.
pub fn tool_ui_metadata(
    name: &str,
    arguments: &serde_json::Value,
    output: Option<&serde_json::Value>,
) -> ToolUiMetadata {
    let lower = name.to_ascii_lowercase();
    let tool_kind = classify_tool(&lower).to_string();
    let path = first_string(
        arguments,
        &[
            "path",
            "file_path",
            "file",
            "target_path",
            "source_path",
            "source",
            "destination",
            "remote_path",
            "local_path",
            "url",
        ],
    )
    .or_else(|| output.and_then(|o| first_string(o, &["path", "file_path", "url"])));
    let pattern = first_string(arguments, &["pattern", "glob", "regex"]);
    let query = first_string(arguments, &["query", "text", "q"]);
    let command = first_string(arguments, &["command", "cmd", "script"]);
    let prompt = first_string(arguments, &["prompt", "description", "task"]);

    let matches_count = output.and_then(|o| {
        array_len(o, "matches")
            .or_else(|| array_len(o, "results"))
            .or_else(|| number_value(o, "count"))
    });
    let provider = output.and_then(|o| first_string(o, &["provider"]));
    let exit_code = output.and_then(|o| number_value(o, "exit_code"));
    let error = output.and_then(|o| first_string(o, &["error"]));

    let primary = match lower.as_str() {
        "bash" | "shell" => command.clone(),
        "glob" | "grep" => pattern.clone().or_else(|| path.clone()),
        "web_search" | "knowledge_search" | "tool_search" | "code_map_search" => query.clone(),
        "web_fetch" => path.clone(),
        "task" => prompt.clone(),
        _ => path
            .clone()
            .or_else(|| command.clone())
            .or_else(|| query.clone())
            .or_else(|| pattern.clone())
            .or_else(|| prompt.clone()),
    };

    let summary = if let Some(err) = error.as_deref() {
        Some(compact(err, 140))
    } else if lower == "web_search" {
        match (provider.as_deref(), matches_count) {
            (Some(provider), Some(count)) => {
                Some(format!("web_search: {provider} returned {count} result(s)"))
            }
            (_, Some(count)) => Some(format!("web_search returned {count} result(s)")),
            _ => query
                .as_deref()
                .map(|q| format!("web_search {}", compact(q, 96))),
        }
    } else if matches!(lower.as_str(), "glob" | "grep") {
        match matches_count {
            Some(count) => Some(format!("{name} {count} match(es)")),
            None => primary
                .as_deref()
                .map(|p| format!("{name} {}", compact(p, 96))),
        }
    } else if lower == "bash" || lower == "shell" {
        primary.as_deref().map(|p| compact(p, 140))
    } else if let Some(primary) = primary.as_deref() {
        Some(format!("{name} {}", compact(primary, 120)))
    } else {
        Some(name.to_string())
    };

    let mut meta = serde_json::Map::new();
    if let Some(path) = path.as_deref() {
        meta.insert("path".into(), serde_json::Value::String(path.to_string()));
    }
    if let Some(pattern) = pattern.as_deref() {
        meta.insert(
            "pattern".into(),
            serde_json::Value::String(pattern.to_string()),
        );
    }
    if let Some(query) = query.as_deref() {
        meta.insert("query".into(), serde_json::Value::String(query.to_string()));
    }
    if let Some(command) = command.as_deref() {
        meta.insert(
            "command".into(),
            serde_json::Value::String(compact(command, 240)),
        );
    }
    if let Some(count) = matches_count {
        meta.insert("matches_count".into(), serde_json::json!(count));
    }
    if let Some(provider) = provider {
        meta.insert("provider".into(), serde_json::Value::String(provider));
    }
    if let Some(code) = exit_code {
        meta.insert("exit_code".into(), serde_json::json!(code));
    }

    ToolUiMetadata {
        tool_kind: Some(tool_kind),
        file_path: path,
        summary,
        meta: if meta.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(meta))
        },
    }
}

fn classify_tool(name: &str) -> &'static str {
    match name {
        "read_file" | "list_dir" | "web_fetch" | "office_read" => "file_read",
        "write_file" | "edit_file" | "multi_edit" | "delete_path" | "move_path" | "git_commit"
        | "remote_push_file" | "remote_push_bundle" | "office_docx_create"
        | "office_xlsx_create" => "file_change",
        "glob" | "grep" | "web_search" | "knowledge_search" | "tool_search" | "code_map_search"
        | "codegraph_locate" | "codegraph_search" => "search",
        "bash" | "shell" | "remote_install" | "remote_require" | "remote_probe" => {
            "command_execution"
        }
        "git_status" | "git_diff" | "git_log" => "git",
        "todo_write" | "task_list" | "enter_plan_mode" | "exit_plan_mode" => "planning",
        "task" => "agent",
        _ => "tool",
    }
}

fn first_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn array_len(value: &serde_json::Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|items| items.len() as u64)
}

fn number_value(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|v| v.as_u64())
}

fn compact(value: &str, max: usize) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut truncated = one_line
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

impl RuntimeEvent {
    /// A stable label for tracing / filtering.
    pub fn label(&self) -> &'static str {
        match self {
            RuntimeEvent::RunStarted { .. } => "run_started",
            RuntimeEvent::SessionRegistered { .. } => "session_registered",
            RuntimeEvent::TurnStarted { .. } => "turn_started",
            RuntimeEvent::ModelRequestStarted { .. } => "model_request_started",
            RuntimeEvent::ModelFirstToken { .. } => "model_first_token",
            RuntimeEvent::ModelRequestCompleted { .. } => "model_request_completed",
            RuntimeEvent::ModelAttemptReset { .. } => "model_attempt_reset",
            RuntimeEvent::ReasoningDelta { .. } => "reasoning_delta",
            RuntimeEvent::ContentDelta { .. } => "content_delta",
            RuntimeEvent::ResponsesStreamEvent { .. } => "responses_stream_event",
            RuntimeEvent::ResponsesWebSearchCall { .. } => "responses_web_search_call",
            RuntimeEvent::McpLifecycle { .. } => "mcp_lifecycle",
            RuntimeEvent::ToolStarted { .. } => "tool_started",
            RuntimeEvent::ToolCompleted { .. } => "tool_completed",
            RuntimeEvent::ToolBlocked { .. } => "tool_blocked",
            RuntimeEvent::HookStarted { .. } => "hook_started",
            RuntimeEvent::HookCompleted { .. } => "hook_completed",
            RuntimeEvent::SubagentStarted { .. } => "subagent_started",
            RuntimeEvent::SubagentCompleted { .. } => "subagent_completed",
            RuntimeEvent::SubagentCancelled { .. } => "subagent_cancelled",
            RuntimeEvent::SubagentNotification { .. } => "subagent_notification",
            RuntimeEvent::WorktreeCreated { .. } => "worktree_created",
            RuntimeEvent::WorktreeRemoved { .. } => "worktree_removed",
            RuntimeEvent::Verification { .. } => "verification",
            RuntimeEvent::CompletionEvidence { .. } => "completion_evidence",
            RuntimeEvent::ContextUsage { .. } => "context_usage",
            RuntimeEvent::ContextCompacted { .. } => "context_compacted",
            RuntimeEvent::RelevantMemoriesInjected { .. } => "relevant_memories_injected",
            RuntimeEvent::StallNudgeInjected { .. } => "stall_nudge_injected",
            RuntimeEvent::Usage { .. } => "usage",
            RuntimeEvent::RunCompleted { .. } => "run_completed",
            RuntimeEvent::RunAwaitingApproval { .. } => "run_awaiting_approval",
            RuntimeEvent::RunFailed { .. } => "run_failed",
            RuntimeEvent::RunCancelled => "run_cancelled",
        }
    }

    /// Whether this event is terminal (the run will emit no more events).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RuntimeEvent::RunCompleted { .. }
                | RuntimeEvent::RunAwaitingApproval { .. }
                | RuntimeEvent::RunFailed { .. }
                | RuntimeEvent::RunCancelled
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
            tool_kind: Some("file_read".into()),
            file_path: Some("a.rs".into()),
            summary: Some("read_file a.rs".into()),
            meta: Some(serde_json::json!({"path": "a.rs"})),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "tool_started");
        assert_eq!(json["name"], "read_file");
        let back: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }
}
