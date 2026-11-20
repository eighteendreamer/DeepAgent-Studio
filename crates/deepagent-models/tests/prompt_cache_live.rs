#![cfg(feature = "http")]

use deepagent_core::error::CoreError;
use deepagent_core::runtime_snapshot::RuntimeSnapshot;
use deepagent_models::{
    ModelClient, ModelConfig, ReasoningEffort, ReqwestTransport, ResponseItem, ResponseRequest,
    ToolSchema, WireMode,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Opt-in provider check. Only synthetic material is sent; output is counters only.
#[tokio::test]
#[ignore = "uses provider quota; set DEEPAGENT_CACHE_TEST_KEY, _URL, _MODEL and _PROTOCOL"]
async fn extended_history_reports_real_provider_cache_usage() {
    let env = |name| std::env::var(name).expect("set the cache probe configuration");
    let protocol = env("DEEPAGENT_CACHE_TEST_PROTOCOL");
    let wire = match protocol.as_str() {
        "messages" => WireMode::Messages,
        "chat" => WireMode::ChatCompletions,
        "responses" => WireMode::Responses,
        _ => panic!("unsupported cache probe protocol"),
    };
    let model = env("DEEPAGENT_CACHE_TEST_MODEL");
    let mut config = ModelConfig::deepseek(env("DEEPAGENT_CACHE_TEST_KEY")).with_wire_mode(wire);
    config.base_url = env("DEEPAGENT_CACHE_TEST_URL");
    let client = ModelClient::new(Arc::new(ReqwestTransport::new()), config);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let rules = format!(
        "Synthetic cache fixture {nonce}. Reply with only the requested marker.\n{}",
        (0..640)
            .map(|n| format!("Sample entry {n}: garden meadow river archive.\n"))
            .collect::<String>()
    );
    let session = deepagent_core::id::SessionId::new();
    let mut history = Vec::new();
    for turn in 1..=3 {
        history.push(ResponseItem::Message {
            role: "user".into(),
            content: format!("Reply only: CACHE-{turn}"),
        });
        let request =
            ResponseRequest::from_response_items(&model, Some(rules.clone()), history.clone())
                .with_cache_session(session)
                .with_max_output_tokens(128)
                .with_reasoning_effort(ReasoningEffort::None);
        let response = client
            .stream_response(request)
            .await
            .and_then(|response| response.into_generation_result())
            .unwrap_or_else(|error| {
                let status = match &error {
                    CoreError::Provider { status, .. } => *status,
                    _ => None,
                };
                panic!(
                    "provider request failed for {protocol} on turn {turn}; HTTP status={status:?}"
                );
            });
        let usage = response.usage.expect("provider did not report usage");
        let created = response
            .raw_usage
            .as_ref()
            .and_then(|usage| usage["cache_creation_input_tokens"].as_u64())
            .unwrap_or_default();
        println!(
            "{protocol} {model} turn={turn} input={} hit={} miss={} created={created}",
            usage.prompt_tokens, usage.prompt_cache_hit_tokens, usage.prompt_cache_miss_tokens
        );
        let visible = response.output_text_projection();
        assert!(
            !visible.is_empty(),
            "provider did not return visible output"
        );
        // Closed turns retain the visible response, not ephemeral reasoning.
        history.push(ResponseItem::Message {
            role: "assistant".into(),
            content: visible,
        });
    }
}

/// Chat-like Messages traffic: change a real host snapshot at a user boundary,
/// call a deterministic no-I/O tool, and retain the actual native reply items.
#[tokio::test]
#[ignore = "uses provider quota; set DEEPAGENT_CACHE_TEST_KEY, _URL and _MODEL for Messages"]
async fn messages_cache_survives_a_runtime_update_and_tool_continuation() {
    let env = |name| std::env::var(name).expect("set the cache probe configuration");
    let model = env("DEEPAGENT_CACHE_TEST_MODEL");
    let mut config =
        ModelConfig::deepseek(env("DEEPAGENT_CACHE_TEST_KEY")).with_wire_mode(WireMode::Messages);
    config.base_url = env("DEEPAGENT_CACHE_TEST_URL");
    let client = ModelClient::new(Arc::new(ReqwestTransport::new()), config);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let stable = format!(
        "Synthetic fixture {nonce}. Follow the latest user request exactly.\n{}",
        (0..640)
            .map(|n| format!("Sample entry {n}: garden meadow river archive.\n"))
            .collect::<String>()
    );
    let user = |content: &str| ResponseItem::Message {
        role: "user".into(),
        content: content.into(),
    };
    let snapshot = |revision: u64, context: &str| {
        RuntimeSnapshot {
            version: 1,
            series: 1,
            revision,
            stable_rules_sha256: "a".repeat(64),
            context: context.into(),
        }
        .to_item()
    };
    let tool = ToolSchema::function(
        "cache_echo",
        "Echo a synthetic marker with no side effects.",
        serde_json::json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}),
    );
    let mut history = vec![
        user("Reply exactly COLD without calling a tool."),
        snapshot(1, "Test environment A"),
    ];
    let mut hits = Vec::new();
    for phase in 0..3 {
        let request =
            ResponseRequest::from_response_items(&model, Some(stable.clone()), history.clone())
                .with_tools(vec![tool.clone()])
                .with_max_output_tokens(256)
                .with_reasoning_effort(ReasoningEffort::None);
        let response = client
            .stream_response(request)
            .await
            .and_then(|response| response.into_generation_result())
            .unwrap_or_else(|error| {
                let status = match error {
                    CoreError::Provider { status, .. } => status,
                    _ => None,
                };
                panic!("Messages cache tool probe failed at phase {phase}; HTTP status={status:?}");
            });
        let usage = response.usage.expect("provider did not report usage");
        println!(
            "messages tool phase={phase} input={} hit={}",
            usage.prompt_tokens, usage.prompt_cache_hit_tokens
        );
        hits.push(usage.prompt_cache_hit_tokens);
        match phase {
            0 => {
                assert_eq!(response.output_text_projection().trim(), "COLD");
                history.extend(response.output_items);
                history.extend([
                    user("Call cache_echo with value P8_TOOL_OK. After the result, reply exactly with the result, without calling another tool."),
                    snapshot(2, "Test environment B"),
                ]);
            }
            1 => {
                let calls = response.tool_invocations_from_items();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].1, "cache_echo");
                assert_eq!(calls[0].2["value"], "P8_TOOL_OK");
                history.extend(response.output_items);
                history.push(ResponseItem::FunctionCallOutput {
                    call_id: calls[0].0.clone(),
                    output: "P8_TOOL_OK".into(),
                });
            }
            _ => assert_eq!(response.output_text_projection().trim(), "P8_TOOL_OK"),
        }
    }
    assert!(
        hits[1..].iter().all(|&read| read > 0),
        "warm requests did not report cache reads: {hits:?}"
    );
}
