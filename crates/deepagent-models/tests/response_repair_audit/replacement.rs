//! Independent H174 historical-fix verification. No production edits.
use super::*;
use deepagent_models::{classify_model_error, response_items_from_legacy_messages, WireMode};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

fn call(id: &str, custom: bool) -> Message {
    Message::assistant("").with_tool_calls(vec![ToolCall {
        id: id.into(),
        name: if custom { "apply_patch" } else { "read_file" }.into(),
        arguments: if custom {
            json!({"patch":"SYNTHETIC_PATCH"})
        } else {
            json!({"path":"SYNTHETIC_PATH"})
        },
    }])
}

fn histories() -> Vec<(&'static str, Vec<Message>)> {
    let f = || call("function-1", false);
    let c = || call("custom-1", true);
    let fo = || Message::tool_result("function-1", "REAL_FUNCTION_RESULT");
    let co = || Message::tool_result("custom-1", "REAL_CUSTOM_RESULT");
    let mut parallel = f();
    parallel.tool_calls.extend(c().tool_calls);
    [
        ("paired-function", vec![f(), fo()]),
        ("paired-custom", vec![c(), co()]),
        ("dangling-function", vec![f()]),
        ("dangling-custom", vec![c()]),
        ("orphan-function", vec![fo()]),
        ("earlier-real-result", vec![fo(), f()]),
        ("mixed-partial", vec![parallel.clone(), fo()]),
        ("parallel-complete", vec![parallel, fo(), co()]),
    ]
    .into_iter()
    .map(|(name, tail)| {
        let mut messages = vec![
            Message::system("FRESH_SYSTEM"),
            Message::user("Continue synthetic history."),
        ];
        messages.extend(tail);
        (name, messages)
    })
    .collect()
}

async fn send(client: &ModelClient, request: ResponseRequest, cancel_aware: bool) -> Result<()> {
    if cancel_aware {
        client
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
            .await?;
    } else {
        client.stream_response(request).await?;
    }
    Ok(())
}

type IdentifiedPayloads = Vec<(String, String)>;

fn pairs(body: &Value, mode: WireMode) -> (IdentifiedPayloads, IdentifiedPayloads) {
    let mut calls = Vec::new();
    let mut outputs = Vec::new();
    if mode == WireMode::Responses {
        for item in body["input"].as_array().unwrap() {
            match item["type"].as_str().unwrap() {
                "function_call" | "custom_tool_call" => calls.push((
                    item["call_id"].as_str().unwrap().into(),
                    item["type"].as_str().unwrap().into(),
                )),
                "function_call_output" | "custom_tool_call_output" => outputs.push((
                    item["call_id"].as_str().unwrap().into(),
                    item["output"].as_str().unwrap().into(),
                )),
                _ => {}
            }
        }
    } else {
        for message in body["messages"].as_array().unwrap() {
            if let Some(tools) = message["tool_calls"].as_array() {
                for tool in tools {
                    calls.push((
                        tool["id"].as_str().unwrap().into(),
                        tool["function"]["name"].as_str().unwrap().into(),
                    ));
                }
            }
            if message["role"] == "tool" {
                outputs.push((
                    message["tool_call_id"].as_str().unwrap().into(),
                    message["content"].as_str().unwrap().into(),
                ));
            }
        }
    }
    (calls, outputs)
}

#[tokio::test]
async fn h174_all_public_refresh_paths_keep_pairs_and_retry_parameters_on_both_wires() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for (name, messages) in histories() {
                let original = messages.clone();
                // These are legacy histories. Resolve their old custom identity
                // explicitly; a new same-name Message call is now a function.
                let (instructions, input) = response_items_from_legacy_messages(&messages);
                let custom_calls: Vec<_> = input
                    .iter()
                    .filter(|item| matches!(item, ResponseInputItem::CustomToolCall { .. }))
                    .cloned()
                    .collect();
                let mut from_items =
                    ResponseRequest::from_response_items("deepseek-flash", instructions, input);
                from_items.max_output_tokens = Some(777);
                from_items.temperature = Some(0.4);
                let mut replaced = ResponseRequest::new(
                    "deepseek-flash",
                    vec![Message::system("STALE_SYSTEM"), call("STALE_CALL", false)],
                );
                // Refreshing a known native custom call must preserve its type,
                // while still discarding the stale instructions/call below.
                replaced.input.extend(custom_calls);
                replaced.max_output_tokens = Some(777);
                replaced.temperature = Some(0.4);
                replaced.replace_messages(&messages);
                assert_eq!(replaced.instructions, from_items.instructions);
                assert_eq!(replaced.input, from_items.input, "{name}");
                assert_eq!(
                    messages, original,
                    "replacement must not rewrite caller history"
                );
                assert_eq!(replaced.max_output_tokens, Some(777));
                assert_eq!(replaced.temperature, Some(0.4));
                replaced.validate().unwrap();
                let mut refreshed =
                    ResponseRequest::new("deepseek-flash", vec![Message::user("stale")]);
                refreshed.max_output_tokens = Some(777);
                refreshed.temperature = Some(0.4);
                refreshed.replace_history_from(&replaced);
                for request in [from_items, replaced, refreshed] {
                    assert_eq!(request.max_output_tokens, Some(777));
                    assert_eq!(request.temperature, Some(0.4));
                    let transport = Arc::new(RecordingTransport::default());
                    let client = ModelClient::new(
                        transport.clone(),
                        ModelConfig::deepseek("offline-audit-placeholder").with_wire_mode(mode),
                    );
                    send(&client, request, cancel_aware).await.unwrap();
                    let recorded = transport.requests.lock().unwrap();
                    assert_eq!(
                        recorded.len(),
                        1,
                        "{name}: no model/tool retry for history repair"
                    );
                    let body = &recorded[0];
                    assert_eq!(
                        body[if mode == WireMode::Responses {
                            "max_output_tokens"
                        } else {
                            "max_tokens"
                        }],
                        777
                    );
                    assert!(!body.to_string().contains("STALE_"));
                    let (calls, outputs) = pairs(body, mode);
                    let mut call_ids = calls.iter().map(|(id, _)| id).collect::<Vec<_>>();
                    let mut output_ids = outputs.iter().map(|(id, _)| id).collect::<Vec<_>>();
                    call_ids.sort();
                    output_ids.sort();
                    assert_eq!(call_ids, output_ids, "{name}");
                    assert!(!calls.is_empty());
                    for (id, output) in &outputs {
                        let real = messages
                            .iter()
                            .find(|m| m.tool_call_id.as_deref() == Some(id.as_str()));
                        if let Some(real) = real {
                            assert_eq!(output, &real.content, "{name}: preserve durable result");
                        } else {
                            assert_eq!(
                                serde_json::from_str::<Value>(output).unwrap()["status"],
                                "error"
                            );
                        }
                    }
                    if mode == WireMode::Responses && calls.iter().any(|(id, _)| id == "custom-1") {
                        assert!(calls.contains(&("custom-1".into(), "custom_tool_call".into())));
                        assert!(body["input"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|item| item["type"] == "custom_tool_call_output"
                                && item["call_id"] == "custom-1"));
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h174_bad_replacement_ids_fail_before_transport_then_valid_replacement_recovers() {
    let bad_histories = vec![
        vec![call(" \n\t", false)],
        vec![Message::tool_result("", "SYNTHETIC_PRIVATE_OUTPUT")],
        vec![
            call("SYNTHETIC_PRIVATE_DUP", false),
            call("SYNTHETIC_PRIVATE_DUP", true),
        ],
        vec![
            call("same", false),
            Message::tool_result("same", "FIRST"),
            Message::tool_result("same", "SECOND"),
        ],
    ];
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for messages in &bad_histories {
                let transport = Arc::new(RecordingTransport::default());
                let client = ModelClient::new(
                    transport.clone(),
                    ModelConfig::deepseek("offline-audit-placeholder").with_wire_mode(mode),
                );
                let mut request =
                    ResponseRequest::new("deepseek-flash", vec![Message::user("seed")]);
                request.replace_messages(messages);
                let error = send(&client, request.clone(), cancel_aware)
                    .await
                    .unwrap_err();
                assert!(matches!(error, deepagent_core::CoreError::Invalid(_)));
                assert!(!classify_model_error(&error).retryable());
                assert!(!error.to_string().contains("SYNTHETIC_PRIVATE"));
                assert!(transport.requests.lock().unwrap().is_empty());
                request.replace_messages(&[
                    call("recovered", false),
                    Message::tool_result("recovered", "REAL_RECOVERED_RESULT"),
                ]);
                send(&client, request, cancel_aware).await.unwrap();
                let recorded = transport.requests.lock().unwrap();
                assert_eq!(recorded.len(), 1);
                assert_eq!(
                    pairs(&recorded[0], mode).1,
                    vec![("recovered".into(), "REAL_RECOVERED_RESULT".into())]
                );
            }
        }
    }
}
