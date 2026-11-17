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
}

/// Credential-free selection; the admitted worker binds credentials at execution time.
pub(crate) struct RunModelPlan {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
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
            client: Arc::new(
                ModelClient::new(transport, config)
                    .with_image_file_cache(settings.image_file_cache()),
            ),
            model: self.model,
            reasoning_effort: self.reasoning_effort,
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
    let model = loaded.catalog.model_for(role).to_string();
    let mut config = model_config(&loaded, role, None)?;
    let reasoning_effort = config
        .effective_reasoning_effort(&model, None)?
        .unwrap_or_default();
    config.api_key = api_key;
    let client = Arc::new(
        ModelClient::new(transport, config).with_image_file_cache(settings.image_file_cache()),
    );
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
        provider_override,
        model_override,
        reasoning_effort_override,
    )?
    .bind(settings, transport)
}

pub(crate) fn resolve_run_model(
    loaded: &crate::settings::AppSettings,
    role: ModelRole,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    reasoning_effort_override: Option<&str>,
) -> Result<RunModelPlan> {
    let model = match model_override {
        Some(id) if loaded.catalog.has_model(id) => id.to_owned(),
        Some(id) => {
            return Err(CoreError::invalid(format!(
                "unknown model '{id}'; select a model from the configured catalog"
            )))
        }
        None => loaded.catalog.model_for(role).to_owned(),
    };
    if let Some(provider) = provider_override {
        if provider != DEEPSEEK_OFFICIAL_PROVIDER {
            return Err(CoreError::invalid(format!(
                "unsupported provider: {provider}"
            )));
        }
    }
    let config = model_config(loaded, role, reasoning_effort_override)?;
    let reasoning_effort = config
        .effective_reasoning_effort(&model, None)?
        .unwrap_or_default();
    Ok(RunModelPlan {
        config,
        override_applied: model_override.is_some(),
        model,
        reasoning_effort,
    })
}

/// One credential-free settings projection for admission, runtime and preview.
pub(crate) fn model_config(
    settings: &crate::settings::AppSettings,
    role: ModelRole,
    run_override: Option<&str>,
) -> Result<ModelConfig> {
    let requested = resolve_reasoning_effort(settings, run_override)?;
    let defaults = ResponseDefaults {
        temperature: settings.responses.effective_temperature(),
        top_p: settings.responses.effective_top_p(),
        max_output_tokens: settings.responses.effective_max_output_tokens(),
        top_logprobs: settings.responses.effective_top_logprobs(),
        // The retained global value is only a compatibility default when a
        // model publishes no default. The same capability validator checks it.
        reasoning_effort: requested.or(Some(settings.reasoning_effort)),
        text: settings.responses.effective_text(),
        tool_choice: settings.responses.effective_tool_choice(),
        user: settings.responses.effective_user(),
    };
    Ok(ModelConfig::from_catalog("", &settings.catalog, role)
        .with_defaults(defaults)
        .with_model_reasoning_defaults(requested.is_none())
        .with_wire_mode(WireMode::Responses))
}

pub(crate) fn resolve_reasoning_effort(
    settings: &crate::settings::AppSettings,
    run_override: Option<&str>,
) -> Result<Option<ReasoningEffort>> {
    let response_effort = settings.responses.effective_reasoning_effort();
    if let Some(value) = run_override {
        return ReasoningEffort::parse(value).map(Some).ok_or_else(|| {
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
    Ok(response_effort.or(settings
        .reasoning_effort_explicit
        .then_some(settings.reasoning_effort)))
}

#[cfg(test)]
mod tests {
    use super::resolve_reasoning_effort;
    use crate::settings::{AppSettings, ResponsesApiSettings};
    use deepagent_models::ReasoningEffort;

    #[test]
    fn reasoning_effort_resolution_normalizes_responses_aliases() {
        let mut settings: AppSettings = serde_json::from_value(serde_json::json!({
            "catalog": { "base_url": deepagent_models::DEEPSEEK_BASE_URL,
                "available": [], "chat_model": "deepseek-v4-flash", "reasoner_model": "deepseek-v4-pro" },
            "discovered_at": 0, "sandbox_mode": "workspace_write"
        })).unwrap();
        settings.reasoning_effort = ReasoningEffort::Low;
        assert_eq!(
            resolve_reasoning_effort(&settings, None).unwrap(),
            Some(ReasoningEffort::Low)
        );
        assert_eq!(
            resolve_reasoning_effort(&settings, Some("max")).unwrap(),
            Some(ReasoningEffort::Max)
        );
        settings.responses = ResponsesApiSettings {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_reasoning_effort(&settings, None).unwrap(),
            Some(ReasoningEffort::High)
        );
        for value in ["minimal", "medium", "xhigh", "simple", "deep"] {
            assert!(
                resolve_reasoning_effort(&settings, Some(value)).is_err(),
                "{value}"
            );
        }
        settings.responses.reasoning_effort = Some("medium".into());
        assert_eq!(
            resolve_reasoning_effort(&settings, None).unwrap(),
            Some(ReasoningEffort::High)
        );
    }

    #[test]
    fn h157_admission_distinguishes_model_default_from_global_and_turn_selections() {
        let mut settings: AppSettings = serde_json::from_value(serde_json::json!({
            "catalog": { "base_url": deepagent_models::DEEPSEEK_BASE_URL,
                "available": [
                    {"id":"primary","effort":{"supported_levels":["low"],"default_level":"low"}},
                    {"id":"fallback","effort":{"supported_levels":["high"],"default_level":"high"}}
                ], "chat_model":"primary", "reasoner_model":"fallback" },
            "discovered_at":0, "sandbox_mode":"workspace_write"
        }))
        .unwrap();
        let resolve = |settings: &AppSettings, effort| {
            super::resolve_run_model(
                settings,
                deepagent_models::ModelRole::Chat,
                None,
                None,
                effort,
            )
        };
        // An old document's retained global high must not be silently downgraded.
        assert!(settings.reasoning_effort_explicit);
        assert!(resolve(&settings, None).is_err());
        settings.reasoning_effort_explicit = false;
        assert_eq!(resolve_reasoning_effort(&settings, None).unwrap(), None);
        let plan = resolve(&settings, None).unwrap();
        assert_eq!(plan.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(plan.config.configured_reasoning_effort(), None);
        assert!(resolve(&settings, Some("high")).is_err());
        assert_eq!(
            resolve(&settings, Some("none")).unwrap().reasoning_effort,
            ReasoningEffort::None
        );
        assert_eq!(
            resolve(&settings, Some("low")).unwrap().reasoning_effort,
            ReasoningEffort::Low
        );
    }
}
