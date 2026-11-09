use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::Result;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_core::message::Message;
use deepagent_hooks::HookRegistry;
use deepagent_runtime::{
    PromptDecision, RuntimeConfig, RuntimeEngine, RuntimeEvent, RuntimeEventSink,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;

/// Runs the UserPromptSubmit lifecycle gate before a user turn is committed to
/// the model transcript.
///
/// This adapter keeps the temporary RuntimeEngine dependency out of
/// ChatService while the v2 kernel is still being split out. Context remains
/// distinct from source input; callers commit both in one admission group.
pub(crate) async fn submit_user_prompt(
    registry: &ToolRegistry,
    hooks: &HookRegistry,
    session_id: SessionId,
    prompt: String,
    cancel: Arc<AtomicBool>,
) -> Result<PromptDecision> {
    let prompt_gate: RuntimeEngine<'_, SystemClock> =
        RuntimeEngine::new(registry, Default::default(), RuntimeConfig::default())
            .with_hooks(hooks)
            .with_cancel(cancel);
    prompt_gate.submit_prompt(session_id, prompt).await
}

/// Local source envelope serialized as ordinary user data, never a new provider
/// item type or system/developer instruction. JSON escapes text within its field.
pub(crate) fn prompt_context_item(
    contexts: &[deepagent_hooks::HookAdditionalContext],
    truncated: bool,
) -> Result<deepagent_core::response_item::ResponseItem> {
    Ok(deepagent_core::response_item::ResponseItem::Message {
        role: "user".into(),
        content: format!(
            "{}{}",
            deepagent_core::response_item::HOOK_CONTEXT_PREFIX,
            serde_json::to_string(&serde_json::json!({
                "kind":"hook_additional_context", "version":1,
                "event":"UserPromptSubmit", "trust":"hook_output_unprivileged",
                "truncated":truncated, "contexts":contexts,
            }))?
        ),
    })
}

/// Persist and emit the short terminal turn used when UserPromptSubmit blocks
/// before the main model loop starts.
pub(crate) fn finalize_blocked_user_prompt<C: Clock>(
    session: &mut Session<'_, C>,
    prompt_to_record: &str,
    message: String,
    sink: &dyn RuntimeEventSink,
) -> Result<String> {
    let session_id = session.id().to_string();
    session.append(EventPayload::MessageAppended {
        message: Message::user(prompt_to_record),
    })?;
    let task = session.create_task(prompt_to_record)?;
    session.transition_task(task, deepagent_core::task::TaskState::Running)?;
    sink.emit(RuntimeEvent::RunStarted {
        task_id: task.to_string(),
    });
    sink.emit(RuntimeEvent::SessionRegistered {
        session_id: session_id.clone(),
        title: session.state().title.clone(),
    });
    sink.emit(RuntimeEvent::TurnStarted { step: 0 });
    sink.emit(RuntimeEvent::ContentDelta {
        text: message.clone(),
    });
    session.append(EventPayload::MessageAppended {
        message: Message::assistant(&message),
    })?;
    session.transition_task(task, deepagent_core::task::TaskState::Completed)?;
    sink.emit(RuntimeEvent::RunCompleted {
        message,
        edited_files: vec![],
    });
    Ok(session_id)
}
