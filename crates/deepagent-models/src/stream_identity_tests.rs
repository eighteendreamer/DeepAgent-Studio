use super::*;
use crate::test_sequence::NumberedAccumulator as ResponseAccumulator;
use serde_json::{json, Value};

#[derive(Default)]
struct Recorded(Vec<ModelStreamEvent>);

impl DeltaObserver for Recorded {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

impl Recorded {
    fn completed(&self) -> usize {
        self.0
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::ToolCallCompleted { .. }))
            .count()
    }
}

fn item() -> Value {
    json!({"type":"function_call", "id":"item-a", "call_id":"call-a",
        "name":"write_file", "arguments":r#"{"path":"a.txt","content":"PRIVATE_CONTENT"}"#})
}

fn assert_malformed_identity(events: Vec<Value>) {
    let mut acc = ResponseAccumulator::new();
    let mut rec = Recorded::default();
    let error = events
        .iter()
        .try_for_each(|event| push(&mut acc, &mut rec, event).map(|_| ()))
        .expect_err("a malformed provider identity must not become an executable call");
    assert!(
        matches!(&error, CoreError::Provider { code: Some(code), .. }
        if code == "malformed_tool_call"),
        "{error:?}"
    );
    assert!(!error.to_string().contains("PRIVATE_CONTENT"));
    assert!(!crate::classify_model_error(&error).retryable());
    assert_eq!(rec.completed(), 0);
}

#[test]
fn h151_required_identity_is_shared_by_added_done_and_terminal_items() {
    for custom in [false, true] {
        for field in ["call_id", "name"] {
            for value in [
                None,
                Some(Value::Null),
                Some(json!("")),
                Some(json!(" \t\n")),
                Some(json!(123)),
                Some(json!(false)),
            ] {
                for item_id in [false, true] {
                    let mut malformed = item();
                    if custom {
                        malformed["type"] = json!("custom_tool_call");
                        malformed["name"] = json!("apply_patch");
                        malformed["input"] = json!("PRIVATE_CONTENT");
                    }
                    if !item_id {
                        malformed.as_object_mut().unwrap().remove("id");
                    }
                    if let Some(value) = value.clone() {
                        malformed[field] = value;
                    } else {
                        malformed.as_object_mut().unwrap().remove(field);
                    }
                    for event in [
                        json!({"type":"response.output_item.added","item":malformed}),
                        done(&malformed, 0),
                        terminal(vec![malformed.clone()]),
                    ] {
                        assert_malformed_identity(vec![event]);
                    }
                }
            }
        }
    }
}

#[test]
fn h151_malformed_final_identity_prevents_publication_of_valid_sibling_calls() {
    let valid = item();
    for field in ["call_id", "name"] {
        let mut malformed = valid.clone();
        malformed.as_object_mut().unwrap().remove(field);
        assert_malformed_identity(vec![added(&valid, 0), done(&malformed, 0)]);
        malformed["id"] = json!("another-item");
        assert_malformed_identity(vec![
            done(&valid, 0),
            terminal(vec![valid.clone(), malformed]),
        ]);
    }
}

#[test]
fn h151_valid_calls_need_no_item_id_and_repeated_done_publishes_once() {
    for custom in [false, true] {
        for arguments in ["", "{}", "  {}  "] {
            let mut valid = item();
            valid.as_object_mut().unwrap().remove("id");
            valid["arguments"] = json!(arguments);
            if custom {
                valid["type"] = json!("custom_tool_call");
                valid["name"] = json!("apply_patch");
                valid["input"] = json!(arguments);
            }
            let mut start = valid.clone();
            start[if custom { "input" } else { "arguments" }] = json!("");
            for events in [
                vec![terminal(vec![valid.clone()])],
                vec![
                    done(&valid, 0),
                    done(&valid, 0),
                    terminal(vec![valid.clone()]),
                ],
                vec![
                    json!({"type":"response.output_item.added","item":start}),
                    json!({"type":if custom {"response.custom_tool_call_input.delta"}
                        else {"response.function_call_arguments.delta"},
                        "call_id":"call-a","delta":arguments}),
                    json!({"type":if custom {"response.custom_tool_call_input.done"}
                        else {"response.function_call_arguments.done"},
                        "call_id":"call-a",(if custom {"input"} else {"arguments"}):arguments}),
                    done(&valid, 0),
                    terminal(vec![valid.clone()]),
                ],
            ] {
                let mut acc = ResponseAccumulator::new();
                let mut rec = Recorded::default();
                for event in events {
                    push(&mut acc, &mut rec, &event).unwrap();
                }
                assert_eq!(rec.completed(), 1);
                let response = acc.finish().unwrap();
                let calls = response.tool_invocations_from_items();
                assert_eq!(calls.len(), 1, "no raw/fallback duplicate identity");
                assert_eq!(calls[0].0, "call-a");
                if custom {
                    assert_eq!(calls[0].2["patch"], arguments);
                } else {
                    assert_eq!(calls[0].2, json!({}));
                }
            }
        }
    }
}

#[test]
fn h151_partial_length_and_provider_failure_do_not_synthesize_calls() {
    for terminal in [
        json!({"type":"response.incomplete","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[]}}),
        json!({"type":"response.failed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","status":"failed","output":[],"error":{"code":"fixture_failure"}}}),
    ] {
        let mut acc = ResponseAccumulator::new();
        let mut rec = Recorded::default();
        push(&mut acc, &mut rec, &added(&item(), 0)).unwrap();
        let result = push(&mut acc, &mut rec, &terminal);
        assert_eq!(rec.completed(), 0);
        if result.is_ok() {
            assert!(acc
                .finish()
                .unwrap()
                .tool_invocations_from_items()
                .is_empty());
        } else {
            assert!(matches!(result, Err(CoreError::Provider { .. })));
        }
    }
}

fn added(item: &Value, index: usize) -> Value {
    let mut item = item.clone();
    item["arguments"] = json!("");
    json!({"type":"response.output_item.added", "output_index":index, "item":item})
}

fn arguments_done(item: &Value) -> Value {
    json!({"type":"response.function_call_arguments.done", "item_id":item["id"],
        "arguments":item["arguments"]})
}

fn done(item: &Value, index: usize) -> Value {
    json!({"type":"response.output_item.done", "output_index":index, "item":item})
}

fn terminal(items: Vec<Value>) -> Value {
    json!({"type":"response.completed", "response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","status":"completed", "output":items,
        "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}})
}

fn push(acc: &mut ResponseAccumulator, rec: &mut Recorded, event: &Value) -> Result<bool> {
    acc.push_sse_data_observed(&event.to_string(), rec)
}

fn assert_conflict(events: Vec<Value>) {
    let mut acc = ResponseAccumulator::new();
    let mut rec = Recorded::default();
    let result = events
        .iter()
        .try_for_each(|event| push(&mut acc, &mut rec, event).map(|_| ()));
    let error = result.expect_err("contradictory complete identities must be rejected");
    assert!(
        matches!(&error, CoreError::Provider { code: Some(code), .. }
        if code == "tool_call_identity_conflict"),
        "{error:?}"
    );
    assert!(!error.to_string().contains("PRIVATE_CONTENT"));
    assert_eq!(rec.completed(), 0, "unvalidated calls must not be prepared");
}

#[test]
fn h259_conflicting_argument_completions_are_rejected_in_both_orders() {
    let a = item();
    let mut b = a.clone();
    b["arguments"] = json!(r#"{"path":"b.txt","content":"OTHER"}"#);
    for reverse in [false, true] {
        let completions = if reverse {
            vec![done(&b, 0), arguments_done(&a)]
        } else {
            vec![arguments_done(&a), done(&b, 0)]
        };
        let mut events = vec![added(&a, 0)];
        events.extend(completions);
        events.push(terminal(vec![b.clone()]));
        assert_conflict(events);
    }
    assert_conflict(vec![added(&a, 0), arguments_done(&a), arguments_done(&b)]);
    assert_conflict(vec![added(&a, 0), done(&a, 0), done(&b, 0)]);
}

#[test]
fn h259_equivalent_and_repeated_completions_publish_one_committed_call() {
    let a = item();
    let mut b = a.clone();
    b["arguments"] = json!(r#"{ "content": "PRIVATE_CONTENT", "path": "a.txt" }"#);
    for reverse in [false, true] {
        let mut acc = ResponseAccumulator::new();
        let mut rec = Recorded::default();
        let mut events = vec![added(&a, 0)];
        events.extend(if reverse {
            vec![done(&b, 0), arguments_done(&a)]
        } else {
            vec![arguments_done(&a), done(&b, 0)]
        });
        events.extend([done(&b, 0), arguments_done(&a)]);
        for event in events {
            assert!(!push(&mut acc, &mut rec, &event).unwrap());
            assert_eq!(
                rec.completed(),
                0,
                "identity remains provisional until terminal"
            );
        }
        assert!(push(&mut acc, &mut rec, &terminal(vec![b.clone()])).unwrap());
        assert_eq!(rec.completed(), 1);
        let response = acc.finish().unwrap();
        let calls = response.tool_invocations_from_items();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call-a");
        assert_eq!(
            calls[0].2,
            serde_json::from_str::<Value>(a["arguments"].as_str().unwrap()).unwrap()
        );
    }
}

#[test]
fn h259_complete_item_identity_cannot_change_its_ids_name_or_kind() {
    let a = item();
    for (field, replacement) in [
        ("call_id", "another-call"),
        ("id", "another-item"),
        ("name", "another-tool"),
        ("type", "custom_tool_call"),
        ("type", "message"),
    ] {
        let mut b = a.clone();
        b[field] = json!(replacement);
        assert_conflict(vec![added(&a, 0), arguments_done(&a), done(&b, 0)]);
    }
    let mut b = a.clone();
    b["id"] = json!("new-item");
    b["call_id"] = json!("new-call");
    assert_conflict(vec![added(&a, 0), done(&b, 0)]);
}

#[test]
fn h259_terminal_output_cannot_replace_or_omit_an_earlier_call() {
    let a = item();
    let mut b = a.clone();
    b["arguments"] = json!(r#"{"path":"b.txt"}"#);
    assert_conflict(vec![
        added(&a, 0),
        arguments_done(&a),
        done(&a, 0),
        terminal(vec![b]),
    ]);
    assert_conflict(vec![added(&a, 0), arguments_done(&a), terminal(vec![])]);
}

#[test]
fn h259_invalid_final_json_does_not_reuse_a_valid_prepared_call() {
    let a = item();
    let mut b = a.clone();
    b["arguments"] = json!("{invalid");
    assert_conflict(vec![added(&a, 0), arguments_done(&a), done(&b, 0)]);
}

#[test]
fn h259_custom_input_is_compared_and_emitted_without_json_or_whitespace_rewriting() {
    let a = json!({"type":"custom_tool_call", "id":"item-a", "call_id":"call-a",
        "name":"apply_patch", "input":"  *** Begin Patch\n*** End Patch\n"});
    let mut start = a.clone();
    start["input"] = json!("");
    let begin = json!({"type":"response.output_item.added", "output_index":0,"item":start});
    let input_done = json!({"type":"response.custom_tool_call_input.done",
        "item_id":"item-a","input":a["input"]});
    let mut acc = ResponseAccumulator::new();
    let mut rec = Recorded::default();
    for event in [begin.clone(), input_done.clone(), done(&a, 0)] {
        push(&mut acc, &mut rec, &event).unwrap();
        assert_eq!(rec.completed(), 0);
    }
    push(&mut acc, &mut rec, &terminal(vec![a.clone()])).unwrap();
    let response = acc.finish().unwrap();
    assert_eq!(
        response.tool_invocations_from_items()[0].2["patch"],
        a["input"]
    );
    let prepared = rec
        .0
        .iter()
        .find_map(|event| match event {
            ModelStreamEvent::ToolCallCompleted { arguments, .. } => Some(arguments),
            _ => None,
        })
        .unwrap();
    assert_eq!(prepared["patch"], a["input"]);
    let mut b = a.clone();
    b["input"] = json!(a["input"].as_str().unwrap().trim());
    assert_conflict(vec![begin, input_done, done(&b, 0)]);
}

#[test]
fn h259_interleaved_sibling_is_not_prepared_when_another_call_conflicts() {
    let a = item();
    let mut b = a.clone();
    b["id"] = json!("item-b");
    b["call_id"] = json!("call-b");
    let mut conflicting = a.clone();
    conflicting["arguments"] = json!("{}");
    assert_conflict(vec![
        added(&a, 0),
        added(&b, 1),
        arguments_done(&b),
        done(&b, 1),
        arguments_done(&a),
        done(&conflicting, 0),
    ]);
}

#[test]
fn h259_incomplete_terminal_checks_done_calls_but_never_promotes_partial_calls() {
    let a = item();
    let mut conflicting = a.clone();
    conflicting["arguments"] = json!("{}");
    let incomplete = |items| {
        json!({"type":"response.incomplete",
        "response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":items}})
    };
    assert_conflict(vec![
        added(&a, 0),
        done(&a, 0),
        incomplete(vec![conflicting]),
    ]);
    let mut acc = ResponseAccumulator::new();
    let mut rec = Recorded::default();
    let partial = added(&a, 0)["item"].clone();
    push(&mut acc, &mut rec, &added(&a, 0)).unwrap();
    push(&mut acc, &mut rec, &incomplete(vec![partial])).unwrap();
    assert_eq!(rec.completed(), 0);
    assert!(acc
        .finish()
        .unwrap()
        .tool_invocations_from_items()
        .is_empty());
}

#[test]
fn h259_terminal_only_calls_and_interleaved_normal_calls_share_one_identity() {
    let a = item();
    let mut b = a.clone();
    b["id"] = json!("item-b");
    b["call_id"] = json!("call-b");
    for terminal_only in [false, true] {
        let mut acc = ResponseAccumulator::new();
        let mut rec = Recorded::default();
        if !terminal_only {
            for event in [
                added(&a, 0),
                added(&b, 1),
                arguments_done(&b),
                arguments_done(&a),
                done(&a, 0),
                done(&b, 1),
            ] {
                push(&mut acc, &mut rec, &event).unwrap();
            }
        }
        push(&mut acc, &mut rec, &terminal(vec![a.clone(), b.clone()])).unwrap();
        assert_eq!(rec.completed(), 2);
        assert_eq!(acc.finish().unwrap().tool_invocations_from_items().len(), 2);
    }
}
