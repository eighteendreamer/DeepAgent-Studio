//! Turns an `ask_user_question` request into a human answer.
//!
//! The kernel blocks the run on a pending question until it is answered or
//! declined, with no deadline (`deepagent_app_core::question_bridge`), so a host
//! that never attaches a notifier silently declines everything. Both chat surfaces
//! reuse the formatting and parsing here: line mode reads stdin through it, and the
//! TUI answers through the same rules from its overlay widget.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use deepagent_app_core::dto::QuestionRequestDto;
use deepagent_builtins::Question;

/// One answer line per question; `None` means the human gave up (EOF).
pub trait AnswerInput {
    fn read_line(&mut self, prompt: &str) -> Option<String>;
}

/// Reads answers from the terminal, printing the prompt on stdout.
pub struct StdinInput;

impl AnswerInput for StdinInput {
    fn read_line(&mut self, prompt: &str) -> Option<String> {
        print!("{prompt}");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }
}

/// Renders the question set: header, text, numbered options, selection kind.
pub fn format_request(request: &QuestionRequestDto) -> String {
    let mut out = String::new();
    for (index, question) in request.questions.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let header = if question.header.is_empty() {
            String::new()
        } else {
            format!("[{}] ", question.header)
        };
        let kind = if question.multi_select {
            "(multiple choice, comma separated)"
        } else {
            "(single choice)"
        };
        out.push_str(&format!(
            "Question {}: {header}{} {kind}\n",
            index + 1,
            question.question
        ));
        for (option, choice) in question.options.iter().enumerate() {
            let description = if choice.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", choice.description)
            };
            out.push_str(&format!(
                "  {}) {}{description}\n",
                option + 1,
                choice.label
            ));
        }
    }
    out.push_str("\nEnter an option number or your own answer. Blank or 'skip' declines.\n");
    out
}

/// Collect one answer per question. Any decline (blank, `skip`, EOF) drops the
/// whole set, because the kernel's answer type is all-or-nothing.
pub fn collect_answers(
    request: &QuestionRequestDto,
    input: &mut impl AnswerInput,
) -> Option<BTreeMap<String, String>> {
    let mut answers = BTreeMap::new();
    for question in &request.questions {
        let prompt = format!(
            "{}> ",
            if question.header.is_empty() {
                &question.question
            } else {
                &question.header
            }
        );
        let value = loop {
            let raw = input.read_line(&prompt)?;
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("skip") {
                return None;
            }
            match resolve_choice(question, trimmed) {
                Some(value) => break value,
                None => println!("  Not one of the options. Enter a number or your own answer."),
            }
        };
        answers.insert(question.question.clone(), value);
    }
    Some(answers)
}

/// Map a reply to option labels when it is a number list; otherwise keep the
/// free text as the answer, exactly as the desktop's "Other" input does.
fn resolve_choice(question: &Question, value: &str) -> Option<String> {
    if !question.multi_select {
        if let Ok(number) = value.parse::<usize>() {
            return (1..=question.options.len())
                .contains(&number)
                .then(|| question.options[number - 1].label.clone());
        }
        return Some(value.to_string());
    }
    let tokens = value
        .split([',', '，', '、', ' '])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let all_numbers =
        !tokens.is_empty() && tokens.iter().all(|token| token.parse::<usize>().is_ok());
    if !all_numbers {
        return Some(value.to_string());
    }
    let numbers = tokens
        .iter()
        .map(|token| token.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if numbers
        .iter()
        .any(|number| *number < 1 || *number > question.options.len())
    {
        return None;
    }
    Some(join_selection(
        numbers
            .iter()
            .map(|number| question.options[number - 1].label.clone())
            .collect(),
    ))
}

/// Join selected labels the way the kernel expects a multi-select answer:
/// de-duplicated, in pick order, comma separated. Both chat surfaces use this so
/// a keystroke answer and a typed answer cannot drift apart.
pub fn join_selection(labels: Vec<String>) -> String {
    let mut unique: Vec<String> = Vec::new();
    for label in labels {
        if !unique.contains(&label) {
            unique.push(label);
        }
    }
    unique.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{collect_answers, format_request, AnswerInput};
    use deepagent_app_core::dto::QuestionRequestDto;
    use deepagent_builtins::{Question, QuestionOption};
    use std::collections::{BTreeMap, VecDeque};

    const CITY: &str = "Which city?";

    struct ScriptedInput {
        lines: VecDeque<String>,
        prompts: Vec<String>,
    }

    impl ScriptedInput {
        fn new(lines: &[&str]) -> Self {
            Self {
                lines: lines.iter().map(|line| (*line).to_string()).collect(),
                prompts: Vec::new(),
            }
        }
    }

    impl AnswerInput for ScriptedInput {
        fn read_line(&mut self, prompt: &str) -> Option<String> {
            self.prompts.push(prompt.to_string());
            self.lines.pop_front()
        }
    }

    fn request(multi: bool) -> QuestionRequestDto {
        QuestionRequestDto {
            question_id: "q-1".to_string(),
            session_id: "session-q".to_string(),
            questions: vec![Question {
                question: CITY.to_string(),
                header: "City".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Beijing".to_string(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "Changsha".to_string(),
                        description: "warmer".to_string(),
                    },
                ],
                multi_select: multi,
            }],
        }
    }

    #[test]
    fn formats_header_text_options_and_selection_kind() {
        let rendered = format_request(&request(true));
        assert!(rendered.contains("[City] Which city? (multiple choice"));
        assert!(rendered.contains("1) Beijing"));
        assert!(rendered.contains("2) Changsha — warmer"));
        assert!(!format_request(&request(false)).contains("multiple choice"));
    }

    #[test]
    fn option_number_resolves_to_its_label() {
        let mut input = ScriptedInput::new(&["2"]);
        let answers = collect_answers(&request(false), &mut input).expect("answered");
        assert_eq!(answers.get(CITY).map(String::as_str), Some("Changsha"));
        assert_eq!(input.prompts, vec!["City> ".to_string()]);
    }

    #[test]
    fn free_text_is_kept_as_its_own_answer() {
        let mut input = ScriptedInput::new(&["somewhere else"]);
        let answers = collect_answers(&request(false), &mut input).expect("answered");
        assert_eq!(
            answers.get(CITY).map(String::as_str),
            Some("somewhere else")
        );
    }

    #[test]
    fn multi_select_joins_picked_labels_without_duplicates() {
        let mut input = ScriptedInput::new(&["1,2,1"]);
        let answers = collect_answers(&request(true), &mut input).expect("answered");
        assert_eq!(
            answers.get(CITY).map(String::as_str),
            Some("Beijing, Changsha")
        );
    }

    #[test]
    fn multi_select_free_text_is_not_split_into_pieces() {
        let mut input = ScriptedInput::new(&["yes and no"]);
        let answers = collect_answers(&request(true), &mut input).expect("answered");
        assert_eq!(answers.get(CITY).map(String::as_str), Some("yes and no"));
    }

    #[test]
    fn out_of_range_option_reprompts_then_accepts_the_next_answer() {
        let mut input = ScriptedInput::new(&["9", "1"]);
        let answers = collect_answers(&request(false), &mut input).expect("answered");
        assert_eq!(answers.get(CITY).map(String::as_str), Some("Beijing"));
        assert_eq!(input.prompts.len(), 2, "a bad choice must ask again");
    }

    #[test]
    fn blank_or_skip_declines_the_whole_set() {
        for decline in ["", "   ", "skip", "SKIP"] {
            let mut input = ScriptedInput::new(&[decline, "ignored"]);
            assert_eq!(
                collect_answers(&request(false), &mut input),
                None,
                "{decline:?} should decline"
            );
        }
    }

    #[test]
    fn eof_declines_instead_of_hanging_the_run() {
        let mut input = ScriptedInput::new(&[]);
        assert_eq!(collect_answers(&request(false), &mut input), None);
    }

    #[tokio::test]
    async fn the_answer_reaches_the_blocked_tool() {
        use deepagent_app_core::question_bridge::{ChannelQuestionResponder, PendingQuestions};
        use deepagent_builtins::QuestionResponder;
        use std::sync::{Arc, Mutex};

        let pending = PendingQuestions::new();
        let registry = pending.clone();
        let seen: Arc<Mutex<Option<BTreeMap<String, String>>>> = Arc::new(Mutex::new(None));
        let captured = seen.clone();
        let responder = ChannelQuestionResponder::new(
            pending.clone(),
            "session-q".to_string(),
            Arc::new(move |dto: QuestionRequestDto| {
                let registry = registry.clone();
                let mut input = ScriptedInput::new(&["2"]);
                let answers = collect_answers(&dto, &mut input);
                assert!(registry.resolve(&dto.session_id, &dto.question_id, answers.clone()));
                *captured.lock().unwrap() = answers;
            }),
        );
        let questions = request(false).questions;
        let handle = tokio::spawn(async move { responder.ask(&questions).await.unwrap() });
        for _ in 0..200 {
            if pending.pending_count() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let expected = Some(BTreeMap::from([(CITY.to_string(), "Changsha".to_string())]));
        assert_eq!(handle.await.unwrap(), expected);
        assert_eq!(*seen.lock().unwrap(), expected);
    }
}
