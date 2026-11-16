//! Application host trust uses the existing SQLite document store and its CAS.
//! Public keys are not credentials; passwords/private keys never enter this collection.

use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_persistence::{document_store::DocumentStore, Database};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{SshError, SshResult};

const COLLECTION: &str = "ssh_host_trust_v1";
const MAX_BODY: usize = 256 * 1024;
const MAX_KEYS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostIdentity {
    pub host: String,
    pub port: u16,
    route: DirectRoute,
}

// Other routes are deliberately not accepted until the transport implements them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DirectRoute {
    Direct,
}

impl SshHostIdentity {
    pub fn direct(host: &str, port: u16) -> SshResult<Self> {
        let host = host.trim();
        if host.is_empty()
            || host.len() > 255
            || port == 0
            || host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(SshError::InvalidConfig(
                "invalid SSH host trust identity".into(),
            ));
        }
        let host = host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| host.to_ascii_lowercase());
        Ok(Self {
            host,
            port,
            route: DirectRoute::Direct,
        })
    }

    fn document_id(&self) -> SshResult<String> {
        let bytes = serde_json::to_vec(self).map_err(|_| invalid_document())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHostKeyRecord {
    pub algorithm: String,
    pub fingerprint: String,
    pub source: String,
    pub trusted_at_ms: i64,
    pub revoked: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRecord {
    public_key: String,
    source: String,
    trusted_at_ms: i64,
    revoked_at_ms: Option<i64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustDocument {
    version: u32,
    identity: SshHostIdentity,
    revision: u64,
    keys: Vec<KeyRecord>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoreSnapshot {
    body: Option<String>,
    document: TrustDocument,
}

impl StoreSnapshot {
    pub(crate) fn keys(&self) -> SshResult<Vec<(PublicKey, bool)>> {
        self.document
            .keys
            .iter()
            .map(|key| {
                let mut public =
                    PublicKey::from_openssh(&key.public_key).map_err(|_| invalid_document())?;
                public.set_comment("");
                Ok((public, key.revoked_at_ms.is_some()))
            })
            .collect()
    }
}

pub struct SshHostKeyStore {
    db: Arc<Database>,
}

impl SshHostKeyStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub(crate) fn snapshot(&self, identity: &SshHostIdentity) -> SshResult<StoreSnapshot> {
        // Revalidate deserialized public identities too; callers cannot inject an alias.
        if &SshHostIdentity::direct(&identity.host, identity.port)? != identity {
            return Err(invalid_document());
        }
        let body = DocumentStore::new(&self.db)
            .get_body_bounded(COLLECTION, &identity.document_id()?, MAX_BODY)
            .map_err(storage_error)?;
        let document: TrustDocument = match &body {
            Some(body) => serde_json::from_str(body).map_err(|_| invalid_document())?,
            None => TrustDocument {
                version: 1,
                identity: identity.clone(),
                revision: 0,
                keys: Vec::new(),
            },
        };
        if document.version != 1
            || document.identity != *identity
            || document.keys.len() > MAX_KEYS
            || (body.is_some() && document.revision == 0)
        {
            return Err(invalid_document());
        }
        let snapshot = StoreSnapshot { body, document };
        let keys = snapshot.keys()?;
        for (index, (key, _)) in keys.iter().enumerate() {
            if keys[..index].iter().any(|(prior, _)| prior == key) {
                return Err(invalid_document());
            }
        }
        Ok(snapshot)
    }

    pub fn list(&self, identity: &SshHostIdentity) -> SshResult<Vec<SshHostKeyRecord>> {
        let snapshot = self.snapshot(identity)?;
        snapshot
            .document
            .keys
            .iter()
            .map(|record| {
                let key =
                    PublicKey::from_openssh(&record.public_key).map_err(|_| invalid_document())?;
                Ok(SshHostKeyRecord {
                    algorithm: key.algorithm().as_str().into(),
                    fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
                    source: record.source.clone(),
                    trusted_at_ms: record.trusted_at_ms,
                    revoked: record.revoked_at_ms.is_some(),
                })
            })
            .collect()
    }

    /// Called only after independent verification or an explicit host-key decision.
    /// Never overwrites another key of the same algorithm or revives a tombstone.
    pub fn trust_verified(
        &self,
        identity: &SshHostIdentity,
        key: &PublicKey,
        source: &str,
    ) -> SshResult<()> {
        self.trust_verified_batch(identity, std::slice::from_ref(key), source)
    }

    pub(crate) fn trust_verified_batch(
        &self,
        identity: &SshHostIdentity,
        keys: &[PublicKey],
        source: &str,
    ) -> SshResult<()> {
        if source.is_empty() || source.len() > 4096 {
            return Err(SshError::InvalidConfig("invalid SSH trust source".into()));
        }
        let mut snapshot = self.snapshot(identity)?;
        let original_count = snapshot.document.keys.len();
        for key in keys {
            let mut public = key.clone();
            public.set_comment("");
            let existing_keys = snapshot.keys()?;
            if existing_keys
                .iter()
                .any(|(existing, revoked)| existing == &public && !revoked)
            {
                continue;
            }
            if existing_keys
                .iter()
                .any(|(existing, _)| existing.algorithm() == public.algorithm())
            {
                return Err(SshError::InvalidConfig("SSH key is revoked or conflicts with existing trust; use explicit key rotation management".into()));
            }
            if snapshot.document.keys.len() >= MAX_KEYS {
                return Err(SshError::InvalidConfig(
                    "SSH trust key limit reached".into(),
                ));
            }
            snapshot.document.keys.push(KeyRecord {
                public_key: public.to_openssh().map_err(|_| invalid_document())?,
                source: source.into(),
                trusted_at_ms: SystemClock.now().as_millis(),
                revoked_at_ms: None,
            });
        }
        if snapshot.document.keys.len() == original_count {
            return Ok(());
        }
        self.commit(snapshot, "trust_verified")
    }

    pub fn revoke(&self, identity: &SshHostIdentity, fingerprint: &str) -> SshResult<()> {
        let mut snapshot = self.snapshot(identity)?;
        let index = snapshot
            .keys()?
            .iter()
            .position(|(key, _)| key.fingerprint(HashAlg::Sha256).to_string() == fingerprint)
            .ok_or_else(|| SshError::InvalidConfig("SSH trusted fingerprint not found".into()))?;
        if snapshot.document.keys[index].revoked_at_ms.is_some() {
            return Ok(());
        }
        snapshot.document.keys[index].revoked_at_ms = Some(SystemClock.now().as_millis());
        self.commit(snapshot, "revoke")
    }

    /// Commit against the snapshot taken BEFORE probing. A competing first
    /// registration (even a different algorithm) must not become a second TOFU.
    pub(crate) fn trust_unknown(
        &self,
        mut expected: StoreSnapshot,
        key: &PublicKey,
    ) -> SshResult<()> {
        if !expected.document.keys.is_empty() {
            return Err(SshError::Persistence(
                "SSH host is no longer unknown".into(),
            ));
        }
        let mut public = key.clone();
        public.set_comment("");
        expected.document.keys.push(KeyRecord {
            public_key: public.to_openssh().map_err(|_| invalid_document())?,
            source: "accept_new".into(),
            trusted_at_ms: SystemClock.now().as_millis(),
            revoked_at_ms: None,
        });
        self.commit(expected, "accept_new")
    }

    fn commit(&self, mut snapshot: StoreSnapshot, action: &'static str) -> SshResult<()> {
        snapshot.document.revision = snapshot
            .document
            .revision
            .checked_add(1)
            .ok_or_else(invalid_document)?;
        let body = serde_json::to_string(&snapshot.document).map_err(|_| invalid_document())?;
        if body.len() > MAX_BODY {
            return Err(invalid_document());
        }
        let id = snapshot.document.identity.document_id()?;
        let store = DocumentStore::new(&self.db);
        let now = SystemClock.now();
        let committed = match snapshot.body {
            Some(prior) => store.replace_body_if_unchanged(COLLECTION, &id, &prior, &body, now),
            None => store.insert_if_absent(COLLECTION, &id, &body, now),
        }
        .map_err(storage_error)?;
        if !committed {
            return Err(SshError::Persistence(
                "SSH trust changed concurrently; refresh and verify again".into(),
            ));
        }
        tracing::info!(target: "deepagent_ssh", host = snapshot.document.identity.host,
            port = snapshot.document.identity.port, action, revision = snapshot.document.revision, "SSH application trust updated");
        Ok(())
    }
}

fn invalid_document() -> SshError {
    SshError::Persistence("invalid SSH application trust document".into())
}
fn storage_error(error: impl std::fmt::Display) -> SshError {
    SshError::Persistence(format!("SSH application trust: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::{ssh_key::rand_core::OsRng, Algorithm, PrivateKey};

    fn public() -> PublicKey {
        PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .clone()
    }

    #[test]
    fn concurrent_first_enrollment_has_one_winner_and_cannot_undo_revocation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("trust.sqlite");
        let first = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
        let second = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
        let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
        let first_snapshot = first.snapshot(&identity).unwrap();
        let second_snapshot = second.snapshot(&identity).unwrap();
        let winner = public();
        first.trust_unknown(first_snapshot, &winner).unwrap();
        assert!(second
            .trust_unknown(second_snapshot.clone(), &public())
            .is_err());
        first
            .revoke(&identity, &winner.fingerprint(HashAlg::Sha256).to_string())
            .unwrap();
        assert!(second.trust_unknown(second_snapshot, &winner).is_err());
        assert!(second
            .trust_unknown(second.snapshot(&identity).unwrap(), &public())
            .is_err());
        assert_eq!(first.list(&identity).unwrap().len(), 1);
        assert!(first.list(&identity).unwrap()[0].revoked);
    }

    #[test]
    fn corrupt_unknown_version_oversized_or_cross_endpoint_documents_fail_closed() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SshHostKeyStore::new(db.clone());
        let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
        store
            .trust_verified(&identity, &public(), "fixture")
            .unwrap();
        let id = identity.document_id().unwrap();
        let original = DocumentStore::new(&db)
            .get(COLLECTION, &id)
            .unwrap()
            .unwrap()
            .body;
        let mut wrong_version: serde_json::Value = serde_json::from_str(&original).unwrap();
        wrong_version["version"] = serde_json::json!(2);
        let mut wrong_identity: serde_json::Value = serde_json::from_str(&original).unwrap();
        wrong_identity["identity"]["port"] = serde_json::json!(2222);
        let mut invalid_key: serde_json::Value = serde_json::from_str(&original).unwrap();
        invalid_key["keys"][0]["public_key"] = serde_json::json!("not-a-key");
        for body in [
            "not-json".into(),
            wrong_version.to_string(),
            wrong_identity.to_string(),
            invalid_key.to_string(),
            "PRIVATE_FIXTURE".repeat(MAX_BODY),
        ] {
            DocumentStore::new(&db)
                .put(COLLECTION, &id, &body, None, SystemClock.now())
                .unwrap();
            let error = store.list(&identity).unwrap_err().to_string();
            assert!(!error.contains("PRIVATE_FIXTURE"));
        }
    }

    #[test]
    fn conflicting_batch_has_no_partial_commit_and_stale_cas_cannot_undo_revocation() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SshHostKeyStore::new(db);
        let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
        assert!(store
            .trust_verified_batch(&identity, &[public(), public()], "conflicting fixture")
            .is_err());
        assert!(store.list(&identity).unwrap().is_empty());
        let key = public();
        store.trust_verified(&identity, &key, "fixture").unwrap();
        let stale = store.snapshot(&identity).unwrap();
        store
            .revoke(&identity, &key.fingerprint(HashAlg::Sha256).to_string())
            .unwrap();
        assert!(store.commit(stale, "must_not_restore").is_err());
        assert!(store.list(&identity).unwrap()[0].revoked);
    }
}
