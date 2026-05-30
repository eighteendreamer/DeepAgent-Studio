//! Secret storage for the API key.
//!
//! The API key is **never** persisted to the SQLite database (which holds only
//! the public [`ModelCatalog`]). Instead it goes through a [`SecretStore`]:
//!
//! - [`KeychainStore`] (feature `keychain`) — OS keychain: Windows Credential
//!   Manager / macOS Keychain / Linux Secret Service. The production path.
//! - [`EnvSecretStore`] — reads `DEEPSEEK_API_KEY` from the environment
//!   (read-only); good for CI / headless / 12-factor deployments.
//! - [`MemorySecretStore`] — in-process only; the default for tests so no real
//!   OS keychain is touched.
//!
//! This mirrors how Claude Code keeps credentials out of plaintext on disk.

use std::collections::HashMap;
use std::sync::Mutex;

use deepagent_core::error::{CoreError, Result};

/// Stores and retrieves a secret (the API key) by a logical name.
pub trait SecretStore: Send + Sync {
    /// Persist `secret` under `name`.
    fn set(&self, name: &str, secret: &str) -> Result<()>;
    /// Retrieve the secret for `name`, or `None` if absent.
    fn get(&self, name: &str) -> Result<Option<String>>;
    /// Delete the secret for `name`. No-op if absent.
    fn delete(&self, name: &str) -> Result<()>;
}

/// In-process secret store (no persistence). Default for tests.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    inner: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| CoreError::other("secret store poisoned"))?
            .insert(name.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| CoreError::other("secret store poisoned"))?
            .get(name)
            .cloned())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| CoreError::other("secret store poisoned"))?
            .remove(name);
        Ok(())
    }
}

/// Reads a secret from the `DEEPSEEK_API_KEY` environment variable.
///
/// Read-only: `set`/`delete` are no-ops (the environment is owned by the
/// operator). Useful for CI and headless deployments where the key is injected
/// via env rather than a UI.
#[derive(Debug, Default)]
pub struct EnvSecretStore {
    var: String,
}

impl EnvSecretStore {
    /// Read from the default `DEEPSEEK_API_KEY` variable.
    pub fn new() -> Self {
        Self {
            var: "DEEPSEEK_API_KEY".to_string(),
        }
    }

    /// Read from a custom variable name.
    pub fn with_var(var: impl Into<String>) -> Self {
        Self { var: var.into() }
    }
}

impl SecretStore for EnvSecretStore {
    fn set(&self, _name: &str, _secret: &str) -> Result<()> {
        // Environment is operator-owned; storing is intentionally a no-op.
        Ok(())
    }

    fn get(&self, _name: &str) -> Result<Option<String>> {
        Ok(std::env::var(&self.var)
            .ok()
            .filter(|v| !v.trim().is_empty()))
    }

    fn delete(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// OS keychain-backed secret store (feature `keychain`).
#[cfg(feature = "keychain")]
#[derive(Debug)]
pub struct KeychainStore {
    service: String,
}

#[cfg(feature = "keychain")]
impl KeychainStore {
    /// Build a keychain store under the given service name (e.g.
    /// `"deepagent-studio"`). Entries are keyed by `(service, name)`.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, name)
            .map_err(|e| CoreError::other(format!("keychain entry error: {e}")))
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeychainStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        self.entry(name)?
            .set_password(secret)
            .map_err(|e| CoreError::other(format!("keychain set error: {e}")))
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        match self.entry(name)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CoreError::other(format!("keychain get error: {e}"))),
        }
    }

    fn delete(&self, name: &str) -> Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CoreError::other(format!("keychain delete error: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemorySecretStore::new();
        assert!(s.get("api_key").unwrap().is_none());
        s.set("api_key", "sk-secret").unwrap();
        assert_eq!(s.get("api_key").unwrap().as_deref(), Some("sk-secret"));
        s.delete("api_key").unwrap();
        assert!(s.get("api_key").unwrap().is_none());
    }

    #[test]
    fn env_store_reads_variable() {
        // Use a unique var to avoid clobbering the real one.
        let store = EnvSecretStore::with_var("DEEPAGENT_TEST_KEY_XYZ");
        assert!(store.get("api_key").unwrap().is_none());
        std::env::set_var("DEEPAGENT_TEST_KEY_XYZ", "sk-from-env");
        assert_eq!(
            store.get("api_key").unwrap().as_deref(),
            Some("sk-from-env")
        );
        // set/delete are no-ops, must not error.
        store.set("api_key", "x").unwrap();
        store.delete("api_key").unwrap();
        std::env::remove_var("DEEPAGENT_TEST_KEY_XYZ");
    }

    #[test]
    fn env_store_empty_is_none() {
        let store = EnvSecretStore::with_var("DEEPAGENT_DEFINITELY_UNSET_VAR_123");
        assert!(store.get("api_key").unwrap().is_none());
    }
}
