//! Secret storage for the API key.
//!
//! API keys are persisted as authenticated ciphertext in SQLite. The encrypted
//! master-key record is stored separately in SQLite and is protected by a
//! device-bound wrapping secret from the OS keychain.
//!
//! - [`SqliteSecretStore`] — production encrypted persistence.
//! - [`KeychainStore`] (feature `keychain`) — protects the wrapping secret.
//! - [`EnvSecretStore`] — reads `DEEPSEEK_API_KEY` from the environment
//!   (read-only); good for CI / headless / 12-factor deployments.
//! - [`MemorySecretStore`] — in-process only; the default for tests so no real
//!   OS keychain is touched.
//!
//! Plaintext secret material is never written to SQLite.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::Database;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

const MASTER_RECORD: &str = "secret_master_key";
const MASTER_WRAP_NAME: &str = "deepagent_secret_wrap_key";
const NONCE_LEN: usize = 12;

/// Stores and retrieves a secret (the API key) by a logical name.
pub trait SecretStore: Send + Sync {
    /// Persist `secret` under `name`.
    fn set(&self, name: &str, secret: &str) -> Result<()>;
    /// Retrieve the secret for `name`, or `None` if absent.
    fn get(&self, name: &str) -> Result<Option<String>>;
    /// Delete the secret for `name`. No-op if absent.
    fn delete(&self, name: &str) -> Result<()>;
}

/// SQLite-backed encrypted secret store. The API-key records and the
/// separately named master-key record are stored in SQLite. The master key is
/// itself encrypted with a device-bound wrapping secret supplied by the
/// platform keychain, so a copied database is not sufficient to decrypt it.
pub struct SqliteSecretStore {
    db: Arc<Database>,
    wrapping: Arc<dyn SecretStore>,
}

impl SqliteSecretStore {
    pub fn new(db: Arc<Database>, wrapping: Arc<dyn SecretStore>) -> Self {
        Self { db, wrapping }
    }

    fn wrapping_key(&self) -> Result<[u8; 32]> {
        let value = match self.wrapping.get(MASTER_WRAP_NAME)? {
            Some(value) if !value.trim().is_empty() => value,
            _ => {
                let value = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    Aes256Gcm::generate_key(&mut OsRng),
                );
                self.wrapping.set(MASTER_WRAP_NAME, &value)?;
                value
            }
        };
        Ok(Sha256::digest(value.as_bytes()).into())
    }

    fn random_nonce() -> [u8; NONCE_LEN] {
        let generated = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&generated);
        nonce
    }

    fn decrypt(
        key: &[u8; 32],
        nonce: &[u8],
        ciphertext: &[u8],
        associated_name: &str,
        label: &str,
    ) -> Result<Vec<u8>> {
        if nonce.len() != NONCE_LEN {
            return Err(CoreError::other(format!(
                "invalid SQLite {label} nonce length"
            )));
        }
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: associated_name.as_bytes(),
                },
            )
            .map_err(|_| CoreError::other(format!("failed to decrypt SQLite {label}")))
    }

    fn master_key(&self) -> Result<[u8; 32]> {
        let wrapping = self.wrapping_key()?;
        self.db.with_conn(|conn| {
            let record = conn
                .query_row(
                "SELECT ciphertext, nonce FROM secret_records WHERE name=?1",
                [MASTER_RECORD],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            if let Some((ciphertext, nonce)) = record {
                let plaintext = Self::decrypt(
                    &wrapping,
                    &nonce,
                    &ciphertext,
                    MASTER_RECORD,
                    "secret master key",
                )?;
                return plaintext
                    .try_into()
                    .map_err(|_| CoreError::other("invalid SQLite secret master key length"));
            }

            let master: [u8; 32] = Aes256Gcm::generate_key(&mut OsRng).into();
            let nonce = Self::random_nonce();
            let ciphertext = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&wrapping))
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: master.as_ref(),
                        aad: MASTER_RECORD.as_bytes(),
                    },
                )
                .map_err(|_| CoreError::other("failed to encrypt SQLite secret master key"))?;
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO secret_records (name,ciphertext,nonce,key_version,updated_at) VALUES (?1,?2,?3,1,?4)",
                rusqlite::params![MASTER_RECORD, ciphertext, nonce.to_vec(), now_ms()],
            )
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            if inserted == 1 {
                return Ok(master);
            }

            // Another process may have inserted the record between our read
            // and write. Always use the committed record in that case.
            let (ciphertext, nonce) = conn
                .query_row(
                    "SELECT ciphertext, nonce FROM secret_records WHERE name=?1",
                    [MASTER_RECORD],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            Self::decrypt(
                &wrapping,
                &nonce,
                &ciphertext,
                MASTER_RECORD,
                "secret master key",
            )?
                .try_into()
                .map_err(|_| CoreError::other("invalid SQLite secret master key length"))
        })
    }
}

impl SecretStore for SqliteSecretStore {
    fn set(&self, name: &str, secret: &str) -> Result<()> {
        if name == MASTER_RECORD || name == MASTER_WRAP_NAME {
            return Err(CoreError::invalid("reserved SQLite secret name"));
        }
        let key = self.master_key()?;
        let nonce = Self::random_nonce();
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| CoreError::other("failed to encrypt SQLite secret"))?;
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO secret_records (name,ciphertext,nonce,key_version,updated_at) VALUES (?1,?2,?3,1,?4) ON CONFLICT(name) DO UPDATE SET ciphertext=excluded.ciphertext, nonce=excluded.nonce, updated_at=excluded.updated_at",
                rusqlite::params![name, ciphertext, nonce.to_vec(), now_ms()],
            )
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            Ok(())
        })
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        if name == MASTER_RECORD || name == MASTER_WRAP_NAME {
            return Ok(None);
        }
        let Some((ciphertext, nonce)) = self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT ciphertext, nonce FROM secret_records WHERE name=?1",
                [name],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Persistence(e.to_string()))
        })?
        else {
            return Ok(None);
        };
        let key = self.master_key()?;
        let plaintext = Self::decrypt(&key, &nonce, &ciphertext, name, "secret")?;
        String::from_utf8(plaintext)
            .map(Some)
            .map_err(|_| CoreError::other("SQLite secret is not UTF-8"))
    }

    fn delete(&self, name: &str) -> Result<()> {
        if name == MASTER_RECORD || name == MASTER_WRAP_NAME {
            return Err(CoreError::invalid("reserved SQLite secret name"));
        }
        self.db.with_conn(|conn| {
            conn.execute("DELETE FROM secret_records WHERE name=?1", [name])
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            Ok(())
        })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
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

    #[test]
    fn sqlite_store_encrypts_roundtrips_and_separates_master_record() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let wrapping: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        let store = SqliteSecretStore::new(db.clone(), wrapping.clone());

        store.set("deepseek_api_key", "sk-super-secret").unwrap();
        assert_eq!(
            store.get("deepseek_api_key").unwrap().as_deref(),
            Some("sk-super-secret")
        );
        assert!(wrapping.get(MASTER_WRAP_NAME).unwrap().is_some());

        db.with_conn(|conn| {
            let records: i64 = conn
                .query_row("SELECT COUNT(*) FROM secret_records", [], |row| row.get(0))
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            let plaintext: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM secret_records WHERE CAST(ciphertext AS TEXT) LIKE '%sk-super-secret%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            assert_eq!(records, 2);
            assert_eq!(plaintext, 0);
            Ok(())
        })
        .unwrap();

        store.delete("deepseek_api_key").unwrap();
        assert!(store.get("deepseek_api_key").unwrap().is_none());
    }

    #[test]
    fn sqlite_store_binds_ciphertext_to_its_logical_field() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SqliteSecretStore::new(db.clone(), Arc::new(MemorySecretStore::new()));
        store.set("deepseek_api_key", "deepseek-secret").unwrap();
        store.set("vision_api_key", "vision-secret").unwrap();

        db.with_conn(|conn| {
            conn.execute(
                "UPDATE secret_records SET ciphertext=(SELECT ciphertext FROM secret_records WHERE name='deepseek_api_key'), nonce=(SELECT nonce FROM secret_records WHERE name='deepseek_api_key') WHERE name='vision_api_key'",
                [],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();

        assert!(store.get("vision_api_key").is_err());
    }
}
