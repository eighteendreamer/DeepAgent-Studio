//! Harness model service (plan §3, §8): the backend of the main settings
//! "模型" page. Providers and models live in the harness registry tables;
//! API keys live in the secret store under a per-provider name and never
//! leave the backend. The canvas provider registry is not used here.

use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::discovery::ModelDiscovery;
use deepagent_models::transport::HttpTransport;
use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider, HARNESS_PROTOCOLS};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::SecretStore;

/// Placeholder limits for a freshly fetched model whose capacity is unknown.
/// The user edits them on the model page; no capability guessing happens here.
const DEFAULT_CONTEXT_WINDOW: i64 = 1_000_000;
const DEFAULT_OUTPUT_LIMIT: i64 = 131_072;

/// Provider as shown to the UI. Carries presence of a key, never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProviderView {
    pub id: String,
    pub channel: String,
    pub logo: String,
    pub model_url: String,
    pub protocol: String,
    pub balance_kind: String,
    pub enabled: bool,
    pub has_api_key: bool,
    pub revision: i64,
}

/// Create or update a provider. `api_key: None` keeps the stored key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProviderInput {
    pub id: Option<String>,
    pub channel: String,
    pub logo: String,
    pub model_url: String,
    pub protocol: String,
    pub balance_kind: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

/// One activated model, ready for the chat model list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveModelView {
    pub provider_id: String,
    pub provider_channel: String,
    pub provider_logo: String,
    pub model_url: String,
    pub model_name: String,
    pub display_name: String,
    /// The model's own protocol when set, otherwise the provider's.
    pub protocol: String,
    pub context_window: i64,
    pub output_limit: i64,
}

/// One model under a provider, as shown on the settings page. Plain DTO so
/// the persistence row never crosses the Tauri boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModelView {
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    /// `None` inherits the provider protocol.
    pub protocol: Option<String>,
    pub context_window: i64,
    pub output_limit: i64,
    pub active: bool,
}

impl From<HarnessModel> for HarnessModelView {
    fn from(model: HarnessModel) -> Self {
        Self {
            provider_id: model.provider_id,
            model_name: model.model_name,
            display_name: model.display_name,
            protocol: model.protocol,
            context_window: model.context_window,
            output_limit: model.output_limit,
            active: model.active,
        }
    }
}

/// Secret name holding one provider's API key.
fn secret_name(provider_id: &str) -> String {
    format!("harness_provider:{provider_id}")
}

pub struct HarnessModelService {
    db: Arc<Database>,
    secrets: Arc<dyn SecretStore>,
    transport: Arc<dyn HttpTransport>,
}

impl HarnessModelService {
    pub fn new(
        db: Arc<Database>,
        secrets: Arc<dyn SecretStore>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            db,
            secrets,
            transport,
        }
    }

    pub fn list_providers(&self) -> Result<Vec<HarnessProviderView>> {
        self.db
            .list_harness_providers()?
            .into_iter()
            .map(|provider| self.view(provider))
            .collect()
    }

    /// Create or update a provider. The key, when given, is written to the
    /// secret store before the row, after the protocol is validated.
    pub fn save_provider(&self, input: HarnessProviderInput) -> Result<HarnessProviderView> {
        if !HARNESS_PROTOCOLS.contains(&input.protocol.as_str()) {
            return Err(CoreError::Persistence(format!(
                "unknown harness protocol `{}`",
                input.protocol
            )));
        }
        let id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("prov-{}", uuid::Uuid::new_v4()));
        let api_key_ref = secret_name(&id);
        if let Some(key) = input
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            self.secrets.set(&api_key_ref, key)?;
        }
        let provider = HarnessProvider {
            id: id.clone(),
            channel: input.channel,
            logo: input.logo,
            model_url: input.model_url,
            protocol: input.protocol,
            api_key_ref,
            balance_kind: input.balance_kind,
            enabled: input.enabled,
            revision: 0,
        };
        self.db.save_harness_provider(&provider)?;
        let saved = self
            .db
            .list_harness_providers()?
            .into_iter()
            .find(|stored| stored.id == id)
            .ok_or_else(|| CoreError::Persistence(format!("provider `{id}` not stored")))?;
        self.view(saved)
    }

    /// Remove a provider, its models and its stored key.
    pub fn remove_provider(&self, provider_id: &str) -> Result<bool> {
        let removed = self.db.remove_harness_provider(provider_id)?;
        self.secrets.delete(&secret_name(provider_id))?;
        Ok(removed)
    }

    /// Fetch the provider's `GET /models` with its stored key and store the
    /// result. New models start inactive; existing activation is kept.
    pub async fn fetch_models(&self, provider_id: &str) -> Result<Vec<HarnessModelView>> {
        let provider = self
            .db
            .list_harness_providers()?
            .into_iter()
            .find(|stored| stored.id == provider_id)
            .ok_or_else(|| {
                CoreError::Persistence(format!("unknown harness provider `{provider_id}`"))
            })?;
        let key = self.secrets.get(&provider.api_key_ref)?.ok_or_else(|| {
            CoreError::Persistence(format!("provider `{provider_id}` has no API key"))
        })?;
        let discovered =
            ModelDiscovery::with_base_url(self.transport.clone(), provider.model_url.clone())
                .list_models(&key)
                .await?;
        let models: Vec<HarnessModel> = discovered
            .into_iter()
            .map(|info| HarnessModel {
                provider_id: provider.id.clone(),
                display_name: info.id.clone(),
                model_name: info.id,
                protocol: None,
                context_window: DEFAULT_CONTEXT_WINDOW,
                output_limit: DEFAULT_OUTPUT_LIMIT,
                capabilities: "{}".into(),
                active: false,
            })
            .collect();
        self.db.save_harness_models(&provider.id, &models)?;
        self.list_models(&provider.id)
    }

    pub fn list_models(&self, provider_id: &str) -> Result<Vec<HarnessModelView>> {
        Ok(self
            .db
            .list_harness_models(provider_id)?
            .into_iter()
            .map(HarnessModelView::from)
            .collect())
    }

    pub fn set_model_active(
        &self,
        provider_id: &str,
        model_name: &str,
        active: bool,
    ) -> Result<bool> {
        self.db
            .set_harness_model_active(provider_id, model_name, active)
    }

    /// Activated models under enabled providers, for the chat model list.
    pub fn active_models(&self) -> Result<Vec<ActiveModelView>> {
        Ok(self
            .db
            .active_harness_models()?
            .into_iter()
            .map(|(provider, model)| ActiveModelView {
                protocol: model.protocol.unwrap_or_else(|| provider.protocol.clone()),
                provider_id: provider.id,
                provider_channel: provider.channel,
                provider_logo: provider.logo,
                model_url: provider.model_url,
                model_name: model.model_name,
                display_name: model.display_name,
                context_window: model.context_window,
                output_limit: model.output_limit,
            })
            .collect())
    }

    fn view(&self, provider: HarnessProvider) -> Result<HarnessProviderView> {
        let has_api_key = self.secrets.get(&provider.api_key_ref)?.is_some();
        Ok(HarnessProviderView {
            id: provider.id,
            channel: provider.channel,
            logo: provider.logo,
            model_url: provider.model_url,
            protocol: provider.protocol,
            balance_kind: provider.balance_kind,
            enabled: provider.enabled,
            has_api_key,
            revision: provider.revision,
        })
    }
}
