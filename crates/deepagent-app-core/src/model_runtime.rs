use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelClient, ModelConfig, ModelRole, ResponseDefaults, ThinkingDepth};

use crate::settings::SettingsService;

pub(crate) struct RunModelSelection {
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) thinking_depth: ThinkingDepth,
    pub(crate) fallback_model: Option<String>,
}

pub(crate) fn build_model_client(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    role: ModelRole,
) -> Result<(Arc<ModelClient>, String, ThinkingDepth)> {
    let loaded = settings
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    let api_key = settings
        .api_key()?
        .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
    let thinking_depth = loaded.thinking_depth;
    let model = loaded.catalog.model_for(role).to_string();
    let defaults = ResponseDefaults {
        temperature: loaded.responses.effective_temperature(),
        top_p: loaded.responses.effective_top_p(),
        max_output_tokens: loaded.responses.effective_max_output_tokens(),
        top_logprobs: loaded.responses.effective_top_logprobs(),
        reasoning_effort: loaded.responses.effective_reasoning_effort(),
        text: loaded.responses.effective_text(),
        tool_choice: loaded.responses.effective_tool_choice(),
        user: loaded.responses.effective_user(),
        native_web_search: loaded.web_search.enabled
            && matches!(
                loaded.web_search.provider,
                crate::settings::WebSearchProvider::DeepSeekFirst
            ),
    };
    let config = ModelConfig::from_catalog(api_key, &loaded.catalog, role).with_defaults(defaults);
    let client = Arc::new(ModelClient::new(transport, config));
    Ok((client, model, thinking_depth))
}

/// Public wrapper for host layers (e.g. the Tauri hook-test command): build
/// a Chat-role model client from persisted settings.
pub fn build_chat_model_client(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
) -> Result<(Arc<ModelClient>, String, ThinkingDepth)> {
    build_model_client(settings, transport, ModelRole::Chat)
}

pub(crate) fn select_run_model(
    settings: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    role: ModelRole,
    fallback_role: ModelRole,
    model_override: Option<&str>,
) -> Result<RunModelSelection> {
    let loaded = settings
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    let api_key = settings
        .api_key()?
        .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
    let thinking_depth = loaded.thinking_depth;
    // Scalar `model` from the config overlay (managed > run > local >
    // project > user > plugin) overrides the catalog's role default — but
    // only when this provider actually serves that model. Compat overlays
    // read `~/.claude/settings.json`, where users keep Claude model names
    // ("sonnet", "opus"); passing those through verbatim would 400 every
    // DeepSeek request. Unknown names are ignored, not errors.
    let model = model_override
        .filter(|id| loaded.catalog.has_model(id))
        .map(str::to_string)
        .unwrap_or_else(|| loaded.catalog.model_for(role).to_string());
    let fallback_model = Some(loaded.catalog.model_for(fallback_role).to_string())
        .filter(|fallback| fallback != &model);
    let defaults = ResponseDefaults {
        temperature: loaded.responses.effective_temperature(),
        top_p: loaded.responses.effective_top_p(),
        max_output_tokens: loaded.responses.effective_max_output_tokens(),
        top_logprobs: loaded.responses.effective_top_logprobs(),
        reasoning_effort: loaded.responses.effective_reasoning_effort(),
        text: loaded.responses.effective_text(),
        tool_choice: loaded.responses.effective_tool_choice(),
        user: loaded.responses.effective_user(),
        native_web_search: loaded.web_search.enabled
            && matches!(
                loaded.web_search.provider,
                crate::settings::WebSearchProvider::DeepSeekFirst
            ),
    };
    let config = ModelConfig::from_catalog(api_key, &loaded.catalog, role).with_defaults(defaults);
    let client = Arc::new(ModelClient::new(transport, config));
    Ok(RunModelSelection {
        client,
        model,
        thinking_depth,
        fallback_model,
    })
}
