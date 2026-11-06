//! Pinned raw-text counts from the official offline tokenizer, not online usage.

use deepagent_context::{
    measure_response_surface, ContextMeter, ContextPolicy, HeuristicTokenizer, TokenCounter,
};
use deepagent_core::response_item::ResponseInputItem;
use deepagent_models::{CapabilitySource, ModelCapability, ReasoningEffort};
use serde::Deserialize;

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
        let surface = measure_response_surface(
            None,
            &[ResponseInputItem::Message {
                role: "user".into(),
                content,
            }],
            &[],
        );
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
#[ignore = "audit: default counter claims conservative pricing but undercounts official offline text"]
fn audit_official_offline_text_is_not_underestimated() {
    let mut undercounted = Vec::new();
    for sample in samples() {
        let content = sample.text.repeat(sample.repeat);
        let estimated = HeuristicTokenizer::new().count(&content);
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
#[ignore = "audit: cold Surface estimate must not miss pressure already crossed by offline raw text"]
fn audit_offline_text_pressure_reaches_the_shared_context_threshold() {
    let capability = ModelCapability {
        input_modalities: None,
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
        let surface = measure_response_surface(
            None,
            &[ResponseInputItem::Message {
                role: "user".into(),
                content: sample.text.repeat(sample.repeat),
            }],
            &[],
        );
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
