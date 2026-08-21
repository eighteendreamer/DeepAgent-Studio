use deepagent_runtime::RuntimeEvent;
use serde::{Deserialize, Serialize};

use crate::requests::PROTOCOL_VERSION;

/// Context supplied by the transport while projecting runtime events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl EventContext {
    pub fn new(thread_id: Option<String>, turn_id: Option<String>) -> Self {
        Self { thread_id, turn_id }
    }
}

/// Stable machine events. The runtime event log remains the source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HarnessEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted {
        #[serde(rename = "threadId")]
        thread_id: String,
        title: Option<String>,
        #[serde(rename = "protocolVersion")]
        protocol_version: u32,
    },
    #[serde(rename = "turn.started")]
    TurnStarted {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId")]
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<usize>,
    },
    #[serde(rename = "item.started")]
    ItemStarted {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(rename = "itemId", default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        item: ItemPayload,
    },
    #[serde(rename = "item.updated")]
    ItemUpdated {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(rename = "itemId", default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        item: ItemPayload,
    },
    #[serde(rename = "item.completed")]
    ItemCompleted {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(rename = "itemId", default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        item: ItemPayload,
    },
    #[serde(rename = "approval.requested")]
    ApprovalRequested {
        #[serde(
            rename = "approvalId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        approval_id: Option<String>,
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        message: String,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        reason: String,
    },
    #[serde(rename = "turn.interrupted")]
    TurnInterrupted {
        #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "error")]
    Error {
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemPayload {
    ContentDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCall {
        #[serde(rename = "callId")]
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    ToolResult {
        #[serde(rename = "callId")]
        call_id: String,
        ok: bool,
        output: serde_json::Value,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    Usage {
        #[serde(rename = "promptTokens")]
        prompt_tokens: u32,
        #[serde(rename = "completionTokens")]
        completion_tokens: u32,
        #[serde(rename = "reasoningTokens")]
        reasoning_tokens: u32,
        #[serde(rename = "totalTokens")]
        total_tokens: u32,
        #[serde(rename = "promptCacheHitTokens")]
        prompt_cache_hit_tokens: u32,
        #[serde(rename = "promptCacheMissTokens")]
        prompt_cache_miss_tokens: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<serde_json::Value>,
    },
    Subagent {
        id: String,
        #[serde(rename = "parentRunId")]
        parent_run_id: String,
        state: Option<String>,
        summary: Option<String>,
        background: bool,
    },
    Runtime {
        #[serde(rename = "eventType")]
        event_type: String,
        data: serde_json::Value,
    },
}

/// Project one existing runtime event into one stable machine event.
pub fn project_runtime_event(event: &RuntimeEvent, context: &EventContext) -> Option<HarnessEvent> {
    let thread_id = context.thread_id.clone();
    let turn_id = context.turn_id.clone();

    match event {
        RuntimeEvent::SessionRegistered { session_id, title } => {
            Some(HarnessEvent::ThreadStarted {
                thread_id: session_id.clone(),
                title: title.clone(),
                protocol_version: PROTOCOL_VERSION,
            })
        }
        RuntimeEvent::RunStarted { task_id } => Some(HarnessEvent::TurnStarted {
            thread_id,
            turn_id: turn_id.unwrap_or_else(|| task_id.clone()),
            step: None,
        }),
        RuntimeEvent::TurnStarted { step } => Some(HarnessEvent::TurnStarted {
            thread_id,
            turn_id: turn_id.unwrap_or_else(|| format!("step-{step}")),
            step: Some(*step),
        }),
        RuntimeEvent::ContentDelta { text } => Some(HarnessEvent::ItemUpdated {
            thread_id,
            turn_id,
            item_id: None,
            item: ItemPayload::ContentDelta { text: text.clone() },
        }),
        RuntimeEvent::ReasoningDelta { text } => Some(HarnessEvent::ItemUpdated {
            thread_id,
            turn_id,
            item_id: None,
            item: ItemPayload::ReasoningDelta { text: text.clone() },
        }),
        RuntimeEvent::ToolStarted {
            name,
            call_id,
            arguments,
            tool_kind,
            file_path,
            summary,
            meta,
        } => Some(HarnessEvent::ItemStarted {
            thread_id,
            turn_id,
            item_id: Some(call_id.clone()),
            item: ItemPayload::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                tool_kind: tool_kind.clone(),
                file_path: file_path.clone(),
                summary: summary.clone(),
                meta: meta.clone(),
            },
        }),
        RuntimeEvent::ToolCompleted {
            name: _,
            call_id,
            ok,
            output,
            duration_ms,
            tool_kind,
            file_path,
            summary,
            meta,
        } => Some(HarnessEvent::ItemCompleted {
            thread_id,
            turn_id,
            item_id: Some(call_id.clone()),
            item: ItemPayload::ToolResult {
                call_id: call_id.clone(),
                ok: *ok,
                output: output.clone(),
                duration_ms: *duration_ms,
                tool_kind: tool_kind.clone(),
                file_path: file_path.clone(),
                summary: summary.clone(),
                meta: meta.clone(),
            },
        }),
        RuntimeEvent::ToolBlocked {
            name,
            reason,
            needs_approval,
        } if *needs_approval => Some(HarnessEvent::ApprovalRequested {
            approval_id: None,
            thread_id,
            turn_id,
            tool_name: Some(name.clone()),
            reason: reason.clone(),
            scope: Some("tool".into()),
        }),
        RuntimeEvent::ToolBlocked { name, reason, .. } => Some(HarnessEvent::Error {
            code: "tool_blocked".into(),
            message: format!("tool '{name}' blocked: {reason}"),
            data: None,
        }),
        RuntimeEvent::Usage {
            prompt_tokens,
            completion_tokens,
            reasoning_tokens,
            total_tokens,
            prompt_cache_hit_tokens,
            prompt_cache_miss_tokens,
            raw_responses_usage,
            ..
        } => Some(HarnessEvent::ItemUpdated {
            thread_id,
            turn_id,
            item_id: Some("usage".into()),
            item: ItemPayload::Usage {
                prompt_tokens: *prompt_tokens,
                completion_tokens: *completion_tokens,
                reasoning_tokens: *reasoning_tokens,
                total_tokens: *total_tokens,
                prompt_cache_hit_tokens: *prompt_cache_hit_tokens,
                prompt_cache_miss_tokens: *prompt_cache_miss_tokens,
                raw: raw_responses_usage.clone(),
            },
        }),
        RuntimeEvent::SubagentStarted {
            id,
            parent_run_id,
            background,
            ..
        } => Some(HarnessEvent::ItemStarted {
            thread_id,
            turn_id,
            item_id: Some(id.clone()),
            item: ItemPayload::Subagent {
                id: id.clone(),
                parent_run_id: parent_run_id.clone(),
                state: Some("running".into()),
                summary: None,
                background: *background,
            },
        }),
        RuntimeEvent::SubagentCompleted {
            id,
            parent_run_id,
            state,
            summary,
            background,
            ..
        } => Some(HarnessEvent::ItemCompleted {
            thread_id,
            turn_id,
            item_id: Some(id.clone()),
            item: ItemPayload::Subagent {
                id: id.clone(),
                parent_run_id: parent_run_id.clone(),
                state: Some(state.clone()),
                summary: Some(summary.clone()),
                background: *background,
            },
        }),
        RuntimeEvent::SubagentCancelled {
            id,
            parent_run_id,
            background,
            ..
        } => Some(HarnessEvent::ItemCompleted {
            thread_id,
            turn_id,
            item_id: Some(id.clone()),
            item: ItemPayload::Subagent {
                id: id.clone(),
                parent_run_id: parent_run_id.clone(),
                state: Some("cancelled".into()),
                summary: None,
                background: *background,
            },
        }),
        RuntimeEvent::SubagentNotification {
            id,
            parent_run_id,
            state,
            summary,
        } => Some(HarnessEvent::ItemUpdated {
            thread_id,
            turn_id,
            item_id: Some(id.clone()),
            item: ItemPayload::Subagent {
                id: id.clone(),
                parent_run_id: parent_run_id.clone(),
                state: Some(state.clone()),
                summary: Some(summary.clone()),
                background: true,
            },
        }),
        RuntimeEvent::RunCompleted { message } => Some(HarnessEvent::TurnCompleted {
            thread_id,
            turn_id,
            message: message.clone(),
        }),
        RuntimeEvent::RunAwaitingApproval { message } => Some(HarnessEvent::ApprovalRequested {
            approval_id: None,
            thread_id,
            turn_id,
            tool_name: None,
            reason: message.clone(),
            scope: Some("turn".into()),
        }),
        RuntimeEvent::RunFailed { reason } => Some(HarnessEvent::TurnFailed {
            thread_id,
            turn_id,
            reason: reason.clone(),
        }),
        RuntimeEvent::RunCancelled => Some(HarnessEvent::TurnInterrupted {
            thread_id,
            turn_id,
            reason: Some("run cancelled".into()),
        }),
        other => Some(HarnessEvent::ItemUpdated {
            thread_id,
            turn_id,
            item_id: None,
            item: ItemPayload::Runtime {
                event_type: other.label().into(),
                data: serde_json::to_value(other).ok()?,
            },
        }),
    }
}
