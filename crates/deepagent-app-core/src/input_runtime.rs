use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use deepagent_core::clock::Clock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::id::SessionId;
use deepagent_core::message::{Message, Role};
use deepagent_core::response_item::{ResponseInputItem, ResponseItem};
use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_persistence::Database;
use deepagent_runtime::{InputEnvelope, InputLeaseRegistry, LeaseDecision};
use deepagent_session::Session;

use crate::runtime_event_log::append_runtime_log;

pub(crate) struct InputLeaseGuard {
    registry: Arc<InputLeaseRegistry>,
    session_id: String,
    run_id: String,
}

pub(crate) struct AcceptedInputTurn<'db, C: Clock> {
    pub(crate) session: Session<'db, C>,
    pub(crate) history: Vec<Message>,
    pub(crate) response_history: Vec<ResponseInputItem>,
    pub(crate) prior_events: Vec<Event>,
    pub(crate) session_id: String,
    pub(crate) lease: InputLeaseGuard,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn accept_input_turn<'db, C, F>(
    db: &'db Database,
    clock: &'db C,
    input_leases: Arc<InputLeaseRegistry>,
    runtime_logs: Option<Arc<RuntimeLogStore>>,
    run_id: &str,
    continue_session: Option<&str>,
    env_mode: Option<&str>,
    project: &str,
    normalized_input: InputEnvelope,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    cancel_active: F,
) -> Result<AcceptedInputTurn<'db, C>>
where
    C: Clock,
    F: Fn(&str) -> bool,
{
    let (mut session, mut history, mut prior_events) =
        bind_session(db, clock, continue_session, env_mode, project)?;
    let session_id = session.id().to_string();
    append_runtime_log(
        &runtime_logs,
        NewRuntimeLogEntry::info("chat", "session_bound")
            .with_run_id(run_id)
            .with_session_id(session_id.clone())
            .with_source("deepagent-app-core::input_runtime")
            .with_message("run bound to session")
            .with_data(serde_json::json!({
                "continued": continue_session.is_some(),
                "project": project,
                "history_messages": history.len(),
                "prior_events": prior_events.len(),
            })),
    );

    let session_input = InputEnvelope {
        session_id: Some(session_id.clone()),
        ..normalized_input.clone()
    };
    let lease_decision = input_leases.acquire(&session_id, run_id, session_input);
    let lease = InputLeaseGuard::new(input_leases.clone(), session_id.clone(), run_id.to_string());

    match lease_decision {
        LeaseDecision::Acquired => {
            append_runtime_log(
                &runtime_logs,
                NewRuntimeLogEntry::info("input", "input_lease_acquired")
                    .with_run_id(run_id)
                    .with_session_id(session_id.clone())
                    .with_source("deepagent-app-core::input_runtime")
                    .with_message("input acquired session dispatch lease")
                    .with_data(serde_json::json!({
                        "input_id": normalized_input.input_id.clone(),
                        "kind": format!("{:?}", normalized_input.kind),
                    })),
            );
        }
        LeaseDecision::Queued { position } => {
            let interrupted_run = input_leases.active_run(&session_id);
            if let Some(active_run) = interrupted_run.as_deref() {
                let _ = cancel_active(active_run);
            }
            append_runtime_log(
                &runtime_logs,
                NewRuntimeLogEntry::info("input", "input_queued")
                    .with_run_id(run_id)
                    .with_session_id(session_id.clone())
                    .with_source("deepagent-app-core::input_runtime")
                    .with_message("input queued behind active session run")
                    .with_data(serde_json::json!({
                        "input_id": normalized_input.input_id.clone(),
                        "position": position,
                        "interrupted_run": interrupted_run,
                    })),
            );
            input_leases
                .wait_for_turn(&session_id, run_id, cancel_flag.as_ref())
                .await?;
            let id = session.id();
            session = Session::recover(db, clock, id)?;
            prior_events =
                deepagent_persistence::event_store::EventStore::new(db).load_session(id)?;
            history = conversation_with_tool_pairs_from_events(&prior_events);
            append_runtime_log(
                &runtime_logs,
                NewRuntimeLogEntry::info("input", "input_dequeued")
                    .with_run_id(run_id)
                    .with_session_id(session_id.clone())
                    .with_source("deepagent-app-core::input_runtime")
                    .with_message("queued input acquired session dispatch lease")
                    .with_data(serde_json::json!({
                        "input_id": normalized_input.input_id.clone(),
                        "history_messages": history.len(),
                        "prior_events": prior_events.len(),
                    })),
            );
        }
    }

    Ok(AcceptedInputTurn {
        session,
        response_history: response_items_from_events(&prior_events),
        history,
        prior_events,
        session_id,
        lease,
    })
}

fn bind_session<'db, C: Clock>(
    db: &'db Database,
    clock: &'db C,
    continue_session: Option<&str>,
    env_mode: Option<&str>,
    project: &str,
) -> Result<(Session<'db, C>, Vec<Message>, Vec<Event>)> {
    match continue_session {
        Some(id_str) => {
            let id = SessionId::from_str(id_str)
                .map_err(|error| CoreError::invalid(format!("bad session id: {error}")))?;
            let session = Session::recover(db, clock, id)?;
            let events =
                deepagent_persistence::event_store::EventStore::new(db).load_session(id)?;
            // Main-run resume keeps the paired tool trajectory (Phase D):
            // the model sees which tools already ran and what they returned,
            // instead of a text-only shadow of the conversation.
            let history = conversation_with_tool_pairs_from_events(&events);
            Ok((session, history, events))
        }
        None => {
            let mode = match env_mode {
                Some("remote") => deepagent_core::SessionMode::Remote,
                _ => deepagent_core::SessionMode::Normal,
            };
            let session = Session::create_in_project(db, clock, None, mode, Some(project))?;
            Ok((session, Vec::new(), Vec::new()))
        }
    }
}

impl InputLeaseGuard {
    pub(crate) fn new(
        registry: Arc<InputLeaseRegistry>,
        session_id: String,
        run_id: String,
    ) -> Self {
        Self {
            registry,
            session_id,
            run_id,
        }
    }
}

impl Drop for InputLeaseGuard {
    fn drop(&mut self) {
        let _ = self.registry.release(&self.session_id, &self.run_id);
    }
}

/// Rebuild a plain conversation (user/assistant text turns) from a session's
/// event log, for seeding the model when continuing an existing session.
///
/// Only [`EventPayload::MessageAppended`] user/assistant turns are taken, and
/// any `tool_calls` are stripped: tool *requests* live as separate
/// `ToolCallRequested`/`ToolCallCompleted` events (not assistant messages), so
/// replaying them as bare `tool_calls` would dangle without their matching
/// `tool` results and the API would reject the request. Plain text turns are
/// enough context for a follow-up question.
pub(crate) fn conversation_from_events(events: &[Event]) -> Vec<Message> {
    let mut out = Vec::new();
    for ev in events {
        if let EventPayload::MessageAppended { message } = &ev.payload {
            if message
                .content
                .starts_with("[Earlier conversation compacted to summary]")
            {
                out.clear();
                out.push(Message::text(message.role, message.content.clone()));
                continue;
            }
            match message.role {
                Role::User | Role::Assistant if !message.content.trim().is_empty() => {
                    out.push(Message::text(message.role, message.content.clone()));
                }
                _ => {}
            }
        }
    }
    out
}

/// Per-tool-result character cap when replaying tool trajectories into a
/// resumed conversation. Keeps a restart from re-spending the whole context
/// window on stale outputs while preserving what the tool did and returned.
const REPLAY_TOOL_RESULT_MAX_CHARS: usize = 2_000;

/// Rebuild a conversation INCLUDING paired tool trajectories from a
/// session's event log (kernel-refactor Phase D: tool-call pairing
/// recovery).
///
/// In addition to the text turns of [`conversation_from_events`], every
/// `ToolCallRequested` becomes an assistant `tool_calls` message and every
/// `ToolCallCompleted` its matching `tool` result. Consecutive requests
/// between boundaries replay as one batched assistant message (mirroring
/// how the model issued them). Orphaned calls — a request whose result
/// never arrived because the process crashed or the run was cancelled
/// mid-flight — get a synthesized failure result so the transcript NEVER
/// contains a dangling `tool_use` (strict providers reject those).
pub(crate) fn conversation_with_tool_pairs_from_events(events: &[Event]) -> Vec<Message> {
    if events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ResponseItemAppended { .. }))
    {
        return conversation_from_response_items(events);
    }
    let mut out: Vec<Message> = Vec::new();
    // Requests not yet flushed into an assistant message, in arrival order.
    let mut open_batch: Vec<deepagent_core::message::ToolCall> = Vec::new();
    // call_id -> index in `out` of the assistant message carrying it, used
    // to pair results and to synthesize failures for orphans at the end.
    let mut unresolved: Vec<String> = Vec::new();

    fn flush_batch(
        out: &mut Vec<Message>,
        open_batch: &mut Vec<deepagent_core::message::ToolCall>,
        unresolved: &mut Vec<String>,
    ) {
        if open_batch.is_empty() {
            return;
        }
        let calls = std::mem::take(open_batch);
        unresolved.extend(calls.iter().map(|call| call.id.clone()));
        out.push(Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: calls,
            tool_call_id: None,
        });
    }

    fn close_orphans(out: &mut Vec<Message>, unresolved: &mut Vec<String>) {
        for call_id in unresolved.drain(..) {
            out.push(Message::tool_result(
                call_id,
                serde_json::json!({
                    "status": "error",
                    "error": "tool result lost: the run was interrupted (crash or cancel) before this call completed",
                })
                .to_string(),
            ));
        }
    }

    for ev in events {
        match &ev.payload {
            EventPayload::MessageAppended { message } => {
                flush_batch(&mut out, &mut open_batch, &mut unresolved);
                close_orphans(&mut out, &mut unresolved);
                if message
                    .content
                    .starts_with("[Earlier conversation compacted to summary]")
                {
                    out.clear();
                    out.push(Message::text(message.role, message.content.clone()));
                    continue;
                }
                match message.role {
                    Role::User | Role::Assistant if !message.content.trim().is_empty() => {
                        out.push(Message::text(message.role, message.content.clone()));
                    }
                    _ => {}
                }
            }
            EventPayload::ToolCallRequested { call } => {
                open_batch.push(call.clone());
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } => {
                flush_batch(&mut out, &mut open_batch, &mut unresolved);
                if let Some(position) = unresolved.iter().position(|id| id == call_id) {
                    unresolved.remove(position);
                    let rendered = bounded_tool_result(*ok, output);
                    out.push(Message::tool_result(call_id.clone(), rendered));
                }
                // A completion without a matching request (partial log) is
                // dropped — a result with no tool_use is equally rejected.
            }
            _ => {}
        }
    }
    flush_batch(&mut out, &mut open_batch, &mut unresolved);
    close_orphans(&mut out, &mut unresolved);
    out
}

pub(crate) fn response_items_from_events(events: &[Event]) -> Vec<ResponseInputItem> {
    if events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ResponseItemAppended { .. }))
    {
        return events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended { item } => Some(item.clone()),
                _ => None,
            })
            .collect();
    }
    let history = conversation_with_tool_pairs_from_events(events);
    let (_, items) = deepagent_models::response_items_from_messages(&history);
    items
}

fn conversation_from_response_items(events: &[Event]) -> Vec<Message> {
    let mut out = Vec::new();
    let mut pending_reasoning: Option<String> = None;
    for event in events {
        let EventPayload::ResponseItemAppended { item } = &event.payload else {
            continue;
        };
        match item {
            ResponseItem::Reasoning { content, .. } => {
                pending_reasoning = (!content.is_empty()).then(|| content.clone());
            }
            ResponseItem::Message { role, content } => {
                let role = match role.as_str() {
                    "assistant" => Role::Assistant,
                    "system" => Role::System,
                    _ => Role::User,
                };
                let mut message = Message::text(role, content.clone());
                if role == Role::Assistant {
                    message.reasoning_content = pending_reasoning.take();
                }
                out.push(message);
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                arguments: raw,
            } => {
                let arguments =
                    serde_json::from_str(raw).unwrap_or_else(|error| serde_json::json!({
                        "__invalid_tool_arguments__": true, "raw": raw, "parse_error": error.to_string()
                    }));
                out.push(Message::assistant("").with_tool_calls(vec![
                    deepagent_core::message::ToolCall {
                        id: call_id.clone(),
                        name: name.clone(),
                        arguments,
                    },
                ]));
            }
            ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
            } => {
                out.push(Message::assistant("").with_tool_calls(vec![
                    deepagent_core::message::ToolCall {
                        id: call_id.clone(),
                        name: name.clone(),
                        arguments: serde_json::json!({"patch": input}),
                    },
                ]));
            }
            ResponseItem::FunctionCallOutput { call_id, output }
            | ResponseItem::CustomToolCallOutput { call_id, output } => {
                out.push(Message::tool_result(call_id.clone(), output.clone()));
            }
            _ => {}
        }
    }
    out
}

fn bounded_tool_result(ok: bool, output: &serde_json::Value) -> String {
    let envelope = if ok {
        serde_json::json!({ "status": "ok", "result": output })
    } else {
        serde_json::json!({ "status": "error", "error": output })
    };
    let mut rendered = envelope.to_string();
    if rendered.chars().count() > REPLAY_TOOL_RESULT_MAX_CHARS {
        let truncated: String = rendered
            .chars()
            .take(REPLAY_TOOL_RESULT_MAX_CHARS)
            .collect();
        let status = if ok { "ok" } else { "error" };
        rendered = serde_json::json!({
            "status": status,
            "truncated_result": truncated,
            "note": "tool output truncated during session replay",
        })
        .to_string();
    }
    rendered
}

/// Extract the cumulative set of discovered-tool names from a session's
/// event stream. Walks every `ToolsDiscovered` payload (each carrying a
/// delta) and returns the union, preserving first-seen order.
pub(crate) fn collect_discovered_tools_from_events(events: &[Event]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in events {
        if let EventPayload::ToolsDiscovered { names } = &e.payload {
            for name in names {
                if seen.insert(name.clone()) {
                    out.push(name.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::SystemClock;
    use deepagent_core::id::{EventId, SessionId};
    use deepagent_runtime::{InputIngress, InputMode};

    #[test]
    fn conversation_keeps_text_turns_only() {
        let sid = SessionId::nil();
        let events = vec![
            Event {
                id: EventId::new(),
                session_id: sid,
                sequence: 0,
                timestamp: deepagent_core::clock::Timestamp::from_millis(0),
                payload: EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            },
            Event {
                id: EventId::new(),
                session_id: sid,
                sequence: 1,
                timestamp: deepagent_core::clock::Timestamp::from_millis(1),
                payload: EventPayload::MessageAppended {
                    message: Message::assistant("hello"),
                },
            },
        ];
        let convo = conversation_from_events(&events);
        assert_eq!(convo.len(), 2);
        assert_eq!(convo[0].role, Role::User);
        assert_eq!(convo[0].content, "hi");
        assert_eq!(convo[1].role, Role::Assistant);
        assert_eq!(convo[1].content, "hello");
    }

    fn event(sid: SessionId, seq: u64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: sid,
            sequence: seq,
            timestamp: deepagent_core::clock::Timestamp::from_millis(seq as i64),
            payload,
        }
    }

    #[test]
    fn tool_pair_replay_rebuilds_batches_and_results() {
        let sid = SessionId::nil();
        let call = |id: &str, name: &str| deepagent_core::message::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
        };
        let events = vec![
            event(
                sid,
                0,
                EventPayload::MessageAppended {
                    message: Message::user("do work"),
                },
            ),
            event(
                sid,
                1,
                EventPayload::ToolCallRequested {
                    call: call("c1", "read_file"),
                },
            ),
            event(
                sid,
                2,
                EventPayload::ToolCallRequested {
                    call: call("c2", "list_dir"),
                },
            ),
            event(
                sid,
                3,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: true,
                    output: serde_json::json!({"content": "x"}),
                    duration_ms: 5,
                },
            ),
            event(
                sid,
                4,
                EventPayload::ToolCallCompleted {
                    call_id: "c2".into(),
                    ok: false,
                    output: serde_json::json!({"error": "denied"}),
                    duration_ms: 5,
                },
            ),
            event(
                sid,
                5,
                EventPayload::MessageAppended {
                    message: Message::assistant("done"),
                },
            ),
        ];

        let convo = conversation_with_tool_pairs_from_events(&events);
        // user, assistant(tool_calls c1+c2), tool c1, tool c2, assistant text
        assert_eq!(convo.len(), 5);
        assert_eq!(convo[1].role, Role::Assistant);
        assert_eq!(convo[1].tool_calls.len(), 2);
        assert_eq!(convo[2].role, Role::Tool);
        assert_eq!(convo[2].tool_call_id.as_deref(), Some("c1"));
        assert!(convo[2].content.contains("\"status\":\"ok\""));
        assert_eq!(convo[3].tool_call_id.as_deref(), Some("c2"));
        assert!(convo[3].content.contains("\"status\":\"error\""));
        assert_eq!(convo[4].content, "done");
    }

    #[test]
    fn tool_pair_replay_synthesizes_failures_for_orphaned_calls() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "orphan".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "x"}),
                    },
                },
            ),
            // Crash: no ToolCallCompleted, no further messages.
        ];

        let convo = conversation_with_tool_pairs_from_events(&events);
        assert_eq!(convo.len(), 2);
        assert_eq!(convo[0].tool_calls.len(), 1);
        assert_eq!(convo[1].role, Role::Tool);
        assert_eq!(convo[1].tool_call_id.as_deref(), Some("orphan"));
        assert!(convo[1].content.contains("tool result lost"));
    }

    #[test]
    fn tool_pair_replay_bounds_oversized_results() {
        let sid = SessionId::nil();
        let big = "y".repeat(REPLAY_TOOL_RESULT_MAX_CHARS * 2);
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "c".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({}),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ToolCallCompleted {
                    call_id: "c".into(),
                    ok: true,
                    output: serde_json::json!({ "content": big }),
                    duration_ms: 1,
                },
            ),
        ];

        let convo = conversation_with_tool_pairs_from_events(&events);
        assert!(convo[1].content.contains("tool output truncated"));
        assert!(convo[1].content.chars().count() < REPLAY_TOOL_RESULT_MAX_CHARS + 300);
    }

    #[test]
    fn responses_item_replay_preserves_reasoning_and_tool_pairs() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "user".into(),
                        content: "do it".into(),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Reasoning {
                        id: None,
                        content: "need a file read".into(),
                    },
                },
            ),
            event(
                sid,
                2,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCall {
                        call_id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"a.txt"}"#.into(),
                    },
                },
            ),
            event(
                sid,
                3,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCallOutput {
                        call_id: "call-1".into(),
                        output: r#"{"status":"ok"}"#.into(),
                    },
                },
            ),
            event(
                sid,
                4,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "assistant".into(),
                        content: "done".into(),
                    },
                },
            ),
        ];

        let convo = conversation_with_tool_pairs_from_events(&events);

        assert_eq!(convo.len(), 4);
        assert_eq!(convo[0].role, Role::User);
        assert_eq!(convo[1].tool_calls[0].id, "call-1");
        assert_eq!(convo[2].role, Role::Tool);
        assert_eq!(convo[3].content, "done");
        assert_eq!(
            convo[3].reasoning_content.as_deref(),
            Some("need a file read")
        );
    }

    #[test]
    fn response_history_replay_keeps_non_message_items_native() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "user".into(),
                        content: "search".into(),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::WebSearchCall {
                        id: "ws_1".into(),
                        status: "completed".into(),
                        action: Some(serde_json::json!({
                            "type": "search",
                            "query": "DeepSeek Responses"
                        })),
                    },
                },
            ),
            event(
                sid,
                2,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "assistant".into(),
                        content: "found".into(),
                    },
                },
            ),
        ];

        let items = response_items_from_events(&events);

        assert_eq!(items.len(), 3);
        assert!(matches!(
            &items[1],
            ResponseItem::WebSearchCall {
                id,
                status,
                action: Some(action),
            } if id == "ws_1"
                && status == "completed"
                && action["query"] == "DeepSeek Responses"
        ));
    }

    #[test]
    fn collect_discovered_unions_across_events_preserving_order() {
        let sid = SessionId::nil();
        let events = vec![
            Event {
                id: EventId::new(),
                session_id: sid,
                sequence: 0,
                timestamp: deepagent_core::clock::Timestamp::from_millis(0),
                payload: EventPayload::ToolsDiscovered {
                    names: vec!["alpha".into(), "beta".into()],
                },
            },
            Event {
                id: EventId::new(),
                session_id: sid,
                sequence: 1,
                timestamp: deepagent_core::clock::Timestamp::from_millis(1),
                payload: EventPayload::ToolsDiscovered {
                    names: vec!["beta".into(), "gamma".into()],
                },
            },
        ];

        assert_eq!(
            collect_discovered_tools_from_events(&events),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[tokio::test]
    async fn accept_turn_creates_session_and_holds_lease_until_drop() {
        let db = Database::open_in_memory().unwrap();
        let clock = SystemClock;
        let leases = Arc::new(InputLeaseRegistry::default());
        let input = InputIngress::normalize(
            None,
            "G:/Code/Kotlin_code",
            "hello",
            InputMode::Prompt,
            vec![],
        )
        .unwrap();

        let accepted = accept_input_turn(
            &db,
            &clock,
            leases.clone(),
            None,
            "run_test",
            None,
            Some("remote"),
            "G:/Code/Kotlin_code",
            input,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| false,
        )
        .await
        .unwrap();

        assert_eq!(accepted.history.len(), 0);
        assert_eq!(accepted.response_history.len(), 0);
        assert_eq!(accepted.prior_events.len(), 0);
        assert_eq!(
            accepted.session.state().mode,
            deepagent_core::SessionMode::Remote
        );
        assert!(leases.is_active_run(&accepted.session_id, "run_test"));
        let session_id = accepted.session_id.clone();
        drop(accepted);
        assert!(!leases.is_active_run(&session_id, "run_test"));
    }
}
