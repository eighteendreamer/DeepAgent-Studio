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
    pub(crate) provider_id: String,
    pub(crate) protocol: String,
    pub(crate) provider_revision: i64,
    pub(crate) config_hash: String,
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) override_applied: bool,
    config: ModelConfig,
    api_key_ref: String,
}

impl RunModelPlan {
    pub(crate) fn bind(
        self,
        settings: &SettingsService,
        transport: Arc<dyn HttpTransport>,
    ) -> Result<RunModelSelection> {
        let mut config = self.config;
        config.api_key = settings
            .harness_api_key(&self.api_key_ref)?
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                CoreError::invalid(format!("provider `{}` has no API key", self.provider_id))
            })?;
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
) -> Result<(Arc<ModelClient>, String, ReasoningEffort)> {
    let selected = select_run_model(settings, transport, None, None, None)?;
    Ok((selected.client, selected.model, selected.reasoning_effort))
}

pub fn build_chat_model_client(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
) -> Result<(Arc<ModelClient>, String, ReasoningEffort)> {
    build_model_client(settings, transport)
}

pub(crate) fn select_run_model(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    reasoning_effort_override: Option<&str>,
) -> Result<RunModelSelection> {
    let loaded = settings
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    resolve_run_model(
        settings.database(),
        &loaded,
        provider_override,
        model_override,
        reasoning_effort_override,
    )?
    .bind(settings, transport)
}

pub(crate) fn resolve_run_model(
    db: &deepagent_persistence::Database,
    loaded: &crate::settings::AppSettings,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    reasoning_effort_override: Option<&str>,
) -> Result<RunModelPlan> {
    // Preserve the old public provider id as an alias for its migrated row.
    // Selection and validation still go through the registry exactly once.
    let provider = match provider_override {
        None | Some(DEEPSEEK_OFFICIAL_PROVIDER) => "deepseek",
        Some(id) => id,
    };
    let model = model_override.unwrap_or(&loaded.catalog.chat_model);
    let route = crate::harness_model_resolver::resolve_active_model(db, provider, model)?;
    let requested = resolve_reasoning_effort(loaded, reasoning_effort_override)?;
    let mut defaults = response_defaults(loaded, requested);
    defaults.max_output_tokens = defaults
        .max_output_tokens
        .or(Some(u32::try_from(route.output_limit).map_err(|_| {
            CoreError::invalid("registry output limit exceeds request capacity")
        })?));
    let config_hash = crate::run_capabilities::capability_hash(&serde_json::json!({
        "provider":route.provider_id, "revision":route.provider_revision,
        "baseUrl":route.base_url, "protocol":route.protocol, "keyRef":route.api_key_ref,
        "capability":route.capability,
    }))?;
    let config = ModelConfig {
        base_url: route.base_url,
        api_key: String::new(),
        defaults,
        wire_mode: route.wire_mode,
        capabilities: vec![route.capability],
        prefer_model_reasoning_default: requested.is_none(),
    };
    let reasoning_effort = config
        .effective_reasoning_effort(&model, None)?
        .unwrap_or_default();
    Ok(RunModelPlan {
        config,
        api_key_ref: route.api_key_ref,
        provider_id: route.provider_id,
        protocol: route.protocol,
        provider_revision: route.provider_revision,
        config_hash,
        override_applied: model_override.is_some(),
        model: model.to_owned(),
        reasoning_effort,
    })
}

/// Compatibility projection for the legacy settings view and sampling preview.
/// Runtime routing always uses the harness registry through resolve_run_model.
pub(crate) fn model_config(
    settings: &crate::settings::AppSettings,
    role: ModelRole,
    run_override: Option<&str>,
) -> Result<ModelConfig> {
    let requested = resolve_reasoning_effort(settings, run_override)?;
    let defaults = response_defaults(settings, requested);
    Ok(ModelConfig::from_catalog("", &settings.catalog, role)
        .with_defaults(defaults)
        .with_model_reasoning_defaults(requested.is_none())
        .with_wire_mode(WireMode::Responses))
}

fn response_defaults(
    settings: &crate::settings::AppSettings,
    requested: Option<ReasoningEffort>,
) -> ResponseDefaults {
    ResponseDefaults {
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
    }
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
        let db = deepagent_persistence::Database::open_in_memory().unwrap();
        let capability = deepagent_models::ModelCapabilityResolver::new()
            .resolve_model(&settings.catalog.available[0]);
        db.seed_legacy_harness_model(
            &settings.catalog.base_url,
            &deepagent_persistence::harness_registry::HarnessModel {
                provider_id: "deepseek".into(),
                model_name: "primary".into(),
                display_name: "primary".into(),
                protocol: None,
                context_window: capability.context_window as i64,
                output_limit: capability.max_output_tokens as i64,
                capabilities: serde_json::to_string(&capability).unwrap(),
                active: true,
            },
        )
        .unwrap();
        let resolve = |settings: &AppSettings, effort| {
            super::resolve_run_model(&db, settings, None, None, effort)
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
