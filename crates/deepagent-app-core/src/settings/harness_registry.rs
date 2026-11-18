//! One-time legacy seed shared by first sign-in and settings migration.

use super::*;
use deepagent_persistence::harness_registry::HarnessModel;

impl SettingsService {
    pub(crate) fn harness_models(&self) -> crate::harness_model_service::HarnessModelService {
        crate::harness_model_service::HarnessModelService::new(
            self.db.clone(),
            self.secrets.clone(),
            self.transport.clone(),
        )
    }

    pub(crate) fn database(&self) -> &Database {
        &self.db
    }

    /// Bind an admitted route's reference, retaining the durable sign-in gate
    /// for migrated credentials. This never substitutes another provider key.
    pub(crate) fn harness_api_key(&self, key_ref: &str) -> Result<Option<String>> {
        if key_ref == API_KEY_NAME {
            self.api_key()
        } else {
            self.secrets.get(key_ref)
        }
    }

    pub(super) fn seed_harness_registry(&self, settings: &AppSettings) -> Result<()> {
        let resolver = ModelCapabilityResolver::new();
        let capability = settings
            .catalog
            .available
            .iter()
            .find(|model| model.id == settings.catalog.chat_model)
            .map(|model| resolver.resolve_model(model))
            .unwrap_or_else(|| resolver.resolve_model_id(&settings.catalog.chat_model));
        let capacity = |value: usize| {
            i64::try_from(value)
                .map_err(|_| CoreError::invalid("model capacity exceeds registry limit"))
        };
        self.db.seed_legacy_harness_model(
            &settings.catalog.base_url,
            &HarnessModel {
                provider_id: "deepseek".into(),
                model_name: settings.catalog.chat_model.clone(),
                display_name: settings.catalog.chat_model.clone(),
                protocol: None,
                context_window: capacity(capability.context_window)?,
                output_limit: capacity(capability.max_output_tokens)?,
                capabilities: serde_json::to_string(&capability)?,
                active: true,
            },
        )
    }
}
