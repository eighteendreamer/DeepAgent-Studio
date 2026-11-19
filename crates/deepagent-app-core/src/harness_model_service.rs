//! Harness model service (plan §3, §8): the backend of the main settings
//! "模型" page. Providers and models live in the harness registry tables;
//! API keys live in the secret store under a per-provider name and never
//! leave the backend. The canvas provider registry is not used here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::balance::{fetch_balance_by_kind, BalanceAmount, BalanceKind};
use deepagent_models::discovery::ModelDiscovery;
use deepagent_models::thinking::ThinkingCapabilities;
use deepagent_models::transport::HttpTransport;
use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider, HARNESS_PROTOCOLS};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::SecretStore;

/// Placeholder limits for a freshly fetched model whose capacity is unknown.
/// The user edits them on the model page; no capability guessing happens here.
const DEFAULT_CONTEXT_WINDOW: i64 = 1_000_000;
const DEFAULT_OUTPUT_LIMIT: i64 = 131_072;
// The plan's 128K Messages fallback is decimal. Some compatible providers
// reject larger requests when their catalog omits the output ceiling.
const DEFAULT_MESSAGES_OUTPUT_LIMIT: i64 = 128_000;

/// Provider as shown to the UI. Carries presence of a key, never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProviderView {
    pub id: String,
    pub channel: String,
    pub logo: String,
    pub website_url: String,
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
    #[serde(default)]
    pub website_url: Option<String>,
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
    pub thinking_levels: Vec<String>,
    #[serde(default)]
    pub thinking_distinct: Vec<String>,
    pub thinking_disable: bool,
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
    pub thinking_levels: Vec<String>,
    pub thinking_distinct: Vec<String>,
    pub thinking_disable: bool,
}

/// Editable model fields. Identity and activation are owned by the registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModelInput {
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    pub protocol: Option<String>,
    pub context_window: i64,
    pub output_limit: i64,
    pub thinking_levels: Vec<String>,
    pub thinking_distinct: Vec<String>,
    pub thinking_disable: bool,
}

/// Normalized balance for one registered harness provider. The API key never
/// crosses this DTO boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBalanceView {
    pub provider_id: String,
    pub balance_kind: String,
    pub amount: BalanceAmount,
    pub available: Option<bool>,
}

impl TryFrom<HarnessModel> for HarnessModelView {
    type Error = CoreError;

    fn try_from(model: HarnessModel) -> Result<Self> {
        let value = serde_json::from_str(&model.capabilities)
            .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
        let thinking = ThinkingCapabilities::from_registry_json(&value)?;
        Ok(Self {
            provider_id: model.provider_id,
            model_name: model.model_name,
            display_name: model.display_name,
            protocol: model.protocol,
            context_window: model.context_window,
            output_limit: model.output_limit,
            active: model.active,
            thinking_levels: thinking
                .thinking_levels
                .iter()
                .map(|level| level.label().into())
                .collect(),
            thinking_distinct: thinking
                .thinking_distinct
                .iter()
                .map(|level| level.label().into())
                .collect(),
            thinking_disable: thinking.thinking_disable,
        })
    }
}

/// Secret name holding one provider's API key.
fn secret_name(provider_id: &str) -> String {
    format!("harness_provider:{provider_id}")
}

fn validate_provider_logo(logo: &str) -> Result<()> {
    use base64::Engine;
    if logo.is_empty() || matches!(logo, "/providers/layso.webp" | "/providers/deepseek.svg") {
        return Ok(());
    }
    let Some((header, encoded)) = logo.split_once(',') else {
        return Err(CoreError::invalid(
            "provider logo must be an uploaded image",
        ));
    };
    if !matches!(
        header,
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/webp;base64"
            | "data:image/gif;base64"
    ) || encoded.len() > 1_398_104
    {
        return Err(CoreError::invalid(
            "provider logo must be PNG, JPEG, WebP or GIF, at most 1 MB",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| CoreError::invalid("invalid provider logo encoding"))?;
    if bytes.is_empty() || bytes.len() > 1_048_576 {
        return Err(CoreError::invalid("provider logo must be at most 1 MB"));
    }
    Ok(())
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
        let previous = self.provider(&id)?;
        let website_url = input.website_url.unwrap_or_else(|| {
            previous
                .as_ref()
                .map(|provider| provider.website_url.clone())
                .unwrap_or_default()
        });
        if !website_url.is_empty() {
            let url = reqwest::Url::parse(&website_url)
                .map_err(|_| CoreError::invalid("invalid provider website URL"))?;
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(CoreError::invalid(
                    "provider website must be an HTTP(S) URL without credentials",
                ));
            }
        }
        if previous
            .as_ref()
            .is_none_or(|provider| provider.logo != input.logo)
        {
            validate_provider_logo(&input.logo)?;
        }
        let api_key_ref = previous
            .map(|provider| provider.api_key_ref)
            .unwrap_or_else(|| secret_name(&id));
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
            website_url,
            model_url: input.model_url,
            protocol: input.protocol,
            api_key_ref,
            balance_kind: input.balance_kind,
            enabled: input.enabled,
            revision: 0,
        };
        self.db.save_harness_provider(&provider)?;
        let saved = self
            .provider(&id)?
            .ok_or_else(|| CoreError::Persistence(format!("provider `{id}` not stored")))?;
        self.view(saved)
    }

    /// Remove a provider, its models and its stored key.
    pub fn remove_provider(&self, provider_id: &str) -> Result<bool> {
        let Some(provider) = self.provider(provider_id)? else {
            return Ok(false);
        };
        let removed = self.db.remove_harness_provider(provider_id)?;
        if removed {
            self.secrets.delete(&provider.api_key_ref)?;
        }
        Ok(removed)
    }

    /// Fetch the provider's `GET /models` with its stored key and store the
    /// result. New models start inactive; saved model configuration is retained.
    pub async fn fetch_models(&self, provider_id: &str) -> Result<Vec<HarnessModelView>> {
        let provider = self.provider(provider_id)?.ok_or_else(|| {
            CoreError::Persistence(format!("unknown harness provider `{provider_id}`"))
        })?;
        let key = self.secrets.get(&provider.api_key_ref)?.ok_or_else(|| {
            CoreError::Persistence(format!("provider `{provider_id}` has no API key"))
        })?;
        let wire_mode = crate::harness_model_resolver::wire_mode_for_protocol(&provider.protocol)?;
        let output_limit_fallback = if wire_mode == deepagent_models::WireMode::Messages {
            DEFAULT_MESSAGES_OUTPUT_LIMIT
        } else {
            DEFAULT_OUTPUT_LIMIT
        };
        let discovered =
            ModelDiscovery::with_base_url(self.transport.clone(), provider.model_url.clone())
                .with_wire_mode(wire_mode)
                .list_models(&key)
                .await?;
        let existing: HashMap<_, _> = self
            .db
            .list_harness_models(&provider.id)?
            .into_iter()
            .map(|model| (model.model_name.clone(), model))
            .collect();
        let mut names = HashSet::new();
        let capacity = |value: Option<u64>, default| match value {
            None => Ok(default),
            Some(value) => i64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    CoreError::invalid("discovered model capacities must be positive i64 values")
                }),
        };
        let models: Vec<HarnessModel> = discovered
            .into_iter()
            .map(|info| {
                if info.id.trim().is_empty() || !names.insert(info.id.clone()) {
                    return Err(CoreError::invalid(
                        "discovered model ids must be nonempty and unique",
                    ));
                }
                if let Some(saved) = existing.get(&info.id) {
                    return Ok(saved.clone());
                }
                let mut capabilities = serde_json::Map::new();
                if let Some(modalities) = info.input_modalities {
                    capabilities
                        .insert("input_modalities".into(), serde_json::to_value(modalities)?);
                }
                if let Some(effort) = info.effort {
                    capabilities.insert("effort".into(), serde_json::to_value(effort)?);
                }
                let capabilities = serde_json::Value::Object(capabilities);
                ThinkingCapabilities::from_registry_json(&capabilities)?;
                Ok(HarnessModel {
                    provider_id: provider.id.clone(),
                    display_name: info.id.clone(),
                    model_name: info.id,
                    protocol: None,
                    context_window: capacity(info.context_window, DEFAULT_CONTEXT_WINDOW)?,
                    output_limit: capacity(info.max_output_tokens, output_limit_fallback)?,
                    capabilities: serde_json::to_string(&capabilities)?,
                    active: false,
                })
            })
            .collect::<Result<_>>()?;
        self.db.save_harness_models(&provider.id, &models)?;
        self.list_models(&provider.id)
    }

    pub fn list_models(&self, provider_id: &str) -> Result<Vec<HarnessModelView>> {
        self.db
            .list_harness_models(provider_id)?
            .into_iter()
            .map(HarnessModelView::try_from)
            .collect()
    }

    pub fn save_model(&self, input: HarnessModelInput) -> Result<HarnessModelView> {
        if input.display_name.trim().is_empty()
            || input.context_window <= 0
            || input.output_limit <= 0
            || u32::try_from(input.output_limit).is_err()
        {
            return Err(CoreError::invalid(
                "model name and positive request capacities are required",
            ));
        }
        if input
            .protocol
            .as_deref()
            .is_some_and(|protocol| !HARNESS_PROTOCOLS.contains(&protocol))
        {
            return Err(CoreError::invalid("unknown harness model protocol"));
        }
        let (_, mut model) = self
            .db
            .harness_model_route(&input.provider_id, &input.model_name)?
            .ok_or_else(|| CoreError::invalid("unknown harness model"))?;
        let mut capabilities: serde_json::Value = serde_json::from_str(&model.capabilities)
            .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
        let fields = capabilities
            .as_object_mut()
            .ok_or_else(|| CoreError::invalid("registry model capabilities must be an object"))?;
        fields.insert(
            "thinking_levels".into(),
            serde_json::to_value(input.thinking_levels)?,
        );
        fields.insert(
            "thinking_distinct".into(),
            serde_json::to_value(input.thinking_distinct)?,
        );
        fields.insert("thinking_disable".into(), input.thinking_disable.into());
        ThinkingCapabilities::from_registry_json(&capabilities)?;
        model.display_name = input.display_name.trim().into();
        model.protocol = input.protocol;
        model.context_window = input.context_window;
        model.output_limit = input.output_limit;
        model.capabilities = serde_json::to_string(&capabilities)?;
        let view = HarnessModelView::try_from(model.clone())?;
        self.db.save_harness_models(&input.provider_id, &[model])?;
        Ok(view)
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

    /// Query one provider through the balance adapter selected in its registry
    /// row. `none` is a successful no-op with no fabricated amount.
    pub async fn fetch_balance(&self, provider_id: &str) -> Result<HarnessBalanceView> {
        let provider = self.provider(provider_id)?.ok_or_else(|| {
            CoreError::Persistence(format!("unknown harness provider `{provider_id}`"))
        })?;
        let kind = BalanceKind::parse(&provider.balance_kind)?;
        let key = if kind == BalanceKind::None {
            String::new()
        } else {
            self.secrets.get(&provider.api_key_ref)?.ok_or_else(|| {
                CoreError::Persistence(format!("provider `{provider_id}` has no API key"))
            })?
        };
        let snapshot =
            fetch_balance_by_kind(self.transport.clone(), &provider.model_url, &key, kind).await?;
        Ok(HarnessBalanceView {
            provider_id: provider.id,
            balance_kind: kind.as_str().into(),
            amount: snapshot.amount,
            available: snapshot.available,
        })
    }

    /// Activated models under enabled providers, for the chat model list.
    pub fn active_models(&self) -> Result<Vec<ActiveModelView>> {
        self.db
            .active_harness_models()?
            .into_iter()
            .map(|(provider, model)| self.active_view(provider, model))
            .collect()
    }

    /// Persist one active model as the next chat selection. The runtime reads
    /// this record only when admitting a new user turn.
    pub fn select_model(&self, provider_id: &str, model_name: &str) -> Result<ActiveModelView> {
        let (provider, model) = self
            .db
            .harness_model_route(provider_id, model_name)?
            .ok_or_else(|| CoreError::invalid("unknown harness model selection"))?;
        if !provider.enabled || !model.active {
            return Err(CoreError::invalid("harness model is not active"));
        }
        let view = self.active_view(provider, model)?;
        self.db
            .set_harness_chat_selection(provider_id, model_name)?;
        Ok(view)
    }

    /// Slash commands use the activated catalog, with a provider prefix for duplicates.
    pub fn select_model_by_name(&self, name: &str) -> Result<ActiveModelView> {
        let matches: Vec<_> = self
            .active_models()?
            .into_iter()
            .filter(|model| {
                model.model_name == name
                    || format!("{}/{}", model.provider_id, model.model_name) == name
            })
            .collect();
        match matches.as_slice() {
            [model] => self.select_model(&model.provider_id, &model.model_name),
            [] => Err(CoreError::invalid(format!(
                "model '{name}' is not activated under an enabled provider"
            ))),
            _ => Err(CoreError::invalid(format!(
                "model '{name}' belongs to multiple providers; use /model <provider_id>/<model_name>: {}",
                matches.iter().map(|model| format!("{}/{}", model.provider_id, model.model_name)).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    pub fn selected_model(&self) -> Result<Option<ActiveModelView>> {
        let Some((provider_id, model_name)) = self.db.harness_chat_selection()? else {
            return Ok(None);
        };
        let Some((provider, model)) = self.db.harness_model_route(&provider_id, &model_name)?
        else {
            return Ok(None);
        };
        if !provider.enabled || !model.active {
            return Ok(None);
        }
        self.active_view(provider, model).map(Some)
    }

    fn provider(&self, provider_id: &str) -> Result<Option<HarnessProvider>> {
        Ok(self
            .db
            .list_harness_providers()?
            .into_iter()
            .find(|provider| provider.id == provider_id))
    }

    fn view(&self, provider: HarnessProvider) -> Result<HarnessProviderView> {
        let has_api_key = self.secrets.get(&provider.api_key_ref)?.is_some();
        Ok(HarnessProviderView {
            id: provider.id,
            channel: provider.channel,
            logo: provider.logo,
            website_url: provider.website_url,
            model_url: provider.model_url,
            protocol: provider.protocol,
            balance_kind: provider.balance_kind,
            enabled: provider.enabled,
            has_api_key,
            revision: provider.revision,
        })
    }

    fn active_view(
        &self,
        provider: HarnessProvider,
        model: HarnessModel,
    ) -> Result<ActiveModelView> {
        let capabilities: serde_json::Value = serde_json::from_str(&model.capabilities)
            .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
        let thinking = ThinkingCapabilities::from_registry_json(&capabilities)?;
        Ok(ActiveModelView {
            protocol: model.protocol.unwrap_or_else(|| provider.protocol.clone()),
            provider_id: provider.id,
            provider_channel: provider.channel,
            provider_logo: provider.logo,
            model_url: provider.model_url,
            model_name: model.model_name,
            display_name: model.display_name,
            context_window: model.context_window,
            output_limit: model.output_limit,
            thinking_levels: thinking
                .thinking_levels
                .iter()
                .map(|level| level.label().to_owned())
                .collect(),
            thinking_distinct: thinking
                .thinking_distinct
                .iter()
                .map(|level| level.label().to_owned())
                .collect(),
            thinking_disable: thinking.thinking_disable,
        })
    }
}
