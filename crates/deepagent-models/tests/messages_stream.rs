//! Messages wire fixtures exercise the existing neutral result/observer boundary.
use deepagent_models::messages::MessagesAccumulator;
use deepagent_models::{
    classify_model_error, DeltaObserver, FinishReason, ModelFailureKind, ModelStreamEvent,
    ResponseItem, Usage,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Observed {
    events: Vec<ModelStreamEvent>,
    usage: Vec<(Usage, Value)>,
}

impl DeltaObserver for Observed {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.events.push(event);
    }

    fn on_usage(&mut self, usage: Usage, raw: &Value) {
        self.usage.push((usage, raw.clone()));
    }
}

fn start() -> Value {
    json!({"type":"message_start","message":{
        "id":"msg-fixture","type":"message","role":"assistant","model":"fixture-model",
        "content":[],"stop_reason":null,"stop_sequence":null,
        "usage":{"input_tokens":11,"output_tokens":0,"cache_read_input_tokens":13,
            "cache_creation_input_tokens":5}
    }})
}

fn block(index: usize, content: Value) -> Value {
    json!({"type":"content_block_start","index":index,"content_block":content})
}

fn delta(index: usize, content: Value) -> Value {
    json!({"type":"content_block_delta","index":index,"delta":content})
}

fn close(index: usize) -> Value {
    json!({"type":"content_block_stop","index":index})
}

fn terminal(reason: &str, usage: Value) -> Value {
    json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},
        "usage":usage})
}

fn push(acc: &mut MessagesAccumulator, observer: &mut Observed, event: Value) -> bool {
    acc.push_sse_data_observed(&event.to_string(), observer)
        .unwrap()
}

fn text_events() -> Vec<Value> {
    vec![
        start(),
        block(0, json!({"type":"text","text":""})),
        delta(0, json!({"type":"text_delta","text":"中文 🦀"})),
        close(0),
        terminal("end_turn", json!({"output_tokens":7})),
        json!({"type":"message_stop"}),
    ]
}

#[test]
fn text_thinking_signature_and_parallel_tools_keep_their_order_and_identity() {
    let mut acc = MessagesAccumulator::new();
    let mut seen = Observed::default();
    let events = vec![
        start(),
        block(0, json!({"type":"thinking","thinking":""})),
        delta(0, json!({"type":"thinking_delta","thinking":"检查"})),
        delta(0, json!({"type":"signature_delta","signature":"sig-"})),
        delta(0, json!({"type":"signature_delta","signature":"one"})),
        close(0),
        block(1, json!({"type":"text","text":"读取"})),
        delta(1, json!({"type":"text_delta","text":"文件 🦀"})),
        close(1),
        block(
            2,
            json!({"type":"tool_use","id":"call-a","name":"read_file","input":{}}),
        ),
        delta(
            2,
            json!({"type":"input_json_delta","partial_json":"{\"path\":"}),
        ),
        delta(
            2,
            json!({"type":"input_json_delta","partial_json":"\"a.txt\"}"}),
        ),
        close(2),
        block(
            3,
            json!({"type":"tool_use","id":"call-b","name":"list_directory","input":{}}),
        ),
        close(3),
        terminal(
            "tool_use",
            json!({"output_tokens":7,
            "output_tokens_details":{"thinking_tokens":3,"prompt":"private-canary"},
            "private":"private-canary"}),
        ),
        json!({"type":"message_stop"}),
    ];
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            push(&mut acc, &mut seen, event.clone()),
            index + 1 == events.len()
        );
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(
        result.output_items,
        vec![
            ResponseItem::Reasoning {
                id: None,
                content: "检查".into(),
                signature: Some("sig-one".into())
            },
            ResponseItem::Message {
                role: "assistant".into(),
                content: "读取文件 🦀".into()
            },
            ResponseItem::FunctionCall {
                call_id: "call-a".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a.txt"}"#.into()
            },
            ResponseItem::FunctionCall {
                call_id: "call-b".into(),
                name: "list_directory".into(),
                arguments: "{}".into()
            },
        ]
    );
    assert_eq!(
        result.usage,
        Some(Usage {
            prompt_tokens: 29,
            completion_tokens: 7,
            reasoning_tokens: 3,
            total_tokens: 36,
            prompt_cache_hit_tokens: 13,
            prompt_cache_miss_tokens: 16
        })
    );
    assert!(!result
        .raw_usage
        .unwrap()
        .to_string()
        .contains("private-canary"));
    assert_eq!(
        seen.events
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. }))
            .count(),
        2
    );
    assert!(seen.events.iter().any(
        |event| matches!(event, ModelStreamEvent::ToolCallCompleted {index:2,id,arguments,..}
            if id == "call-a" && arguments == &json!({"path":"a.txt"}))
    ));
    assert_eq!(
        seen.events
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::Finished { .. }))
            .count(),
        1
    );
}

#[test]
fn usage_snapshots_replace_counters_and_preserve_omitted_cache_counts() {
    let mut acc = MessagesAccumulator::new();
    let mut seen = Observed::default();
    push(&mut acc, &mut seen, start());
    push(
        &mut acc,
        &mut seen,
        terminal("end_turn", json!({"output_tokens":4})),
    );
    push(
        &mut acc,
        &mut seen,
        terminal(
            "end_turn",
            json!({"input_tokens":17,"output_tokens":7,
        "cache_creation":{"ephemeral_5m_input_tokens":2,"ephemeral_1h_input_tokens":3}}),
        ),
    );
    push(&mut acc, &mut seen, json!({"type":"message_stop"}));
    let response = acc.finish().unwrap();
    let usage = response.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 35);
    assert_eq!(usage.completion_tokens, 7);
    assert_eq!(usage.total_tokens, 42);
    assert_eq!(usage.prompt_cache_hit_tokens, 13);
    assert_eq!(usage.prompt_cache_miss_tokens, 22);
    assert_eq!(seen.usage.len(), 3);
    assert_eq!(
        response.raw_usage.unwrap()["cache_creation"]["ephemeral_1h_input_tokens"],
        3
    );
}

#[test]
fn overload_keeps_latest_usage_and_uses_the_shared_redacted_provider_error() {
    let mut acc = MessagesAccumulator::new();
    let mut seen = Observed::default();
    push(&mut acc, &mut seen, start());
    push(
        &mut acc,
        &mut seen,
        terminal("end_turn", json!({"output_tokens":7})),
    );
    let error = acc
        .push_sse_data_observed(
            &json!({"type":"error","error":{
                "type":"overloaded_error","message":"Busy Bearer private-canary sk-private-canary"
            }})
            .to_string(),
            &mut seen,
        )
        .unwrap_err();
    let kind = classify_model_error(&error);
    assert_eq!(kind, ModelFailureKind::Overloaded);
    assert!(!kind.retryable());
    assert!(!error.to_string().contains("private-canary"));
    assert_eq!(seen.usage.last().unwrap().0.completion_tokens, 7);
    assert!(!seen
        .events
        .iter()
        .any(|event| matches!(event, ModelStreamEvent::Finished { .. })));
    assert!(acc.finish().is_err());
}

#[test]
fn length_cannot_promote_a_partial_tool_into_an_executable_call() {
    let mut acc = MessagesAccumulator::new();
    let mut seen = Observed::default();
    for event in [
        start(),
        block(
            0,
            json!({"type":"tool_use","id":"partial","name":"read_file","input":{}}),
        ),
        delta(
            0,
            json!({"type":"input_json_delta","partial_json":"{\"path\":"}),
        ),
        terminal("max_tokens", json!({"output_tokens":8})),
        json!({"type":"message_stop"}),
    ] {
        push(&mut acc, &mut seen, event);
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.finish_reason, Some(FinishReason::Length));
    assert!(result.tool_invocations_from_items().is_empty());
    assert!(!seen
        .events
        .iter()
        .any(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. })));
}

#[test]
fn every_truncated_prefix_is_rejected_even_after_a_stop_reason() {
    let events = text_events();
    for length in 0..events.len() {
        let mut acc = MessagesAccumulator::new();
        let mut seen = Observed::default();
        for event in &events[..length] {
            push(&mut acc, &mut seen, event.clone());
        }
        assert!(acc.finish().is_err(), "accepted prefix length {length}");
    }
}

#[test]
fn refusal_is_a_typed_failure_and_unknown_events_do_not_drop_text() {
    for (stop, reason) in [
        ("end_turn", FinishReason::Stop),
        ("stop_sequence", FinishReason::Stop),
        ("refusal", FinishReason::ContentFilter),
    ] {
        let mut acc = MessagesAccumulator::new();
        let mut seen = Observed::default();
        for event in text_events().into_iter().take(4).chain([
            json!({"type":"ping"}),
            json!({"type":"future_event","unknown":"private-canary"}),
            terminal(stop, json!({"output_tokens":7})),
            json!({"type":"message_stop"}),
        ]) {
            push(&mut acc, &mut seen, event);
        }
        let result = acc.finish().unwrap();
        assert_eq!(result.output_text_projection(), "中文 🦀");
        assert_eq!(result.finish_reason, Some(reason));
        assert_eq!(result.into_generation_result().is_err(), stop == "refusal");
    }
}

#[test]
fn invalid_lifecycle_or_block_identity_poisons_the_result() {
    for events in [
        vec![close(0)],
        vec![start(), start()],
        vec![start(), json!({"type":"message_stop"})],
        vec![start(), block(1, json!({"type":"text","text":""}))],
        vec![
            start(),
            block(0, json!({"type":"text","text":""})),
            close(0),
            close(0),
        ],
        vec![
            start(),
            block(0, json!({"type":"text","text":""})),
            delta(
                0,
                json!({"type":"thinking_delta","thinking":"private-canary"}),
            ),
        ],
        vec![
            start(),
            block(0, json!({"type":"text","text":""})),
            delta(9, json!({"type":"text_delta","text":"private-canary"})),
        ],
        vec![
            start(),
            block(
                0,
                json!({"type":"tool_use","id":"a","name":"read_file","input":{}}),
            ),
            close(0),
            block(
                1,
                json!({"type":"tool_use","id":"a","name":"read_file","input":{}}),
            ),
        ],
        vec![
            start(),
            block(
                0,
                json!({"type":"tool_use","id":"","name":"read_file","input":{}}),
            ),
        ],
        vec![
            start(),
            block(
                0,
                json!({"type":"redacted_thinking","data":"private-canary"}),
            ),
        ],
        vec![start(), terminal("pause_turn", json!({"output_tokens":7}))],
    ] {
        let mut acc = MessagesAccumulator::new();
        let mut seen = Observed::default();
        let mut failed = false;
        for event in events {
            if let Err(error) = acc.push_sse_data_observed(&event.to_string(), &mut seen) {
                assert!(!error.to_string().contains("private-canary"));
                failed = true;
                break;
            }
        }
        assert!(failed);
        assert!(acc.finish().is_err());
    }
}

#[test]
fn malformed_tool_arguments_never_reach_the_execution_observer() {
    for input in [
        "{\"secret\":\"private-canary\"",
        "[]",
        "null",
        "\"private-canary\"",
    ] {
        let mut acc = MessagesAccumulator::new();
        let mut seen = Observed::default();
        push(&mut acc, &mut seen, start());
        push(
            &mut acc,
            &mut seen,
            block(
                0,
                json!({"type":"tool_use",
            "id":"a","name":"read_file","input":{}}),
            ),
        );
        push(
            &mut acc,
            &mut seen,
            delta(0, json!({"type":"input_json_delta","partial_json":input})),
        );
        let error = acc
            .push_sse_data_observed(&close(0).to_string(), &mut seen)
            .unwrap_err();
        assert!(!error.to_string().contains("private-canary"));
        assert!(!seen
            .events
            .iter()
            .any(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. })));
        assert!(acc.finish().is_err());
    }
}

#[test]
fn invalid_or_overflowing_usage_is_rejected_without_echoing_values() {
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ] {
        for bad in [
            json!(-1),
            json!(1.5),
            json!("private-canary"),
            json!(u64::MAX),
            Value::Null,
        ] {
            let mut acc = MessagesAccumulator::new();
            let mut seen = Observed::default();
            push(&mut acc, &mut seen, start());
            let mut usage = json!({"output_tokens":7});
            usage[field] = bad;
            let error = acc
                .push_sse_data_observed(&terminal("end_turn", usage).to_string(), &mut seen)
                .unwrap_err();
            assert!(!error.to_string().contains("private-canary"));
            assert_eq!(seen.usage.len(), 1);
            assert!(acc.finish().is_err());
        }
    }
    for usage in [
        json!({"output_tokens":u32::MAX}),
        json!({"input_tokens":u32::MAX,"output_tokens":7}),
        json!({"output_tokens":7,"output_tokens_details":{"thinking_tokens":8}}),
    ] {
        let mut acc = MessagesAccumulator::new();
        let mut seen = Observed::default();
        push(&mut acc, &mut seen, start());
        assert!(acc
            .push_sse_data_observed(&terminal("end_turn", usage).to_string(), &mut seen)
            .is_err());
        assert!(acc.finish().is_err());
    }
}

#[test]
fn completed_stream_cannot_emit_a_second_terminal() {
    let mut acc = MessagesAccumulator::new();
    let mut seen = Observed::default();
    for event in text_events() {
        push(&mut acc, &mut seen, event);
    }
    assert!(acc
        .push_sse_data_observed(r#"{"type":"message_stop"}"#, &mut seen)
        .is_err());
    assert_eq!(
        seen.events
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::Finished { .. }))
            .count(),
        1
    );
    assert!(acc.finish().is_err());
}
