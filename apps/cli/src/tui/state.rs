//! Run and transcript state, kept apart from the terminal so it can be driven by
//! synthetic kernel events in tests.
//!
//! The mapping is deliberately narrow: everything that belongs in the transcript
//! or in the status line comes from `RuntimeEvent`; model-request bookkeeping stays
//! in the persisted event log and is not re-derived here.

use deepagent_app_core::dto::{ApprovalRequestDto, QuestionRequestDto};
use deepagent_runtime::RuntimeEvent;

use super::history::HistoryCell;

/// Outcome text a tool cell carries until the matching completion arrives.
pub const TOOL_RUNNING: &str = "running";

/// Token totals accumulated across the turns of one run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenTotals {
    pub prompt: u64,
    pub completion: u64,
    pub cache_hit: u64,
}

impl TokenTotals {
    pub fn total(&self) -> u64 {
        self.prompt + self.completion
    }

    /// Share of prompt tokens served from DeepSeek's context cache.
    pub fn cache_hit_percent(&self) -> Option<u64> {
        if self.prompt == 0 {
            return None;
        }
        Some(self.cache_hit.saturating_mul(100) / self.prompt)
    }
}

/// What the host must do after a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continue {
    Nothing,
    /// The run reached a terminal state; start the next queued prompt if any.
    DrainQueue,
}

#[derive(Debug)]
pub struct ChatState {
    pub thread_id: Option<String>,
    pub run_id: Option<String>,
    pub busy: bool,
    pub cells: Vec<HistoryCell>,
    pub streaming: String,
    pub reasoning: String,
    pub queued: Vec<String>,
    pub approval: Option<ApprovalRequestDto>,
    pub question: Option<QuestionRequestDto>,
    pub tokens: TokenTotals,
    pub notice: Option<String>,
}

impl ChatState {
    pub fn new(thread_id: Option<String>) -> Self {
        Self {
            thread_id,
            run_id: None,
            busy: false,
            cells: Vec::new(),
            streaming: String::new(),
            reasoning: String::new(),
            queued: Vec::new(),
            approval: None,
            question: None,
            tokens: TokenTotals::default(),
            notice: None,
        }
    }

    /// Record what the human typed, whether it runs now or waits in the queue.
    pub fn record_prompt(&mut self, prompt: String) {
        self.cells.push(HistoryCell::UserPrompt { text: prompt });
    }

    /// Take the prompt to run now, or queue it behind the run in flight.
    ///
    /// The kernel has no in-turn injection channel, so "steer" here means the
    /// queued prompt starts as the next turn of the same thread.
    pub fn submit(&mut self, prompt: String) -> Option<String> {
        if self.busy {
            self.queued.push(prompt);
            return None;
        }
        self.busy = true;
        Some(prompt)
    }

    pub fn take_next_queued(&mut self) -> Option<String> {
        if self.queued.is_empty() {
            return None;
        }
        Some(self.queued.remove(0))
    }

    /// Flush the run's terminal bookkeeping and ask the host to drain the queue.
    pub fn on_runtime(&mut self, event: RuntimeEvent) -> Continue {
        match event {
            RuntimeEvent::SessionRegistered { session_id, .. } => {
                self.thread_id = Some(session_id);
                Continue::Nothing
            }
            RuntimeEvent::RunStarted { task_id } => {
                self.run_id = Some(task_id);
                self.busy = true;
                Continue::Nothing
            }
            RuntimeEvent::ContentDelta { text } => {
                self.streaming.push_str(&text);
                Continue::Nothing
            }
            RuntimeEvent::ReasoningDelta { text } => {
                self.reasoning.push_str(&text);
                Continue::Nothing
            }
            RuntimeEvent::ToolStarted {
                name,
                call_id,
                summary,
                ..
            } => {
                // Narration that preceded the tool call is its own row; it must be
                // committed before the tool line so the transcript keeps its order.
                self.commit_streaming();
                self.cells.push(HistoryCell::Tool {
                    call_id,
                    name,
                    summary: summary.unwrap_or_default(),
                    outcome: TOOL_RUNNING.to_string(),
                });
                Continue::Nothing
            }
            RuntimeEvent::ToolCompleted {
                call_id,
                ok,
                duration_ms,
                name,
                summary,
                ..
            } => {
                let outcome = if ok {
                    format!("done in {duration_ms}ms")
                } else {
                    format!("failed in {duration_ms}ms")
                };
                if !self.settle_tool(&call_id, outcome.clone()) {
                    self.cells.push(HistoryCell::Tool {
                        call_id,
                        name,
                        summary: summary.unwrap_or_default(),
                        outcome,
                    });
                }
                Continue::Nothing
            }
            RuntimeEvent::ToolBlocked {
                name,
                reason,
                needs_approval,
            } => {
                let state = if needs_approval {
                    "awaiting approval"
                } else {
                    "blocked"
                };
                self.cells.push(HistoryCell::Tool {
                    call_id: String::new(),
                    name,
                    summary: reason,
                    outcome: state.to_string(),
                });
                Continue::Nothing
            }
            RuntimeEvent::Usage {
                prompt_tokens,
                completion_tokens,
                prompt_cache_hit_tokens,
                ..
            } => {
                self.tokens.prompt += u64::from(prompt_tokens);
                self.tokens.completion += u64::from(completion_tokens);
                self.tokens.cache_hit += u64::from(prompt_cache_hit_tokens);
                Continue::Nothing
            }
            RuntimeEvent::RunAwaitingApproval { message } => {
                self.notice = Some(message);
                Continue::Nothing
            }
            RuntimeEvent::RunCompleted {
                message,
                edited_files,
            } => {
                let streamed = !self.streaming.trim().is_empty();
                self.commit_streaming();
                // The loop already printed the answer when it streamed it; only a
                // run that produced no deltas needs the final message as a row.
                if !streamed && !message.trim().is_empty() {
                    self.cells.push(HistoryCell::Assistant { text: message });
                }
                if !edited_files.is_empty() {
                    let paths = edited_files
                        .iter()
                        .map(|file| file.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.cells.push(HistoryCell::Notice {
                        text: format!("edited {} file(s): {paths}", edited_files.len()),
                    });
                }
                self.finish_run();
                Continue::DrainQueue
            }
            RuntimeEvent::RunFailed { reason } => {
                self.commit_streaming();
                self.fail(reason);
                self.finish_run();
                Continue::DrainQueue
            }
            RuntimeEvent::RunCancelled { edited_files } => {
                self.commit_streaming();
                let suffix = if edited_files.is_empty() {
                    String::new()
                } else {
                    format!(" ({} file(s) edited)", edited_files.len())
                };
                self.cells.push(HistoryCell::Notice {
                    text: format!("run cancelled{suffix}"),
                });
                self.finish_run();
                Continue::DrainQueue
            }
            _ => Continue::Nothing,
        }
    }

    /// The run's future resolved. A failed future still needs a row, but not two:
    /// the loop usually emitted `RunFailed` itself first.
    pub fn on_finished(&mut self, result: Result<String, String>) {
        match result {
            Ok(session_id) => {
                self.thread_id = Some(session_id);
                self.finish_run();
            }
            Err(reason) => {
                self.fail(reason);
                self.finish_run();
            }
        }
    }

    /// The tool row that is still waiting for its outcome. It cannot be flushed
    /// yet, so the viewport shows it in the tail instead of going quiet.
    pub fn running_tool(&self) -> Option<&HistoryCell> {
        self.cells.iter().find(|cell| tool_is_running(cell))
    }

    /// Cells that will not change again, in transcript order, ready to be written
    /// into the terminal's scrollback.
    pub fn drain_flushable(&mut self) -> Vec<HistoryCell> {
        let mut flushed = Vec::new();
        while let Some(cell) = self.cells.first() {
            if tool_is_running(cell) {
                break;
            }
            flushed.push(self.cells.remove(0));
        }
        flushed
    }

    /// Move streamed assistant text and reasoning into the transcript.
    fn commit_streaming(&mut self) {
        if !self.reasoning.trim().is_empty() {
            self.cells.push(HistoryCell::Reasoning {
                text: std::mem::take(&mut self.reasoning),
            });
        } else {
            self.reasoning.clear();
        }
        if !self.streaming.trim().is_empty() {
            self.cells.push(HistoryCell::Assistant {
                text: std::mem::take(&mut self.streaming),
            });
        } else {
            self.streaming.clear();
        }
    }

    fn settle_tool(&mut self, call_id: &str, outcome: String) -> bool {
        if call_id.is_empty() {
            return false;
        }
        for cell in &mut self.cells {
            if let HistoryCell::Tool {
                call_id: known,
                outcome: existing,
                ..
            } = cell
            {
                if known == call_id && existing == TOOL_RUNNING {
                    *existing = outcome;
                    return true;
                }
            }
        }
        false
    }

    fn fail(&mut self, reason: String) {
        let already_reported = self
            .cells
            .last()
            .map(|cell| {
                cell == &HistoryCell::Failure {
                    text: reason.clone(),
                }
            })
            .unwrap_or(false);
        if !already_reported {
            self.cells.push(HistoryCell::Failure { text: reason });
        }
    }

    fn finish_run(&mut self) {
        self.busy = false;
        self.run_id = None;
        self.approval = None;
        self.question = None;
        self.streaming.clear();
        self.reasoning.clear();
    }
}

fn tool_is_running(cell: &HistoryCell) -> bool {
    matches!(cell, HistoryCell::Tool { outcome, .. } if outcome == TOOL_RUNNING)
}

#[cfg(test)]
mod tests {
    use super::{ChatState, Continue, TokenTotals};
    use deepagent_runtime::RuntimeEvent;
    use serde_json::Value;

    fn texts(state: &ChatState) -> Vec<String> {
        state
            .cells
            .iter()
            .map(|cell| {
                cell.to_lines(200)
                    .iter()
                    .map(|line| {
                        line.spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect::<String>()
                    })
                    .collect::<String>()
            })
            .collect()
    }

    fn usage(prompt: u32, completion: u32, cache_hit: u32) -> RuntimeEvent {
        RuntimeEvent::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            reasoning_tokens: 0,
            total_tokens: prompt + completion,
            prompt_cache_hit_tokens: cache_hit,
            prompt_cache_miss_tokens: prompt.saturating_sub(cache_hit),
            cost_yuan: None,
            raw_responses_usage: None,
        }
    }

    fn tool_started(call_id: &str, name: &str, summary: &str) -> RuntimeEvent {
        RuntimeEvent::ToolStarted {
            name: name.to_string(),
            call_id: call_id.to_string(),
            arguments: Value::Null,
            tool_kind: None,
            file_path: None,
            summary: Some(summary.to_string()),
            meta: None,
        }
    }

    fn tool_completed(call_id: &str, name: &str, ok: bool, duration_ms: u64) -> RuntimeEvent {
        RuntimeEvent::ToolCompleted {
            name: name.to_string(),
            call_id: call_id.to_string(),
            ok,
            output: Value::Null,
            duration_ms,
            tool_kind: None,
            file_path: None,
            summary: None,
            meta: None,
        }
    }

    #[test]
    fn submit_runs_immediately_and_queues_while_busy() {
        let mut state = ChatState::new(None);
        assert_eq!(state.submit("first".to_string()).as_deref(), Some("first"));
        assert!(state.busy);
        assert_eq!(state.submit("second".to_string()), None);
        assert_eq!(state.submit("third".to_string()), None);
        assert_eq!(state.queued, vec!["second", "third"]);
    }

    #[test]
    fn a_running_tool_line_is_updated_in_place_by_its_completion() {
        let mut state = ChatState::new(None);
        state.on_runtime(tool_started("c1", "read_file", "src/main.rs"));
        assert_eq!(texts(&state), vec!["* read_file: src/main.rs — running"]);
        state.on_runtime(tool_completed("c1", "read_file", true, 12));
        assert_eq!(
            texts(&state),
            vec!["* read_file: src/main.rs — done in 12ms"]
        );
        assert_eq!(
            state.cells.len(),
            1,
            "the completion must update, not append"
        );
    }

    #[test]
    fn a_failed_tool_marks_its_own_line_failed() {
        let mut state = ChatState::new(None);
        state.on_runtime(tool_started("c1", "bash", "cargo test"));
        state.on_runtime(tool_completed("c1", "bash", false, 900));
        assert_eq!(texts(&state)[0], "* bash: cargo test — failed in 900ms");
    }

    #[test]
    fn a_running_tool_stays_visible_until_it_is_settled() {
        let mut state = ChatState::new(None);
        assert!(state.running_tool().is_none());
        state.on_runtime(tool_started("c1", "bash", "cargo build"));
        assert!(
            state.running_tool().is_some(),
            "the viewport has to show the in-flight tool"
        );
        state.on_runtime(tool_completed("c1", "bash", true, 4000));
        assert!(state.running_tool().is_none());
    }

    #[test]
    fn narration_before_a_tool_call_becomes_its_own_row() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::ContentDelta {
            text: "我先看一下".to_string(),
        });
        state.on_runtime(RuntimeEvent::ContentDelta {
            text: "这个文件".to_string(),
        });
        state.on_runtime(tool_started("c1", "read_file", "a.rs"));
        assert_eq!(
            texts(&state),
            vec!["  我先看一下这个文件", "* read_file: a.rs — running"]
        );
        assert!(
            state.streaming.is_empty(),
            "committed narration must not repeat"
        );
    }

    #[test]
    fn an_unsettled_tool_line_is_not_flushed_before_it_completes() {
        let mut state = ChatState::new(None);
        state.on_runtime(tool_started("c1", "bash", "ls"));
        state.on_runtime(RuntimeEvent::RunCompleted {
            message: String::new(),
            edited_files: vec![],
        });
        assert!(
            state.drain_flushable().is_empty(),
            "a running tool row must wait for its outcome"
        );
        state.on_runtime(tool_completed("c1", "bash", true, 3));
        let flushed = state.drain_flushable();
        assert_eq!(flushed.len(), 1);
    }

    #[test]
    fn completion_without_a_streamed_body_still_shows_the_final_message() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::RunCompleted {
            message: "done here".to_string(),
            edited_files: vec![],
        });
        assert_eq!(texts(&state), vec!["  done here"]);
        assert!(!state.busy);
    }

    #[test]
    fn completion_after_streaming_does_not_duplicate_the_message() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::ContentDelta {
            text: "streamed".to_string(),
        });
        state.on_runtime(RuntimeEvent::RunCompleted {
            message: "streamed".to_string(),
            edited_files: vec![],
        });
        assert_eq!(texts(&state), vec!["  streamed"]);
    }

    #[test]
    fn edited_files_are_reported_on_completion() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::RunCompleted {
            message: String::new(),
            edited_files: vec![
                deepagent_runtime::EditedFile {
                    path: "src/a.rs".to_string(),
                    lines_added: 3,
                    lines_removed: 1,
                },
                deepagent_runtime::EditedFile {
                    path: "src/b.rs".to_string(),
                    lines_added: 2,
                    lines_removed: 0,
                },
            ],
        });
        assert_eq!(
            texts(&state),
            vec!["* edited 2 file(s): src/a.rs, src/b.rs"]
        );
    }

    #[test]
    fn failure_and_cancellation_leave_distinct_rows_and_free_the_run_slot() {
        let mut state = ChatState::new(None);
        state.busy = true;
        state.on_runtime(RuntimeEvent::RunFailed {
            reason: "step limit".to_string(),
        });
        assert_eq!(texts(&state), vec!["! step limit"]);
        assert!(!state.busy);

        state.on_runtime(RuntimeEvent::RunCancelled {
            edited_files: vec![deepagent_runtime::EditedFile {
                path: "src/a.rs".to_string(),
                lines_added: 1,
                lines_removed: 0,
            }],
        });
        assert_eq!(
            texts(&state).last().map(String::as_str),
            Some("* run cancelled (1 file(s) edited)")
        );
    }

    #[test]
    fn terminal_events_ask_the_host_to_start_the_queued_prompt() {
        let mut state = ChatState::new(None);
        state.busy = true;
        assert_eq!(
            state.on_runtime(RuntimeEvent::RunCompleted {
                message: String::new(),
                edited_files: vec![],
            }),
            Continue::DrainQueue
        );
        state.busy = true;
        assert_eq!(
            state.on_runtime(RuntimeEvent::RunFailed {
                reason: "boom".to_string()
            }),
            Continue::DrainQueue
        );
    }

    #[test]
    fn a_failed_future_is_reported_once() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::RunFailed {
            reason: "upstream 500".to_string(),
        });
        state.on_finished(Err("upstream 500".to_string()));
        assert_eq!(texts(&state), vec!["! upstream 500"]);
    }

    #[test]
    fn a_successful_future_keeps_the_thread_open_for_the_next_turn() {
        let mut state = ChatState::new(None);
        state.on_finished(Ok("session-42".to_string()));
        assert_eq!(state.thread_id.as_deref(), Some("session-42"));
        assert!(!state.busy);
    }

    #[test]
    fn usage_accumulates_across_model_calls() {
        let mut state = ChatState::new(None);
        state.on_runtime(usage(1000, 100, 800));
        state.on_runtime(usage(1000, 50, 200));
        assert_eq!(state.tokens.total(), 2150);
        assert_eq!(state.tokens.cache_hit_percent(), Some(50));
    }

    #[test]
    fn cache_percent_is_unknown_without_a_prompt() {
        assert_eq!(TokenTotals::default().cache_hit_percent(), None);
    }

    #[test]
    fn reasoning_is_committed_as_its_own_row_before_the_answer() {
        let mut state = ChatState::new(None);
        state.on_runtime(RuntimeEvent::ReasoningDelta {
            text: "think".to_string(),
        });
        state.on_runtime(RuntimeEvent::ContentDelta {
            text: "say".to_string(),
        });
        state.on_runtime(RuntimeEvent::RunCompleted {
            message: String::new(),
            edited_files: vec![],
        });
        assert_eq!(texts(&state), vec!["  think", "  say"]);
    }
}
