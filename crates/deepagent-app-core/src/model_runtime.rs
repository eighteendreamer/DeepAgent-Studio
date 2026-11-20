use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelClient, ModelConfig, ReasoningEffort, ResponseDefaults};

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
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
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
        let origin = deepagent_core::event::ModelOrigin {
            provider_id: self.provider_id,
            model_name: self.model.clone(),
            protocol: self.protocol,
        };
        Ok(RunModelSelection {
            client: Arc::new(
                ModelClient::new(transport, config)
                    .with_model_origin(origin)
                    .with_image_file_cache(settings.image_file_cache()),
            ),
            model: self.model,
            // This value sizes local context policy only. ModelConfig resolves
            // the explicit application level; no level means no effort control.
            reasoning_effort: self.reasoning_effort.unwrap_or_default(),
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

/// Auxiliary requests prefer off only when the model declares it. This is an
/// internal preference, never a downgrade of an explicit user selection.
pub(crate) fn auxiliary_reasoning_effort(
    client: &ModelClient,
    model: &str,
) -> Option<ReasoningEffort> {
    if client
        .effective_reasoning_effort(model, Some(ReasoningEffort::None))
        .is_ok()
    {
        Some(ReasoningEffort::None)
    } else {
        client.configured_reasoning_effort()
    }
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
    let persisted_selection = db.harness_chat_model_selection()?;
    let provider = provider_override
        .or_else(|| {
            persisted_selection
                .as_ref()
                .map(|selection| selection.provider_id.as_str())
        })
        .ok_or_else(|| CoreError::invalid("select a configured provider and model first"))?;
    let model = model_override
        .or_else(|| {
            persisted_selection
                .as_ref()
                .map(|selection| selection.model_name.as_str())
        })
        .ok_or_else(|| CoreError::invalid("select a configured model first"))?;
    let route = crate::harness_model_resolver::resolve_active_model(db, provider, model)?;
    // A route selection owns its optional effort. A different route never
    // inherits the previous model's effort or the legacy global fallback.
    let requested = if let Some(value) = reasoning_effort_override {
        Some(
            ReasoningEffort::parse(value)
                .ok_or_else(|| CoreError::invalid("invalid reasoning effort override"))?,
        )
    } else if let Some(selected) = persisted_selection.as_ref() {
        if selected.provider_id == provider && selected.model_name == model {
            selected
                .reasoning_effort
                .as_deref()
                .map(|effort| {
                    ReasoningEffort::parse(effort)
                        .ok_or_else(|| CoreError::invalid("invalid selected reasoning effort"))
                })
                .transpose()?
        } else {
            None
        }
    } else {
        None
    };
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
    let reasoning_effort = config.effective_reasoning_effort(model, None)?;
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

fn response_defaults(
    settings: &crate::settings::AppSettings,
    requested: Option<ReasoningEffort>,
) -> ResponseDefaults {
    ResponseDefaults {
        temperature: settings.responses.effective_temperature(),
        top_p: settings.responses.effective_top_p(),
        max_output_tokens: settings.responses.effective_max_output_tokens(),
        top_logprobs: settings.responses.effective_top_logprobs(),
        reasoning_effort: requested,
        text: settings.responses.effective_text(),
        tool_choice: settings.responses.effective_tool_choice(),
        user: settings.responses.effective_user(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemorySecretStore, SettingsService};
    use deepagent_models::MockTransport;
    use deepagent_persistence::Database;

    #[test]
    fn general_settings_never_import_or_select_a_model() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json("{}")),
            Arc::new(MemorySecretStore::new()),
        );
        let general = settings.load().unwrap().unwrap();
        assert!(db.harness_chat_model_selection().unwrap().is_none());
        assert!(db.active_harness_models().unwrap().is_empty());
        assert!(!settings.view().unwrap().unwrap().configured);
        assert!(resolve_run_model(&db, &general, None, None, None).is_err());
        let encoded = serde_json::to_value(general).unwrap();
        assert!(encoded.get("catalog").is_none());
        assert!(encoded.get("discovered_at").is_none());
    }

    #[tokio::test]
    async fn admission_uses_only_the_exact_registry_selection() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        );
        crate::registry_fixture::configure(&settings, "synthetic-registry-key")
            .await
            .unwrap();
        let first =
            resolve_run_model(&db, &settings.load().unwrap().unwrap(), None, None, None).unwrap();
        settings
            .harness_models()
            .select_model("deepseek", "deepseek-v4-pro")
            .unwrap();
        let next =
            resolve_run_model(&db, &settings.load().unwrap().unwrap(), None, None, None).unwrap();
        assert_eq!(first.model, "deepseek-flash");
        assert_eq!(next.model, "deepseek-v4-pro");
        assert_eq!(next.provider_id, "deepseek");
        assert!(resolve_run_model(
            &db,
            &settings.load().unwrap().unwrap(),
            Some("missing"),
            None,
            None
        )
        .is_err());
        db.set_harness_model_active("deepseek", "deepseek-v4-pro", false)
            .unwrap();
        assert!(
            resolve_run_model(&db, &settings.load().unwrap().unwrap(), None, None, None).is_err()
        );
    }

    #[tokio::test]
    async fn legacy_global_effort_cannot_override_the_selected_route() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        );
        crate::registry_fixture::configure(&settings, "synthetic-effort-key")
            .await
            .unwrap();
        settings.set_reasoning_effort(ReasoningEffort::Low).unwrap();
        let mut general = settings.load().unwrap().unwrap();
        general.responses.reasoning_effort = Some("high".into());
        assert_eq!(
            resolve_run_model(&db, &general, None, None, None)
                .unwrap()
                .reasoning_effort,
            Some(ReasoningEffort::Low)
        );
        assert!(resolve_run_model(&db, &general, None, None, Some("ultra")).is_err());
        assert_eq!(
            resolve_run_model(&db, &general, None, None, Some("none"))
                .unwrap()
                .reasoning_effort,
            Some(ReasoningEffort::None)
        );
    }
}
