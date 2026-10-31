//! Request identities must be rejected before either provider wire projection.

use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::{json, Value};
use std::sync::Mutex;

use crate::transport::EventSink;

#[derive(Default)]
struct RecordingTransport(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let events = if request.url.ends_with("/responses") {
            vec![r#"{"type":"response.completed","response":{"status":"completed"}}"#]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        for event in events {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn items(id: &str, custom: bool, shape: usize) -> Vec<ResponseInputItem> {
    let call = if custom {
        ResponseInputItem::CustomToolCall {
            call_id: id.into(),
            name: "apply_patch".into(),
            input: "PRIVATE_ARGUMENTS".into(),
        }
    } else {
        ResponseInputItem::FunctionCall {
            call_id: id.into(),
            name: "read_file".into(),
            arguments: r#"{"path":"PRIVATE_ARGUMENTS"}"#.into(),
        }
    };
    let output = if custom {
        ResponseInputItem::CustomToolCallOutput {
            call_id: id.into(),
            output: "PRIVATE_OUTPUT".into(),
        }
    } else {
        ResponseInputItem::FunctionCallOutput {
            call_id: id.into(),
            output: "PRIVATE_OUTPUT".into(),
        }
    };
    let mut input = vec![ResponseInputItem::Message {
        role: "user".into(),
        content: "Continue.".into(),
    }];
    match shape {
        0 => input.extend([call, output]),
        1 => input.push(call),
        2 => input.push(output),
        _ => unreachable!(),
    }
    input
}

async fn send(
    client: &ModelClient,
    request: ResponseRequest,
    cancel_aware: bool,
) -> Result<Response> {
    if cancel_aware {
        client
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
            .await
    } else {
        client.stream_response(request).await
    }
}

fn assert_invalid(error: CoreError) {
    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    assert_eq!(
        crate::classify_model_error(&error),
        crate::ModelFailureKind::InvalidRequest
    );
    assert!(!crate::classify_model_error(&error).retryable());
    let text = error.to_string();
    assert!(text.contains("call_id"), "{text}");
    assert!(
        !text.contains("PRIVATE_"),
        "untrusted history must not appear in diagnostics"
    );
}

fn wire_ids(body: &Value, mode: WireMode) -> Vec<&str> {
    if mode == WireMode::Responses {
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["call_id"].as_str())
            .collect()
    } else {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| {
                message["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .chain(message["tool_call_id"].as_str())
            })
            .collect()
    }
}

#[tokio::test]
async fn h148_blank_ids_never_reach_either_wire_or_cancel_aware_transport() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for custom in [false, true] {
                for id in ["", " \t\n", "\u{2003}"] {
                    for shape in 0..3 {
                        let transport = Arc::new(RecordingTransport::default());
                        let client = ModelClient::new(
                            transport.clone(),
                            ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                        );
                        let request = ResponseRequest::from_response_items(
                            "deepseek-flash",
                            None,
                            items(id, custom, shape),
                        );
                        assert_invalid(
                            send(&client, request, cancel_aware)
                                .await
                                .expect_err("blank native identity must fail before transport"),
                        );
                        assert!(transport.0.lock().unwrap().is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn h148_validation_diagnostics_are_indexed_nonretryable_and_redacted() {
    let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("Continue.")]);
    // Public input mutation must not bypass validation after construction/repair.
    request.input.push(ResponseInputItem::CustomToolCallOutput {
        call_id: " \n\t".into(),
        output: "PRIVATE_OUTPUT".into(),
    });
    let error = request
        .validate()
        .expect_err("mutated input needs the same identity gate");
    assert!(error.to_string().contains("item 1"));
    assert!(error.to_string().contains("custom_tool_call_output"));
    assert_invalid(error);
}

#[tokio::test]
async fn h148_history_refresh_and_public_input_cannot_bypass_the_send_gate() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for refresh in 0..3 {
            let transport = Arc::new(RecordingTransport::default());
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
            );
            let mut request =
                ResponseRequest::new("deepseek-flash", vec![Message::user("Continue.")]);
            match refresh {
                0 => request.input = items("", false, 0),
                1 => {
                    let mut assistant = Message::assistant("");
                    assistant
                        .tool_calls
                        .push(deepagent_core::message::ToolCall {
                            id: "\t".into(),
                            name: "read_file".into(),
                            arguments: json!({"path":"PRIVATE_ARGUMENTS"}),
                        });
                    request.replace_messages(&[
                        assistant,
                        Message::tool_result("\t", "PRIVATE_OUTPUT"),
                    ]);
                }
                2 => request.replace_history_from(&ResponseRequest::from_response_items(
                    "deepseek-flash",
                    None,
                    items("", true, 2),
                )),
                _ => unreachable!(),
            }
            assert_invalid(
                send(&client, request.clone(), true)
                    .await
                    .expect_err("refreshed history must be checked"),
            );
            assert!(transport.0.lock().unwrap().is_empty());
            // Validation does not poison the request: an explicit legitimate
            // replacement remains usable, without rewriting the old history.
            request.replace_messages(&[Message::user("Fresh input.")]);
            send(&client, request, false).await.unwrap();
            assert_eq!(transport.0.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn h148_valid_identity_and_existing_missing_result_repair_remain_compatible() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for custom in [false, true] {
                for shape in 0..3 {
                    let transport = Arc::new(RecordingTransport::default());
                    let client = ModelClient::new(
                        transport.clone(),
                        ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                    );
                    let request = ResponseRequest::from_response_items(
                        "deepseek-flash",
                        None,
                        items("call-h148-control", custom, shape),
                    );
                    request.validate().unwrap();
                    send(&client, request, cancel_aware).await.unwrap();
                    let bodies = transport.0.lock().unwrap();
                    assert_eq!(bodies.len(), 1);
                    let ids = wire_ids(&bodies[0], mode);
                    assert_eq!(ids, ["call-h148-control", "call-h148-control"]);
                }
            }
        }
    }
}

fn h149_history(
    ids: [&str; 2],
    first_custom: bool,
    second_custom: bool,
    shape: usize,
) -> Vec<ResponseInputItem> {
    let first = items(ids[0], first_custom, 0);
    let mut second = items(ids[1], second_custom, 0);
    match &mut second[1] {
        ResponseInputItem::FunctionCall {
            name, arguments, ..
        } => {
            *name = "write_file".into();
            *arguments = r#"{"path":"PRIVATE_SECOND_ARGUMENTS"}"#.into();
        }
        ResponseInputItem::CustomToolCall { input, .. } => {
            *input = "PRIVATE_SECOND_ARGUMENTS".into();
        }
        _ => unreachable!(),
    }
    match &mut second[2] {
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => {
            *output = "PRIVATE_SECOND_OUTPUT".into();
        }
        _ => unreachable!(),
    }
    let (a, ao) = (first[1].clone(), first[2].clone());
    let (b, bo) = (second[1].clone(), second[2].clone());
    let mut history = vec![first[0].clone()];
    history.extend(match shape {
        0 => vec![a, b, ao, bo], // parallel calls, then both real results
        1 => vec![a, ao, b, bo], // same identity reused in another turn/group
        2 => vec![a, b],         // cancellation/replay left results missing
        3 => vec![ao, bo],       // duplicate orphan results
        4 => vec![a, ao, bo],    // a single call with two results
        5 => vec![a, b, ao],     // two calls with one result
        6 => vec![a.clone(), a, ao.clone(), ao], // even identical items conflict
        _ => unreachable!(),
    });
    history
}

#[test]
fn h149_repair_preserves_ambiguous_history_instead_of_losing_evidence() {
    for first_custom in [false, true] {
        for second_custom in [false, true] {
            for shape in 0..7 {
                let history = h149_history(
                    ["PRIVATE_DUPLICATE_ID"; 2],
                    first_custom,
                    second_custom,
                    shape,
                );
                assert_eq!(crate::responses::repair_tool_call_pairs(&history), history);
                let request =
                    ResponseRequest::from_response_items("deepseek-flash", None, history.clone());
                assert_eq!(request.input, history);
                assert_eq!(
                    crate::responses::repair_tool_call_pairs(&request.input),
                    history
                );
            }
        }
    }
}

#[tokio::test]
async fn h149_duplicate_ids_never_reach_either_wire_or_cancel_aware_transport() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for first_custom in [false, true] {
                for second_custom in [false, true] {
                    for shape in 0..7 {
                        let transport = Arc::new(RecordingTransport::default());
                        let client = ModelClient::new(
                            transport.clone(),
                            ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                        );
                        let request = ResponseRequest::from_response_items(
                            "deepseek-flash",
                            None,
                            h149_history(
                                ["PRIVATE_DUPLICATE_ID"; 2],
                                first_custom,
                                second_custom,
                                shape,
                            ),
                        );
                        assert_invalid(
                            send(&client, request, cancel_aware)
                                .await
                                .expect_err("duplicate identity must fail before transport"),
                        );
                        assert!(transport.0.lock().unwrap().is_empty());
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h149_history_refresh_public_edits_and_legitimate_replacement_are_checked() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for refresh in 0..4 {
            let transport = Arc::new(RecordingTransport::default());
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
            );
            let mut request =
                ResponseRequest::new("deepseek-flash", vec![Message::user("Continue.")]);
            let history = h149_history(["PRIVATE_DUPLICATE_ID"; 2], false, true, 0);
            match refresh {
                0 => request.input = history,
                1 => {
                    let messages = crate::responses::messages_from_response_items(None, &history);
                    request.replace_messages(&messages);
                }
                2 => request.replace_history_from(&ResponseRequest::from_response_items(
                    "deepseek-flash",
                    None,
                    history,
                )),
                3 => {
                    let messages = crate::responses::messages_from_response_items(None, &history);
                    request = ResponseRequest::new("deepseek-flash", messages);
                }
                _ => unreachable!(),
            }
            assert_invalid(
                send(&client, request.clone(), true)
                    .await
                    .expect_err("all public history refresh paths need the identity gate"),
            );
            assert!(transport.0.lock().unwrap().is_empty());
            // Public replacement of the authoritative input must work, even
            // when the retained Chat-order snapshot was previously invalid.
            request.input = items("h149-legitimate-replacement", true, 0);
            send(&client, request, false).await.unwrap();
            let bodies = transport.0.lock().unwrap();
            assert_eq!(bodies.len(), 1);
            assert_eq!(
                wire_ids(&bodies[0], mode),
                ["h149-legitimate-replacement"; 2]
            );
        }
    }
}

#[test]
fn h149_duplicate_diagnostics_are_indexed_nonretryable_and_redacted() {
    let mut request = ResponseRequest::new("deepseek-flash", vec![]);
    request.input = h149_history(["PRIVATE_DUPLICATE_ID"; 2], false, true, 0);
    let error = request.validate().expect_err("duplicate call identity");
    let text = error.to_string();
    assert!(text.contains("item 2") && text.contains("item 1"), "{text}");
    assert!(text.contains("custom_tool_call"), "{text}");
    assert_invalid(error);
}

#[tokio::test]
async fn h149_unique_parallel_and_sequential_histories_remain_compatible() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for first_custom in [false, true] {
            for second_custom in [false, true] {
                for shape in 0..3 {
                    let transport = Arc::new(RecordingTransport::default());
                    let client = ModelClient::new(
                        transport.clone(),
                        ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                    );
                    let request = ResponseRequest::from_response_items(
                        "deepseek-flash",
                        None,
                        h149_history(
                            ["h149-first", "h149-second"],
                            first_custom,
                            second_custom,
                            shape,
                        ),
                    );
                    send(&client, request, true).await.unwrap();
                    let bodies = transport.0.lock().unwrap();
                    assert_eq!(bodies.len(), 1);
                    let expected = if shape == 1 {
                        ["h149-first", "h149-first", "h149-second", "h149-second"]
                    } else {
                        ["h149-first", "h149-second", "h149-first", "h149-second"]
                    };
                    assert_eq!(wire_ids(&bodies[0], mode), expected);
                    if shape != 2 {
                        assert!(bodies[0].to_string().contains("PRIVATE_SECOND_OUTPUT"));
                    }
                }
            }
        }
    }
}
