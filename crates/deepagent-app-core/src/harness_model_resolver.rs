//! ModelResolver (plan §8): the single place that turns one explicitly selected,
//! activated harness model into a credential-free route.
//!
//! The resolver never falls back to another model. A disabled provider, an
//! inactive model, or a protocol without a runtime codec is an error. The API key
//! is not read here; the admitted worker binds it through SettingsService.

use deepagent_core::error::{CoreError, Result};
use deepagent_models::{CapabilitySource, ModelCapability, WireMode};
use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider};
use deepagent_persistence::Database;

/// Maps a registry protocol onto a wire the runtime can speak.
pub fn wire_mode_for_protocol(protocol: &str) -> Result<WireMode> {
    match protocol {
        "responses" => Ok(WireMode::Responses),
        "chat_completions" => Ok(WireMode::ChatCompletions),
        "messages" => Ok(WireMode::Messages),
        other => Err(CoreError::invalid(format!("unknown protocol `{other}`"))),
    }
}

/// A credential-free route for one activated model.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub provider_id: String,
    pub model_name: String,
    pub base_url: String,
    pub wire_mode: WireMode,
    pub context_window: i64,
    pub output_limit: i64,
    pub capability: ModelCapability,
    pub protocol: String,
    pub provider_revision: i64,
    pub(crate) api_key_ref: String,
}

/// Resolve the explicitly selected model. Both the provider and the model must be
/// enabled/activated; no other model is considered.
pub fn resolve_active_model(
    db: &Database,
    provider_id: &str,
    model_name: &str,
) -> Result<ResolvedRoute> {
    let (provider, model) = db
        .harness_model_route(provider_id, model_name)?
        .ok_or_else(|| {
            CoreError::invalid(format!(
                "unknown model `{model_name}` under provider `{provider_id}`"
            ))
        })?;
    if !provider.enabled {
        return Err(CoreError::invalid(format!(
            "provider `{provider_id}` is disabled"
        )));
    }
    if !model.active {
        return Err(CoreError::invalid(format!(
            "model `{model_name}` is not activated"
        )));
    }
    let route = route(provider, model)?;
    tracing::debug!(
        provider_id = %route.provider_id,
        model_name = %route.model_name,
        wire_mode = ?route.wire_mode,
        "resolved active harness model"
    );
    Ok(route)
}

fn route(provider: HarnessProvider, model: HarnessModel) -> Result<ResolvedRoute> {
    // A model may override the provider protocol (plan §2).
    let protocol = model
        .protocol
        .as_deref()
        .unwrap_or(&provider.protocol)
        .to_owned();
    let wire_mode = wire_mode_for_protocol(&protocol)?;
    let capability = registry_capability(&model)?;
    Ok(ResolvedRoute {
        provider_id: provider.id,
        model_name: model.model_name,
        base_url: provider.model_url,
        wire_mode,
        context_window: model.context_window,
        output_limit: model.output_limit,
        api_key_ref: provider.api_key_ref,
        capability,
        protocol,
        provider_revision: provider.revision,
    })
}

fn registry_capability(model: &HarnessModel) -> Result<ModelCapability> {
    let capacity = |value: i64| {
        usize::try_from(value)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| CoreError::invalid("registry model capacities must be positive"))
    };
    let defaults = ModelCapability {
        model_id: model.model_name.clone(),
        context_window: capacity(model.context_window)?,
        max_output_tokens: capacity(model.output_limit)?,
        supports_tools: true,
        supports_thinking: true,
        supports_json_output: true,
        input_modalities: None,
        effort: None,
        thinking: None,
        capability_source: CapabilitySource::UserOverride,
        fallback_reason: None,
    };
    // Existing seeds store the full capability; fetched models may store a
    // partial object. Never infer another provider's features from its model id.
    let mut value = serde_json::to_value(&defaults)?;
    let stored: serde_json::Value = serde_json::from_str(&model.capabilities)
        .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
    let fields = stored
        .as_object()
        .ok_or_else(|| CoreError::invalid("registry model capabilities must be an object"))?;
    value.as_object_mut().unwrap().extend(fields.clone());
    // Registry columns own identity and limits; embedded legacy metadata may be stale.
    value["model_id"] = defaults.model_id.into();
    value["context_window"] = defaults.context_window.into();
    value["max_output_tokens"] = defaults.max_output_tokens.into();
    let thinking = deepagent_models::thinking::ThinkingCapabilities::from_registry_json(&stored)?;
    let mut capability: ModelCapability = serde_json::from_value(value)
        .map_err(|_| CoreError::invalid("invalid registry model capability fields"))?;
    capability.thinking = Some(thinking);
    Ok(capability)
}
