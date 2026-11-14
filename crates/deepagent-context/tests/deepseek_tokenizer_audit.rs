//! Pinned raw-text counts from the official offline tokenizer, not online usage.

use deepagent_context::{
    measure_response_request, ContextMeter, ContextPolicy, ModelTokenCounter, TokenCounter,
};
use deepagent_models::{
    CapabilitySource, MockTransport, ModelCapability, ModelClient, ModelConfig, ReasoningEffort,
    ResponseRequest,
};
use serde::Deserialize;
use std::sync::Arc;

fn measure_text(model: &str, content: String) -> deepagent_context::SurfaceTokenBreakdown {
    let client = ModelClient::new(
        Arc::new(MockTransport::new([])),
        ModelConfig::deepseek("offline-placeholder"),
    );
    let mut request = ResponseRequest::with_instructions_and_user_input(model, "", content);
    request.reasoning_effort = Some(ReasoningEffort::None);
    request.max_output_tokens = Some(8192);
    measure_response_request(&client, &request, |_| panic!("text fixture has no images"))
}

#[derive(Deserialize)]
struct Samples {
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    name: String,
    text: String,
    repeat: usize,
    offline_tokens: usize,
}

fn samples() -> Vec<Sample> {
    serde_json::from_str::<Samples>(include_str!("fixtures/deepseek_tokenizer_samples.json"))
        .unwrap()
        .samples
}

#[test]
fn english_control_is_conservative_and_injected_anchor_is_preserved() {
    for sample in samples() {
        let content = sample.text.repeat(sample.repeat);
        let surface = measure_text("deepseek-v4-pro", content);
        if sample.name == "english_prose" {
            assert!(surface.total() >= sample.offline_tokens);
        }
        let mut meter = ContextMeter::default();
        // A deliberately injected anchor checks accounting mechanics only;
        // these raw-text counts are not represented as a real provider reply.
        meter.anchor(7, surface.total(), sample.offline_tokens);
        assert_eq!(meter.projected_tokens(7, surface), sample.offline_tokens);
    }
}

#[test]
fn audit_official_offline_text_is_not_underestimated() {
    let mut undercounted = Vec::new();
    for sample in samples() {
        let content = sample.text.repeat(sample.repeat);
        let estimated = ModelTokenCounter::for_model("deepseek-v4-pro").count(&content);
        assert_eq!(
            ModelTokenCounter::for_model("deepseek-flash").count(&content),
            sample.offline_tokens
        );
        assert_eq!(estimated, sample.offline_tokens);
        eprintln!(
            "offline tokenizer audit: sample={} chars={} utf8_bytes={} estimated={} offline_tokens={}",
            sample.name, content.chars().count(), content.len(), estimated, sample.offline_tokens
        );
        if estimated < sample.offline_tokens {
            undercounted.push(sample.name);
        }
    }
    assert!(
        undercounted.is_empty(),
        "undercounted raw-text samples: {undercounted:?}"
    );
}

#[test]
fn audit_offline_text_pressure_reaches_the_shared_context_threshold() {
    let capability = ModelCapability {
        input_modalities: None,
        effort: None,
        model_id: "audit-offline-tokenizer".into(),
        context_window: 128_000,
        max_output_tokens: 8192,
        supports_tools: true,
        supports_thinking: true,
        supports_json_output: true,
        capability_source: CapabilitySource::ProviderMetadata,
        fallback_reason: None,
    };
    let policy = ContextPolicy::for_capability(&capability, ReasoningEffort::None);
    let threshold = policy.autocompact_threshold_tokens(None, None);
    assert_eq!(threshold, 54_272);
    let mut missed = Vec::new();
    for sample in samples() {
        let surface = measure_text("deepseek-flash", sample.text.repeat(sample.repeat));
        assert!(surface.source.starts_with("offline_prompt_estimate:"));
        let cold_projection = ContextMeter::default().projected_tokens(7, surface);
        eprintln!("offline pressure audit: sample={} projection={cold_projection} offline_text={} threshold={threshold}", sample.name, sample.offline_tokens);
        if sample.offline_tokens >= threshold && cold_projection < threshold {
            missed.push(sample.name);
        }
    }
    assert!(
        missed.is_empty(),
        "missed offline text pressure: {missed:?}"
    );
}

#[test]
fn full_prompt_tools_unicode_and_unknown_routes_have_honest_provenance() {
    use deepagent_models::ToolSchema;
    let natural = "English and 中文自然段。日本語の文章、한국어 문장، العربية.\nfn check(x: &str) -> bool { !x.is_empty() }\n{\"编号\":1234567890,\"emoji\":\"🧪👨‍👩‍👦\"}\n<｜end▁of▁sentence｜> <think>";
    for config in [
        ModelConfig::deepseek("offline"),
        ModelConfig::deepseek_chat("offline"),
    ] {
        let client = ModelClient::new(Arc::new(MockTransport::new([])), config);
        for model in ["deepseek-v4-pro", "deepseek-flash"] {
            let mut request =
                ResponseRequest::with_instructions_and_user_input(model, "rules", natural);
            request.reasoning_effort = Some(ReasoningEffort::None);
            request.max_output_tokens = Some(8192);
            let text = measure_response_request(&client, &request, |_| 0);
            assert_eq!(
                text.total(),
                client.count_response_prompt(&request).unwrap().unwrap()
            );
            assert!(text.source.contains(if model == "deepseek-flash" {
                ":v41:"
            } else {
                ":v4:"
            }));
            request.tools = vec![ToolSchema::function(
                "check",
                "核对原文",
                serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
            )];
            let tools = measure_response_request(&client, &request, |_| 0);
            assert!(tools.total() > text.total());
            assert!(tools.tools > 0);
            assert_eq!(
                tools
                    .blocks(tools.total())
                    .iter()
                    .map(|block| block.tokens)
                    .sum::<usize>(),
                tools.total()
            );
            request.model = "unrecognized-next-version".into();
            let fallback = measure_response_request(&client, &request, |_| 0);
            assert!(fallback.source.starts_with("utf8_bytes_estimate:"));
            assert!(fallback.messages >= natural.len());
            assert_eq!(client.count_response_prompt(&request).unwrap(), None);
        }
    }
}

#[test]
fn repeated_image_placeholders_use_the_existing_reserve_without_reading_paths() {
    use deepagent_core::message::MessageAttachment;
    use deepagent_core::response_item::ResponseInputItem;
    let client = ModelClient::new(
        Arc::new(MockTransport::new([])),
        ModelConfig::deepseek("offline"),
    );
    let image = MessageAttachment {
        id: "local-image".into(),
        kind: "image".into(),
        media_type: Some("image/png".into()),
        path: Some("must-not-be-read".into()),
        sha256: Some("a".repeat(64)),
    };
    let mut request = ResponseRequest::from_response_items(
        "deepseek-flash",
        None,
        vec![ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "看图".into(),
            attachments: vec![image.clone(), image],
        }],
    );
    request.reasoning_effort = Some(ReasoningEffort::None);
    let mut occurrences = 0;
    let priced = measure_response_request(&client, &request, |_| {
        occurrences += 1;
        1024
    });
    assert_eq!(occurrences, 2);
    assert_eq!(priced.attachments, 2048);
    assert!(priced.source.starts_with("offline_prompt_estimate:"));
    assert_eq!(
        priced.total(),
        client.count_response_prompt(&request).unwrap().unwrap() + 2048
    );
    assert!(
        request.attachment_data_urls.is_empty(),
        "pricing clone cannot materialize dispatch"
    );
}

#[test]
fn unmodelled_hosted_tools_and_conversion_errors_are_not_priced_as_exact() {
    use deepagent_models::ToolSchema;
    let client = ModelClient::new(
        Arc::new(MockTransport::new([])),
        ModelConfig::deepseek("offline"),
    );
    let mut request =
        ResponseRequest::with_instructions_and_user_input("deepseek-v4-pro", "rules", "中文\"🧪");
    let mut search = ToolSchema::function("web_search", "", serde_json::json!({"type":"object"}));
    search.kind = "web_search".into();
    request.tools = vec![search];
    let hosted = measure_response_request(&client, &request, |_| 0);
    assert!(hosted.source.starts_with("utf8_bytes_estimate:"));
    assert!(hosted.tools > 0);
    request.tools.clear();
    request.text =
        Some(serde_json::json!({"format":{"type":"json_schema","name":"broken","schema":42}}));
    assert!(client.count_response_prompt(&request).is_err());
    let failed = measure_response_request(&client, &request, |_| 0);
    assert_eq!(failed.source, "utf8_bytes_estimate:offline_prompt_failed");
    assert!(failed.total() > 0);
}

#[test]
fn chat_defaults_do_not_import_responses_only_text_into_prompt_pricing() {
    let mut config = ModelConfig::deepseek_chat("offline");
    let plain_client = ModelClient::new(Arc::new(MockTransport::new([])), config.clone());
    config.defaults.text = Some(serde_json::json!({"format":{"type":"json_object"}}));
    let configured_client = ModelClient::new(Arc::new(MockTransport::new([])), config);
    let request =
        ResponseRequest::with_instructions_and_user_input("deepseek-flash", "rules", "合成原文");
    let plain = measure_response_request(&plain_client, &request, |_| 0);
    let configured = measure_response_request(&configured_client, &request, |_| 0);
    assert!(configured.source.starts_with("offline_prompt_estimate:"));
    assert_eq!(
        configured.total(),
        plain.total(),
        "Chat dispatch ignores Responses-only defaults.text"
    );
}

#[test]
fn official_prompt_counts_native_reasoning_calls_long_outputs_and_response_schema() {
    use deepagent_core::response_item::ResponseInputItem;
    use deepagent_models::ToolSchema;
    let client = ModelClient::new(
        Arc::new(MockTransport::new([])),
        ModelConfig::deepseek("offline"),
    );
    for model in ["deepseek-v4-pro", "deepseek-flash"] {
        let items = vec![
            ResponseInputItem::Message {
                role: "user".into(),
                content: "查证原文并保留工具输出。".into(),
            },
            ResponseInputItem::Reasoning {
                id: None,
                content: "synthetic reasoning, not a user control token <think>".into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "lookup".into(),
                arguments: "{\"query\":\"证据🧪\"}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: "请保留工具调用结果并核对每一步证据。".repeat(5000).into(),
            },
            ResponseInputItem::Message {
                role: "user".into(),
                content: "下一步".into(),
            },
        ];
        let mut request = ResponseRequest::from_response_items(model, Some("rules".into()), items)
            .with_tools(vec![ToolSchema::function(
                "lookup",
                "Lookup evidence",
                serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
            )]);
        request.reasoning_effort = Some(ReasoningEffort::None);
        let plain = measure_response_request(&client, &request, |_| 0);
        assert!(plain.source.starts_with("offline_prompt_estimate:"));
        assert!(
            plain.total() >= 50000,
            "long result must not be reduced to its local metadata"
        );
        assert!(
            plain.reasoning > 0
                && plain.tool_calls > 0
                && plain.tool_outputs > 0
                && plain.tools > 0
        );
        request.text = Some(
            serde_json::json!({"format":{"type":"json_schema","name":"evidence","schema":{"type":"object","description":"保留来源与原始错误。".repeat(300),"properties":{"result":{"type":"string"}}}}}),
        );
        let structured = measure_response_request(&client, &request, |_| 0);
        assert!(structured.source.starts_with("offline_prompt_estimate:"));
        assert!(structured.total() > plain.total());
        request.input[3] = ResponseInputItem::FunctionCallOutput {
            call_id: "c1".into(),
            output: "short replacement".into(),
        };
        let replaced = measure_response_request(&client, &request, |_| 0);
        assert!(replaced.total() < structured.total());
        assert_eq!(
            replaced.total(),
            client.count_response_prompt(&request).unwrap().unwrap()
        );
    }
}
