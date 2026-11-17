//! Domain trust history, not another runtime/run event store. Each entry is
//! committed inside the existing trust document CAS; reads never open SSH.

use crate::service::SshServiceImpl;
use crate::{SshError, SshHostIdentity, SshHostKeyRotationRecord, SshResult};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_AUDIT_RECORDS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshHostTrustAction {
    TrustVerified,
    Revoke,
    AcceptNew,
    KnownHostsImport,
    ExplicitHostKeyRotation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostKeyFingerprint {
    pub algorithm: String,
    pub fingerprint: String,
}

impl From<&PublicKey> for SshHostKeyFingerprint {
    fn from(key: &PublicKey) -> Self {
        Self {
            algorithm: key.algorithm().as_str().into(),
            fingerprint: fingerprint(key),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostTrustAuditRecord {
    pub revision: u64,
    pub at_ms: i64,
    pub action: SshHostTrustAction,
    pub trusted_fingerprints: Vec<String>,
    pub revoked_fingerprints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<SshHostKeyFingerprint>,
}

impl SshHostTrustAuditRecord {
    pub(crate) fn from_legacy(rotation: SshHostKeyRotationRecord) -> Self {
        Self {
            revision: rotation.revision,
            at_ms: rotation.at_ms,
            action: SshHostTrustAction::ExplicitHostKeyRotation,
            trusted_fingerprints: vec![rotation.fingerprint.clone()],
            revoked_fingerprints: rotation.retired_fingerprints,
            replacement: Some(SshHostKeyFingerprint {
                algorithm: rotation.algorithm,
                fingerprint: rotation.fingerprint,
            }),
        }
    }

    pub(crate) fn rotation(&self) -> Option<SshHostKeyRotationRecord> {
        self.replacement
            .as_ref()
            .map(|key| SshHostKeyRotationRecord {
                revision: self.revision,
                at_ms: self.at_ms,
                algorithm: key.algorithm.clone(),
                fingerprint: key.fingerprint.clone(),
                retired_fingerprints: self.revoked_fingerprints.clone(),
            })
    }
}

pub(crate) enum TrustMutation {
    Verified,
    Revoke,
    AcceptNew,
    FileImport,
    Rotation(SshHostKeyFingerprint),
}

impl TrustMutation {
    pub(crate) fn action(&self) -> SshHostTrustAction {
        match self {
            Self::Verified => SshHostTrustAction::TrustVerified,
            Self::Revoke => SshHostTrustAction::Revoke,
            Self::AcceptNew => SshHostTrustAction::AcceptNew,
            Self::FileImport => SshHostTrustAction::KnownHostsImport,
            Self::Rotation(_) => SshHostTrustAction::ExplicitHostKeyRotation,
        }
    }

    pub(crate) fn record(
        &self,
        revision: u64,
        at_ms: i64,
        before: &[(PublicKey, bool)],
        after: &[(PublicKey, bool)],
    ) -> SshHostTrustAuditRecord {
        let changed = |revoked: bool| {
            after
                .iter()
                .filter(|(key, state)| {
                    *state == revoked
                        && !before
                            .iter()
                            .any(|(old, prior)| old == key && *prior == revoked)
                })
                .map(|(key, _)| fingerprint(key))
                .collect()
        };
        SshHostTrustAuditRecord {
            revision,
            at_ms,
            action: self.action(),
            trusted_fingerprints: changed(false),
            revoked_fingerprints: changed(true),
            replacement: match self {
                Self::Rotation(key) => Some(key.clone()),
                _ => None,
            },
        }
    }
}

fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub(crate) fn valid_history(
    records: &[SshHostTrustAuditRecord],
    revision: u64,
    keys: &[(PublicKey, bool)],
) -> bool {
    if records.len() > MAX_AUDIT_RECORDS {
        return false;
    }
    let known = keys
        .iter()
        .map(|(key, _)| SshHostKeyFingerprint::from(key))
        .collect::<Vec<_>>();
    let mut previous_revision = 0;
    for record in records {
        if record.revision <= previous_revision
            || record.revision > revision
            || record.at_ms < 0
            || record.trusted_fingerprints.len() > keys.len()
            || record.revoked_fingerprints.len() > keys.len()
            || record.replacement.is_some()
                != (record.action == SshHostTrustAction::ExplicitHostKeyRotation)
            || record
                .replacement
                .as_ref()
                .is_some_and(|key| !known.contains(key))
        {
            return false;
        }
        let affected = record
            .trusted_fingerprints
            .iter()
            .chain(&record.revoked_fingerprints)
            .collect::<Vec<_>>();
        for (index, key) in affected.iter().enumerate() {
            if !known.iter().any(|known| &known.fingerprint == *key)
                || affected[..index].contains(key)
            {
                return false;
            }
        }
        previous_revision = record.revision;
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHostTrustAudit {
    pub connection_id: String,
    pub host: String,
    pub port: u16,
    pub route: &'static str,
    pub revision: u64,
    pub records: Vec<SshHostTrustAuditRecord>,
    pub last_used_at_ms: Option<i64>,
    pub activity_error: Option<String>,
}

impl SshServiceImpl {
    pub async fn host_trust_audit(&self, id: &str) -> SshResult<SshHostTrustAudit> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        let identity = SshHostIdentity::direct(&config.host, config.port)?;
        let (revision, records, last_used_at_ms, activity_error) =
            tokio::task::spawn_blocking(move || {
                let (revision, records) = store.audit_snapshot(&identity)?;
                let (last_used_at_ms, activity_error) = match store.activity(&identity) {
                    Ok(used) => (used.values().copied().max(), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                Ok::<_, SshError>((revision, records, last_used_at_ms, activity_error))
            })
            .await
            .map_err(|error| SshError::Internal(format!("SSH trust audit reader: {error}")))??;
        Ok(SshHostTrustAudit {
            connection_id: config.id,
            host: config.host,
            port: config.port,
            route: "direct",
            revision,
            records,
            last_used_at_ms,
            activity_error,
        })
    }
}
