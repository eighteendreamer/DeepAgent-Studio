//! Audit provider configuration commit ordering around secret persistence.

use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService,
};
use deepagent_app_core::{MemorySecretStore, SecretStore};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::Database;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct FailOnceSecretStore {
    inner: MemorySecretStore,
    fail_next_set: AtomicBool,
}

impl FailOnceSecretStore {
    fn new() -> Self {
        Self {
            inner: MemorySecretStore::new(),
            fail_next_set: AtomicBool::new(false),
        }
    }

    fn fail_next_set(&self) {
        self.fail_next_set.store(true, Ordering::SeqCst);
    }
}

impl SecretStore for FailOnceSecretStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        if self.fail_next_set.swap(false, Ordering::SeqCst) {
            return Err(CoreError::other("synthetic secret write failure"));
        }
        self.inner.set(name, secret)
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        self.inner.get(name)
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.inner.delete(name)
    }
}

fn provider_input(id: &str, base_url: &str, api_key: &str) -> CanvasProviderInput {
    CanvasProviderInput {
        id: id.to_string(),
        name: "Audit provider".to_string(),
        protocol: "openai".to_string(),
        base_url: base_url.to_string(),
        enabled: true,
        models: vec![CanvasModelConfig {
            id: "audit-model".to_string(),
            name: "Audit model".to_string(),
            description: String::new(),
            enabled: true,
            scenarios: vec![deepagent_app_core::canvas_provider_service::CanvasScenario::Text],
            priority: 0,
            max_reference_images: None,
        }],
        logo: None,
        api_key: Some(api_key.to_string()),
    }
}

#[test]
#[ignore = "audit reproducer: provider config must not commit when API-key persistence fails"]
fn audit_provider_save_is_atomic_across_config_and_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Database::open(dir.path().join("state.sqlite")).expect("database"));
    let secrets = Arc::new(FailOnceSecretStore::new());
    let service = CanvasProviderService::new(db, secrets.clone());

    let first = service
        .save_provider(provider_input(
            "audit-provider",
            "http://old.invalid/v1",
            "old-key",
        ))
        .expect("initial provider save");
    assert_eq!(first.models.first().expect("model").id, "audit-model");
    let before = service
        .resolve_model("audit-provider", "audit-model")
        .expect("initial resolution");
    assert_eq!(before.provider.base_url, "http://old.invalid/v1");
    assert_eq!(before.api_key.as_deref(), Some("old-key"));

    secrets.fail_next_set();
    let result = service.save_provider(provider_input(
        "audit-provider",
        "http://new.invalid/v1",
        "new-key",
    ));
    assert!(
        result.is_err(),
        "synthetic secret failure must reach caller"
    );

    let after = service
        .resolve_model("audit-provider", "audit-model")
        .expect("provider remains resolvable");
    eprintln!(
        "provider save failure: result_error=true base_url={} credential_version={} api_key={:?}",
        after.provider.base_url, after.provider.credential_version, after.api_key
    );
    assert_eq!(
        after.provider.base_url, "http://old.invalid/v1",
        "failed secret persistence must not publish the new provider config"
    );
    assert_eq!(
        after.api_key.as_deref(),
        Some("old-key"),
        "the old key must remain paired with the old provider config"
    );
}
