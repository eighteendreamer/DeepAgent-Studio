//! Access to the single provider/model registry and its referenced credentials.
use super::*;

impl SettingsService {
    pub fn harness_models(&self) -> crate::harness_model_service::HarnessModelService {
        crate::harness_model_service::HarnessModelService::new(
            self.db.clone(),
            self.secrets.clone(),
            self.transport.clone(),
        )
    }

    pub(crate) fn database(&self) -> &Database {
        &self.db
    }

    pub(crate) fn harness_api_key(&self, key_ref: &str) -> Result<Option<String>> {
        self.secrets.get(key_ref)
    }

    /// Official search uses only the enabled official provider's credential.
    /// A custom inference endpoint must never send its credential to DeepSeek.
    pub(super) fn deepseek_search_api_key(&self) -> Result<Option<String>> {
        let provider = self
            .db
            .list_harness_providers()?
            .into_iter()
            .find(|provider| {
                provider.id == "deepseek"
                    && provider.enabled
                    && provider.model_url.trim_end_matches('/')
                        == deepagent_models::DEEPSEEK_BASE_URL
            });
        provider
            .map(|provider| self.secrets.get(&provider.api_key_ref))
            .transpose()
            .map(Option::flatten)
    }
}
