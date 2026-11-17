//! One-time legacy seed shared by first sign-in and settings migration.

use super::*;
use deepagent_persistence::harness_registry::HarnessModel;

impl SettingsService {
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
