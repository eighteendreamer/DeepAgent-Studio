//! H-258: the existing model request lifecycle owns only its speculative text.
use super::*;

pub(super) fn started(step: usize) -> RuntimeEvent {
    RuntimeEvent::ModelRequestStarted {
        step,
        model: "synthetic".into(),
        reasoning_effort: "none".into(),
        message_count: 1,
        tool_count: 0,
    }
}

fn reset(step: usize, attempt: usize) -> RuntimeEvent {
    RuntimeEvent::ModelAttemptReset {
        step,
        attempt,
        reason: "synthetic connection error".into(),
    }
}

fn complete(step: usize) -> RuntimeEvent {
    RuntimeEvent::ModelRequestCompleted {
        step,
        elapsed_ms: 1,
    }
}

fn content(text: &str) -> RuntimeEvent {
    RuntimeEvent::ContentDelta { text: text.into() }
}

fn reasoning(text: &str) -> RuntimeEvent {
    RuntimeEvent::ReasoningDelta { text: text.into() }
}

fn finished() -> RuntimeEvent {
    RuntimeEvent::RunCompleted {
        message: "valid".into(),
        edited_files: vec![],
    }
}

#[test]
fn h258_each_retry_restores_the_same_unicode_baseline_and_keeps_real_cells() {
    for reasoning_first in [false, true] {
        let mut state = ChatState::new(None);
        state.streaming = "已提交正文🙂|".into();
        state.reasoning = "已提交推理é|".into();
        state.cells.push(HistoryCell::Notice {
            text: "real immutable fact".into(),
        });
        state.on_runtime(started(1));
        for attempt in 1..=3 {
            for event in if reasoning_first {
                [reasoning("discarded"), content("discarded")]
            } else {
                [content("discarded"), reasoning("discarded")]
            } {
                state.on_runtime(event);
            }
            state.on_runtime(reset(1, attempt));
            assert_eq!(state.streaming, "已提交正文🙂|");
            assert_eq!(state.reasoning, "已提交推理é|");
        }
        state.on_runtime(content("valid"));
        state.on_runtime(reasoning("valid thought"));
        state.on_runtime(complete(1));
        state.on_runtime(finished());
        assert_eq!(
            state.drain_flushable(),
            vec![
                HistoryCell::Notice {
                    text: "real immutable fact".into()
                },
                HistoryCell::Reasoning {
                    text: "已提交推理é|valid thought".into()
                },
                HistoryCell::Assistant {
                    text: "已提交正文🙂|valid".into()
                },
            ]
        );
    }
}

#[test]
fn h258_resets_cannot_rewind_a_different_or_already_committed_step() {
    let mut state = ChatState::new(None);
    state.on_runtime(started(0));
    state.on_runtime(content("valid"));
    state.on_runtime(reset(1, 1));
    assert_eq!(state.streaming, "valid");
    state.on_runtime(complete(0));
    state.on_runtime(reset(0, 1));
    assert_eq!(state.streaming, "valid");
    state.on_runtime(started(1));
    state.on_runtime(content("discarded"));
    state.on_runtime(reset(0, 2));
    assert_eq!(state.streaming, "validdiscarded");
    state.on_runtime(reset(1, 1));
    assert_eq!(state.streaming, "valid");
}

#[test]
fn h258_terminal_failure_cancellation_and_future_error_discard_only_uncommitted_text() {
    for terminal in 0..3 {
        let mut state = ChatState::new(None);
        state.streaming = "committed|".into();
        state.on_runtime(started(1));
        state.on_runtime(content("discarded"));
        state.on_runtime(reasoning("discarded thought"));
        match terminal {
            0 => {
                state.on_runtime(RuntimeEvent::RunFailed {
                    reason: "failed".into(),
                });
            }
            1 => {
                state.on_runtime(RuntimeEvent::RunCancelled {
                    edited_files: vec![],
                });
            }
            _ => state.on_finished(Err("future failed".into())),
        }
        let cells = state.drain_flushable();
        assert!(!format!("{cells:?}").contains("discarded"));
        assert!(cells.contains(&HistoryCell::Assistant {
            text: "committed|".into()
        }));
        assert!(!state.busy);
        state.on_runtime(started(2));
        state.on_runtime(content("next run"));
        state.on_runtime(reset(2, 1));
        assert!(state.streaming.is_empty());
    }
}

#[test]
fn h258_tools_and_committed_narration_survive_later_attempt_resets_and_flushes() {
    let mut state = ChatState::new(None);
    state.on_runtime(started(0));
    state.on_runtime(content("committed narration"));
    state.on_runtime(complete(0));
    state.on_runtime(RuntimeEvent::ToolStarted {
        name: "read_file".into(),
        call_id: "read-1".into(),
        arguments: serde_json::json!({"path":"synthetic.txt"}),
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: None,
    });
    let mut scrollback = state.drain_flushable();
    assert_eq!(
        scrollback,
        [HistoryCell::Assistant {
            text: "committed narration".into()
        }]
    );
    state.on_runtime(RuntimeEvent::ToolCompleted {
        content: Vec::new(),
        name: "read_file".into(),
        call_id: "read-1".into(),
        ok: true,
        output: serde_json::json!("synthetic"),
        duration_ms: 1,
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: None,
    });
    scrollback.extend(state.drain_flushable());
    state.on_runtime(started(1));
    state.on_runtime(content("discarded"));
    assert!(state.drain_flushable().is_empty());
    state.on_runtime(reset(1, 1));
    assert!(state.streaming.is_empty());
    assert!(state.drain_flushable().is_empty());
    state.on_runtime(content("valid"));
    state.on_runtime(complete(1));
    state.on_runtime(finished());
    scrollback.extend(state.drain_flushable());
    assert_eq!(scrollback.len(), 3);
    assert!(
        matches!(&scrollback[1], HistoryCell::Tool { outcome, .. } if outcome == "done in 1ms")
    );
    assert_eq!(
        scrollback[2],
        HistoryCell::Assistant {
            text: "valid".into()
        }
    );
    assert!(!format!("{scrollback:?}").contains("discarded"));
}

#[test]
fn h258_uncommitted_text_never_flushes_even_if_a_real_tool_row_arrives_early() {
    let mut state = ChatState::new(None);
    state.on_runtime(started(0));
    state.on_runtime(content("discarded"));
    state.on_runtime(RuntimeEvent::ToolStarted {
        name: "read_file".into(),
        call_id: "real-call".into(),
        arguments: serde_json::Value::Null,
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: None,
    });
    assert!(state.drain_flushable().is_empty());
    state.on_runtime(reset(0, 1));
    assert!(!format!("{:?}", state.cells).contains("discarded"));
    assert!(state.running_tool().is_some());
}
