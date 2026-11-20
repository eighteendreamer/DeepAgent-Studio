//! Harness model registry (plan §3, §8).
//!
//! The registry lives in `harness_providers` / `harness_models` (migration V24)
//! and is separate from the canvas provider registry in the document store.
//! `status` on a model is the activation flag; `status` on a provider is
//! whether the provider is enabled. A model reaches the chat list only when
//! both are on.

use deepagent_core::error::CoreError;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{map_sqlite, Database, Result};

/// Wire protocols a harness provider may speak (plan §5). One list, one place.
pub const HARNESS_PROTOCOLS: &[&str] = &["responses", "chat_completions", "messages"];

const CHAT_SELECTION_COLLECTION: &str = "harness_selection";
const CHAT_SELECTION_ID: &str = "chat";

/// One atomic route selection. Omitted effort uses the model's resolved
/// application initial level; the runtime never substitutes a global level.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HarnessChatSelection {
    pub provider_id: String,
    pub model_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// One provider entry. The API key is referenced by `api_key_ref`, never held here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessProvider {
    pub id: String,
    pub channel: String,
    pub logo: String,
    pub website_url: String,
    pub model_url: String,
    pub protocol: String,
    pub api_key_ref: String,
    pub balance_kind: String,
    pub enabled: bool,
    /// Assigned by the store on every save; the input value is ignored.
    pub revision: i64,
}

/// One model under a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessModel {
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    /// `None` inherits the provider protocol (plan §2).
    pub protocol: Option<String>,
    pub context_window: i64,
    pub output_limit: i64,
    /// JSON object text, e.g. thinking capabilities.
    pub capabilities: String,
    pub active: bool,
}

/// Install defaults once. Reopening never restores deleted providers or overwrites
/// a user's endpoint, protocol, credentials, model activation or chat selection.
pub(crate) fn seed_builtin_harness_providers(conn: &Connection) -> Result<()> {
    let transaction = conn.unchecked_transaction().map_err(map_sqlite)?;
    let now = now_millis();
    let inserted = transaction
        .execute(
            "INSERT INTO documents (collection, id, body, created_at, updated_at)
         VALUES ('migrations', 'harness_builtin_providers_v1', '{\"version\":1}', ?1, ?1)
         ON CONFLICT(collection, id) DO NOTHING",
            [now],
        )
        .map_err(map_sqlite)?;
    if inserted != 0 {
        for (id, channel, url, website, logo, key, balance) in [
            (
                "layso",
                "Layso",
                "https://api.layso.ai/v1",
                "https://layso.ai/",
                "/providers/layso.webp",
                "harness_provider:layso",
                "auto",
            ),
            (
                "deepseek",
                "DeepSeek",
                "https://api.deepseek.com",
                "https://www.deepseek.com/",
                "/providers/deepseek.svg",
                "harness_provider:deepseek",
                "official",
            ),
        ] {
            transaction
                .execute(
                    "INSERT INTO harness_providers
                 (id, channel, model_url, website_url, logo, protocol, api_key_ref,
                  balance_kind, status, revision, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'responses', ?6, ?7, 1, 1, ?8, ?8)
                 ON CONFLICT(id) DO NOTHING",
                    params![id, channel, url, website, logo, key, balance, now],
                )
                .map_err(map_sqlite)?;
            // Supply missing branding on existing official routes only.
            transaction
                .execute(
                    "UPDATE harness_providers SET
                    website_url = CASE WHEN website_url = '' THEN ?2 ELSE website_url END,
                    logo = CASE WHEN logo = '' THEN ?3 ELSE logo END,
                    revision = revision + 1, updated_at = ?5
                 WHERE id = ?1 AND rtrim(model_url, '/') = ?4
                   AND (website_url = '' OR logo = '')",
                    params![id, website, logo, url, now],
                )
                .map_err(map_sqlite)?;
        }
        tracing::info!("installed built-in harness provider defaults");
    }
    transaction.commit().map_err(map_sqlite)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn ensure_known_protocol(protocol: &str) -> Result<()> {
    if HARNESS_PROTOCOLS.contains(&protocol) {
        Ok(())
    } else {
        Err(CoreError::Persistence(format!(
            "unknown harness protocol `{protocol}`"
        )))
    }
}

fn provider_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessProvider> {
    Ok(HarnessProvider {
        id: row.get(0)?,
        channel: row.get(1)?,
        logo: row.get(2)?,
        model_url: row.get(3)?,
        protocol: row.get(4)?,
        api_key_ref: row.get(5)?,
        balance_kind: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        revision: row.get(8)?,
        website_url: row.get(9)?,
    })
}

const PROVIDER_COLUMNS: &str = "id, channel, logo, model_url, protocol, api_key_ref,
    balance_kind, status, revision, website_url";

impl Database {
    /// Return the persisted chat selection, if the user has chosen one from
    /// the harness model list. No selection means no configured chat route.
    pub fn harness_chat_selection(&self) -> Result<Option<(String, String)>> {
        Ok(self
            .harness_chat_model_selection()?
            .map(|selected| (selected.provider_id, selected.model_name)))
    }

    pub fn harness_chat_model_selection(&self) -> Result<Option<HarnessChatSelection>> {
        self.with_conn(|conn| {
            let body: Option<String> = conn
                .query_row(
                    "SELECT body FROM documents WHERE collection = ?1 AND id = ?2",
                    params![CHAT_SELECTION_COLLECTION, CHAT_SELECTION_ID],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some(body) = body else { return Ok(None) };
            let value: HarnessChatSelection = serde_json::from_str(&body).map_err(|error| {
                CoreError::Persistence(format!("invalid harness selection: {error}"))
            })?;
            if value.provider_id.trim().is_empty()
                || value.model_name.trim().is_empty()
                || value
                    .reasoning_effort
                    .as_ref()
                    .is_some_and(|effort| effort.trim().is_empty())
            {
                Err(CoreError::Persistence(
                    "harness selection must contain provider_id and model_name".into(),
                ))
            } else {
                Ok(Some(value))
            }
        })
    }

    /// Persist the next chat selection without touching the current run's
    /// frozen model snapshot.
    pub fn set_harness_chat_selection(&self, provider_id: &str, model_name: &str) -> Result<()> {
        self.set_harness_chat_model_selection(provider_id, model_name, None)
    }

    pub fn set_harness_chat_model_selection(
        &self,
        provider_id: &str,
        model_name: &str,
        reasoning_effort: Option<&str>,
    ) -> Result<()> {
        let provider_id = provider_id.trim();
        let model_name = model_name.trim();
        if provider_id.is_empty()
            || model_name.is_empty()
            || reasoning_effort.is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(CoreError::invalid(
                "harness chat selection must not be empty",
            ));
        }
        let body = serde_json::to_string(&HarnessChatSelection {
            provider_id: provider_id.into(),
            model_name: model_name.into(),
            reasoning_effort: reasoning_effort.map(str::to_owned),
        })?;
        let now = now_millis();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO documents (collection, id, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(collection, id) DO UPDATE SET body = excluded.body,
                    updated_at = excluded.updated_at",
                params![CHAT_SELECTION_COLLECTION, CHAT_SELECTION_ID, body, now],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Read a provider and its selected model in one SQLite statement. Keep
    /// inactive rows so admission can explain why an exact selection is invalid.
    pub fn harness_model_route(
        &self,
        provider_id: &str,
        model_name: &str,
    ) -> Result<Option<(HarnessProvider, HarnessModel)>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT p.id, p.channel, p.logo, p.model_url, p.protocol, p.api_key_ref,
                        p.balance_kind, p.status, p.revision, p.website_url,
                        m.provider_id, m.model_name, m.display_name, m.protocol,
                        m.context_window, m.output_limit, m.capabilities, m.status
                 FROM harness_models m JOIN harness_providers p ON p.id = m.provider_id
                 WHERE p.id = ?1 AND m.model_name = ?2",
                params![provider_id, model_name],
                route_from_row,
            )
            .optional()
            .map_err(map_sqlite)
        })
    }

    /// Insert or replace a provider. Revision increments on every save; the
    /// creation time is kept from the first save.
    pub fn save_harness_provider(&self, provider: &HarnessProvider) -> Result<()> {
        ensure_known_protocol(&provider.protocol)?;
        let now = now_millis();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO harness_providers
                    (id, channel, logo, model_url, protocol, api_key_ref, balance_kind,
                     status, revision, created_at, updated_at, website_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    channel = excluded.channel,
                    logo = excluded.logo,
                    website_url = excluded.website_url,
                    model_url = excluded.model_url,
                    protocol = excluded.protocol,
                    api_key_ref = excluded.api_key_ref,
                    balance_kind = excluded.balance_kind,
                    status = excluded.status,
                    revision = harness_providers.revision + 1,
                    updated_at = excluded.updated_at",
                params![
                    provider.id,
                    provider.channel,
                    provider.logo,
                    provider.model_url,
                    provider.protocol,
                    provider.api_key_ref,
                    provider.balance_kind,
                    i64::from(provider.enabled),
                    now,
                    provider.website_url,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn list_harness_providers(&self) -> Result<Vec<HarnessProvider>> {
        self.with_conn(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {PROVIDER_COLUMNS} FROM harness_providers ORDER BY channel, id"
                ))
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([], provider_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)
        })
    }

    /// Remove a provider and all of its models. Returns whether it existed.
    pub fn remove_harness_provider(&self, provider_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM harness_models WHERE provider_id = ?1",
                [provider_id],
            )
            .map_err(map_sqlite)?;
            let removed = conn
                .execute("DELETE FROM harness_providers WHERE id = ?1", [provider_id])
                .map_err(map_sqlite)?;
            Ok(removed > 0)
        })
    }

    /// Store models fetched from a provider. New models start inactive. For a
    /// model that already exists, its descriptive fields are refreshed and its
    /// activation is left unchanged.
    pub fn save_harness_models(&self, provider_id: &str, models: &[HarnessModel]) -> Result<()> {
        let now = now_millis();
        self.with_conn(|conn| {
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM harness_providers WHERE id = ?1",
                    [provider_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if exists.is_none() {
                return Err(CoreError::Persistence(format!(
                    "unknown harness provider `{provider_id}`"
                )));
            }
            for model in models {
                if model.provider_id != provider_id {
                    return Err(CoreError::Persistence(format!(
                        "model `{}` belongs to `{}`, not `{provider_id}`",
                        model.model_name, model.provider_id
                    )));
                }
                if let Some(protocol) = &model.protocol {
                    ensure_known_protocol(protocol)?;
                }
                conn.execute(
                    "INSERT INTO harness_models
                        (id, provider_id, model_name, display_name, protocol,
                         context_window, output_limit, capabilities, status, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(provider_id, model_name) DO UPDATE SET
                        display_name = excluded.display_name,
                        protocol = excluded.protocol,
                        context_window = excluded.context_window,
                        output_limit = excluded.output_limit,
                        capabilities = excluded.capabilities,
                        updated_at = excluded.updated_at",
                    params![
                        format!("{provider_id}:{}", model.model_name),
                        provider_id,
                        model.model_name,
                        model.display_name,
                        model.protocol,
                        model.context_window,
                        model.output_limit,
                        model.capabilities,
                        i64::from(model.active),
                        now,
                    ],
                )
                .map_err(map_sqlite)?;
            }
            Ok(())
        })
    }

    /// Activate or deactivate one model. Returns whether the model exists.
    pub fn set_harness_model_active(
        &self,
        provider_id: &str,
        model_name: &str,
        active: bool,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let updated = conn
                .execute(
                    "UPDATE harness_models SET status = ?3, updated_at = ?4
                     WHERE provider_id = ?1 AND model_name = ?2",
                    params![provider_id, model_name, i64::from(active), now_millis()],
                )
                .map_err(map_sqlite)?;
            Ok(updated > 0)
        })
    }

    /// Every model stored under one provider, activated or not, by model name.
    pub fn list_harness_models(&self, provider_id: &str) -> Result<Vec<HarnessModel>> {
        self.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT provider_id, model_name, display_name, protocol,
                            context_window, output_limit, capabilities, status
                     FROM harness_models WHERE provider_id = ?1 ORDER BY model_name",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([provider_id], |row| {
                    Ok(HarnessModel {
                        provider_id: row.get(0)?,
                        model_name: row.get(1)?,
                        display_name: row.get(2)?,
                        protocol: row.get(3)?,
                        context_window: row.get(4)?,
                        output_limit: row.get(5)?,
                        capabilities: row.get(6)?,
                        active: row.get::<_, i64>(7)? != 0,
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)
        })
    }

    /// Models that may appear in the chat list: activated, under an enabled
    /// provider. Ordered by provider channel, then model name.
    pub fn active_harness_models(&self) -> Result<Vec<(HarnessProvider, HarnessModel)>> {
        self.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT p.id, p.channel, p.logo, p.model_url, p.protocol, p.api_key_ref,
                            p.balance_kind, p.status, p.revision, p.website_url,
                            m.provider_id, m.model_name, m.display_name, m.protocol,
                            m.context_window, m.output_limit, m.capabilities, m.status
                     FROM harness_models m
                     JOIN harness_providers p ON p.id = m.provider_id
                     WHERE m.status = 1 AND p.status = 1
                     ORDER BY p.channel, p.id, m.model_name",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([], route_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)
        })
    }
}

fn route_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(HarnessProvider, HarnessModel)> {
    Ok((
        provider_from_row(row)?,
        HarnessModel {
            provider_id: row.get(10)?,
            model_name: row.get(11)?,
            display_name: row.get(12)?,
            protocol: row.get(13)?,
            context_window: row.get(14)?,
            output_limit: row.get(15)?,
            capabilities: row.get(16)?,
            active: row.get::<_, i64>(17)? != 0,
        },
    ))
}
