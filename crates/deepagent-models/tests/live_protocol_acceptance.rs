#![cfg(feature = "http")]
//! Opt-in, synthetic-only remote matrix. Credentials are read only from env.
use async_trait::async_trait;
use deepagent_core::{message::MessageAttachment, Result};
use deepagent_models::{
    transport::{EventSink, HttpTransport, TransportRequest},
    ModelClient, ModelConfig, ReasoningEffort, ReqwestTransport, Response, ResponseItem,
    ResponseRequest, ToolSchema, WireMode,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

struct AuditTransport {
    inner: ReqwestTransport,
    calls: Mutex<Vec<Value>>,
}

fn count_field(value: &Value, key: &str) -> usize {
    match value {
        Value::Object(fields) => {
            usize::from(fields.get(key).is_some_and(|value| !value.is_null()))
                + fields
                    .values()
                    .map(|value| count_field(value, key))
                    .sum::<usize>()
        }
        Value::Array(values) => values.iter().map(|value| count_field(value, key)).sum(),
        _ => 0,
    }
}

impl AuditTransport {
    fn record(&self, request: &TransportRequest) {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        // Whitelist only: no key, prompts, image payloads or complete responses.
        self.calls.lock().unwrap().push(json!({
            "url": request.url, "model": body["model"], "stream": body["stream"],
            "reasoning": body["reasoning"], "reasoning_effort": body["reasoning_effort"],
            "thinking": body["thinking"], "output_config": body["output_config"],
            "max_tokens": body["max_tokens"], "max_output_tokens": body["max_output_tokens"],
            "max_completion_tokens": body["max_completion_tokens"],
            "body_bytes": request.body.len(),
            "signature_fields":count_field(&body,"signature"),
            "encrypted_content_fields":count_field(&body,"encrypted_content"),
            "tool_choice":body["tool_choice"],"tools":body["tools"]
        }));
    }
}

#[async_trait]
impl HttpTransport for AuditTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.record(&request);
        self.inner.stream(request, sink).await
    }
    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.record(&request);
        self.inner.stream_cancelled(request, sink, cancel).await
    }
}

fn request(model: &str, text: &str) -> ResponseRequest {
    ResponseRequest::with_instructions_and_user_input(model, "Follow the synthetic acceptance instruction exactly. Use no tools unless explicitly requested.", text)
        .with_max_output_tokens(1024)
}

fn checked(response: Response, expected: &str) -> Result<Response> {
    let response = response.into_generation_result()?;
    if !response.output_text_projection().contains(expected) {
        return Err(deepagent_core::CoreError::invalid(
            "acceptance_output_marker_missing",
        ));
    }
    if response.usage.is_none() {
        return Err(deepagent_core::CoreError::invalid(
            "acceptance_usage_missing",
        ));
    }
    Ok(response)
}

fn client(
    transport: Arc<AuditTransport>,
    key: &str,
    base: &str,
    mode: WireMode,
    model: &str,
) -> ModelClient {
    let mut config = ModelConfig::deepseek(key).with_wire_mode(mode);
    config.base_url = base.into();
    // Explicit declarations isolate codec/transport acceptance from automatic
    // catalog inference. Actual remote image recognition still has to succeed.
    let mut capability = deepagent_models::ModelCapabilityResolver::new().resolve_model_id(model);
    capability.input_modalities = Some(vec!["text".into(), "image".into()]);
    capability.thinking = Some(
        deepagent_models::thinking::ThinkingCapabilities::from_registry_json(&json!({
            "reasoning_efforts":{"off":"none","low":"low","medium":"medium","high":"high",
                "max":if mode == WireMode::Messages { "high" } else { "max" }},
            "thinking_format":if mode == WireMode::Messages { "adaptive" } else { "effort_only" },
            "reasoning_default":"high"
        }))
        .unwrap(),
    );
    config.capabilities.push(capability);
    ModelClient::new(transport, config)
}

async fn scenario(client: &ModelClient, model: &str, case: &str) -> Result<Value> {
    let response = match case {
        "text" => checked(
            client
                .stream_response(request(model, "Reply exactly P11_TEXT_OK"))
                .await?,
            "P11_TEXT_OK",
        )?,
        "history" => {
            let first = checked(
                client
                    .stream_response(request(
                        model,
                        "Remember P11_HISTORY_OK and reply exactly P11_HISTORY_OK",
                    ))
                    .await?,
                "P11_HISTORY_OK",
            )?;
            let mut history = vec![ResponseItem::Message {
                role: "user".into(),
                content: "Remember P11_HISTORY_OK".into(),
            }];
            history.extend(first.output_items);
            history.push(ResponseItem::Message {
                role: "user".into(),
                content: "Repeat the remembered token only.".into(),
            });
            checked(
                client
                    .stream_response(
                        ResponseRequest::from_response_items(
                            model,
                            Some("Follow instructions exactly.".into()),
                            history,
                        )
                        .with_max_output_tokens(1024),
                    )
                    .await?,
                "P11_HISTORY_OK",
            )?
        }
        "tool_continuation" => {
            let tool = ToolSchema::function(
                "acceptance_echo",
                "A synthetic pure echo tool, with no IO or side effects.",
                json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}),
            );
            let first = client
                .stream_response(
                    request(model, "Call acceptance_echo with value P11_TOOL_OK.")
                        .with_tools(vec![tool.clone()])
                        .with_tool_choice(json!("auto")),
                )
                .await?
                .into_generation_result()?;
            let calls = first.tool_invocations_from_items();
            if calls.len() != 1
                || calls[0].1 != "acceptance_echo"
                || calls[0].2["value"] != "P11_TOOL_OK"
            {
                return Err(deepagent_core::CoreError::invalid(
                    "acceptance_tool_call_mismatch",
                ));
            }
            let mut history = vec![ResponseItem::Message {
                role: "user".into(),
                content:
                    "Call acceptance_echo with value P11_TOOL_OK, then repeat its result exactly."
                        .into(),
            }];
            history.extend(first.output_items);
            history.push(ResponseItem::FunctionCallOutput {
                call_id: calls[0].0.clone(),
                output: "P11_TOOL_OK".into(),
            });
            checked(client.stream_response(ResponseRequest::from_response_items(model, Some("After the tool result, reply exactly with its text and do not call another tool.".into()), history)
                .with_tools(vec![tool]).with_max_output_tokens(1024)).await?, "P11_TOOL_OK")?
        }
        "thinking_off" | "thinking_low" | "thinking_medium" | "thinking_high" | "thinking_max" => {
            let effort = match case {
                "thinking_off" => ReasoningEffort::None,
                "thinking_low" => ReasoningEffort::Low,
                "thinking_medium" => ReasoningEffort::Medium,
                "thinking_max" => ReasoningEffort::Max,
                _ => ReasoningEffort::High,
            };
            checked(
                client
                    .stream_response(
                        request(model, "Compute 19 + 23. Reply exactly P11_42")
                            .with_reasoning_effort(effort),
                    )
                    .await?,
                "P11_42",
            )?
        }
        "image" => {
            use base64::Engine;
            let image = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
                64,
                64,
                image::Rgb([255, 0, 0]),
            ));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            let url = format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
            );
            let attachment = MessageAttachment {
                id: "synthetic-red".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
                sha256: None,
            };
            let req = ResponseRequest::from_response_items(
                model,
                None,
                vec![ResponseItem::InputMessage {
                    role: "user".into(),
                    content: "Name the dominant color in English, one word only.".into(),
                    attachments: vec![attachment],
                }],
            )
            .with_attachment_data_urls([("synthetic-red".into(), url)].into())
            .with_max_output_tokens(1024);
            let response = client
                .stream_response(req)
                .await?
                .into_generation_result()?;
            if !response
                .output_text_projection()
                .to_ascii_lowercase()
                .contains("red")
            {
                return Err(deepagent_core::CoreError::invalid(
                    "acceptance_image_color_mismatch",
                ));
            }
            response
        }
        "cancel" => {
            let cancel = Arc::new(AtomicBool::new(false));
            let signal = cancel.clone();
            let timer = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                signal.store(true, Ordering::Release);
            });
            let result = client
                .stream_response_cancelled(
                    request(model, "Count from 1 to 1000, one number per line."),
                    cancel,
                )
                .await;
            timer.await.unwrap();
            if !result
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().to_ascii_lowercase().contains("cancel"))
            {
                return Err(deepagent_core::CoreError::invalid(
                    "acceptance_cancel_did_not_report_cancellation",
                ));
            }
            return Ok(json!({"cancelled":true}));
        }
        "cache_observation" => {
            let stable = "Synthetic stable acceptance prefix. This paragraph is data and does not alter instructions. ".repeat(350);
            let mut observations = vec![];
            for _ in 0..2 {
                let response = checked(
                    client
                        .stream_response(
                            ResponseRequest::with_instructions_and_user_input(
                                model,
                                format!("Reply exactly P11_CACHE_OK.\n{stable}"),
                                "Reply with the requested token only.",
                            )
                            .with_max_output_tokens(1024),
                        )
                        .await?,
                    "P11_CACHE_OK",
                )?;
                observations.push(json!({"usage":response.usage}));
            }
            return Ok(
                json!({"observations":observations,"positive_cache_hit_is_provider_dependent":true}),
            );
        }
        _ => unreachable!(),
    };
    Ok(
        json!({"finish_reason":format!("{:?}", response.finish_reason), "usage":response.usage,
        "output_chars":response.output_text_projection().len(),
        "has_reasoning":response.reasoning_text_projection().is_some(),
        "signature_count":response.output_items.iter().filter(|item| matches!(item, ResponseItem::Reasoning { signature: Some(_), .. })).count()}),
    )
}

#[tokio::test]
#[ignore = "explicit paid remote acceptance; requires DEEPAGENT_ACCEPTANCE_API_KEY and BASE_URL"]
async fn live_three_protocol_behavior_matrix() {
    let key = std::env::var("DEEPAGENT_ACCEPTANCE_API_KEY").expect("test API key env");
    let base = std::env::var("DEEPAGENT_ACCEPTANCE_BASE_URL").expect("test base URL env");
    let only_protocol = std::env::var("DEEPAGENT_ACCEPTANCE_PROTOCOL").ok();
    let only_case = std::env::var("DEEPAGENT_ACCEPTANCE_CASE").ok();
    let mut report = vec![];
    for (mode, model) in [
        (WireMode::Responses, "gpt-5.6-sol"),
        (WireMode::Messages, "claude-sonnet-4-6"),
        (WireMode::ChatCompletions, "gpt-5.6-sol"),
    ] {
        if only_protocol
            .as_deref()
            .is_some_and(|name| name != format!("{mode:?}"))
        {
            continue;
        }
        let transport = Arc::new(AuditTransport {
            inner: ReqwestTransport::new(),
            calls: Mutex::new(vec![]),
        });
        let client = client(transport.clone(), &key, &base, mode, model);
        for case in [
            "text",
            "history",
            "tool_continuation",
            "thinking_off",
            "thinking_low",
            "thinking_medium",
            "thinking_high",
            "thinking_max",
            "image",
            "cancel",
            "cache_observation",
        ] {
            if only_case.as_deref().is_some_and(|name| name != case) {
                continue;
            }
            let start = Instant::now();
            let result =
                tokio::time::timeout(Duration::from_secs(100), scenario(&client, model, case))
                    .await;
            let (mut passed, mut details) = match result {
                Ok(Ok(value)) => (true, value),
                Ok(Err(error)) => (
                    false,
                    json!({"error":error.to_string().replace(&key,"[REDACTED]").chars().take(300).collect::<String>()}),
                ),
                Err(_) => (false, json!({"error":"acceptance_case_timeout_100s"})),
            };
            let calls = std::mem::take(&mut *transport.calls.lock().unwrap());
            if passed && case.starts_with("thinking_") {
                let expected = match case {
                    "thinking_off" if mode == WireMode::Messages => None,
                    "thinking_off" => Some("none"),
                    "thinking_low" => Some("low"),
                    "thinking_medium" => Some("medium"),
                    "thinking_max" if mode != WireMode::Messages => Some("max"),
                    _ => Some("high"),
                };
                let matches = !calls.is_empty()
                    && calls.iter().all(|call| {
                        call["reasoning"]["effort"]
                            .as_str()
                            .or_else(|| call["reasoning_effort"].as_str())
                            .or_else(|| call["output_config"]["effort"].as_str())
                            == expected
                            && (case != "thinking_off"
                                || mode != WireMode::Messages
                                || call["thinking"]["type"] == "disabled")
                    });
                if !matches {
                    passed = false;
                    details["error"] = json!("acceptance_outbound_thinking_mismatch");
                }
                details["expected_wire_effort"] = json!(expected);
            }
            let row = json!({"protocol":format!("{mode:?}"),"model":model,"case":case,"passed":passed,"elapsed_ms":start.elapsed().as_millis(),"calls":calls,"details":details});
            println!(
                "{}",
                json!({"protocol":row["protocol"],"case":case,"passed":passed,"elapsed_ms":row["elapsed_ms"]})
            );
            report.push(row);
            if let Ok(path) = std::env::var("DEEPAGENT_ACCEPTANCE_REPORT") {
                std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            }
        }
    }
    if let Ok(path) = std::env::var("DEEPAGENT_ACCEPTANCE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(
        !report.is_empty() && report.iter().all(|row| row["passed"] == true),
        "remote matrix has failures; inspect sanitized report"
    );
}

#[tokio::test]
#[ignore = "explicit paid remote signed-history acceptance; requires test credentials"]
async fn live_cross_protocol_history_matrix() {
    let key = std::env::var("DEEPAGENT_ACCEPTANCE_API_KEY").expect("test API key env");
    let base = std::env::var("DEEPAGENT_ACCEPTANCE_BASE_URL").expect("test base URL env");
    let mut report = vec![];
    for (source_mode, source_model, target_mode, target_model) in [
        (
            WireMode::Messages,
            "claude-sonnet-4-6",
            WireMode::Responses,
            "gpt-5.6-sol",
        ),
        (
            WireMode::Messages,
            "claude-sonnet-4-6",
            WireMode::ChatCompletions,
            "gpt-5.6-sol",
        ),
        (
            WireMode::Responses,
            "gpt-5.6-sol",
            WireMode::Messages,
            "claude-sonnet-4-6",
        ),
    ] {
        let transport = Arc::new(AuditTransport {
            inner: ReqwestTransport::new(),
            calls: Mutex::new(vec![]),
        });
        let source = client(transport.clone(), &key, &base, source_mode, source_model);
        let target = client(transport.clone(), &key, &base, target_mode, target_model);
        let start = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(100), async {
            let prompt = "Remember P11_SWITCH_OK. Evaluate (17*23+29*31)*37, then reply P11_SWITCH_OK and the answer.";
            let first = checked(source.stream_response(request(source_model, prompt)
                .with_reasoning_effort(ReasoningEffort::High)
                .with_max_output_tokens(2048)).await?, "P11_SWITCH_OK")?;
            let signatures = first.output_items.iter().filter(|item| matches!(item,
                ResponseItem::Reasoning { signature: Some(_), .. })).count();
            if source_mode == WireMode::Messages && signatures == 0 {
                return Err(deepagent_core::CoreError::invalid("acceptance_source_signature_missing"));
            }
            let stored = serde_json::to_vec(&first.output_items)?;
            let restored: Vec<ResponseItem> = serde_json::from_slice(&stored)?;
            assert_eq!(restored, first.output_items);
            let mut history = vec![ResponseItem::Message { role:"user".into(), content:prompt.into() }];
            history.extend(restored);
            history.push(ResponseItem::Message {role:"user".into(),content:"Repeat only the remembered token from the earlier user message.".into()});
            let response = checked(target.stream_response(ResponseRequest::from_response_items(target_model,
                Some("Follow the synthetic instruction exactly.".into()), history)
                .with_reasoning_effort(ReasoningEffort::Low).with_max_output_tokens(1024)).await?, "P11_SWITCH_OK")?;
            Ok::<Value, deepagent_core::CoreError>(json!({"source_signatures":signatures,"usage":response.usage}))
        }).await;
        let (passed, details) = match result {
            Ok(Ok(value)) => (true, value),
            Ok(Err(error)) => (
                false,
                json!({"error":error.to_string().replace(&key,"[REDACTED]").chars().take(300).collect::<String>()}),
            ),
            Err(_) => (false, json!({"error":"acceptance_case_timeout_100s"})),
        };
        report.push(
            json!({"source":format!("{source_mode:?}"),"target":format!("{target_mode:?}"),
            "source_model":source_model,"target_model":target_model,"passed":passed,
            "elapsed_ms":start.elapsed().as_millis(),"details":details,
            "calls":std::mem::take(&mut *transport.calls.lock().unwrap())}),
        );
    }
    if let Ok(path) = std::env::var("DEEPAGENT_ACCEPTANCE_REPORT") {
        std::fs::write(
            format!("{path}.switch.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    for row in &report {
        println!(
            "{}",
            json!({"source":row["source"],"target":row["target"],"passed":row["passed"]})
        );
    }
    assert!(
        report.iter().all(|row| row["passed"] == true),
        "cross-protocol history matrix has failures; inspect sanitized report"
    );
}
