use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{
    ModelClient, ModelConfig, ModelRole, ReasoningEffort, ResponseDefaults, WireMode,
    DEEPSEEK_OFFICIAL_PROVIDER,
};

use crate::settings::SettingsService;

pub(crate) struct RunModelSelection {
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) fallback_model: Option<String>,
}

/// Credential-free selection; the admitted worker binds credentials at execution time.
pub(crate) struct RunModelPlan {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) fallback_model: Option<String>,
    pub(crate) override_applied: bool,
    config: ModelConfig,
}

impl RunModelPlan {
    pub(crate) fn bind(
        self,
        settings: &SettingsService,
        transport: Arc<dyn HttpTransport>,
    ) -> Result<RunModelSelection> {
        let mut config = self.config;
        config.api_key = settings
            .api_key()?
            .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
        Ok(RunModelSelection {
            client: Arc::new(ModelClient::new(transport, config)),
            model: self.model,
            reasoning_effort: self.reasoning_effort,
            fallback_model: self.fallback_model,
        })
    }
}

pub(crate) fn build_model_client(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    role: ModelRole,
) -> Result<(Arc<ModelClient>, String, ReasoningEffort)> {
    let loaded = settings
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    let api_key = settings
        .api_key()?
        .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
    let reasoning_effort = resolve_reasoning_effort(&loaded, None)?;
    let model = loaded.catalog.model_for(role).to_string();
    let defaults = ResponseDefaults {
        temperature: loaded.responses.effective_temperature(),
        top_p: loaded.responses.effective_top_p(),
        max_output_tokens: loaded.responses.effective_max_output_tokens(),
        top_logprobs: loaded.responses.effective_top_logprobs(),
        reasoning_effort: Some(reasoning_effort),
        text: loaded.responses.effective_text(),
        tool_choice: loaded.responses.effective_tool_choice(),
        user: loaded.responses.effective_user(),
    };
    let config = ModelConfig::from_catalog(api_key, &loaded.catalog, role).with_defaults(defaults);
    let client = Arc::new(ModelClient::new(transport, config));
    Ok((client, model, reasoning_effort))
}

pub fn build_chat_model_client(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
) -> Result<(Arc<ModelClient>, String, ReasoningEffort)> {
    build_model_client(settings, transport, ModelRole::Chat)
}

pub(crate) fn select_run_model(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    role: ModelRole,
    fallback_role: ModelRole,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    reasoning_effort_override: Option<&str>,
) -> Result<RunModelSelection> {
    let loaded = settings
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    resolve_run_model(
        &loaded,
        role,
        fallback_role,
        provider_override,
        model_override,
        reasoning_effort_override,
    )?
    .bind(settings, transport)
}

pub(crate) fn resolve_run_model(
    loaded: &crate::settings::AppSettings,
    role: ModelRole,
    fallback_role: ModelRole,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    reasoning_effort_override: Option<&str>,
) -> Result<RunModelPlan> {
    let reasoning_effort = resolve_reasoning_effort(loaded, reasoning_effort_override)?;
    let model = match model_override {
        Some(id) if loaded.catalog.has_model(id) => id.to_owned(),
        Some(id) => {
            return Err(CoreError::invalid(format!(
                "unknown model '{id}'; select a model from the configured catalog"
            )))
        }
        None => loaded.catalog.model_for(role).to_owned(),
    };
    let fallback_model = Some(loaded.catalog.model_for(fallback_role).to_string())
        .filter(|fallback| fallback != &model);
    let defaults = ResponseDefaults {
        temperature: loaded.responses.effective_temperature(),
        top_p: loaded.responses.effective_top_p(),
        max_output_tokens: loaded.responses.effective_max_output_tokens(),
        top_logprobs: loaded.responses.effective_top_logprobs(),
        reasoning_effort: Some(reasoning_effort),
        text: loaded.responses.effective_text(),
        tool_choice: loaded.responses.effective_tool_choice(),
        user: loaded.responses.effective_user(),
    };
    if let Some(provider) = provider_override {
        if provider != DEEPSEEK_OFFICIAL_PROVIDER {
            return Err(CoreError::invalid(format!(
                "unsupported provider: {provider}"
            )));
        }
    }
    let config = ModelConfig::from_catalog("", &loaded.catalog, role)
        .with_defaults(defaults)
        .with_wire_mode(WireMode::Responses);
    Ok(RunModelPlan {
        config,
        override_applied: model_override.is_some(),
        model,
        reasoning_effort,
        fallback_model,
    })
}

fn resolve_reasoning_effort(
    settings: &crate::settings::AppSettings,
    run_override: Option<&str>,
) -> Result<ReasoningEffort> {
    let response_effort = settings.responses.effective_reasoning_effort();
    if let Some(value) = run_override {
        return ReasoningEffort::parse(value).ok_or_else(|| {
            CoreError::invalid(format!(
                "reasoning effort must be none, low, high, or max; got '{value}'"
            ))
        });
    }
    let has_responses_effort = settings.responses.reasoning_effort.is_some()
        || settings.responses.developer.contains_key("reasoning");
    if has_responses_effort && response_effort.is_none() {
        return Err(CoreError::invalid(
            "stored Responses reasoning effort is invalid; select none, low, high, or max",
        ));
    }
    Ok(response_effort.unwrap_or(settings.reasoning_effort))
}

#[cfg(test)]
mod tests {
    use super::resolve_reasoning_effort;
    use crate::settings::{AppSettings, ResponsesApiSettings};
    use deepagent_models::ReasoningEffort;

    #[test]
    fn reasoning_effort_resolution_uses_only_canonical_values() {
        let mut settings: AppSettings = serde_json::from_value(serde_json::json!({
            "catalog": { "base_url": deepagent_models::DEEPSEEK_BASE_URL,
                "available": [], "chat_model": "deepseek-v4-flash", "reasoner_model": "deepseek-v4-pro" },
            "discovered_at": 0, "sandbox_mode": "workspace_write"
        })).unwrap();
        settings.reasoning_effort = ReasoningEffort::Low;
        assert_eq!(
            resolve_reasoning_effort(&settings, None).unwrap(),
            ReasoningEffort::Low
        );
        assert_eq!(
            resolve_reasoning_effort(&settings, Some("max")).unwrap(),
            ReasoningEffort::Max
        );
        settings.responses = ResponsesApiSettings {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_reasoning_effort(&settings, None).unwrap(),
            ReasoningEffort::High
        );
        for value in ["minimal", "medium", "xhigh", "simple", "deep"] {
            assert!(
                resolve_reasoning_effort(&settings, Some(value)).is_err(),
                "{value}"
            );
        }
        settings.responses.reasoning_effort = Some("medium".into());
        assert!(resolve_reasoning_effort(&settings, None).is_err());
    }
}
