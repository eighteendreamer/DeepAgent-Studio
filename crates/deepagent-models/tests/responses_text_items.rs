//! H-81: display deltas and completed message items have different ownership.

#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use sequence_fixture::NumberedAccumulator as ResponseAccumulator;

use deepagent_models::{
    DeltaObserver, FinishReason, ModelStreamEvent, Response, ResponseOutputItem,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Display(String);

impl DeltaObserver for Display {
    fn on_event(&mut self, event: ModelStreamEvent) {
        if let ModelStreamEvent::ContentDelta { text } = event {
            self.0.push_str(&text);
        }
    }
}

fn message(id: &str, text: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}

fn added(id: &str, index: u64) -> Value {
    json!({"type":"response.output_item.added","output_index":index,
        "item":{"type":"message","id":id,"role":"assistant","content":[]}})
}

fn delta(id: &str, index: u64, part: u64, text: &str) -> Value {
    json!({"type":"response.output_text.delta","item_id":id,"output_index":index,
        "content_index":part,"delta":text})
}

fn done(id: &str, index: u64, text: &str) -> Value {
    json!({"type":"response.output_item.done","output_index":index,"item":message(id,text)})
}

fn terminal(output: Option<Vec<Value>>, incomplete: bool) -> Value {
    let mut event = json!({"type":if incomplete {"response.incomplete"} else {"response.completed"},
        "response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":if incomplete {"incomplete"} else {"completed"},
        "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}});
    if let Some(output) = output {
        event["response"]["output"] = output.into();
    }
    if incomplete {
        event["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
    }
    event
}

fn assemble(events: Vec<Value>) -> (Response, String) {
    let mut acc = ResponseAccumulator::new();
    let mut display = Display::default();
    for event in events {
        acc.push_sse_data_observed(&event.to_string(), &mut display)
            .unwrap();
    }
    (acc.finish().unwrap(), display.0)
}

fn texts(response: &Response) -> Vec<&str> {
    response
        .output_items
        .iter()
        .filter_map(|item| match item {
            ResponseOutputItem::Message { role, content } if role == "assistant" => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn h81_completed_items_are_authoritative_not_a_joined_or_empty_fallback() {
    for parts in [["first|", "second"], ["same", "same"], ["", ""]] {
        for deltas in [false, true] {
            for item_done in [false, true] {
                let mut events = Vec::new();
                let mut output = Vec::new();
                for (index, text) in parts.iter().enumerate() {
                    let id = format!("message-{index}");
                    events.push(added(&id, index as u64));
                    if deltas {
                        events.push(delta(&id, index as u64, 0, text));
                    }
                    if item_done {
                        events.push(done(&id, index as u64, text));
                    }
                    output.push(message(&id, text));
                }
                events.push(terminal(Some(output), false));
                let (response, display) = assemble(events);
                assert_eq!(texts(&response), parts);
                assert_eq!(response.output_items.len(), 2);
                assert_eq!(response.output_text_projection(), parts.concat());
                assert_eq!(
                    display,
                    if deltas {
                        parts.concat()
                    } else {
                        String::new()
                    }
                );
                assert_eq!(response.usage.unwrap().total_tokens, 5);
                assert_eq!(response.raw_usage.unwrap()["total_tokens"], 5);
            }
        }
    }
}

#[test]
fn h81_missing_done_falls_back_only_for_its_own_item_even_with_identical_text() {
    for incomplete in [false, true] {
        for parts in [["complete|", "partial"], ["same", "same"]] {
            let (response, display) = assemble(vec![
                added("a", 0),
                delta("a", 0, 0, parts[0]),
                done("a", 0, parts[0]),
                added("b", 1),
                delta("b", 1, 0, parts[1]),
                terminal(None, incomplete),
            ]);
            assert_eq!(texts(&response), parts);
            assert_eq!(response.output_text_projection(), display);
            assert_eq!(
                response.finish_reason,
                Some(if incomplete {
                    FinishReason::Length
                } else {
                    FinishReason::Stop
                })
            );
        }
    }
}

#[test]
fn h81_interleaved_content_parts_keep_their_item_and_part_order() {
    let (response, display) = assemble(vec![
        added("a", 0),
        added("b", 1),
        delta("a", 0, 1, "尾\r\n\0"),
        delta("b", 1, 0, "sibling"),
        delta("a", 0, 0, "head"),
        delta("a", 0, 0, "|"),
        terminal(None, true),
    ]);
    assert_eq!(texts(&response), ["head|尾\r\n\0", "sibling"]);
    assert_eq!(display, "尾\r\n\0siblinghead|");
}

#[test]
fn h81_done_content_never_borrows_a_siblings_text_or_replaces_explicit_empty() {
    let mut no_content = message("b", "unused");
    no_content.as_object_mut().unwrap().remove("content");
    let (response, _) = assemble(vec![
        added("a", 0),
        added("b", 1),
        delta("a", 0, 0, "provisional"),
        delta("b", 1, 0, "own fallback"),
        done("a", 0, ""),
        json!({"type":"response.output_item.done","output_index":1,"item":no_content}),
        terminal(None, false),
    ]);
    assert_eq!(texts(&response), ["", "own fallback"]);
}

#[test]
fn h81_completed_messages_keep_added_order_and_missing_deltas_do_not_make_empty_items() {
    let (response, _) = assemble(vec![
        added("a", 0),
        added("b", 1),
        done("b", 1, "second"),
        done("a", 0, "first"),
        added("uncompleted-empty", 2),
        terminal(None, false),
    ]);
    assert_eq!(texts(&response), ["first", "second"]);
}

#[test]
fn h81_unscoped_legacy_deltas_still_form_one_message_without_a_terminal_guess() {
    for item_done in [false, true] {
        let mut events = vec![
            json!({"type":"response.output_text.delta","delta":"legacy "}),
            json!({"type":"response.output_text.delta","delta":"answer"}),
        ];
        if item_done {
            let mut item = message("unused", "legacy answer");
            item.as_object_mut().unwrap().remove("id");
            events.push(json!({"type":"response.output_item.done","item":item}));
        }
        events.push(terminal(None, false));
        let (response, display) = assemble(events);
        assert_eq!(texts(&response), ["legacy answer"]);
        assert_eq!(response.output_text_projection(), display);
    }
    let mut acc = ResponseAccumulator::new();
    acc.push_sse_data_observed(
        &delta("partial", 0, 0, "not complete").to_string(),
        &mut Display::default(),
    )
    .unwrap();
    assert!(acc
        .finish()
        .unwrap_err()
        .to_string()
        .contains("terminal response event"));
}

#[test]
fn h81_text_fallback_does_not_duplicate_reasoning_search_or_tool_items() {
    let tool = json!({"type":"function_call","id":"fc","call_id":"call","name":"read_file","arguments":"{}"});
    let search = json!({"type":"web_search_call","id":"search","status":"completed","action":{"query":"fixture"}});
    let (response, _) = assemble(vec![
        added("a", 0),
        delta("a", 0, 0, "first"),
        done("a", 0, "first"),
        json!({"type":"response.output_item.done","output_index":1,"item":search}),
        json!({"type":"response.output_item.done","output_index":2,"item":tool}),
        added("b", 3),
        delta("b", 3, 0, "second"),
        json!({"type":"response.reasoning_text.delta","delta":"reasoning"}),
        terminal(Some(vec![message("a", "first"), search, tool]), false),
    ]);
    assert_eq!(texts(&response), ["first", "second"]);
    assert_eq!(response.output_items.len(), 5);
    assert_eq!(
        response.reasoning_text_projection().as_deref(),
        Some("reasoning")
    );
    assert_eq!(
        response.tool_invocations_from_items(),
        vec![("call".into(), "read_file".into(), json!({}))]
    );
    assert!(
        matches!(&response.output_items[1], ResponseOutputItem::WebSearchCall { id, .. } if id == "search")
    );
}

#[test]
fn h81_identity_can_arrive_in_stages_but_conflicts_never_choose_a_siblings_text() {
    for id_first in [false, true] {
        let mut fragment = delta("a", 0, 0, "own text");
        fragment
            .as_object_mut()
            .unwrap()
            .remove(if id_first { "output_index" } else { "item_id" });
        let (response, _) = assemble(vec![
            fragment,
            done("a", 0, "own text"),
            terminal(None, false),
        ]);
        assert_eq!(texts(&response), ["own text"]);
    }
    for conflict in [
        json!({"type":"response.output_text.delta","delta":"PRIVATE_DELTA"}),
        delta("a", 1, 0, "PRIVATE_DELTA"),
        delta("different-private-id", 0, 0, "PRIVATE_DELTA"),
    ] {
        let mut acc = ResponseAccumulator::new();
        let mut display = Display::default();
        for event in [added("a", 0), added("b", 1)] {
            acc.push_sse_data_observed(&event.to_string(), &mut display)
                .unwrap();
        }
        let error = acc
            .push_sse_data_observed(&conflict.to_string(), &mut display)
            .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE"));
        assert!(!error.to_string().contains("private-id"));
        assert!(display.0.is_empty());
    }
}

#[test]
fn h81_legacy_reasoning_still_precedes_its_uncompleted_text_fallback() {
    let (response, _) = assemble(vec![
        json!({"type":"response.reasoning_text.delta","delta":"think first"}),
        json!({"type":"response.output_text.delta","delta":"answer"}),
        terminal(None, false),
    ]);
    assert_eq!(
        response.output_items,
        vec![
            ResponseOutputItem::Reasoning {
                id: None,
                content: "think first".into()
            },
            ResponseOutputItem::Message {
                role: "assistant".into(),
                content: "answer".into()
            },
        ]
    );
}

#[tokio::test]
async fn h81_cancel_after_text_fragments_does_not_turn_fallback_into_a_successful_response() {
    use deepagent_core::{error::Result, message::Message};
    use deepagent_models::{
        transport::{EventSink, HttpTransport, TransportRequest},
        ModelClient, ModelConfig, ResponseRequest,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct PartialTransport(Arc<tokio::sync::Notify>);
    #[async_trait::async_trait]
    impl HttpTransport for PartialTransport {
        async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            let mut sequence = sequence_fixture::FixtureSequence::default();
            sink.on_event(&sequence.event(&added("a", 0).to_string()))?;
            sink.on_event(&sequence.event(&delta("a", 0, 0, "visible partial").to_string()))?;
            self.0.notify_one();
            std::future::pending().await
        }
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let cancel = Arc::new(AtomicBool::new(false));
    let client = ModelClient::new(
        Arc::new(PartialTransport(entered.clone())),
        ModelConfig::deepseek("offline-placeholder"),
    );
    let cancellation = async {
        entered.notified().await;
        cancel.store(true, Ordering::Release);
    };
    let mut display = Display::default();
    let request = ResponseRequest::new("deepseek-flash", vec![Message::user("offline")]);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            client.stream_response_observed_cancelled(request, &mut display, cancel.clone()),
            cancellation
        )
        .0
    })
    .await
    .expect("bounded cancellation");
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(display.0, "visible partial");
}
