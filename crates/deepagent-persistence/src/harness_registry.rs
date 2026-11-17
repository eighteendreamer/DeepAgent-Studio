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

/// Base URL of the DeepSeek route that existed before the registry. Used only
/// to create the one-time legacy provider row; runtime code reads `model_url`
/// from the registry and never this constant.
const LEGACY_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

/// Legacy secret name written by the old settings page.
const LEGACY_DEEPSEEK_KEY_NAME: &str = "deepseek_api_key";

/// One provider entry. The API key is referenced by `api_key_ref`, never held here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessProvider {
    pub id: String,
    pub channel: String,
    pub logo: String,
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

/// Legacy import: create the provider row for an existing DeepSeek key. The key
/// is referenced by name, never copied. Safe on every open.
pub(crate) fn import_legacy_deepseek_provider(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO harness_providers
            (id, channel, logo, model_url, protocol, api_key_ref, balance_kind,
             status, revision, created_at, updated_at)
         SELECT 'deepseek', 'DeepSeek', '', ?1, 'responses', name, 'auto',
                1, 1, updated_at, updated_at
         FROM secret_records
         WHERE name = ?2",
        [LEGACY_DEEPSEEK_BASE_URL, LEGACY_DEEPSEEK_KEY_NAME],
    )
    .map_err(map_sqlite)?;
    Ok(())
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
    })
}

const PROVIDER_COLUMNS: &str = "id, channel, logo, model_url, protocol, api_key_ref,
    balance_kind, status, revision";

impl Database {
    /// Insert or replace a provider. Revision increments on every save; the
    /// creation time is kept from the first save.
    pub fn save_harness_provider(&self, provider: &HarnessProvider) -> Result<()> {
        ensure_known_protocol(&provider.protocol)?;
        let now = now_millis();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO harness_providers
                    (id, channel, logo, model_url, protocol, api_key_ref, balance_kind,
                     status, revision, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    channel = excluded.channel,
                    logo = excluded.logo,
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

    /// Models that may appear in the chat list: activated, under an enabled
    /// provider. Ordered by provider channel, then model name.
    pub fn active_harness_models(&self) -> Result<Vec<(HarnessProvider, HarnessModel)>> {
        self.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT p.id, p.channel, p.logo, p.model_url, p.protocol, p.api_key_ref,
                            p.balance_kind, p.status, p.revision,
                            m.provider_id, m.model_name, m.display_name, m.protocol,
                            m.context_window, m.output_limit, m.capabilities
                     FROM harness_models m
                     JOIN harness_providers p ON p.id = m.provider_id
                     WHERE m.status = 1 AND p.status = 1
                     ORDER BY p.channel, p.id, m.model_name",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([], |row| {
                    let provider = HarnessProvider {
                        id: row.get(0)?,
                        channel: row.get(1)?,
                        logo: row.get(2)?,
                        model_url: row.get(3)?,
                        protocol: row.get(4)?,
                        api_key_ref: row.get(5)?,
                        balance_kind: row.get(6)?,
                        enabled: row.get::<_, i64>(7)? != 0,
                        revision: row.get(8)?,
                    };
                    let model = HarnessModel {
                        provider_id: row.get(9)?,
                        model_name: row.get(10)?,
                        display_name: row.get(11)?,
                        protocol: row.get(12)?,
                        context_window: row.get(13)?,
                        output_limit: row.get(14)?,
                        capabilities: row.get(15)?,
                        active: true,
                    };
                    Ok((provider, model))
                })
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)
        })
    }
}
