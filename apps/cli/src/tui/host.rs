//! The TUI's single seam to AppCore.
//!
//! Every kernel callback (`on_event`, `on_approval`, the question notifier) is
//! turned into data on one channel instead of blocking the calling thread: the run
//! stays parked on its pending decision until the UI loop answers it through
//! [`ChatService::resolve_approval_scoped`] / [`ChatService::resolve_question`].

use deepagent_app_core::dto::{ApprovalRequestDto, QuestionRequestDto};
use deepagent_app_core::{ChatService, HarnessRunOverrides};
use deepagent_runtime::RuntimeEvent;
use tokio::sync::mpsc::UnboundedSender;

use crate::tui::questions::{collect_answers, format_request, StdinInput};

/// Anything the UI loop has to react to that did not come from the keyboard.
#[derive(Debug)]
pub enum UiEvent {
    Runtime(RuntimeEvent),
    Approval(ApprovalRequestDto),
    Question(QuestionRequestDto),
    /// The run's future resolved: `Ok(session_id)` or a rendered failure reason.
    Finished(Result<String, String>),
}

/// Attach the human question channel. Line mode answers on stdin; the TUI answers
/// from its overlay through the same event the desktop emits.
pub fn attach_question_channel(chat: ChatService, sender: UnboundedSender<UiEvent>) -> ChatService {
    chat.with_question_notifier(move |question| {
        let _ = sender.send(UiEvent::Question(question));
    })
}

/// The same channel contract for the non-terminal fallback, which can block on
/// stdin instead of waiting for an overlay.
pub fn attach_line_question_channel(chat: ChatService) -> ChatService {
    let chat_for_answers = chat.clone();
    chat.with_question_notifier(move |question: QuestionRequestDto| {
        println!("\n{}", format_request(&question));
        let answers = collect_answers(&question, &mut StdinInput);
        if !chat_for_answers.resolve_question(&question.question_id, answers) {
            eprintln!("question was no longer pending: {}", question.question_id);
        }
    })
}

/// Start one turn on its own task and stream the kernel's callbacks into `sender`.
pub fn start_turn(
    chat: ChatService,
    prompt: String,
    thread_id: Option<String>,
    overrides: HarnessRunOverrides,
    sender: UnboundedSender<UiEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let events = sender.clone();
        let approvals = sender.clone();
        let result = chat
            .run_in_session_with_overrides(
                &prompt,
                thread_id.as_deref(),
                None,
                None,
                Vec::new(),
                None,
                false,
                None,
                overrides,
                move |event| {
                    let _ = events.send(UiEvent::Runtime(event));
                },
                move |approval| {
                    let _ = approvals.send(UiEvent::Approval(approval));
                },
            )
            .await;
        let _ = sender.send(UiEvent::Finished(result.map_err(|error| error.to_string())));
    })
}
