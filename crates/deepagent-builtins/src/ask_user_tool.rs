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

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

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
    async fn ask(
        &self,
        questions: &[Question],
    ) -> Result<Option<std::collections::BTreeMap<String, String>>>;
}

/// A responder that always declines (the safe headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclineResponder;

#[async_trait]
impl QuestionResponder for DeclineResponder {
    async fn ask(
        &self,
        _questions: &[Question],
    ) -> Result<Option<std::collections::BTreeMap<String, String>>> {
        Ok(None)
    }
}

/// The `ask_user_question` tool over a pluggable [`QuestionResponder`].
pub struct AskUserQuestionTool<R: QuestionResponder> {
    responder: R,
}

impl<R: QuestionResponder> AskUserQuestionTool<R> {
    /// Build the tool with a responder (the desktop app's dialog bridge, or
    /// [`DeclineResponder`] for headless).
    pub fn new(responder: R) -> Self {
        Self { responder }
    }
}

#[async_trait]
impl<R: QuestionResponder> Tool for AskUserQuestionTool<R> {
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
        let questions: Vec<Question> = match args.get("questions") {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(q) => q,
                Err(e) => return Ok(ToolOutput::failure(format!("invalid 'questions': {e}"))),
            },
            None => return Ok(ToolOutput::failure("missing 'questions'")),
        };
        if questions.is_empty() || questions.len() > 4 {
            return Ok(ToolOutput::failure("provide 1-4 questions"));
        }
        for q in &questions {
            if q.options.len() < 2 || q.options.len() > 4 {
                return Ok(ToolOutput::failure(format!(
                    "question \"{}\" must have 2-4 options",
                    q.question
                )));
            }
        }

        match self.responder.ask(&questions).await? {
            Some(answers) => Ok(ToolOutput::success(serde_json::json!({
                "answered": true,
                "answers": answers,
            }))),
            None => Ok(ToolOutput::success(serde_json::json!({
                "answered": false,
                "note": "The user declined to answer. Proceed using your best judgement, or ask again only if truly blocked.",
            }))),
        }
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
        let tool = AskUserQuestionTool::new(FixedResponder);
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
        let tool = AskUserQuestionTool::new(DeclineResponder);
        let out = tool.invoke(sample_args()).await.unwrap();
        assert!(out.ok);
        assert_eq!(out.value["answered"], false);
    }

    #[tokio::test]
    async fn rejects_bad_option_count() {
        let tool = AskUserQuestionTool::new(FixedResponder);
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
        let tool = AskUserQuestionTool::new(DeclineResponder);
        let d = tool.descriptor();
        assert_eq!(d.name, ASK_USER_QUESTION_TOOL_NAME);
        assert!(d.parameters["properties"]["questions"].is_object());
    }
}
