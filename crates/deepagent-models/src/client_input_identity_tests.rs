//! Request identities and native kinds must be safe on both wire projections.

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
            vec![
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
            ]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        let mut sequence = crate::test_sequence::FixtureSequence::default();
        for event in events {
            if sink.on_event(&sequence.event(event))? {
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

fn h80_histories(
    first_custom: bool,
    shape: usize,
) -> (Vec<ResponseInputItem>, Vec<ResponseInputItem>) {
    let mut valid = h149_history(
        ["h80-first", "h80-second"],
        first_custom,
        !first_custom,
        shape % 2,
    );
    if shape >= 2 {
        valid.insert(
            if shape % 2 == 0 { 3 } else { 2 },
            ResponseInputItem::Message {
                role: "assistant".into(),
                content: "Tool narration.".into(),
            },
        );
    }
    let mut mixed = valid.clone();
    for item in &mut mixed {
        *item = match item {
            ResponseInputItem::FunctionCallOutput { call_id, output } => {
                ResponseInputItem::CustomToolCallOutput {
                    call_id: call_id.clone(),
                    output: output.clone(),
                }
            }
            ResponseInputItem::CustomToolCallOutput { call_id, output } => {
                ResponseInputItem::FunctionCallOutput {
                    call_id: call_id.clone(),
                    output: output.clone(),
                }
            }
            _ => continue,
        };
    }
    (valid, mixed)
}

#[test]
fn h80_unique_mixed_output_kinds_are_repaired_losslessly_and_idempotently() {
    for first_custom in [false, true] {
        for shape in 0..4 {
            let (mut valid, mut mixed) = h80_histories(first_custom, shape);
            // Preserve bytes, including non-JSON text, Unicode, NUL and CRLF.
            for history in [&mut valid, &mut mixed] {
                for item in history {
                    match item {
                        ResponseInputItem::FunctionCallOutput { output, .. }
                        | ResponseInputItem::CustomToolCallOutput { output, .. } => {
                            let deepagent_core::tool_content::ToolResultBody::Text(text) = output
                            else {
                                panic!("identity fixture expects a legacy textual tool result");
                            };
                            text.push_str("\r\n\0中文\t{ raw: not JSON }");
                        }
                        _ => {}
                    }
                }
            }
            let original = mixed.clone();
            let expected = crate::responses::repair_tool_call_pairs(&valid);
            let repaired = crate::responses::repair_tool_call_pairs(&mixed);
            assert_ne!(mixed, expected);
            assert_eq!(repaired, expected);
            assert_eq!(
                crate::responses::repair_tool_call_pairs(&repaired),
                repaired
            );
            assert_eq!(mixed, original, "repair must not mutate the source history");
            let request = ResponseRequest::from_response_items("deepseek-flash", None, mixed);
            request.validate().unwrap();
            assert_eq!(request.input, expected);
            assert_eq!(
                serde_json::to_value(request).unwrap()["input"],
                json!(expected)
            );
        }
    }
}

fn request_from_history(history: Vec<ResponseInputItem>, refresh: usize) -> ResponseRequest {
    match refresh {
        0 => ResponseRequest::from_response_items("deepseek-flash", None, history),
        1 => {
            let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("stale")]);
            request.replace_history_from(&ResponseRequest::from_response_items(
                "deepseek-flash",
                None,
                history,
            ));
            request
        }
        2 => ResponseRequest::new(
            "deepseek-flash",
            crate::responses::messages_from_response_items(None, &history),
        ),
        3 => {
            let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("stale")]);
            request.replace_messages(&crate::responses::messages_from_response_items(
                None, &history,
            ));
            request
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn h80_constructor_and_history_refresh_send_corrected_pairs_on_both_wires() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for first_custom in [false, true] {
                for shape in 0..4 {
                    for refresh in 0..4 {
                        let (valid, mixed) = h80_histories(first_custom, shape);
                        let transport = Arc::new(RecordingTransport::default());
                        let client = ModelClient::new(
                            transport.clone(),
                            ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                        );
                        send(&client, request_from_history(valid, refresh), cancel_aware)
                            .await
                            .unwrap();
                        send(&client, request_from_history(mixed, refresh), cancel_aware)
                            .await
                            .unwrap();
                        let bodies = transport.0.lock().unwrap();
                        assert_eq!(bodies.len(), 2);
                        assert_eq!(
                            bodies[1], bodies[0],
                            "mixed input must match its valid control"
                        );
                        assert!(bodies[1].to_string().contains("PRIVATE_SECOND_OUTPUT"));
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h80_public_input_edits_are_rejected_before_both_transports_without_sticky_failure() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for first_custom in [false, true] {
                for reversed in [false, true] {
                    let (valid, mut mixed) = h80_histories(first_custom, 0);
                    if reversed {
                        mixed.reverse();
                    }
                    let transport = Arc::new(RecordingTransport::default());
                    let client = ModelClient::new(
                        transport.clone(),
                        ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                    );
                    let mut request = ResponseRequest::new("deepseek-flash", vec![]);
                    request.input = mixed.clone();
                    let error = request
                        .validate()
                        .expect_err("public input bypassed repair");
                    let diagnostic = error.to_string();
                    assert!(
                        diagnostic.contains("item") && diagnostic.contains("kind"),
                        "{diagnostic}"
                    );
                    assert!(
                        !diagnostic.contains("h80-first") && !diagnostic.contains("h80-second")
                    );
                    assert_invalid(error);
                    assert_invalid(
                        send(&client, request.clone(), cancel_aware)
                            .await
                            .expect_err("no mixed wire"),
                    );
                    assert_eq!(request.input, mixed);
                    assert!(transport.0.lock().unwrap().is_empty());
                    request.input = valid;
                    send(&client, request, cancel_aware).await.unwrap();
                    assert_eq!(transport.0.lock().unwrap().len(), 1);
                }
            }
        }
    }
}

#[test]
fn h80_mixed_kind_repair_never_hides_blank_or_duplicate_identities() {
    for id in ["", " \t", "PRIVATE_DUPLICATE_ID"] {
        let (_, mut mixed) = h80_histories(true, 0);
        for item in &mut mixed {
            match item {
                ResponseInputItem::FunctionCall { call_id, .. }
                | ResponseInputItem::CustomToolCall { call_id, .. }
                | ResponseInputItem::FunctionCallOutput { call_id, .. }
                | ResponseInputItem::CustomToolCallOutput { call_id, .. } => *call_id = id.into(),
                _ => {}
            }
        }
        assert_eq!(crate::responses::repair_tool_call_pairs(&mixed), mixed);
        let request = ResponseRequest::from_response_items("deepseek-flash", None, mixed.clone());
        assert_eq!(request.input, mixed);
        assert_invalid(
            request
                .validate()
                .expect_err("kind repair must not choose an ambiguous call"),
        );
    }
}

fn h261_histories(
    first_custom: bool,
    second_custom: bool,
    layout: usize,
) -> (Vec<ResponseInputItem>, Vec<ResponseInputItem>) {
    let first = items("h261-first", first_custom, 0);
    let second = items("h261-second", second_custom, 0);
    let (a, mut ao, b, mut bo) = (
        first[1].clone(),
        first[2].clone(),
        second[1].clone(),
        second[2].clone(),
    );
    for (item, payload) in [
        (&mut ao, "PRIVATE_FIRST_RESULT\r\n\0中文\t{ raw: not JSON }"),
        (&mut bo, "PRIVATE_SECOND_RESULT"),
    ] {
        match item {
            ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. } => *output = payload.into(),
            _ => unreachable!(),
        }
    }
    let narration = ResponseInputItem::Message {
        role: "assistant".into(),
        content: "Tool narration.".into(),
    };
    let (unordered, ordered) = match layout {
        0 => (
            vec![ao.clone(), bo.clone(), a.clone(), b.clone()],
            vec![a, b, ao, bo],
        ),
        1 => (
            vec![ao.clone(), a.clone(), bo.clone(), b.clone()],
            vec![a, ao, b, bo],
        ),
        2 => (
            vec![bo.clone(), a.clone(), b.clone(), ao.clone()],
            vec![a, b, ao, bo],
        ),
        3 => (
            vec![
                ao.clone(),
                a.clone(),
                narration.clone(),
                bo.clone(),
                b.clone(),
            ],
            vec![a, ao, narration, b, bo],
        ),
        _ => unreachable!(),
    };
    let prefix = vec![
        first[0].clone(),
        ResponseInputItem::Reasoning {
            id: Some("h261-reasoning".into()),
            content: "Reasoning.".into(),
            signature: None,
        },
        ResponseInputItem::WebSearchCall {
            id: "h261-search".into(),
            status: "completed".into(),
            action: None,
        },
    ];
    (
        [prefix.clone(), unordered].concat(),
        [prefix, ordered].concat(),
    )
}

#[test]
fn h261_early_outputs_are_preserved_losslessly_with_stable_call_groups() {
    for first_custom in [false, true] {
        for second_custom in [false, true] {
            for layout in 0..4 {
                let (unordered, expected) = h261_histories(first_custom, second_custom, layout);
                let original = unordered.clone();
                let repaired = crate::responses::repair_tool_call_pairs(&unordered);
                assert_eq!(repaired, expected);
                assert_eq!(
                    crate::responses::repair_tool_call_pairs(&repaired),
                    repaired
                );
                assert_eq!(unordered, original);
                let request =
                    ResponseRequest::from_response_items("deepseek-flash", None, unordered);
                request.validate().unwrap();
                assert_eq!(request.input, expected);
                // Provider reasoning uses content parts, unlike the durable
                // ResponseItem's internal plain-string representation.
                let mut expected_wire = json!(expected);
                expected_wire[1]["content"] = json!([{
                    "type": "reasoning_text", "text": "Reasoning."
                }]);
                assert_eq!(
                    serde_json::to_value(request).unwrap()["input"],
                    expected_wire
                );
            }
        }
    }
}

#[tokio::test]
async fn h261_early_results_reach_both_wires_through_shared_history_refresh() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for first_custom in [false, true] {
                for second_custom in [false, true] {
                    for layout in 0..4 {
                        for refresh in 0..4 {
                            let (unordered, ordered) =
                                h261_histories(first_custom, second_custom, layout);
                            let transport = Arc::new(RecordingTransport::default());
                            let client = ModelClient::new(
                                transport.clone(),
                                ModelConfig::deepseek("offline-placeholder").with_wire_mode(mode),
                            );
                            send(
                                &client,
                                request_from_history(ordered, refresh),
                                cancel_aware,
                            )
                            .await
                            .unwrap();
                            send(
                                &client,
                                request_from_history(unordered, refresh),
                                cancel_aware,
                            )
                            .await
                            .unwrap();
                            let bodies = transport.0.lock().unwrap();
                            assert_eq!(bodies.len(), 2);
                            assert_eq!(
                                bodies[1], bodies[0],
                                "late call must consume its real result"
                            );
                            let body = bodies[1].to_string();
                            assert!(
                                body.contains("PRIVATE_FIRST_RESULT")
                                    && body.contains("PRIVATE_SECOND_RESULT")
                            );
                            assert!(!body.contains("tool result missing"));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn h261_late_calls_cannot_hide_blank_or_ambiguous_execution_identities() {
    for id in ["", " \t", "PRIVATE_DUPLICATE_ID"] {
        let (mut unordered, _) = h261_histories(false, true, 0);
        for item in &mut unordered {
            match item {
                ResponseInputItem::FunctionCall { call_id, .. }
                | ResponseInputItem::CustomToolCall { call_id, .. }
                | ResponseInputItem::FunctionCallOutput { call_id, .. }
                | ResponseInputItem::CustomToolCallOutput { call_id, .. } => *call_id = id.into(),
                _ => {}
            }
        }
        assert_eq!(
            crate::responses::repair_tool_call_pairs(&unordered),
            unordered
        );
        let request =
            ResponseRequest::from_response_items("deepseek-flash", None, unordered.clone());
        assert_eq!(request.input, unordered);
        assert_invalid(
            request
                .validate()
                .expect_err("a late call is not permission to choose an identity"),
        );
    }
}

#[test]
fn h261_real_empty_results_and_truly_missing_pairs_keep_existing_recovery_policy() {
    for custom in [false, true] {
        for payload in ["", "tool result missing", "{\"status\":\"error\"}"] {
            let mut pair = items("h261-result-control", custom, 0);
            match &mut pair[2] {
                ResponseInputItem::FunctionCallOutput { output, .. }
                | ResponseInputItem::CustomToolCallOutput { output, .. } => {
                    *output = payload.into()
                }
                _ => unreachable!(),
            }
            assert_eq!(crate::responses::repair_tool_call_pairs(&pair), pair);
        }
        let missing = crate::responses::repair_tool_call_pairs(&items("h261-missing", custom, 1));
        let output = match &missing[2] {
            ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. } => output,
            _ => unreachable!(),
        };
        assert!(output.text().contains("tool result missing"));
        let orphan = items("h261-orphan", custom, 2);
        let repaired = crate::responses::repair_tool_call_pairs(&orphan);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[2], orphan[1]);
        assert!(
            matches!(&repaired[1],
                ResponseInputItem::FunctionCall { name, .. } if !custom && name == "unknown_tool"
            ) || matches!(&repaired[1],
                ResponseInputItem::CustomToolCall { name, .. } if custom && name == "apply_patch"
            )
        );
    }
}
