//! Harness model service (plan §3, §8): the backend of the main settings
//! "模型" page. Providers and models live in the harness registry tables;
//! API keys live in the secret store under a per-provider name and never
//! leave the backend. The canvas provider registry is not used here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::balance::{fetch_balance_by_kind, BalanceAmount, BalanceKind};
use deepagent_models::discovery::ModelDiscovery;
use deepagent_models::thinking::{
    ReasoningEfforts, ThinkingCapabilities, ThinkingFormat, ThinkingLevel,
};
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
    pub reasoning_effort: Option<String>,
    pub default_reasoning_effort: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<ReasoningEfforts>,
    pub effective_reasoning_efforts: ReasoningEfforts,
    pub thinking_format: Option<ThinkingFormat>,
    pub effective_thinking_format: ThinkingFormat,
    pub default_reasoning_effort: Option<ThinkingLevel>,
    pub effective_default_reasoning_effort: Option<ThinkingLevel>,
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
    #[serde(default, deserialize_with = "deserialize_reasoning_declaration")]
    pub reasoning_efforts: Option<ReasoningEfforts>,
    #[serde(default)]
    pub default_reasoning_effort: Option<ThinkingLevel>,
    #[serde(default)]
    pub thinking_format: Option<ThinkingFormat>,
}

fn deserialize_reasoning_declaration<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<ReasoningEfforts>, D::Error> {
    ReasoningEfforts::deserialize(deserializer).map(Some)
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

impl HarnessModelView {
    fn from_model(provider: &HarnessProvider, model: HarnessModel) -> Result<Self> {
        let value = serde_json::from_str(&model.capabilities)
            .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
        let thinking = crate::harness_model_resolver::registry_thinking(provider, &model, &value)?;
        let reasoning_efforts = ThinkingCapabilities::declaration(&value)?.or_else(|| {
            value
                .get("thinking_levels")
                .map(|_| thinking.reasoning_efforts.clone())
        });
        Ok(Self {
            reasoning_efforts,
            thinking_format: value
                .get("thinking_format")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|_| CoreError::invalid("invalid reasoning transport format"))?,
            effective_thinking_format: thinking.thinking_format,
            effective_reasoning_efforts: thinking.reasoning_efforts.clone(),
            default_reasoning_effort: value
                .get("reasoning_default")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()
                .map_err(|_| CoreError::invalid("invalid reasoning default level"))?,
            effective_default_reasoning_effort: thinking.default_effort,
            provider_id: model.provider_id,
            model_name: model.model_name,
            display_name: model.display_name,
            protocol: model.protocol,
            context_window: model.context_window,
            output_limit: model.output_limit,
            active: model.active,
            thinking_levels: thinking
                .levels()
                .iter()
                .map(|level| level.label().into())
                .collect(),
            thinking_distinct: thinking
                .distinct_levels()
                .iter()
                .map(|level| level.label().into())
                .collect(),
            thinking_disable: thinking.allows(ThinkingLevel::Off),
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

    /// Sign out of configured providers; model configuration stays in SQLite.
    pub fn clear_credentials(&self) -> Result<()> {
        for provider in self.db.list_harness_providers()? {
            self.secrets.delete(&provider.api_key_ref)?;
        }
        Ok(())
    }

    /// Read-only diagnostic against the exact selected provider and protocol.
    pub async fn validate_selected_provider(&self) -> Result<usize> {
        let selected = self
            .selected_model()?
            .ok_or_else(|| CoreError::invalid("select a model in Settings first"))?;
        let provider = self
            .provider(&selected.provider_id)?
            .ok_or_else(|| CoreError::invalid("selected provider is missing"))?;
        let key = self
            .secrets
            .get(&provider.api_key_ref)?
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| CoreError::invalid("selected provider has no API key"))?;
        let wire = crate::harness_model_resolver::wire_mode_for_protocol(&selected.protocol)?;
        let models = ModelDiscovery::with_base_url(self.transport.clone(), provider.model_url)
            .with_wire_mode(wire)
            .list_models(&key)
            .await
            .map_err(|error| CoreError::other(error.to_string().replace(&key, "[redacted]")))?;
        if models.is_empty() {
            return Err(CoreError::invalid("selected provider returned no models"));
        }
        Ok(models.len())
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
                ThinkingCapabilities::for_model(&info.id, &capabilities)?;
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
        let Some(provider) = self.provider(provider_id)? else {
            return Ok(Vec::new());
        };
        self.db
            .list_harness_models(provider_id)?
            .into_iter()
            .map(|model| HarnessModelView::from_model(&provider, model))
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
        let (provider, mut model) = self
            .db
            .harness_model_route(&input.provider_id, &input.model_name)?
            .ok_or_else(|| CoreError::invalid("unknown harness model"))?;
        let mut capabilities: serde_json::Value = serde_json::from_str(&model.capabilities)
            .map_err(|_| CoreError::invalid("invalid registry model capabilities JSON"))?;
        let fields = capabilities
            .as_object_mut()
            .ok_or_else(|| CoreError::invalid("registry model capabilities must be an object"))?;
        for legacy in ["thinking_levels", "thinking_distinct", "thinking_disable"] {
            fields.remove(legacy);
        }
        if let Some(efforts) = input.reasoning_efforts {
            fields.insert("reasoning_efforts".into(), serde_json::to_value(efforts)?);
        } else {
            fields.remove("reasoning_efforts");
        }
        if let Some(default) = input.default_reasoning_effort {
            fields.insert("reasoning_default".into(), serde_json::to_value(default)?);
        } else {
            fields.remove("reasoning_default");
        }
        if let Some(format) = input.thinking_format {
            fields.insert("thinking_format".into(), serde_json::to_value(format)?);
        } else {
            fields.remove("thinking_format");
        }
        model.protocol = input.protocol.clone();
        crate::harness_model_resolver::registry_thinking(&provider, &model, &capabilities)?;
        model.display_name = input.display_name.trim().into();
        model.protocol = input.protocol;
        model.context_window = input.context_window;
        model.output_limit = input.output_limit;
        model.capabilities = serde_json::to_string(&capabilities)?;
        let view = HarnessModelView::from_model(&provider, model.clone())?;
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
        self.select_model_with_reasoning(provider_id, model_name, None)
    }

    pub fn select_model_with_reasoning(
        &self,
        provider_id: &str,
        model_name: &str,
        effort: Option<&str>,
    ) -> Result<ActiveModelView> {
        let (provider, model) = self
            .db
            .harness_model_route(provider_id, model_name)?
            .ok_or_else(|| CoreError::invalid("unknown harness model selection"))?;
        if !provider.enabled || !model.active {
            return Err(CoreError::invalid("harness model is not active"));
        }
        let mut view = self.active_view(provider, model)?;
        let route =
            crate::harness_model_resolver::resolve_active_model(&self.db, provider_id, model_name)?;
        let requested = effort
            .map(|effort| {
                deepagent_models::ReasoningEffort::parse(effort)
                    .ok_or_else(|| CoreError::invalid("unknown reasoning effort"))
            })
            .transpose()?;
        view.reasoning_effort = route
            .capability
            .resolve_reasoning_effort(requested)?
            .map(|effort| effort.label().into());
        self.db.set_harness_chat_model_selection(
            provider_id,
            model_name,
            view.reasoning_effort.as_deref(),
        )?;
        Ok(view)
    }

    /// Slash commands use the activated catalog, with a provider prefix for duplicates.
    pub fn select_model_by_name(&self, name: &str) -> Result<ActiveModelView> {
        let model = self.find_active_model(name)?;
        self.select_model(&model.provider_id, &model.model_name)
    }

    pub(crate) fn find_active_model(&self, name: &str) -> Result<ActiveModelView> {
        let matches: Vec<_> = self
            .active_models()?
            .into_iter()
            .filter(|model| {
                model.model_name == name
                    || format!("{}/{}", model.provider_id, model.model_name) == name
            })
            .collect();
        match matches.as_slice() {
            [model] => Ok(model.clone()),
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
        let Some(selection) = self.db.harness_chat_model_selection()? else {
            return Ok(None);
        };
        let Some((provider, model)) = self
            .db
            .harness_model_route(&selection.provider_id, &selection.model_name)?
        else {
            return Ok(None);
        };
        if !provider.enabled || !model.active {
            return Ok(None);
        }
        let mut view = self.active_view(provider, model)?;
        let route = crate::harness_model_resolver::resolve_active_model(
            &self.db,
            &selection.provider_id,
            &selection.model_name,
        )?;
        let requested = selection
            .reasoning_effort
            .as_deref()
            .map(|effort| {
                deepagent_models::ReasoningEffort::parse(effort)
                    .ok_or_else(|| CoreError::invalid("unknown stored reasoning effort"))
            })
            .transpose()?;
        view.reasoning_effort = route
            .capability
            .resolve_reasoning_effort(requested)?
            .map(|effort| effort.label().into());
        Ok(Some(view))
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
        let thinking =
            crate::harness_model_resolver::registry_thinking(&provider, &model, &capabilities)?;
        let default_reasoning_effort = thinking
            .default_effort
            .map(deepagent_models::ReasoningEffort::from)
            .map(|effort| effort.label().to_owned());
        Ok(ActiveModelView {
            reasoning_effort: None,
            default_reasoning_effort,
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
                .levels()
                .iter()
                .map(|level| level.label().to_owned())
                .collect(),
            thinking_distinct: thinking
                .distinct_levels()
                .iter()
                .map(|level| level.label().to_owned())
                .collect(),
            thinking_disable: thinking.allows(ThinkingLevel::Off),
        })
    }
}
