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

/// Model-only reminder appended once before the next user input after an
/// interrupted run. It deliberately is not persisted as a chat message: the
/// durable `RunTerminal` event is the source of truth and the reminder is a
/// projection for the next model request.
const ABORTED_TURN_REMINDER: &str = "The previous turn was aborted by the user. A tool or command may have executed only partially. Inspect the current workspace and process state before retrying. Do not assume the previous operation did not happen.";

fn latest_terminal_was_cancelled(events: &[Event]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::RunTerminal { kind, .. } => Some(kind == "cancelled"),
            _ => None,
        })
        .unwrap_or(false)
}

fn append_abort_reminder_if_needed(
    events: &[Event],
    history: &mut Vec<Message>,
    response_history: &mut Vec<ResponseInputItem>,
) {
    if !latest_terminal_was_cancelled(events) {
        return;
    }
    history.push(Message::user(ABORTED_TURN_REMINDER));
    response_history.push(ResponseInputItem::Message {
        role: "user".to_string(),
        content: ABORTED_TURN_REMINDER.to_string(),
    });
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
    project: Option<&str>,
    normalized_input: InputEnvelope,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    cancel_active: F,
) -> Result<AcceptedInputTurn<'db, C>>
where
    C: Clock,
    F: Fn(&str) -> bool,
{
    let (session_identity, created_session) = match continue_session {
        Some(id_str) => {
            let id = SessionId::from_str(id_str)
                .map_err(|error| CoreError::invalid(format!("bad session id: {error}")))?;
            (id, None)
        }
        None => {
            let mode = match env_mode {
                Some("remote") => deepagent_core::SessionMode::Remote,
                _ => deepagent_core::SessionMode::Normal,
            };
            let session = Session::create_in_project(db, clock, None, mode, project)?;
            (session.id(), Some(session))
        }
    };
    let session_id = session_identity.to_string();

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
            append_runtime_log(
                &runtime_logs,
                NewRuntimeLogEntry::info("input", "input_dequeued")
                    .with_run_id(run_id)
                    .with_session_id(session_id.clone())
                    .with_source("deepagent-app-core::input_runtime")
                    .with_message("queued input acquired session dispatch lease")
                    .with_data(serde_json::json!({
                        "input_id": normalized_input.input_id.clone(),
                    })),
            );
        }
    }

    if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
        return Err(CoreError::other(
            "cancelled before session history recovery",
        ));
    }
    let (session, mut history, prior_events) = match created_session {
        Some(session) => (session, Vec::new(), Vec::new()),
        None => {
            let session = Session::recover(db, clock, session_identity)?;
            let prior_events = deepagent_persistence::event_store::EventStore::new(db)
                .load_session(session_identity)?;
            let history = model_history_from_events(&prior_events);
            (session, history, prior_events)
        }
    };
    if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
        return Err(CoreError::other(
            "cancelled during session history recovery",
        ));
    }
    append_runtime_log(
        &runtime_logs,
        NewRuntimeLogEntry::info("chat", "session_bound")
            .with_run_id(run_id)
            .with_session_id(session_id.clone())
            .with_source("deepagent-app-core::input_runtime")
            .with_message("run acquired session lease and recovered history")
            .with_data(serde_json::json!({
                "continued": continue_session.is_some(),
                "project": project,
                "history_messages": history.len(),
                "prior_events": prior_events.len(),
            })),
    );

    let mut response_history = response_items_from_events(&prior_events);
    append_abort_reminder_if_needed(&prior_events, &mut history, &mut response_history);

    Ok(AcceptedInputTurn {
        session,
        response_history,
        history,
        prior_events,
        session_id,
        lease,
    })
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

/// Header on the message that stands in for a compacted prefix.
///
/// Older builds wrote this text as an ordinary assistant/user message and only
/// ONE of the three history projections honoured it, so the model-facing item
/// view kept replaying the full transcript. It is now rendered by the writer and
/// recognised by the single boundary resolver below, i.e. by every view.
pub(crate) const COMPACTED_SUMMARY_HEADER: &str = "[Earlier conversation compacted to summary]";

/// Name the file the dropped detail actually lives in.
///
/// A summary is a lossy rewrite, and the model has no other way to recover a
/// specific it lost. Claude Code writes the transcript path into the compacted
/// body for exactly this reason (`compact/prompt.ts:349-351`). Appended wherever
/// a durable boundary is written — the two places that turn a summary into the
/// session's model-facing history.
pub(crate) fn with_transcript_pointer(summary: String, transcript_dir: &std::path::Path) -> String {
    format!(
        "{summary}\n[完整记录（未压缩的原始事件）: {}]",
        transcript_dir.display()
    )
}

/// Where the model-facing view starts, and what replaces everything before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactionBoundary {
    /// Text injected in place of the compacted prefix (header included).
    rendered: String,
    /// First event sequence still visible verbatim.
    first_kept_sequence: deepagent_core::event::Sequence,
}

/// Resolve the newest compaction boundary in a session log.
///
/// Recognises both shapes so one rule covers every projection and every session
/// written since compaction existed:
/// - `ContextCompacted { summary: Some(..) }` — the durable boundary. Raw events
///   are never rewritten or deleted; only the view is cut, so export, replay and
///   search still see the whole transcript (Claude Code does the same: an
///   append-only transcript plus `getMessagesAfterCompactBoundary`).
/// - A persisted message starting with [`COMPACTED_SUMMARY_HEADER`] — how a
///   session compacted by an older build recorded it.
fn compaction_boundary(events: &[Event]) -> Option<CompactionBoundary> {
    events.iter().rev().find_map(|event| {
        let keep_after = |rendered: String| {
            Some(CompactionBoundary {
                rendered,
                first_kept_sequence: event.sequence.saturating_add(1),
            })
        };
        match &event.payload {
            EventPayload::ContextCompacted {
                summary: Some(summary),
                first_kept_sequence,
                ..
            } => Some(CompactionBoundary {
                rendered: format!("{COMPACTED_SUMMARY_HEADER}\n{summary}"),
                first_kept_sequence: first_kept_sequence.unwrap_or(event.sequence),
            }),
            EventPayload::MessageAppended { message }
                if message.content.starts_with(COMPACTED_SUMMARY_HEADER) =>
            {
                keep_after(message.content.clone())
            }
            _ => None,
        }
    })
}

/// Sequence of the event that recorded `wanted`.
///
/// Used to turn a message-space compaction split into an event-space boundary.
/// Matching by content rather than counting records is what keeps it honest: a
/// batch of parallel tool calls collapses into ONE history entry but spans
/// several events, so history indexes and event sequences are not 1:1.
///
/// The FIRST matching event wins. If the same text occurs twice that puts the
/// boundary earlier than strictly necessary, which keeps a few extra raw turns
/// visible instead of risking cutting mid-pair — redundancy is cheap here,
/// a broken tool pair is not. `None` means the caller must not write a
/// verbatim-keeping boundary (it falls back to "summary plus whatever follows").
fn without_user_tag(content: &str) -> &str {
    if let Some((text, suffix)) = content.rsplit_once("\n[id:u") {
        if suffix
            .strip_suffix(']')
            .is_some_and(|id| id.parse::<usize>().is_ok())
        {
            return text;
        }
    }
    content
}

pub(crate) fn sequence_of_message(
    events: &[Event],
    wanted: &Message,
) -> Option<deepagent_core::event::Sequence> {
    let wanted_role = match wanted.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        _ => "",
    };
    events.iter().find_map(|event| match &event.payload {
        EventPayload::MessageAppended { message }
            if message.role == wanted.role
                && without_user_tag(&message.content) == without_user_tag(&wanted.content) =>
        {
            Some(event.sequence)
        }
        EventPayload::ResponseItemAppended {
            item: deepagent_core::response_item::ResponseItem::Message { role, content },
        } if !wanted_role.is_empty()
            && role == wanted_role
            && without_user_tag(content) == without_user_tag(&wanted.content) =>
        {
            Some(event.sequence)
        }
        _ => None,
    })
}

/// Migrate only the legacy prefix. Once native events start, those events own
/// the Surface and the compatibility Message/Tool events are not appended again.
fn indexed_response_items(
    events: &[Event],
) -> Vec<(deepagent_core::event::Sequence, ResponseInputItem)> {
    let first_native = events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ResponseItemAppended { .. }))
        .unwrap_or(events.len());
    let mut out = Vec::new();
    let mut custom_calls = std::collections::HashSet::new();
    for (index, event) in events.iter().enumerate() {
        if let EventPayload::ResponseItemAppended { item } = &event.payload {
            out.push((event.sequence, item.clone()));
            continue;
        }
        if index >= first_native {
            continue;
        }
        let generated = match &event.payload {
            EventPayload::MessageAppended { message } => {
                let mut message = message.clone();
                message.tool_calls.clear(); // Legacy calls have their own ToolCallRequested events.
                deepagent_models::response_items_from_messages(&[message]).1
            }
            EventPayload::ToolCallRequested { call } => {
                if call.name == "apply_patch" {
                    custom_calls.insert(call.id.clone());
                }
                deepagent_models::response_items_from_messages(&[
                    Message::assistant("").with_tool_calls(vec![call.clone()])
                ])
                .1
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } => {
                let output = bounded_tool_result(*ok, output);
                vec![if custom_calls.contains(call_id) {
                    ResponseInputItem::CustomToolCallOutput {
                        call_id: call_id.clone(),
                        output,
                    }
                } else {
                    ResponseInputItem::FunctionCallOutput {
                        call_id: call_id.clone(),
                        output,
                    }
                }]
            }
            _ => Vec::new(),
        };
        // The first native item may echo its immediately preceding source event.
        // Exclude that source only; all older legacy turns must survive migration.
        let echoed = index + 1 == first_native
            && events.get(first_native).is_some_and(|event| {
                let EventPayload::ResponseItemAppended { item } = &event.payload else {
                    return false;
                };
                generated.first().is_some_and(|generated| {
                    generated == item
                        || match (generated, item) {
                            (
                                ResponseInputItem::FunctionCallOutput { call_id: a, .. },
                                ResponseInputItem::FunctionCallOutput { call_id: b, .. },
                            )
                            | (
                                ResponseInputItem::CustomToolCallOutput { call_id: a, .. },
                                ResponseInputItem::CustomToolCallOutput { call_id: b, .. },
                            ) => a == b,
                            _ => false,
                        }
                })
            });
        if !echoed {
            out.extend(generated.into_iter().map(|item| (event.sequence, item)));
        }
    }
    let mut items: Vec<_> = out.iter().map(|(_, item)| item.clone()).collect();
    deepagent_runtime::tag_response_user_items(&mut items);
    for ((_, item), tagged) in out.iter_mut().zip(items) {
        *item = tagged;
    }
    out
}

/// Resolve a native item against the durable log without reconstructing calls.
/// User tags are derived over the complete log before cuts, so tagged live
/// user turns still resolve after earlier compactions or snips.
pub(crate) fn sequence_of_response_item(
    events: &[Event],
    wanted: &ResponseInputItem,
) -> Option<deepagent_core::event::Sequence> {
    let indexed = indexed_response_items(events);
    let user_tag = |item: &ResponseInputItem| -> Option<String> {
        match item {
            ResponseInputItem::Message { role, content }
            | ResponseInputItem::InputMessage { role, content, .. }
                if role == "user" =>
            {
                content
                    .rsplit_once("[id:u")
                    .and_then(|(_, rest)| rest.split_once(']'))
                    .map(|(id, _)| id.to_string())
            }
            _ => None,
        }
    };
    indexed
        .iter()
        .find_map(|(sequence, item)| {
            let same = item == wanted
                || match (item, wanted) {
                    (
                        ResponseInputItem::FunctionCallOutput { call_id: a, .. },
                        ResponseInputItem::FunctionCallOutput { call_id: b, .. },
                    )
                    | (
                        ResponseInputItem::CustomToolCallOutput { call_id: a, .. },
                        ResponseInputItem::CustomToolCallOutput { call_id: b, .. },
                    ) => a == b,
                    _ => user_tag(wanted).is_some_and(|tag| user_tag(item).as_ref() == Some(&tag)),
                };
            same.then_some(*sequence)
        })
        .or_else(|| {
            events.iter().find_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended { item } if item == wanted => {
                    Some(event.sequence)
                }
                _ => None,
            })
        })
}

/// The event slice the model should see, plus the summary standing in for the
/// compacted prefix. Shared by the Message view and the item view so the two can
/// never disagree about where history starts.
fn events_for_model_view(events: &[Event]) -> (Vec<Event>, Option<String>) {
    match compaction_boundary(events) {
        None => (events.to_vec(), None),
        Some(boundary) => (
            events
                .iter()
                .filter(|event| event.sequence >= boundary.first_kept_sequence)
                .cloned()
                .collect(),
            Some(boundary.rendered),
        ),
    }
}

/// Rebuild the COMPLETE conversation (user/assistant text turns) from a
/// session's event log, ignoring any compaction boundary.
///
/// For the history the model actually gets, use [`model_history_from_events`].
/// This one is for consumers that must see everything: transcript export and
/// session titling.
///
/// Only [`EventPayload::MessageAppended`] user/assistant turns are taken, and
/// any `tool_calls` are stripped: tool *requests* live as separate
/// `ToolCallRequested`/`ToolCallCompleted` events (not assistant messages), so
/// replaying them as bare `tool_calls` would dangle without their matching
/// `tool` results and the API would reject the request. Plain text turns are
/// enough context for a follow-up question.
pub(crate) fn full_history_from_events(events: &[Event]) -> Vec<Message> {
    let mut out = Vec::new();
    for ev in events {
        if let EventPayload::MessageAppended { message } = &ev.payload {
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

/// Collect the union of every persisted snipped tag across the session's log.
fn persisted_snipped_tags(events: &[Event]) -> Vec<String> {
    let mut tags = std::collections::BTreeSet::new();
    for event in events {
        if let EventPayload::ContextSnipped { tags: snipped } = &event.payload {
            tags.extend(snipped.iter().cloned());
        }
    }
    tags.into_iter().collect()
}

/// The history the model is actually shown.
///
/// Paired tool trajectories first (see [`build_conversation`]): every
/// `ToolCallRequested` becomes an assistant `tool_calls` message and every
/// `ToolCallCompleted` its matching `tool` result; consecutive requests between
/// boundaries replay as one batched assistant message, and orphaned calls — a
/// request whose result never arrived because the process crashed or the run was
/// cancelled mid-flight — get a synthesized failure result so the transcript
/// NEVER contains a dangling `tool_use` (strict providers reject those).
///
/// Then the two history-cutting mechanisms are re-applied in order: the
/// compaction boundary (a compacted prefix returns as its summary, raw events
/// untouched) and the persisted snips, so a restart reconstructs the same
/// working history instead of resurrecting compacted or removed segments.
///
/// This is the ONLY place a compaction boundary is interpreted, and the item
/// view goes through the same resolver — that is what keeps the run's Message
/// history, the provider-native history, `/compact` and `/context` from becoming
/// separate readings of one log.
pub(crate) fn model_history_from_events(events: &[Event]) -> Vec<Message> {
    if events.iter().any(|event| {
        matches!(
            event.payload,
            EventPayload::ResponseItemAppended { .. }
                | EventPayload::ContextSurfacePatched { .. }
                | EventPayload::ContextSnipped { .. }
        )
    }) {
        let mut messages = deepagent_models::messages_from_response_items(
            None,
            &response_items_from_events(events),
        );
        // Tags are model routing metadata, not UI transcript content.
        for message in &mut messages {
            if message.role == Role::User {
                message.content = without_user_tag(&message.content).to_string();
            }
        }
        return messages;
    }
    let (kept, summary) = events_for_model_view(events);
    let mut out = Vec::new();
    if let Some(summary) = summary {
        out.push(Message::user(summary));
    }
    out.extend(build_conversation(&kept));
    let tags = persisted_snipped_tags(&kept);
    if tags.is_empty() {
        return out;
    }
    deepagent_runtime::snip_segments(
        &out,
        &tags,
        deepagent_runtime::SNIP_PROTECTED_RECENT_MESSAGES,
    )
}

fn build_conversation(events: &[Event]) -> Vec<Message> {
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
            attachments: Vec::new(),
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

/// The provider-native item history, honouring the SAME compaction boundary as
/// [`model_history_from_events`].
///
/// Before this, only one projection honoured compaction, so the model kept
/// replaying the full transcript while `/context` and titling believed a summary
/// had replaced it.
pub(crate) fn response_items_from_events(events: &[Event]) -> Vec<ResponseInputItem> {
    if !events.iter().any(|event| {
        matches!(
            event.payload,
            EventPayload::ResponseItemAppended { .. }
                | EventPayload::ContextSurfacePatched { .. }
                | EventPayload::ContextSnipped { .. }
        )
    }) {
        let history = model_history_from_events(events);
        return deepagent_models::response_items_from_messages(&history).1;
    }
    let boundary = compaction_boundary(events);
    let mut indexed = indexed_response_items(events);
    if let Some(boundary) = &boundary {
        indexed.retain(|(seq, _)| *seq >= boundary.first_kept_sequence);
    }
    for event in events {
        if let EventPayload::ContextSurfacePatched {
            version: 1,
            replacements,
            ..
        } = &event.payload
        {
            for replacement in replacements {
                if replacement.source_sequence >= event.sequence {
                    continue;
                }
                if let Some((_, item)) = indexed
                    .iter_mut()
                    .find(|(seq, _)| *seq == replacement.source_sequence)
                {
                    match item {
                        ResponseInputItem::FunctionCallOutput { call_id, output }
                        | ResponseInputItem::CustomToolCallOutput { call_id, output }
                            if call_id == &replacement.call_id =>
                        {
                            *output = replacement.replacement_output.clone()
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let mut items: Vec<_> = indexed.into_iter().map(|(_, item)| item).collect();
    if let Some(boundary) = boundary {
        items.insert(
            0,
            ResponseInputItem::Message {
                role: "user".into(),
                content: boundary.rendered,
            },
        );
    }
    deepagent_runtime::snip_response_items(
        &items,
        &persisted_snipped_tags(events),
        deepagent_runtime::SNIP_PROTECTED_RECENT_MESSAGES,
    )
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

/// Rebuild the cumulative set of MCP server names whose `instructions` have
/// already been announced to the model in this session. Walks every
/// `McpInstructionsDelta` payload: `added` names enter the set, `removed`
/// names leave it — the resume-time counterpart to Claude Code's
/// `mcpInstructionsDelta` attachment scan, so a restarted session does not
/// re-announce instructions that a prior run already surfaced.
pub(crate) fn collect_announced_mcp_servers_from_events(events: &[Event]) -> HashSet<String> {
    let mut announced = HashSet::new();
    for e in events {
        if let EventPayload::McpInstructionsDelta { added, removed } = &e.payload {
            for name in added {
                announced.insert(name.clone());
            }
            for name in removed {
                announced.remove(name);
            }
        }
    }
    announced
}

/// Rebuild the cumulative set of long-term memory entry ids already surfaced
/// into this session's model context. Walks every `MemoriesInjected` delta —
/// the replay counterpart to Claude Code's `collectSurfacedMemories`, which
/// also derives the set from the transcript rather than from process state.
/// Preserves first-seen order so injection de-dup and the injection cap behave
/// identically after a restart.
pub(crate) fn collect_surfaced_memory_ids_from_events(events: &[Event]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in events {
        if let EventPayload::MemoriesInjected { ids, .. } = &e.payload {
            for id in ids {
                if seen.insert(id.clone()) {
                    out.push(id.clone());
                }
            }
        }
    }
    out
}

/// Compute the MCP-instructions delta between the currently-connected servers
/// that declared instructions (`entries`) and what has already been announced
/// this session (`announced`). Returns `(added, removed)`:
/// - `added` — server names in `entries` not yet announced (render and persist).
/// - `removed` — names previously announced but with no current instructions
///   (disconnected or since dropped instructions); persist to retract.
///
/// One implementation shared by `build_run_context` (renders `added`) and the
/// run bound (persists the whole delta as an event), so the two can never
/// disagree about what was announced.
pub(crate) fn mcp_instructions_delta(
    entries: &[(String, String)],
    announced: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let current: HashSet<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let added: Vec<String> = entries
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| !announced.contains(name))
        .collect();
    let removed: Vec<String> = {
        let mut out: Vec<String> = announced
            .iter()
            .filter(|name| !current.contains(*name))
            .cloned()
            .collect();
        out.sort();
        out
    };
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrading_a_legacy_session_keeps_its_prefix_without_duplicate_native_echo() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::MessageAppended {
                    message: Message::user("legacy question"),
                },
            ),
            event(
                sid,
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant("legacy answer"),
                },
            ),
            event(
                sid,
                2,
                EventPayload::MessageAppended {
                    message: Message::user("new question"),
                },
            ),
            event(
                sid,
                3,
                EventPayload::ResponseItemAppended {
                    item: ResponseInputItem::Message {
                        role: "user".into(),
                        content: "new question".into(),
                    },
                },
            ),
        ];
        let items = response_items_from_events(&events);
        assert_eq!(items.len(), 3);
        assert!(
            matches!(&items[0], ResponseInputItem::Message { content, .. } if content.contains("legacy question"))
        );
        assert!(
            matches!(&items[2], ResponseInputItem::Message { content, .. } if content.contains("[id:u2]"))
        );
        let messages = model_history_from_events(&events);
        assert_eq!(messages[0].content, "legacy question");
        assert_eq!(messages[2].content, "new question");
    }

    #[test]
    fn surface_patches_also_replay_on_legacy_tool_outputs() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "legacy-c1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path":"a.txt"}),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ToolCallCompleted {
                    call_id: "legacy-c1".into(),
                    ok: true,
                    output: serde_json::json!("original"),
                    duration_ms: 1,
                },
            ),
            event(
                sid,
                2,
                EventPayload::ContextSurfacePatched {
                    version: 1,
                    strategy: "micro_compact".into(),
                    replacements: vec![deepagent_core::event::ToolResultReplacement {
                        source_sequence: 1,
                        call_id: "legacy-c1".into(),
                        replacement_output: "cleared".into(),
                    }],
                },
            ),
        ];
        let items = response_items_from_events(&events);
        assert!(
            matches!(&items[1], ResponseInputItem::FunctionCallOutput { output, .. } if output == "cleared")
        );
        assert_eq!(model_history_from_events(&events)[1].content, "cleared");
    }
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
        let convo = full_history_from_events(&events);
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
    fn abort_reminder_is_projected_only_after_latest_cancelled_terminal() {
        let sid = SessionId::nil();
        let cancelled = vec![event(
            sid,
            0,
            EventPayload::RunTerminal {
                kind: "cancelled".into(),
                reason: Some("cancelled by user".into()),
            },
        )];
        let mut messages = vec![Message::assistant("partial")];
        let mut items = vec![ResponseInputItem::Message {
            role: "assistant".into(),
            content: "partial".into(),
        }];
        append_abort_reminder_if_needed(&cancelled, &mut messages, &mut items);
        assert!(messages
            .last()
            .is_some_and(|message| { message.content.contains("previous turn was aborted") }));
        assert!(items.last().is_some_and(|item| matches!(
            item,
            ResponseInputItem::Message { role, content }
                if role == "user" && content.contains("previous turn was aborted")
        )));

        let succeeded = vec![event(
            sid,
            1,
            EventPayload::RunTerminal {
                kind: "succeeded".into(),
                reason: None,
            },
        )];
        let before = messages.len();
        append_abort_reminder_if_needed(&succeeded, &mut messages, &mut items);
        assert_eq!(messages.len(), before);
    }

    #[test]
    fn collect_surfaced_memory_ids_unions_deltas_in_first_seen_order() {
        // Scenario: two runs of one session each injected memories; a third run
        // must see the union so the provider is told to skip both. Order is
        // preserved because injection budget accounting depends on it.
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::MemoriesInjected {
                    ids: vec!["a".into(), "b".into()],
                    count: 2,
                    latency_ms: 10,
                    trigger: "prefetch".into(),
                },
            ),
            event(
                sid,
                1,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            ),
            event(
                sid,
                2,
                EventPayload::MemoriesInjected {
                    ids: vec!["b".into(), "c".into()],
                    count: 2,
                    latency_ms: 12,
                    trigger: "prefetch".into(),
                },
            ),
        ];
        assert_eq!(
            collect_surfaced_memory_ids_from_events(&events),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn collect_surfaced_memory_ids_is_empty_without_injections() {
        let sid = SessionId::nil();
        let events = vec![event(
            sid,
            0,
            EventPayload::MemoryEntryWritten {
                id: "m-1".into(),
                tier: "failure".into(),
                scope: "global".into(),
                importance: Some(0.9),
            },
        )];
        // A write is not a surfacing: it must not enter the de-dup set.
        assert!(collect_surfaced_memory_ids_from_events(&events).is_empty());
        assert!(collect_surfaced_memory_ids_from_events(&[]).is_empty());
    }

    fn boundary(summary: &str, keep_from: Option<u64>) -> EventPayload {
        EventPayload::ContextCompacted {
            tokens_before: 5_000,
            tokens_after: 400,
            strategy: "model".to_string(),
            summary: Some(summary.to_string()),
            first_kept_sequence: keep_from,
        }
    }

    fn conversation_fixture(sid: SessionId) -> Vec<Event> {
        vec![
            event(
                sid,
                0,
                EventPayload::MessageAppended {
                    message: Message::user("first question"),
                },
            ),
            event(
                sid,
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant("first answer"),
                },
            ),
            event(
                sid,
                2,
                EventPayload::MessageAppended {
                    message: Message::user("later question"),
                },
            ),
        ]
    }

    #[test]
    fn model_history_replaces_the_compacted_prefix_with_its_summary() {
        let sid = SessionId::nil();
        let mut events = conversation_fixture(sid);
        events.push(event(sid, 3, boundary("goal: ship it", Some(2))));

        let history = model_history_from_events(&events);
        assert_eq!(history.len(), 2, "summary + the kept tail: {history:?}");
        assert_eq!(history[0].role, Role::User);
        assert!(
            history[0].content.starts_with(COMPACTED_SUMMARY_HEADER)
                && history[0].content.contains("goal: ship it"),
            "{:?}",
            history[0].content
        );
        assert_eq!(history[1].content, "later question");
    }

    #[test]
    fn both_views_agree_on_where_history_starts() {
        // The regression this closes: only one projection honoured compaction, so
        // the model kept replaying the whole transcript while titling and
        // /context believed a summary had replaced it.
        let sid = SessionId::nil();
        let mut events = conversation_fixture(sid);
        events.push(event(sid, 3, boundary("goal: ship it", Some(2))));

        let messages = model_history_from_events(&events);
        let items = response_items_from_events(&events);
        assert_eq!(messages.len(), 2);
        let item_texts: Vec<String> = items
            .iter()
            .filter_map(|item| match item {
                deepagent_core::response_item::ResponseItem::Message { content, .. } => {
                    Some(content.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(item_texts.len(), 2, "{item_texts:?}");
        assert!(item_texts[0].contains("goal: ship it"));
        assert_eq!(item_texts[1], "later question");
        assert_eq!(
            messages[1].content, item_texts[1],
            "the two views must not disagree about the kept tail"
        );
    }

    #[test]
    fn boundary_without_keep_from_keeps_only_what_follows_it() {
        // Manual compaction writes this shape: the summary replaces everything
        // before the boundary event, and no guessing of message->event indexes.
        let sid = SessionId::nil();
        let mut events = conversation_fixture(sid);
        events.push(event(sid, 3, boundary("goal: ship it", None)));
        events.push(event(
            sid,
            4,
            EventPayload::MessageAppended {
                message: Message::user("next prompt"),
            },
        ));

        let history = model_history_from_events(&events);
        assert_eq!(history.len(), 2, "{history:?}");
        assert!(history[0].content.contains("goal: ship it"));
        assert_eq!(history[1].content, "next prompt");
    }

    #[test]
    fn legacy_marker_messages_are_honoured_by_both_views() {
        // Sessions compacted by an older build stored the boundary as a message.
        let sid = SessionId::nil();
        let mut events = conversation_fixture(sid);
        events.push(event(
            sid,
            3,
            EventPayload::MessageAppended {
                message: Message::user(format!("{COMPACTED_SUMMARY_HEADER}\nold summary")),
            },
        ));

        let history = model_history_from_events(&events);
        assert_eq!(history.len(), 1, "only the marker survives: {history:?}");
        assert!(history[0].content.contains("old summary"));
        let items = response_items_from_events(&events);
        assert_eq!(items.len(), 1, "item view must match: {items:?}");
    }

    #[test]
    fn full_history_keeps_everything_across_a_boundary() {
        // Export and titling read the complete transcript on purpose; a boundary
        // changes the model's view, never the record.
        let sid = SessionId::nil();
        let mut events = conversation_fixture(sid);
        events.push(event(sid, 3, boundary("goal: ship it", Some(2))));

        let full = full_history_from_events(&events);
        assert_eq!(full.len(), 3, "{full:?}");
        assert_eq!(full[0].content, "first question");
        assert_eq!(full[1].content, "first answer");
        assert_eq!(full[2].content, "later question");
        // and the model view really is smaller than it
        assert_eq!(model_history_from_events(&events).len(), 2);
    }

    #[test]
    fn sequence_of_message_maps_history_entries_back_to_events() {
        // The automatic compactor splits in message space but must record the
        // boundary in event space, so a history entry has to resolve to the event
        // that produced it — on both log shapes.
        let sid = SessionId::nil();
        let plain = conversation_fixture(sid);
        assert_eq!(
            sequence_of_message(
                &plain,
                &Message::text(Role::Assistant, "first answer".to_string())
            ),
            Some(1)
        );
        assert_eq!(
            sequence_of_message(&plain, &Message::user("never said".to_string())),
            None,
            "an entry that is not in the log must not invent a boundary"
        );

        let item_sid = SessionId::nil();
        let item_events = vec![event(
            item_sid,
            7,
            EventPayload::ResponseItemAppended {
                item: deepagent_core::response_item::ResponseItem::Message {
                    role: "user".to_string(),
                    content: "from the item log".to_string(),
                },
            },
        )];
        assert_eq!(
            sequence_of_message(
                &item_events,
                &Message::user("from the item log".to_string())
            ),
            Some(7),
            "provider-native sessions resolve through their item events"
        );
    }

    #[test]
    fn boundary_never_splits_a_tool_call_from_its_result() {
        // Whatever the writer chooses, the resulting model view must stay
        // API-legal: no tool result without its call, no orphan call.
        let sid = SessionId::nil();
        let mut events = vec![
            event(
                sid,
                0,
                EventPayload::MessageAppended {
                    message: Message::user("investigate"),
                },
            ),
            event(
                sid,
                1,
                EventPayload::ToolCallRequested {
                    call: deepagent_core::message::ToolCall {
                        id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({"path": "a.txt"}),
                    },
                },
            ),
            event(
                sid,
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "call-1".to_string(),
                    ok: true,
                    output: serde_json::json!("contents"),
                    duration_ms: 5,
                },
            ),
            event(
                sid,
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant("done"),
                },
            ),
        ];
        events.push(event(sid, 4, boundary("phase one summary", Some(1))));

        let history = model_history_from_events(&events);
        let text = history
            .iter()
            .map(|m| format!("{:?}:{}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            text.contains("call-1") || text.contains("contents"),
            "tool pair must survive intact: {text}"
        );
        // Cutting at the request (sequence 1) keeps the request AND its result.
        let history2 = {
            let mut e = events.clone();
            if let Some(last) = e.last_mut() {
                if let EventPayload::ContextCompacted {
                    first_kept_sequence,
                    ..
                } = &mut last.payload
                {
                    *first_kept_sequence = Some(2);
                }
            }
            model_history_from_events(&e)
        };
        let orphan = history2.iter().any(|m| {
            m.role == Role::Tool
                && !history2
                    .iter()
                    .any(|o| o.tool_calls.iter().any(|c| c.id == "call-1"))
        });
        assert!(
            !orphan,
            "a boundary must not leave a tool result without its call: {history2:?}"
        );
    }

    #[test]
    fn manual_compaction_boundary_survives_a_reload() {
        // End-to-end shape: /compact writes a boundary into the session log, and
        // the NEXT run rebuilds its history from the reloaded events and honours
        // it — the whole point of making the boundary durable.
        let db = Database::open_in_memory().unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut session = Session::create(&db, &clock, Some("compact-me")).expect("create session");
        for turn in 0..8 {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(format!("question {turn}")),
                })
                .unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::assistant(format!("answer {turn}")),
                })
                .unwrap();
        }
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        let history = model_history_from_events(&events);
        assert_eq!(history.len(), 16, "precondition: nothing compacted yet");

        let transcript_dir = session.transcript_dir();
        let (before, after) = crate::slash_runtime::append_heuristic_compaction(
            &mut session,
            &history,
            deepagent_core::compaction_strategy::MANUAL,
        )
        .expect("manual compaction");
        assert!(before > 0, "tokens counted from the real history");
        // The deterministic summarizer keeps the head turns and drops chatter;
        // it must never make the context bigger.
        assert!(after <= before, "{before} -> {after}");

        let reloaded = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        let compacted = model_history_from_events(&reloaded);
        assert_eq!(compacted.len(), 1, "{compacted:?}");
        assert!(
            compacted[0].content.starts_with(COMPACTED_SUMMARY_HEADER),
            "{:?}",
            compacted[0].content
        );
        // The model's only way back to detail the summary dropped is the pointer
        // to the verbatim log, and it has to survive the reload (§5.1).
        let pointer = transcript_dir.display().to_string();
        assert!(
            compacted[0].content.contains(&pointer),
            "boundary summary lost the transcript pointer: {:?}",
            compacted[0].content
        );
        // The pointer must name a directory that actually holds the log, not just
        // any path — the model is meant to open it.
        let files: Vec<String> = std::fs::read_dir(&transcript_dir)
            .expect("transcript directory exists")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            files.iter().any(|name| name.ends_with(".jsonl.zst")),
            "pointer names a directory with no event log: {files:?}"
        );
        // The raw turns are still in the log for export / replay.
        assert_eq!(full_history_from_events(&reloaded).len(), 16);
    }

    /// Phase 2 DoD, the mechanism half of "第二次运行不再产生总结模型调用": the
    /// trigger is computed on the model view, so a durable boundary must make the
    /// next run decide not to compact. The other half — that a note-based boundary
    /// really issues no request — is pinned by
    /// `run_assembler::compaction_source_tests::a_covered_note_compacts_without_calling_the_model`.
    #[test]
    fn a_durable_boundary_stops_the_next_run_from_re_triggering_compaction() {
        let db = Database::open_in_memory().unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut session = Session::create(&db, &clock, Some("re-trigger")).expect("create session");
        for turn in 0..8 {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(format!(
                        "question {turn} carrying enough prose to be counted as real context"
                    )),
                })
                .unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::assistant(format!(
                        "answer {turn} carrying enough prose to be counted as real context"
                    )),
                })
                .unwrap();
        }

        // The same rendering and counter the run assembler uses for its decision.
        let counter = deepagent_context::HeuristicTokenizer::new();
        let total = |history: &[Message]| -> usize {
            history
                .iter()
                .map(|message| {
                    deepagent_context::TokenCounter::count(
                        &counter,
                        &format!("{:?}: {}", message.role, message.content),
                    )
                })
                .sum()
        };

        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let events = store.load_session(session.id()).unwrap();
        let history = model_history_from_events(&events);
        let before = total(&history);
        crate::slash_runtime::append_heuristic_compaction(
            &mut session,
            &history,
            deepagent_core::compaction_strategy::MANUAL,
        )
        .expect("manual compaction");

        let reloaded = store.load_session(session.id()).unwrap();
        let after = total(&model_history_from_events(&reloaded));
        assert!(
            after < before,
            "the boundary must shrink what the next run measures: {before} -> {after}"
        );

        // Any trigger between the two totals compacts on the first run and takes
        // no summarizing call on the second, which is exactly what the boundary is
        // for.
        let policy = deepagent_context::CompactionPolicy {
            trigger_tokens: (before + after) / 2,
            ..Default::default()
        };
        assert!(
            policy.should_compact(before),
            "the pre-boundary view must exceed the trigger: {before} <= {}",
            policy.trigger_tokens
        );
        assert!(
            !policy.should_compact(after),
            "the reloaded view re-triggers compaction: {after} > {}",
            policy.trigger_tokens
        );
    }

    #[test]
    fn rebuild_reapplies_persisted_snips() {
        let sid = SessionId::nil();
        let user = |text: &str| EventPayload::MessageAppended {
            message: Message::user(text),
        };
        let assistant = |text: &str| EventPayload::MessageAppended {
            message: Message::assistant(text),
        };
        // 12 messages so the protected tail (last 8) starts at index 4, leaving
        // the tagged segment at index 1 removable.
        let mut events = vec![
            event(sid, 0, user("first")),
            event(sid, 1, user("[id:u1] old task")),
            event(sid, 2, assistant("old answer")),
            event(sid, 3, user("[id:u2] live task")),
        ];
        for i in 0..8u64 {
            events.push(event(sid, 4 + i, assistant(&format!("filler {i}"))));
        }

        let full = model_history_from_events(&events);
        assert!(full.iter().any(|m| m.content.contains("old task")));

        events.push(event(
            sid,
            events.len() as u64,
            EventPayload::ContextSnipped {
                tags: vec!["u1".to_string()],
            },
        ));
        let snipped = model_history_from_events(&events);
        assert!(!snipped.iter().any(|m| m.content.contains("old task")));
        assert!(!snipped.iter().any(|m| m.content.contains("old answer")));
        assert!(snipped.iter().any(|m| m.content.contains("live task")));
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

        let convo = model_history_from_events(&events);
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

    /// A cut that lands inside a tool pair must never hand the model a result with
    /// no matching call: the API rejects such a request, and the boundary code's own
    /// rule is that keeping one extra turn is cheaper than breaking a pair. The
    /// crash-shaped orphan (a call with no result) already has a test; this is the
    /// compaction-shaped one.
    #[test]
    fn a_boundary_cut_inside_a_tool_pair_leaves_no_orphan_result() {
        let sid = SessionId::nil();
        let call = deepagent_core::message::ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
        };
        let events = vec![
            event(
                sid,
                0,
                EventPayload::MessageAppended {
                    message: Message::user("read a.txt"),
                },
            ),
            event(sid, 1, EventPayload::ToolCallRequested { call }),
            event(
                sid,
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "call-1".into(),
                    ok: true,
                    output: serde_json::json!({"status": "ok"}),
                    duration_ms: 3,
                },
            ),
            event(
                sid,
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant("read it"),
                },
            ),
            // The boundary asks to start at the result, which skips its call.
            event(sid, 4, boundary("goal: read the file", Some(2))),
        ];

        let convo = model_history_from_events(&events);
        let mut announced: std::collections::HashSet<String> = std::collections::HashSet::new();
        for message in &convo {
            for tool_call in &message.tool_calls {
                announced.insert(tool_call.id.clone());
            }
            if message.role == Role::Tool {
                assert!(
                    message
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| announced.contains(id)),
                    "a tool result reached the model without its call: {convo:?}"
                );
            }
        }
        // Dropping the unmatched result is the reader's fallback: the boundary
        // writer is the place that keeps pairs whole (see `sequence_of_message`),
        // and a summary standing in for the prefix already covers this turn.
        assert_eq!(
            convo.len(),
            2,
            "only the summary and the surviving turn may remain: {convo:?}"
        );

        // The item view is what a Responses request is built from, so the same
        // rule has to hold there.
        let items = response_items_from_events(&events);
        let mut announced_items: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for item in &items {
            if let deepagent_core::response_item::ResponseItem::FunctionCall { call_id, .. } = item
            {
                announced_items.insert(call_id.clone());
            }
            if let deepagent_core::response_item::ResponseItem::FunctionCallOutput {
                call_id, ..
            } = item
            {
                assert!(
                    announced_items.contains(call_id),
                    "an orphan FunctionCallOutput would be rejected by the API: {items:?}"
                );
            }
        }
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

        let convo = model_history_from_events(&events);
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

        let convo = model_history_from_events(&events);
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

        let convo = model_history_from_events(&events);

        assert_eq!(convo.len(), 4);
        assert_eq!(convo[0].role, Role::User);
        assert_eq!(convo[1].tool_calls[0].id, "call-1");
        assert_eq!(convo[2].role, Role::Tool);
        assert_eq!(convo[3].content, "done");
        assert_eq!(
            convo[1].reasoning_content.as_deref(),
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
    fn native_response_replay_applies_persisted_snip_without_losing_live_calls() {
        let sid = SessionId::nil();
        let mut events = vec![
            event(
                sid,
                0,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "user".into(),
                        content: "[id:u1] old".into(),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCall {
                        call_id: "old".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"old"}"#.into(),
                    },
                },
            ),
            event(
                sid,
                2,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCallOutput {
                        call_id: "old".into(),
                        output: "old result".into(),
                    },
                },
            ),
            event(
                sid,
                3,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "assistant".into(),
                        content: "old done".into(),
                    },
                },
            ),
            event(
                sid,
                4,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "user".into(),
                        content: "[id:u2] live".into(),
                    },
                },
            ),
        ];
        for index in 5..12 {
            events.push(event(
                sid,
                index,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message {
                        role: "assistant".into(),
                        content: format!("tail {index}"),
                    },
                },
            ));
        }
        events.push(event(
            sid,
            12,
            EventPayload::ContextSnipped {
                tags: vec!["u1".into()],
            },
        ));

        let items = response_items_from_events(&events);
        assert!(!items.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCall { call_id, .. } if call_id == "old"
        )));
        assert!(items.iter().any(|item| matches!(
            item,
            ResponseItem::Message { content, .. } if content.contains("[id:u2]")
        )));
    }

    #[test]
    fn native_response_replay_applies_tool_result_surface_patch() {
        let sid = SessionId::nil();
        let events = vec![
            event(
                sid,
                0,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCall {
                        call_id: "c1".into(),
                        name: "read_file".into(),
                        arguments: "{}".into(),
                    },
                },
            ),
            event(
                sid,
                1,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::FunctionCallOutput {
                        call_id: "c1".into(),
                        output: "full result".into(),
                    },
                },
            ),
            event(
                sid,
                2,
                EventPayload::ContextSurfacePatched {
                    version: 1,
                    strategy: "micro_compact".into(),
                    replacements: vec![deepagent_core::event::ToolResultReplacement {
                        source_sequence: 1,
                        call_id: "c1".into(),
                        replacement_output: "[tool result content cleared]".into(),
                    }],
                },
            ),
        ];
        let items = response_items_from_events(&events);
        assert!(items.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCallOutput { call_id, output }
                if call_id == "c1" && output == "[tool result content cleared]"
        )));
    }

    /// A boundary records the sequence its model view starts at. Forking copies a
    /// session by *re-appending* the prefix, so that recorded number is only
    /// meaningful while the copy keeps the source's numbering — an assumption
    /// neither layer states or enforces. If it ever breaks, a forked session
    /// quietly shows less history than it has, so pin the invariant here.
    #[test]
    fn a_forked_session_keeps_the_boundary_cut_point() {
        let db = Database::open_in_memory().unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut source = deepagent_session::Session::create(&db, &clock, Some("source")).unwrap();
        let source_id = source.id();
        for message in [
            Message::user("first question"),
            Message::assistant("first answer"),
            Message::user("second question"),
            Message::assistant("second answer"),
        ] {
            source
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        let store = deepagent_persistence::event_store::EventStore::new(&db);
        // The cut is taken from the real stream: a session log starts with its own
        // bookkeeping events, so absolute sequence numbers cannot be assumed.
        let prefix = store.load_session(source_id).unwrap();
        let cut = sequence_of_message(&prefix, &Message::user("second question"))
            .expect("the appended turn must map back to an event");
        let boundary_sequence = prefix
            .last()
            .expect("the prefix holds the turns")
            .sequence
            .saturating_add(1);
        source
            .append(EventPayload::ContextCompacted {
                tokens_before: 9_000,
                tokens_after: 2_000,
                strategy: "session_memory".to_string(),
                summary: Some("- decided: one vault".to_string()),
                first_kept_sequence: Some(cut),
            })
            .unwrap();
        let source_events = store.load_session(source_id).unwrap();
        let before = model_history_from_events(&source_events);
        assert_eq!(
            before.len(),
            3,
            "the view is the summary plus the turns at or after the cut: {before:?}"
        );

        let forked =
            deepagent_session::Session::fork(&db, &clock, source_id, boundary_sequence).unwrap();
        let copied = store.load_session(forked.id()).unwrap();
        assert_eq!(
            copied
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            source_events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            "the copied prefix must keep the numbering the boundary refers to"
        );
        let after = model_history_from_events(&copied);
        assert_eq!(
            after.len(),
            before.len(),
            "a fork at the boundary must show the window the source showed: {after:?}"
        );
        assert!(
            after[0].content.contains("- decided: one vault"),
            "the fork must inherit the summary standing in for the prefix: {after:?}"
        );

        // Forking before the boundary leaves it out, which is the only
        // acceptable way to lose a cut point: with no boundary the view keeps its
        // whole (shorter) prefix, never a stale slice of it.
        let early = deepagent_session::Session::fork(&db, &clock, source_id, cut).unwrap();
        let early_events = store.load_session(early.id()).unwrap();
        assert!(
            !early_events
                .iter()
                .any(|event| matches!(event.payload, EventPayload::ContextCompacted { .. })),
            "the early fork must stop before the boundary"
        );
        let early_view = model_history_from_events(&early_events);
        // The invariant is that nothing gets sliced by a stale number: the turn
        // that sits BEFORE any plausible cut point must still be there. Exact
        // counts depend on unrelated conversation rules, so they are not asserted.
        assert!(
            early_view
                .iter()
                .any(|message| message.content == "first question"),
            "a fork with no boundary must not be sliced by the source's cut point: {early_view:?}"
        );
        assert!(
            !early_view
                .iter()
                .any(|message| message.content.contains("decided: one vault")),
            "no boundary means no summary standing in for a prefix: {early_view:?}"
        );
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
            Some("G:/Code/Kotlin_code"),
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
