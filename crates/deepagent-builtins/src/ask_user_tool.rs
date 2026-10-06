//! `ask_user_question` — interactive multiple-choice questions, modeled on
//! Claude Code's `AskUserQuestion` tool (same JSON schema shape).
//!
//! The agent uses this to gather preferences, clarify ambiguity, or offer the
//! user a decision *mid-run*. It is read-only (it changes nothing in the
//! workspace) but **requires user interaction**: the actual question→answer
//! round-trip is delegated to a [`QuestionResponder`] the host wires up (the
//! desktop app shows a dialog; tests/headless answer programmatically or
//! decline). This mirrors how the approval gate bridges the kernel to the UI.

use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

/// The tool name advertised to the model.
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "ask_user_question";

/// A single option the user can pick for a question.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuestionOption {
    /// Short display label (1–5 words) for the choice.
    pub label: String,
    /// Explanation of what choosing this option means / its trade-offs.
    #[serde(default)]
    pub description: String,
}

/// A single question with 2–4 options.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Question {
    /// The complete question text (should end with a question mark).
    pub question: String,
    /// Very short chip/tag label (e.g. "Auth method", "Library").
    #[serde(default)]
    pub header: String,
    /// The available choices (2–4).
    pub options: Vec<QuestionOption>,
    /// Allow selecting multiple options.
    #[serde(default)]
    pub multi_select: bool,
}

/// Resolves a set of questions into the user's answers.
///
/// `ask` returns a map of `question text -> answer string` (multi-select
/// answers comma-joined). Returning `None` means the user declined to answer.
/// The default implementation declines (safe for headless/CI).
#[async_trait]
pub trait QuestionResponder: Send + Sync {
    /// Present `questions` to the user and collect answers, or `None` if they
    /// declined.
    async fn ask(&self, questions: &[Question]) -> Result<Option<BTreeMap<String, String>>>;
}

/// A responder that always declines (the safe headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclineResponder;

#[async_trait]
impl QuestionResponder for DeclineResponder {
    async fn ask(&self, _questions: &[Question]) -> Result<Option<BTreeMap<String, String>>> {
        Ok(None)
    }
}

/// A responder that answers with the option the model recommended.
///
/// Sub-agents have no human watching their own turn, so they get this instead of
/// the UI channel. The tool schema asks the model to list the recommended choice
/// first and tag it `"(Recommended)"`; either signal is honoured, and a question
/// whose options carry no marker falls back to that first option.
#[derive(Debug, Default, Clone, Copy)]
pub struct RecommendedResponder;

#[async_trait]
impl QuestionResponder for RecommendedResponder {
    async fn ask(&self, questions: &[Question]) -> Result<Option<BTreeMap<String, String>>> {
        let mut answers = BTreeMap::new();
        for question in questions {
            let label = recommended_option(question)
                .map(|option| option.label.clone())
                .unwrap_or_default();
            answers.insert(question.question.clone(), label);
        }
        Ok(Some(answers))
    }
}

/// The option the model marked as recommended, if any.
fn recommended_option(question: &Question) -> Option<&QuestionOption> {
    question
        .options
        .iter()
        .find(|option| option.label.to_ascii_lowercase().contains("recommended"))
        .or_else(|| question.options.first())
}

/// The `ask_user_question` tool over a host-supplied [`QuestionResponder`].
pub struct AskUserQuestionTool {
    responder: Arc<dyn QuestionResponder>,
}

impl AskUserQuestionTool {
    /// Build the tool with a responder (the desktop app's question channel, a
    /// [`RecommendedResponder`] for sub-agents, or [`DeclineResponder`] for
    /// headless hosts).
    pub fn new(responder: Arc<dyn QuestionResponder>) -> Self {
        Self { responder }
    }
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    /// Waiting on a human has no deadline by design (see the comment above
    /// `invoke_with_context`): the run's cancellation flag is the only escape
    /// besides an answer, so an outer budget would silently drop questions.
    fn bounded_by_run_timeout(&self) -> bool {
        false
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: ASK_USER_QUESTION_TOOL_NAME.into(),
            description: "Ask the user multiple-choice questions to gather preferences, clarify \
                ambiguity, or get a decision while you work. Use when you genuinely need the \
                user's input to proceed — not for things you can determine yourself. Provide 1-4 \
                questions, each with 2-4 distinct options; the user can always pick \"Other\". If \
                you recommend an option, list it first and add \"(Recommended)\" to its label."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "The questions to ask (1-4).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The complete question. Clear, specific, ending with a question mark."
                                },
                                "header": {
                                    "type": "string",
                                    "description": "Very short chip label (max ~12 chars), e.g. \"Library\", \"Approach\"."
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "description": "2-4 distinct, mutually exclusive choices (unless multi_select). Do not add an 'Other' option; it is provided automatically.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "Concise display text (1-5 words)."
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "What this option means or what happens if chosen."
                                            }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multi_select": {
                                    "type": "boolean",
                                    "description": "Allow selecting multiple options. Default false."
                                }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
            // Read-only and concurrency-safe in the registry sense, but it does
            // require user interaction; the host decides how to surface it.
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    /// Waiting for a human has no deadline, so the run's cancellation flag is
    /// the only escape besides an answer: without this race, Stop could never
    /// interrupt a question that is on screen.
    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let questions = match parse_questions(&args) {
            Ok(questions) => questions,
            Err(failure) => return Ok(failure),
        };
        let cancel = context.cancel_flag();
        tokio::select! {
            result = self.responder.ask(&questions) => Ok(match result? {
                Some(answers) => ToolOutput::success(serde_json::json!({
                    "answered": true,
                    "answers": answers,
                })),
                None => unanswered_output(DECLINED_NOTE),
            }),
            _ = wait_until_cancelled(&cancel) => Ok(unanswered_output(CANCELLED_NOTE)),
        }
    }
}

/// Note returned when the user actively declines. Kept as one constant so the
/// model always sees the same instruction.
const DECLINED_NOTE: &str = "The user declined to answer. Proceed using your best judgement, \
    or ask again only if truly blocked.";

/// Note returned when the run was interrupted before an answer arrived.
const CANCELLED_NOTE: &str = "The run was cancelled before the user answered. Do not assume an \
    answer; report the pending question instead.";

fn unanswered_output(note: &str) -> ToolOutput {
    ToolOutput::success(serde_json::json!({
        "answered": false,
        "note": note,
    }))
}

/// Validate the tool arguments, returning the failure output to report.
fn parse_questions(args: &serde_json::Value) -> std::result::Result<Vec<Question>, ToolOutput> {
    let Some(value) = args.get("questions") else {
        return Err(ToolOutput::failure("missing 'questions'"));
    };
    let questions: Vec<Question> = serde_json::from_value(value.clone())
        .map_err(|e| ToolOutput::failure(format!("invalid 'questions': {e}")))?;
    if questions.is_empty() || questions.len() > 4 {
        return Err(ToolOutput::failure("provide 1-4 questions"));
    }
    for q in &questions {
        if q.options.len() < 2 || q.options.len() > 4 {
            return Err(ToolOutput::failure(format!(
                "question \"{}\" must have 2-4 options",
                q.question
            )));
        }
    }
    Ok(questions)
}

/// Resolve once `cancel` is set. Polled rather than signalled because the
/// run-scoped flag is a plain atomic shared with every other tool.
async fn wait_until_cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedResponder;
    #[async_trait]
    impl QuestionResponder for FixedResponder {
        async fn ask(
            &self,
            questions: &[Question],
        ) -> Result<Option<std::collections::BTreeMap<String, String>>> {
            let mut m = std::collections::BTreeMap::new();
            for q in questions {
                m.insert(q.question.clone(), q.options[0].label.clone());
            }
            Ok(Some(m))
        }
    }

    fn sample_args() -> serde_json::Value {
        serde_json::json!({
            "questions": [{
                "question": "Which library should we use?",
                "header": "Library",
                "options": [
                    {"label": "serde", "description": "standard"},
                    {"label": "miniserde", "description": "tiny"}
                ]
            }]
        })
    }

    #[tokio::test]
    async fn returns_answers_when_user_responds() {
        let tool = AskUserQuestionTool::new(Arc::new(FixedResponder));
        let out = tool.invoke(sample_args()).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["answered"], true);
        assert_eq!(
            out.value["answers"]["Which library should we use?"],
            "serde"
        );
    }

    #[tokio::test]
    async fn declined_when_responder_declines() {
        let tool = AskUserQuestionTool::new(Arc::new(DeclineResponder));
        let out = tool.invoke(sample_args()).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["answered"], false);
    }

    #[tokio::test]
    async fn rejects_bad_option_count() {
        let tool = AskUserQuestionTool::new(Arc::new(FixedResponder));
        let bad = serde_json::json!({
            "questions": [{
                "question": "Only one?",
                "options": [{"label": "a"}]
            }]
        });
        let out = tool.invoke(bad).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn descriptor_has_questions_schema() {
        let tool = AskUserQuestionTool::new(Arc::new(DeclineResponder));
        let d = tool.descriptor();
        assert_eq!(d.name, ASK_USER_QUESTION_TOOL_NAME);
        assert!(d.parameters["properties"]["questions"].is_object());
    }

    fn question(options: &[&str]) -> Question {
        Question {
            question: "Which route?".into(),
            header: String::new(),
            options: options
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                })
                .collect(),
            multi_select: false,
        }
    }

    #[tokio::test]
    async fn recommended_responder_prefers_the_marked_option() {
        let answers = RecommendedResponder
            .ask(&[question(&["Slow route", "Fast route (Recommended)"])])
            .await
            .unwrap()
            .expect("recommended responder always answers");
        assert_eq!(answers["Which route?"], "Fast route (Recommended)");
    }

    #[tokio::test]
    async fn recommended_responder_falls_back_to_the_first_option() {
        let answers = RecommendedResponder
            .ask(&[question(&["Alpha", "Beta"])])
            .await
            .unwrap()
            .expect("recommended responder always answers");
        assert_eq!(answers["Which route?"], "Alpha");
    }

    /// A responder that never answers proves the only way out of the unbounded
    /// wait is the run's cancellation flag.
    struct SilentResponder;

    #[async_trait]
    impl QuestionResponder for SilentResponder {
        async fn ask(&self, _questions: &[Question]) -> Result<Option<BTreeMap<String, String>>> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancelling_the_run_ends_a_pending_question() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let cancel = Arc::new(AtomicBool::new(false));
        let tool = AskUserQuestionTool::new(Arc::new(SilentResponder));
        let context = ToolExecutionContext::new(cancel.clone());
        let handle =
            tokio::spawn(async move { tool.invoke_with_context(sample_args(), context).await });

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert!(!handle.is_finished(), "a question must stay pending");
        cancel.store(true, Ordering::Release);

        let output = handle.await.unwrap().unwrap();
        assert!(output.ok);
        assert_eq!(output.value["answered"], false);
        assert!(output.value["note"]
            .as_str()
            .is_some_and(|note| note.contains("cancelled")));
    }
}
