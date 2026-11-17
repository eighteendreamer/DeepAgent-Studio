//! Harness model registry: storage-side helpers (plan §3.1).
//!
//! The registry lives in `harness_providers` / `harness_models` (migration V24).
//! It is separate from the canvas provider registry in the document store.

use rusqlite::Connection;

use crate::{map_sqlite, Result};

/// Base URL of the DeepSeek route that existed before the registry. Used only
/// to create the one-time legacy provider row; runtime code reads `model_url`
/// from the registry and never this constant.
const LEGACY_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

/// Legacy secret name written by the old settings page.
const LEGACY_DEEPSEEK_KEY_NAME: &str = "deepseek_api_key";

/// Create the provider row for an existing DeepSeek key. The key is referenced
/// by name, never copied. Safe to run on every open: the insert is ignored when
/// the row already exists.
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
