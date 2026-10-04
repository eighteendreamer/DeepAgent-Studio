//! Bridges `ask_user_question` requests to the host UI and back.
//!
//! Same shape as [`crate::approval_bridge`]: the tool registers a pending
//! decision keyed by an id, notifies the UI, and awaits a `oneshot` reply.
//!
//! Two deliberate differences from approvals:
//!
//! - **No policy gate.** [`crate::settings::ApprovalPolicy::FullAccess`] must not
//!   swallow a question: it grants the agent permission to act, it does not make
//!   the agent an authority on what the user wants. So the responder is wired
//!   straight to the UI regardless of the permission profile.
//! - **No timeout.** A question stays pending until it is answered, declined, or
//!   the run is cancelled. [`deepagent_builtins::AskUserQuestionTool`] races the
//!   wait against the run's cancellation flag, and [`PendingGuard`] forgets the
//!   entry when the wait is abandoned so cancelled runs cannot pile up.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_builtins::{Question, QuestionResponder};
use deepagent_core::error::Result;
use tokio::sync::oneshot;

use crate::dto::QuestionRequestDto;

type Answer = Option<BTreeMap<String, String>>;

/// Shared registry of in-flight question sets, keyed by `question_id`.
#[derive(Clone, Default)]
pub struct PendingQuestions {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<Answer>>>>,
}

impl PendingQuestions {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending question set, returning the receiver to await.
    fn register(&self, question_id: String) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(question_id, tx);
        rx
    }

    /// Resolve a pending question with the user's answers, or `None` when they
    /// declined. Returns `false` when no such question is pending (already
    /// answered, or the run moved on).
    pub fn resolve(&self, question_id: &str, answers: Answer) -> bool {
        let sender = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(question_id);
        match sender {
            Some(tx) => tx.send(answers).is_ok(),
            None => false,
        }
    }

    /// Forget a question that nobody is waiting for any more. A no-op once the
    /// question has been resolved, so it is safe to call on any exit path.
    fn abandon(&self, question_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(question_id);
    }

    /// How many questions are currently awaiting an answer.
    pub fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// Clears a pending registration when the wait ends for any reason, including
/// the tool future being dropped by a cancelled run.
struct PendingGuard {
    pending: PendingQuestions,
    question_id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.abandon(&self.question_id);
    }
}

/// A [`QuestionResponder`] that shows the questions in the host UI and awaits
/// the answer.
pub struct ChannelQuestionResponder {
    pending: PendingQuestions,
    /// Called when a question set needs human attention (UI emit). Boxed so the
    /// responder stays transport-agnostic, mirroring the approval gate.
    notify: Arc<dyn Fn(QuestionRequestDto) + Send + Sync>,
}

impl ChannelQuestionResponder {
    /// Build a responder over a shared [`PendingQuestions`] and a UI notify hook.
    pub fn new(
        pending: PendingQuestions,
        notify: Arc<dyn Fn(QuestionRequestDto) + Send + Sync>,
    ) -> Self {
        Self { pending, notify }
    }
}

#[async_trait]
impl QuestionResponder for ChannelQuestionResponder {
    async fn ask(&self, questions: &[Question]) -> Result<Answer> {
        let question_id = format!("q-{}", uuid::Uuid::new_v4());
        let receiver = self.pending.register(question_id.clone());
        let _guard = PendingGuard {
            pending: self.pending.clone(),
            question_id: question_id.clone(),
        };
        (self.notify)(QuestionRequestDto {
            question_id,
            questions: questions.to_vec(),
        });
        // A closed channel means nobody answered: the UI declined, or the wait
        // was abandoned. Report "no answer" instead of inventing one.
        Ok(receiver.await.unwrap_or(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Vec<Question> {
        vec![Question {
            question: "Which city?".into(),
            header: "City".into(),
            options: vec![
                deepagent_builtins::QuestionOption {
                    label: "Beijing".into(),
                    description: String::new(),
                },
                deepagent_builtins::QuestionOption {
                    label: "Changsha".into(),
                    description: String::new(),
                },
            ],
            multi_select: false,
        }]
    }

    /// Drive one question through the responder and report what the UI was shown.
    async fn drive_question(
        pending: &PendingQuestions,
    ) -> (String, Vec<Question>, tokio::task::JoinHandle<Answer>) {
        let seen = Arc::new(Mutex::new(None::<QuestionRequestDto>));
        let notify = seen.clone();
        let responder = ChannelQuestionResponder::new(
            pending.clone(),
            Arc::new(move |dto| *notify.lock().unwrap() = Some(dto)),
        );
        let handle = tokio::spawn(async move { responder.ask(&questions()).await.unwrap() });
        for _ in 0..200 {
            if pending.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let shown = seen.lock().unwrap().take().expect("the UI was notified");
        (shown.question_id, shown.questions, handle)
    }

    #[tokio::test]
    async fn ui_answer_travels_back_to_the_tool() {
        let pending = PendingQuestions::new();
        let (question_id, shown, handle) = drive_question(&pending).await;
        assert_eq!(shown[0].question, "Which city?");
        assert_eq!(shown[0].options.len(), 2);

        let mut answers = BTreeMap::new();
        answers.insert("Which city?".to_string(), "Changsha".to_string());
        assert!(pending.resolve(&question_id, Some(answers.clone())));
        assert_eq!(handle.await.unwrap(), Some(answers));
        assert_eq!(pending.pending_count(), 0, "the wait must clean up");
    }

    #[tokio::test]
    async fn declining_returns_no_answer() {
        let pending = PendingQuestions::new();
        let (question_id, _, handle) = drive_question(&pending).await;
        assert!(pending.resolve(&question_id, None));
        assert_eq!(handle.await.unwrap(), None);
    }

    #[tokio::test]
    async fn resolving_an_unknown_question_is_rejected() {
        let pending = PendingQuestions::new();
        assert!(!pending.resolve("q-nope", None));
    }

    #[tokio::test]
    async fn abandoning_the_wait_clears_the_registry() {
        // A cancelled run drops the tool future; the registration must not stay.
        let pending = PendingQuestions::new();
        let responder = ChannelQuestionResponder::new(pending.clone(), Arc::new(|_| {}));
        let handle = tokio::spawn(async move { responder.ask(&questions()).await.unwrap() });
        for _ in 0..200 {
            if pending.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        handle.abort();
        for _ in 0..200 {
            if pending.pending_count() == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("an abandoned question stayed registered");
    }
}
