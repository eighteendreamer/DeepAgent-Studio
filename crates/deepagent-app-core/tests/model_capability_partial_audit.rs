//! Audit partial /models metadata through discovery, catalog and context policy.

#[cfg(test)]
#[path = "../../deepagent-models/tests/support/metadata.rs"]
mod metadata_fixture;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_context::ContextPolicy;
use deepagent_core::error::{CoreError, Result};
use deepagent_models::discovery::ModelDiscovery;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelCapability, ModelClient, ReasoningEffort};
use serde_json::{json, Value};

struct CatalogTransport {
    entry: Value,
    gets: Mutex<Vec<String>>,
}

#[async_trait]
impl HttpTransport for CatalogTransport {
    async fn get_json(&self, url: &str, _api_key: &str) -> Result<String> {
        self.gets.lock().unwrap().push(url.to_string());
        Ok(json!({"object":"list", "data":[self.entry]}).to_string())
    }

    async fn stream(&self, _request: TransportRequest, _sink: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other(
            "this audit must not send inference requests",
        ))
    }
}

async fn observe(model: &str, context: Option<u64>, output: Option<u64>) -> ModelCapability {
    let mut entry = json!({"id":model,"object":"model","owned_by":"deepseek"});
    if let Some(value) = context {
        entry["context_window"] = value.into();
    }
    if let Some(value) = output {
        entry["max_output_tokens"] = value.into();
    }
    let transport = Arc::new(CatalogTransport {
        entry,
        gets: Mutex::new(Vec::new()),
    });
    let catalog = ModelDiscovery::deepseek(transport.clone())
        .list_models("offline-capability-audit")
        .await
        .unwrap();
    let catalog: Vec<deepagent_models::ModelInfo> =
        serde_json::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].context_window, context);
    assert_eq!(catalog[0].max_output_tokens, output);

    assert_eq!(
        *transport.gets.lock().unwrap(),
        ["https://api.deepseek.com/models"]
    );
    let config = crate::metadata_fixture::config("offline-capability-audit", &catalog);
    let client = ModelClient::new(transport, config);
    let capability = client.capability(model);
    let policy = ContextPolicy::for_capability(&capability, ReasoningEffort::High);
    assert_eq!(policy.context_window, capability.context_window);
    assert_eq!(policy.max_output_tokens, capability.max_output_tokens);
    let mut expected_capability = capability.clone();
    if let Some(output) = output {
        expected_capability.max_output_tokens = output as usize;
    }
    let expected_policy =
        ContextPolicy::for_capability(&expected_capability, ReasoningEffort::High);
    eprintln!(
        "partial capability audit: {}",
        json!({"model":model,"catalog_context":context,"catalog_output":output,
            "resolved_context":capability.context_window,
            "resolved_output":capability.max_output_tokens,"source":capability.capability_source,
            "reserved_output":policy.reserved_output_tokens,"prompt_budget":policy.prompt_budget,
            "expected_reserved_output":expected_policy.reserved_output_tokens,
            "expected_prompt_budget":expected_policy.prompt_budget})
    );
    capability
}

#[tokio::test]
async fn complete_catalog_metadata_retains_both_capacity_fields() {
    for (model, context, output) in [
        ("deepseek-flash", 1_048_576, 393_216),
        ("deepseek-flash", 1_048_576, 8_192),
        ("audit-capacity-model", 131_072, 8_192),
    ] {
        let resolved = observe(model, Some(context), Some(output)).await;
        assert_eq!(resolved.context_window, context as usize);
        assert_eq!(resolved.max_output_tokens, output as usize);
    }
}

#[tokio::test]
async fn absent_metadata_keeps_existing_fallbacks() {
    let known = observe("deepseek-flash", None, None).await;
    assert_eq!(known.context_window, 1_000_000);
    assert_eq!(known.max_output_tokens, 384_000);
    let unknown = observe("audit-capacity-model", None, None).await;
    assert_eq!(unknown.context_window, 128_000);
    assert_eq!(unknown.max_output_tokens, 32_000);
}

#[tokio::test]
async fn output_only_metadata_must_not_be_discarded_by_context_fallback() {
    let mut violations = Vec::new();
    for model in ["deepseek-flash", "audit-capacity-model"] {
        let resolved = observe(model, None, Some(8_192)).await;
        if resolved.max_output_tokens != 8_192 {
            violations.push((model, resolved.max_output_tokens));
        }
    }
    assert!(
        violations.is_empty(),
        "provider output capacity was replaced by fallback: {violations:?}"
    );
}
