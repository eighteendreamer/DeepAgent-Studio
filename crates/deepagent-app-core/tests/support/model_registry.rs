//! Offline fixtures use the same provider, discovery, activation and selection
//! operations as the model settings page. There is no legacy settings catalog.
#![allow(dead_code)]

use deepagent_app_core::harness_model_service::HarnessProviderInput;
use deepagent_app_core::{SettingsService, SettingsView};
use deepagent_core::{CoreError, Result};

pub async fn configure(settings: &SettingsService, key: &str) -> Result<SettingsView> {
    if key.trim().is_empty() {
        return Err(CoreError::invalid("fixture key must not be empty"));
    }
    let service = settings.harness_models();
    service.save_provider(HarnessProviderInput {
        id: Some("deepseek".into()),
        channel: "DeepSeek".into(),
        logo: String::new(),
        website_url: None,
        model_url: deepagent_models::DEEPSEEK_BASE_URL.into(),
        protocol: "responses".into(),
        balance_kind: "official".into(),
        enabled: true,
        api_key: Some(key.into()),
    })?;
    let models = service.fetch_models("deepseek").await?;
    let model = models
        .first()
        .ok_or_else(|| CoreError::invalid("fixture returned no models"))?;
    for model in &models {
        service.set_model_active("deepseek", &model.model_name, true)?;
    }
    service.select_model("deepseek", &model.model_name)?;
    settings
        .view()?
        .ok_or_else(|| CoreError::invalid("fixture settings missing"))
}

pub fn client(settings: &SettingsService) -> std::sync::Arc<deepagent_models::ModelClient> {
    // Public runtime construction proves that fixtures configure the real route.
    let (client, _, _) = deepagent_app_core::model_runtime::build_chat_model_client(
        settings,
        std::sync::Arc::new(deepagent_models::MockTransport::with_get_json("{}")),
    )
    .unwrap();
    client
}
