//! Non-authoritative successful-authentication metadata. Keeping it outside the
//! trust document prevents a timestamp write from invalidating live SSH trust.

use crate::{SshError, SshHostIdentity, SshResult};
use deepagent_core::clock::Timestamp;
use deepagent_persistence::{document_store::DocumentStore, Database};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const COLLECTION: &str = "ssh_host_activity_v1";
const MAX_BODY: usize = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activity {
    version: u32,
    identity: SshHostIdentity,
    used: BTreeMap<String, i64>,
}

fn read(db: &Database, identity: &SshHostIdentity) -> SshResult<(Option<String>, Activity)> {
    let body = DocumentStore::new(db)
        .get_body_bounded(COLLECTION, &identity.document_id()?, MAX_BODY)
        .map_err(storage_error)?;
    let activity = match &body {
        Some(body) => serde_json::from_str::<Activity>(body).map_err(|_| invalid())?,
        None => Activity {
            version: 1,
            identity: identity.clone(),
            used: BTreeMap::new(),
        },
    };
    if activity.version != 1
        || activity.identity != *identity
        || activity.used.len() > crate::host_key_store::MAX_KEYS
        || activity.used.iter().any(|(key, at)| {
            *at < 0
                || !key.strip_prefix("SHA256:").is_some_and(|hash| {
                    hash.len() == 43
                        && hash
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
                })
        })
    {
        return Err(invalid());
    }
    Ok((body, activity))
}

pub(crate) fn list(db: &Database, identity: &SshHostIdentity) -> SshResult<BTreeMap<String, i64>> {
    Ok(read(db, identity)?.1.used)
}

pub(crate) fn record(
    db: &Database,
    identity: &SshHostIdentity,
    key: &PublicKey,
    now: Timestamp,
) -> SshResult<()> {
    if now.as_millis() < 0 {
        return Err(invalid());
    }
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    for _ in 0..3 {
        let (expected, mut activity) = read(db, identity)?;
        if activity
            .used
            .get(&fingerprint)
            .is_some_and(|previous| *previous >= now.as_millis())
        {
            return Ok(()); // A delayed writer must not move the clock backwards.
        }
        if !activity.used.contains_key(&fingerprint)
            && activity.used.len() >= crate::host_key_store::MAX_KEYS
        {
            return Err(SshError::Persistence(
                "SSH host activity capacity reached; no history was removed".into(),
            ));
        }
        activity.used.insert(fingerprint.clone(), now.as_millis());
        let body = serde_json::to_string(&activity).map_err(|_| invalid())?;
        if body.len() > MAX_BODY {
            return Err(invalid());
        }
        let changed = deepagent_persistence::document_store::DocumentBodyChange {
            collection: COLLECTION,
            id: &identity.document_id()?,
            expected: expected.as_deref(),
            body: &body,
        };
        if DocumentStore::new(db)
            .compare_exchange_bodies(&[changed], now)
            .map_err(storage_error)?
        {
            return Ok(());
        }
    }
    Err(SshError::Persistence(
        "SSH host activity changed concurrently; metadata was not updated".into(),
    ))
}

fn invalid() -> SshError {
    SshError::Persistence("invalid SSH host activity metadata; trust is unaffected".into())
}
fn storage_error(error: impl std::fmt::Display) -> SshError {
    SshError::Persistence(format!("SSH host activity: {error}"))
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
    fn clock_is_monotonic_reopens_and_never_mutates_trust() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("app.sqlite");
        let db = Database::open(&path).unwrap();
        let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
        let key = public();
        record(&db, &identity, &key, Timestamp::from_millis(200)).unwrap();
        record(&db, &identity, &key, Timestamp::from_millis(100)).unwrap();
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            list(&reopened, &identity)
                .unwrap()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![200]
        );
        assert!(list(
            &db,
            &SshHostIdentity::direct("fixture.invalid", 2222).unwrap()
        )
        .unwrap()
        .is_empty());
        let store = crate::SshHostKeyStore::new(std::sync::Arc::new(reopened));
        assert!(store.list(&identity).unwrap().is_empty());
        assert!(store.audit_history(&identity).unwrap().is_empty());
    }

    #[test]
    fn concurrent_handles_preserve_distinct_keys_and_capacity_never_evicts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("app.sqlite");
        let one = Database::open(&path).unwrap();
        let two = Database::open(&path).unwrap();
        let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
        let a = public();
        let b = public();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                record(&one, &identity, &a, Timestamp::from_millis(100))
            });
            let second = scope.spawn(|| {
                barrier.wait();
                record(&two, &identity, &b, Timestamp::from_millis(200))
            });
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
        });
        assert_eq!(list(&one, &identity).unwrap().len(), 2);
        for _ in 2..crate::host_key_store::MAX_KEYS {
            record(&one, &identity, &public(), Timestamp::from_millis(300)).unwrap();
        }
        let before = list(&one, &identity).unwrap();
        assert!(record(&one, &identity, &public(), Timestamp::from_millis(400)).is_err());
        assert_eq!(list(&one, &identity).unwrap(), before);
    }
}
