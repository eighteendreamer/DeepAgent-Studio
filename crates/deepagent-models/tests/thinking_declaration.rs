//! Per-model reasoning declarations must drive admission and the actual wire.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{error::Result, message::Message};
use deepagent_models::{
    thinking::ThinkingCapabilities,
    transport::{EventSink, HttpTransport, TransportRequest},
    ModelCapabilityResolver, ModelClient, ModelConfig, ReasoningEffort, ResponseDefaults,
    ResponseRequest, WireMode,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Capture(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Capture {
    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        unreachable!("no discovery is required for a declared model")
    }

    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        // No response is needed to inspect the request at this boundary.
        Err(deepagent_core::error::CoreError::other("capture complete"))
    }
}

fn client(mode: WireMode, mut declaration: Value) -> (ModelClient, Arc<Capture>) {
    // These fixtures explicitly exercise the Messages effort channel. Native
    // budget projection has separate tests and cannot carry arbitrary strings.
    if mode == WireMode::Messages && declaration.get("thinking_format").is_none() {
        declaration["thinking_format"] = json!("adaptive");
    }
    let transport = Arc::new(Capture::default());
    let mut capability = ModelCapabilityResolver::new().resolve_model_id("declared-model");
    capability.thinking = Some(
        ThinkingCapabilities::for_route(
            "https://example.test/v1",
            "declared-model",
            mode,
            &declaration,
        )
        .unwrap(),
    );
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig {
            base_url: "https://example.test/v1".into(),
            api_key: "fixture".into(),
            defaults: ResponseDefaults {
                max_output_tokens: Some(1024),
                ..Default::default()
            },
            wire_mode: mode,
            capabilities: vec![capability],
            prefer_model_reasoning_default: false,
        },
    );
    (client, transport)
}

#[tokio::test]
async fn h029_messages_projects_runtime_system_items_and_the_selected_effort_together() {
    use deepagent_core::response_item::ResponseItem;

    let (client, transport) = client(
        WireMode::Messages,
        json!({"reasoning_efforts":{"high":"high"}}),
    );
    let mut request = ResponseRequest::from_response_items(
        "declared-model",
        None,
        vec![
            ResponseItem::Message {
                role: "system".into(),
                content: "stable rules".into(),
            },
            ResponseItem::InputMessage {
                role: "system".into(),
                content: "environment".into(),
                attachments: vec![],
            },
            ResponseItem::Message {
                role: "user".into(),
                content: "greeting".into(),
            },
        ],
    )
    .with_reasoning_effort(ReasoningEffort::High);
    request.instructions = Some("instructions".into());
    let _ = client.stream_response(request).await;
    let requests = transport.0.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "runtime system messages must reach the Messages transport"
    );
    assert_eq!(
        requests[0]["system"],
        json!([
            {"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}},
            {"type":"text","text":"stable rules"},
            {"type":"text","text":"environment"}
        ])
    );
    assert_eq!(
        requests[0]["messages"],
        json!([{"role":"user","content":[{"type":"text","text":"greeting","cache_control":{"type":"ephemeral"}}]}])
    );
    assert_eq!(requests[0]["thinking"]["type"], "adaptive");
    assert_eq!(requests[0]["output_config"]["effort"], "high");
}

#[tokio::test]
async fn thinking_declaration_maps_wire_values_on_every_protocol_and_cancel_entry() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for cancelled in [false, true] {
            let (client, transport) = client(
                mode,
                json!({
                    "reasoning_efforts": {"off":null,"low":"low","max":"ultra"}
                }),
            );
            let request = ResponseRequest::new("declared-model", vec![Message::user("fixture")])
                .with_reasoning_effort(ReasoningEffort::Max);
            let _ = if cancelled {
                client
                    .stream_response_cancelled(request, Arc::new(false.into()))
                    .await
            } else {
                client.stream_response(request).await
            };
            let requests = transport.0.lock().unwrap();
            assert_eq!(requests.len(), 1, "{mode:?} cancelled={cancelled}");
            let wire = match mode {
                WireMode::Responses => &requests[0]["reasoning"]["effort"],
                WireMode::ChatCompletions => &requests[0]["reasoning_effort"],
                WireMode::Messages => &requests[0]["output_config"]["effort"],
            };
            assert_eq!(wire, "ultra", "{mode:?} cancelled={cancelled}");
        }
    }
}

#[tokio::test]
async fn thinking_declaration_rejects_unoffered_levels_before_transport() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for declaration in [
            json!({"reasoning_efforts":false}),
            json!({"reasoning_efforts":{"off":null,"max":"ultra"}}),
        ] {
            let (client, transport) = client(mode, declaration);
            let error = client
                .stream_response(
                    ResponseRequest::new("declared-model", vec![Message::user("fixture")])
                        .with_reasoning_effort(ReasoningEffort::High),
                )
                .await
                .unwrap_err();
            assert!(transport.0.lock().unwrap().is_empty(), "{mode:?}: {error}");
            assert!(error
                .to_string()
                .contains("does not support reasoning effort"));
        }
    }
}

#[tokio::test]
async fn declared_default_uses_the_same_wire_mapping_as_an_explicit_choice() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for (default, expected) in [("max", json!("ultra")), ("off", Value::Null)] {
            let (client, transport) = client(
                mode,
                json!({
                    "reasoning_efforts":{"off":null,"max":"ultra"}, "reasoning_default":default
                }),
            );
            let _ = client
                .stream_response(ResponseRequest::new(
                    "declared-model",
                    vec![Message::user("fixture")],
                ))
                .await;
            let requests = transport.0.lock().unwrap();
            assert_eq!(requests.len(), 1, "{mode:?} {default}");
            let wire = match mode {
                WireMode::Responses => &requests[0]["reasoning"]["effort"],
                WireMode::ChatCompletions => &requests[0]["reasoning_effort"],
                WireMode::Messages => &requests[0]["output_config"]["effort"],
            };
            assert_eq!(wire, &expected, "{mode:?} {default}");
            if default == "off" && mode == WireMode::Messages {
                assert_eq!(requests[0]["thinking"]["type"], "disabled");
            }
        }
    }
    for default in [json!("high"), json!("ultra"), Value::Null] {
        assert!(ThinkingCapabilities::from_registry_json(&json!({
            "reasoning_efforts":{"max":"ultra"},"reasoning_default":default
        }))
        .is_err());
    }
}

#[tokio::test]
async fn thinking_declaration_distinguishes_off_omission_from_explicit_none() {
    for (off, expected) in [(Value::Null, Value::Null), (json!("none"), json!("none"))] {
        let (client, transport) = client(
            WireMode::Responses,
            json!({
                "reasoning_efforts":{"off":off,"high":"high"}
            }),
        );
        let _ = client
            .stream_response(
                ResponseRequest::new("declared-model", vec![Message::user("fixture")])
                    .with_reasoning_effort(ReasoningEffort::None),
            )
            .await;
        assert_eq!(
            transport.0.lock().unwrap()[0]["reasoning"]["effort"],
            expected
        );
    }
}

#[tokio::test]
async fn provider_defaults_ignore_global_fallback_and_match_the_wire_on_all_protocols() {
    for mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        for declared_default in [false, true] {
            let transport = Arc::new(Capture::default());
            let mut capability = ModelCapabilityResolver::new().resolve_model_id("declared-model");
            capability.thinking = Some(
                ThinkingCapabilities::from_registry_json(&if declared_default {
                    json!({"reasoning_efforts":{"low":"light"}})
                } else {
                    json!({"reasoning_efforts":false})
                })
                .unwrap(),
            );
            capability.effort = Some(deepagent_models::discovery::ModelEffortMetadata {
                supported_levels: vec!["low".into()],
                default_level: Some("low".into()),
            });
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig {
                    base_url: "https://example.test/v1".into(),
                    api_key: "fixture".into(),
                    defaults: ResponseDefaults {
                        max_output_tokens: Some(1024),
                        reasoning_effort: Some(ReasoningEffort::High),
                        ..Default::default()
                    },
                    wire_mode: mode,
                    capabilities: vec![capability],
                    prefer_model_reasoning_default: true,
                },
            );
            let expected = declared_default.then_some(ReasoningEffort::Low);
            assert_eq!(
                client
                    .effective_reasoning_effort("declared-model", None)
                    .unwrap(),
                expected
            );
            let _ = client
                .stream_response(ResponseRequest::new(
                    "declared-model",
                    vec![Message::user("fixture")],
                ))
                .await;
            let requests = transport.0.lock().unwrap();
            assert_eq!(requests.len(), 1, "{mode:?} default={declared_default}");
            let wire = match mode {
                WireMode::Responses => &requests[0]["reasoning"]["effort"],
                WireMode::ChatCompletions => &requests[0]["reasoning_effort"],
                WireMode::Messages => &requests[0]["output_config"]["effort"],
            };
            assert_eq!(
                wire,
                &if declared_default {
                    json!("light")
                } else {
                    Value::Null
                },
                "{mode:?}"
            );
        }
    }
}

#[test]
fn thinking_declaration_rejects_empty_null_and_invalid_maps() {
    for efforts in [
        json!({}),
        Value::Null,
        json!(true),
        json!({"off":null}),
        json!({"low":null}),
        json!({"max":""}),
        json!({"ultra":"ultra"}),
    ] {
        assert!(
            ThinkingCapabilities::from_registry_json(&json!({"reasoning_efforts":efforts}))
                .is_err(),
            "invalid declaration was accepted: {efforts}"
        );
    }
}

#[tokio::test]
async fn h029_discovered_deepseek_efforts_keep_the_route_alias_on_the_wire() {
    use deepagent_models::thinking::ThinkingLevel;

    let base_url = "https://api.deepseek.com";
    let model = "deepseek-flash";
    let mode = WireMode::Responses;
    let transport = Arc::new(Capture::default());
    let mut capability = ModelCapabilityResolver::new().resolve_model_id(model);
    let thinking = ThinkingCapabilities::for_route(
        base_url,
        model,
        mode,
        &json!({"effort":{"supported_levels":["low","high","max"],"default_level":"high"}}),
    )
    .unwrap();
    assert_eq!(
        thinking.distinct_levels(),
        [
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::High,
            ThinkingLevel::Max
        ]
    );
    capability.thinking = Some(thinking);
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig {
            base_url: base_url.into(),
            api_key: "fixture".into(),
            defaults: ResponseDefaults::default(),
            wire_mode: mode,
            capabilities: vec![capability],
            prefer_model_reasoning_default: true,
        },
    );
    let request = ResponseRequest::new(model, vec![Message::user("fixture")])
        .with_reasoning_effort(ReasoningEffort::Medium);
    let result = client.stream_response(request).await;
    let bodies = transport.0.lock().unwrap();
    assert_eq!(bodies.len(), 1, "{result:?}");
    assert_eq!(bodies[0]["reasoning"]["effort"], "high");
}

#[tokio::test]
async fn h029_user_choices_reach_each_protocol_without_provider_default() {
    for (model, mode, expected_format, expected_max) in [
        ("deepseek-flash", WireMode::Responses, None, "max"),
        (
            "deepseek-flash",
            WireMode::ChatCompletions,
            Some("enabled"),
            "max",
        ),
        ("deepseek-flash", WireMode::Messages, Some("enabled"), "max"),
        (
            "claude-opus-5-5",
            WireMode::Messages,
            Some("adaptive"),
            "max",
        ),
        (
            "claude-sonnet-5",
            WireMode::Messages,
            Some("adaptive"),
            "max",
        ),
        ("gpt-6.2-sol", WireMode::Responses, None, "max"),
        ("grok-4.7", WireMode::Responses, None, "xhigh"),
    ] {
        for requested in [
            None,
            Some(ReasoningEffort::Max),
            Some(ReasoningEffort::None),
        ] {
            let transport = Arc::new(Capture::default());
            let mut capability = ModelCapabilityResolver::new().resolve_model_id(model);
            capability.thinking = Some(
                ThinkingCapabilities::for_route("https://gateway.test/v1", model, mode, &json!({}))
                    .unwrap(),
            );
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig {
                    base_url: "https://gateway.test/v1".into(),
                    api_key: "fixture".into(),
                    defaults: ResponseDefaults {
                        max_output_tokens: Some(8192),
                        ..Default::default()
                    },
                    wire_mode: mode,
                    capabilities: vec![capability],
                    prefer_model_reasoning_default: true,
                },
            );
            let mut request = ResponseRequest::new(model, vec![Message::user("fixture")]);
            request.reasoning_effort = requested;
            let result = client.stream_response(request).await;
            let bodies = transport.0.lock().unwrap();
            if model.starts_with("gpt-") && requested == Some(ReasoningEffort::None) {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("does not support reasoning effort"));
                assert!(bodies.is_empty(), "cannot invent off without a declaration");
                continue;
            }
            assert_eq!(bodies.len(), 1, "{model} {mode:?} {requested:?}");
            let body = &bodies[0];
            let effort = match mode {
                WireMode::Responses => &body["reasoning"]["effort"],
                WireMode::ChatCompletions => &body["reasoning_effort"],
                WireMode::Messages => &body["output_config"]["effort"],
            };
            let coerced_low = model.starts_with("grok-") || model.starts_with("claude-");
            let off = requested == Some(ReasoningEffort::None) && !coerced_low;
            if off && mode == WireMode::Messages {
                assert!(effort.is_null());
                assert_eq!(body["thinking"]["type"], "disabled");
            } else {
                let expected = if off {
                    "none"
                } else if requested == Some(ReasoningEffort::None) {
                    "low"
                } else if requested == Some(ReasoningEffort::Max) {
                    expected_max
                } else {
                    "high"
                };
                assert_eq!(effort, expected, "{model} {mode:?} {requested:?}");
                if let Some(format) = expected_format {
                    assert_eq!(
                        body["thinking"]["type"],
                        if off { "disabled" } else { format }
                    );
                } else {
                    assert!(body.get("thinking").is_none());
                }
            }
        }
    }
}

#[tokio::test]
async fn h029_direct_chat_uses_the_same_control_projection() {
    for (format, expected) in [
        ("effort_only", Value::Null),
        ("enabled", json!("enabled")),
        ("adaptive", json!("adaptive")),
    ] {
        let (client, transport) = client(
            WireMode::ChatCompletions,
            json!({
                "reasoning_efforts":{"off":"low","low":"low","high":"high"},
                "thinking_format":format
            }),
        );
        let _ = client
            .stream_chat_completion(
                deepagent_models::ChatCompletionRequest::new(
                    "declared-model",
                    vec![Message::user("fixture")],
                )
                .with_reasoning_effort(ReasoningEffort::None),
            )
            .await;
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["reasoning_effort"], "low");
        assert_eq!(requests[0]["thinking"]["type"], expected);
    }
}

#[tokio::test]
async fn h029_messages_budget_choices_match_the_actual_request_budgets() {
    for (effort, budget) in [
        (ReasoningEffort::Low, 1024),
        (ReasoningEffort::Medium, 1024),
        (ReasoningEffort::High, 1228),
        (ReasoningEffort::Max, 2047),
    ] {
        let (client, transport) = client(
            WireMode::Messages,
            json!({
                "thinking_format":"budget", "max_output_tokens":2048,
                "reasoning_efforts":{"off":"none","low":"low","medium":"medium","high":"high","max":"max"}
            }),
        );
        let mut request = ResponseRequest::new("declared-model", vec![Message::user("fixture")])
            .with_reasoning_effort(effort);
        request.max_output_tokens = Some(2048);
        let _ = client.stream_response(request).await;
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["thinking"],
            json!({"type":"enabled","budget_tokens":budget})
        );
        assert!(requests[0].get("output_config").is_none());
    }
}
