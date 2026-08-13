use std::sync::Arc;

use deepagent_persistence::runtime_log_store::{NewRuntimeLogEntry, RuntimeLogStore};
use deepagent_runtime::RuntimeEvent;

pub(crate) fn append_runtime_log(
    logs: &Option<Arc<RuntimeLogStore>>,
    mut entry: NewRuntimeLogEntry,
) {
    let Some(logs) = logs else {
        return;
    };
    entry.data = redact_runtime_log_value(entry.data);
    entry.message = entry
        .message
        .take()
        .map(|message| redact_runtime_log_text(&message));
    if let Err(err) = logs.append(entry) {
        tracing::warn!(error = %err, "failed to append runtime diagnostic log");
    }
}

pub(crate) fn spawn_runtime_event_pump<F>(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    logs: Option<Arc<RuntimeLogStore>>,
    run_id: String,
    session_id: String,
    on_event: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(RuntimeEvent) + Send + 'static,
{
    tokio::spawn(async move {
        let mut delta_buffer = RuntimeLogDeltaBuffer::default();
        while let Some(ev) = rx.recv().await {
            match &ev {
                RuntimeEvent::ContentDelta { text } => {
                    delta_buffer.push(&logs, &run_id, &session_id, "content_delta", text)
                }
                RuntimeEvent::ReasoningDelta { text } => {
                    delta_buffer.push(&logs, &run_id, &session_id, "reasoning_delta", text)
                }
                _ => {
                    delta_buffer.flush(&logs, &run_id, &session_id);
                    append_runtime_log(&logs, runtime_event_log_entry(&run_id, &session_id, &ev));
                }
            }
            on_event(ev);
        }
        delta_buffer.flush(&logs, &run_id, &session_id);
    })
}

pub(crate) fn redact_runtime_log_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let redacted = if is_secret_log_key(&key) {
                        serde_json::Value::String("<redacted>".into())
                    } else if is_prompt_log_key(&key) {
                        redact_text_payload(value)
                    } else {
                        redact_runtime_log_value(value)
                    };
                    (key, redacted)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(redact_runtime_log_value).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(scrub_secret_literals(&text)),
        other => other,
    }
}

fn is_secret_log_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("api_key")
        || key.contains("apikey")
        || key.contains("authorization")
        || key.contains("password")
        || key.contains("secret")
        || key == "token"
        || key.ends_with("_token")
}

fn is_prompt_log_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "prompt"
            | "raw_prompt"
            | "effective_prompt"
            | "final_user_prompt"
            | "system_prompt"
            | "content"
            | "text"
            | "reasoning"
            | "reasoning_text"
    )
}

fn redact_text_payload(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::json!({
            "redacted": true,
            "chars": text.chars().count(),
        }),
        other => redact_runtime_log_value(other),
    }
}

fn scrub_secret_literals(input: &str) -> String {
    let mut out = Vec::new();
    let mut redact_next = false;
    for token in input.split_whitespace() {
        if redact_next {
            out.push("<redacted>");
            redact_next = token.eq_ignore_ascii_case("bearer");
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if lower == "bearer" || lower == "authorization:" {
            out.push(token);
            redact_next = true;
            continue;
        }
        if token.starts_with("sk-")
            || lower.contains("api_key=")
            || lower.contains("apikey=")
            || lower.contains("password=")
            || lower.contains("secret=")
        {
            out.push("<redacted>");
        } else {
            out.push(token);
        }
    }
    let scrubbed = out.join(" ");
    if scrubbed.chars().count() > 2_000 {
        format!(
            "{}...[truncated, chars={}]",
            scrubbed.chars().take(2_000).collect::<String>(),
            scrubbed.chars().count()
        )
    } else {
        scrubbed
    }
}

fn redact_runtime_log_text(input: &str) -> String {
    scrub_secret_literals(input)
}

fn runtime_event_log_entry(
    run_id: &str,
    session_id: &str,
    event: &RuntimeEvent,
) -> NewRuntimeLogEntry {
    let category = runtime_event_category(event);
    let level = runtime_event_level(event).to_string();
    let mut entry = NewRuntimeLogEntry::info(category, responses_diagnostic_event(event))
        .with_run_id(run_id)
        .with_session_id(session_id)
        .with_source("deepagent-runtime")
        .with_message(runtime_event_message(event))
        .with_data(serde_json::to_value(event).unwrap_or_else(|err| {
            serde_json::json!({
                "serialization_error": err.to_string(),
                "event": event.label(),
            })
        }));
    entry.level = level;
    if let Some(task_id) = runtime_event_task_id(event) {
        entry = entry.with_task_id(task_id);
    }
    if let Some(correlation_id) = runtime_event_correlation_id(event) {
        entry = entry.with_correlation_id(correlation_id);
    }
    entry
}

fn responses_diagnostic_event(event: &RuntimeEvent) -> &str {
    match event {
        RuntimeEvent::ModelRequestStarted { .. } => "responses_request_started",
        RuntimeEvent::ModelRequestCompleted { .. } => "responses_completed",
        RuntimeEvent::Usage { .. } => "responses_usage_received",
        RuntimeEvent::ResponsesStreamEvent {
            event_type,
            item_type,
            ..
        } => match event_type.as_str() {
            "response.output_item.added" => "responses_output_item_added",
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                "responses_function_call_completed"
            }
            "response.output_item.done"
                if matches!(
                    item_type.as_deref(),
                    Some("function_call" | "custom_tool_call")
                ) =>
            {
                "responses_function_call_completed"
            }
            "response.completed" => "responses_completed",
            "response.incomplete" => "responses_incomplete",
            "response.failed" => "responses_failed",
            _ => "responses_stream_event",
        },
        _ => event.label(),
    }
}

#[derive(Default)]
struct RuntimeLogDeltaBuffer {
    event: Option<&'static str>,
    text: String,
    chunks: usize,
}

impl RuntimeLogDeltaBuffer {
    fn push(
        &mut self,
        logs: &Option<Arc<RuntimeLogStore>>,
        run_id: &str,
        session_id: &str,
        event: &'static str,
        text: &str,
    ) {
        if self.event.is_some_and(|current| current != event) || self.text.len() >= 1024 {
            self.flush(logs, run_id, session_id);
        }
        self.event = Some(event);
        self.text.push_str(text);
        self.chunks += 1;
    }

    fn flush(&mut self, logs: &Option<Arc<RuntimeLogStore>>, run_id: &str, session_id: &str) {
        if self.text.is_empty() {
            return;
        }
        let event = self.event.unwrap_or("content_delta");
        let text = std::mem::take(&mut self.text);
        let chunks = std::mem::replace(&mut self.chunks, 0);
        self.event = None;
        append_runtime_log(
            logs,
            NewRuntimeLogEntry::info("model", format!("{event}_batch"))
                .with_run_id(run_id)
                .with_session_id(session_id)
                .with_source("deepagent-runtime")
                .with_message(format!("batched {chunks} {event} chunk(s)"))
                .with_data(serde_json::json!({
                    "event": event,
                    "text": text,
                    "chunks": chunks,
                })),
        );
    }
}

fn runtime_event_category(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ModelFirstToken { .. }
        | RuntimeEvent::ModelRequestCompleted { .. }
        | RuntimeEvent::ModelAttemptReset { .. }
        | RuntimeEvent::ReasoningDelta { .. }
        | RuntimeEvent::ContentDelta { .. }
        | RuntimeEvent::ResponsesStreamEvent { .. }
        | RuntimeEvent::ResponsesWebSearchCall { .. }
        | RuntimeEvent::Usage { .. } => "model",
        RuntimeEvent::ToolStarted { .. }
        | RuntimeEvent::ToolCompleted { .. }
        | RuntimeEvent::ToolBlocked { .. } => "tool",
        RuntimeEvent::HookStarted { .. } | RuntimeEvent::HookCompleted { .. } => "hook",
        RuntimeEvent::SubagentStarted { .. }
        | RuntimeEvent::SubagentCompleted { .. }
        | RuntimeEvent::SubagentCancelled { .. }
        | RuntimeEvent::SubagentNotification { .. }
        | RuntimeEvent::WorktreeCreated { .. }
        | RuntimeEvent::WorktreeRemoved { .. } => "subagent",
        RuntimeEvent::RunCancelled => "cancel",
        RuntimeEvent::RunStarted { .. }
        | RuntimeEvent::SessionRegistered { .. }
        | RuntimeEvent::TurnStarted { .. }
        | RuntimeEvent::RunCompleted { .. }
        | RuntimeEvent::RunAwaitingApproval { .. }
        | RuntimeEvent::RunFailed { .. } => "runtime",
        RuntimeEvent::Verification { .. } | RuntimeEvent::CompletionEvidence { .. } => {
            "verification"
        }
        RuntimeEvent::ContextUsage { .. }
        | RuntimeEvent::ContextCompacted { .. }
        | RuntimeEvent::RelevantMemoriesInjected { .. }
        | RuntimeEvent::StallNudgeInjected { .. } => "context",
    }
}

fn runtime_event_level(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::RunFailed { .. } => "error",
        RuntimeEvent::ToolCompleted { ok, .. } if !ok => "warn",
        RuntimeEvent::ToolBlocked { needs_approval, .. } => {
            if *needs_approval {
                "info"
            } else {
                "warn"
            }
        }
        RuntimeEvent::HookCompleted { outcome, .. }
            if outcome == "blocked" || outcome == "error" =>
        {
            "warn"
        }
        RuntimeEvent::RunAwaitingApproval { .. } => "info",
        RuntimeEvent::RunCancelled => "warn",
        RuntimeEvent::SubagentCompleted { state, .. } if state == "failed" => "error",
        RuntimeEvent::SubagentCancelled { .. } => "warn",
        _ => "info",
    }
}

fn runtime_event_task_id(event: &RuntimeEvent) -> Option<String> {
    match event {
        RuntimeEvent::RunStarted { task_id } => Some(task_id.clone()),
        _ => None,
    }
}

fn runtime_event_correlation_id(event: &RuntimeEvent) -> Option<String> {
    match event {
        RuntimeEvent::ToolStarted { call_id, .. } | RuntimeEvent::ToolCompleted { call_id, .. } => {
            Some(call_id.clone())
        }
        RuntimeEvent::HookStarted { id, .. } | RuntimeEvent::HookCompleted { id, .. } => {
            Some(id.clone())
        }
        RuntimeEvent::SubagentStarted { id, .. }
        | RuntimeEvent::SubagentCompleted { id, .. }
        | RuntimeEvent::SubagentCancelled { id, .. }
        | RuntimeEvent::SubagentNotification { id, .. } => Some(id.clone()),
        RuntimeEvent::WorktreeCreated { subagent_id, .. }
        | RuntimeEvent::WorktreeRemoved { subagent_id, .. } => Some(subagent_id.clone()),
        _ => None,
    }
}

fn runtime_event_message(event: &RuntimeEvent) -> String {
    match event {
        RuntimeEvent::RunStarted { task_id } => format!("run started for task {task_id}"),
        RuntimeEvent::SessionRegistered { session_id, .. } => {
            format!("session registered {session_id}")
        }
        RuntimeEvent::TurnStarted { step } => format!("turn {step} started"),
        RuntimeEvent::ModelRequestStarted {
            step,
            model,
            message_count,
            tool_count,
            ..
        } => format!(
            "model request started step={step} model={model} messages={message_count} tools={tool_count}"
        ),
        RuntimeEvent::ModelFirstToken {
            step,
            kind,
            elapsed_ms,
        } => format!("first {kind} token at step {step} after {elapsed_ms}ms"),
        RuntimeEvent::ModelRequestCompleted { step, elapsed_ms } => {
            format!("model request completed step={step} elapsed={elapsed_ms}ms")
        }
        RuntimeEvent::ModelAttemptReset {
            step,
            attempt,
            reason,
        } => format!("model stream reset step={step} attempt={attempt}: {reason}"),
        RuntimeEvent::ReasoningDelta { text } => {
            format!("reasoning delta {} chars", text.chars().count())
        }
        RuntimeEvent::ContentDelta { text } => {
            format!("content delta {} chars", text.chars().count())
        }
        RuntimeEvent::ResponsesStreamEvent {
            event_type,
            item_id,
            item_type,
            delta_chars,
        } => format!(
            "responses stream event type={event_type} item_id={} item_type={} delta_chars={}",
            item_id.as_deref().unwrap_or("none"),
            item_type.as_deref().unwrap_or("none"),
            delta_chars.map_or_else(|| "none".to_string(), |value| value.to_string()),
        ),
        RuntimeEvent::ResponsesWebSearchCall {
            call_id,
            status,
            action_type,
            queries_count,
        } => format!(
            "native web search call={call_id} status={status} action_type={} queries_count={queries_count}",
            action_type.as_deref().unwrap_or("none"),
        ),
        RuntimeEvent::ToolStarted { name, call_id, .. } => {
            format!("tool {name} started ({call_id})")
        }
        RuntimeEvent::ToolCompleted {
            name,
            call_id,
            ok,
            duration_ms,
            ..
        } => format!("tool {name} completed ok={ok} ({call_id}) in {duration_ms}ms"),
        RuntimeEvent::ToolBlocked {
            name,
            needs_approval,
            reason,
        } => format!("tool {name} blocked needs_approval={needs_approval}: {reason}"),
        RuntimeEvent::HookStarted {
            event, id, shell, ..
        } => {
            let shell = shell.as_deref().unwrap_or("auto");
            format!("hook {event} started shell={shell} ({id})")
        }
        RuntimeEvent::HookCompleted {
            event,
            id,
            shell,
            outcome,
            duration_ms,
            ..
        } => {
            let shell = shell.as_deref().unwrap_or("auto");
            format!("hook {event} completed shell={shell} outcome={outcome} ({id}) in {duration_ms}ms")
        }
        RuntimeEvent::SubagentStarted {
            id,
            agent_type,
            background,
            ..
        } => format!("sub-agent {id} started type={agent_type} background={background}"),
        RuntimeEvent::SubagentCompleted {
            id,
            state,
            duration_ms,
            ..
        } => format!("sub-agent {id} completed state={state} in {duration_ms}ms"),
        RuntimeEvent::SubagentCancelled {
            id, duration_ms, ..
        } => format!("sub-agent {id} cancelled in {duration_ms}ms"),
        RuntimeEvent::SubagentNotification { id, state, .. } => {
            format!("background sub-agent result ready id={id} state={state}")
        }
        RuntimeEvent::WorktreeCreated { subagent_id, path } => {
            format!("worktree created for {subagent_id}: {path}")
        }
        RuntimeEvent::WorktreeRemoved { subagent_id, path } => {
            format!("worktree removed for {subagent_id}: {path}")
        }
        RuntimeEvent::Verification { passed, detail } => {
            format!("verification passed={passed}: {detail}")
        }
        RuntimeEvent::CompletionEvidence { mutations } => {
            let changed = mutations
                .iter()
                .filter(|item| {
                    item.kind != deepagent_runtime::checkpoint::MutationKind::Unchanged
                })
                .count();
            format!(
                "completion evidence: {changed}/{} captured path(s) changed",
                mutations.len()
            )
        }
        RuntimeEvent::ContextUsage { .. } => "context usage snapshot".to_string(),
        RuntimeEvent::ContextCompacted {
            tokens_before,
            tokens_after,
            strategy,
            ..
        } => format!(
            "context compacted strategy={strategy} tokens={tokens_before}->{tokens_after}"
        ),
        RuntimeEvent::RelevantMemoriesInjected { count, latency_ms } => {
            format!("relevant memories injected count={count} latency_ms={latency_ms}")
        }
        RuntimeEvent::StallNudgeInjected {
            step,
            category,
            confidence,
            ..
        } => format!("stall nudge injected step={step} category={category} confidence={confidence}"),
        RuntimeEvent::Usage { total_tokens, .. } => format!("usage total_tokens={total_tokens}"),
        RuntimeEvent::RunCompleted { .. } => "run completed".to_string(),
        RuntimeEvent::RunAwaitingApproval { message } => {
            format!("run awaiting approval: {message}")
        }
        RuntimeEvent::RunFailed { reason } => format!("run failed: {reason}"),
        RuntimeEvent::RunCancelled => "run cancelled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_prompts_and_secrets() {
        let redacted = redact_runtime_log_value(serde_json::json!({
            "prompt": "delete secret",
            "nested": {
                "api_key": "sk-123",
                "message": "safe text",
                "token": "abc"
            },
            "array": ["password=abc", {"content": "model words"}]
        }));

        assert_eq!(redacted["prompt"]["redacted"], true);
        assert_eq!(redacted["nested"]["api_key"], "<redacted>");
        assert_eq!(redacted["nested"]["token"], "<redacted>");
        assert_eq!(redacted["nested"]["message"], "safe text");
        assert_eq!(redacted["array"][0], "<redacted>");
        assert_eq!(redacted["array"][1]["content"]["redacted"], true);
    }

    #[test]
    fn append_redacts_message_text() {
        let logs = Arc::new(RuntimeLogStore::open_in_memory().unwrap());
        append_runtime_log(
            &Some(logs.clone()),
            NewRuntimeLogEntry::info("model", "provider_error")
                .with_message("request failed with sk-test123 and Authorization: Bearer abc123")
                .with_data(serde_json::json!({ "ok": true })),
        );

        let entries = logs.recent(1).unwrap();
        let message = entries[0].message.as_deref().unwrap_or_default();
        assert!(message.contains("<redacted>"));
        assert!(!message.contains("sk-test123"));
        assert!(!message.contains("abc123"));
    }

    #[tokio::test]
    async fn event_pump_batches_content_and_forwards_events() {
        let logs = Arc::new(RuntimeLogStore::open_in_memory().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_for_pump = seen.clone();
        let pump = spawn_runtime_event_pump(
            rx,
            Some(logs.clone()),
            "run-test".to_string(),
            "session-test".to_string(),
            move |event| {
                seen_for_pump
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(event.label().to_string());
            },
        );

        tx.send(RuntimeEvent::ContentDelta { text: "he".into() })
            .unwrap();
        tx.send(RuntimeEvent::ContentDelta { text: "llo".into() })
            .unwrap();
        tx.send(RuntimeEvent::RunCompleted {
            message: "done".into(),
        })
        .unwrap();
        drop(tx);
        pump.await.unwrap();

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                "content_delta".to_string(),
                "content_delta".to_string(),
                "run_completed".to_string()
            ]
        );
        let entries = logs.recent(10).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.category == "model" && entry.event == "content_delta_batch"));
        assert!(entries
            .iter()
            .any(|entry| entry.category == "runtime" && entry.event == "run_completed"));
    }
}
