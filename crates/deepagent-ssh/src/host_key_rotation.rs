//! Explicit management workflow; normal connection decisions cannot rotate keys.
//! Shares enrollment's bounded, expiring, single-use challenge registry.

use crate::connection_challenge::{config_proof, HostKeyPurpose, PendingHostKey};
use crate::connection_operation::ConnectionPermit;
use crate::host_trust::HostTrust;
use crate::service::SshServiceImpl;
use crate::timing::ChallengeDeadline;
use crate::{
    SshClient, SshConnectionConfig, SshError, SshHostIdentity, SshHostKeyChallenge,
    SshHostKeyRotationRecord, SshKnownHostKey, SshResult,
};
use russh::keys::{HashAlg, PublicKey};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SshHostKeyRotationPreview {
    pub challenge: SshHostKeyChallenge,
    pub previous_keys: Vec<SshKnownHostKey>,
}

/// A committed receipt is never converted to a write failure by later cleanup.
#[derive(Debug, Clone, Serialize)]
pub struct SshHostKeyRotationResult {
    pub audit: SshHostKeyRotationRecord,
    pub disconnect_failed_ids: Vec<String>,
}

async fn probe(
    config: &crate::SshConnectionConfig,
    store: std::sync::Arc<crate::SshHostKeyStore>,
    deadline: ChallengeDeadline,
) -> SshResult<PublicKey> {
    config
        .timing
        .connecting_until(
            SshClient::observe_host_key(config, Some(store)),
            Some(deadline.instant),
        )
        .await
}

impl SshServiceImpl {
    pub async fn begin_host_key_rotation(&self, id: &str) -> SshResult<SshHostKeyRotationPreview> {
        self.ensure_loaded().await;
        let permit = self.connection_operations.acquire(id).await?;
        let config = self.operation_config(id, &permit).await?;
        let store = self.application_trust_store()?;
        let attempt_id = self.host_challenges.reserve(id)?;
        let result = permit
            .run(async {
                let trust = HostTrust::capture_rotation(&config, store.clone()).await?;
                if trust.is_unknown() {
                    return Err(HostTrust::error(
                        &config,
                        "unknown identity requires first-connection verification, not rotation",
                    ));
                }
                let key = probe(
                    &config,
                    store.clone(),
                    self.host_challenges.deadline(id, &attempt_id)?,
                )
                .await?;
                let previous_keys = trust
                    .rotation_previous(&config, &key)?
                    .into_iter()
                    .map(|(line_number, key)| SshKnownHostKey {
                        line_number,
                        algorithm: key.algorithm().as_str().into(),
                        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
                    })
                    .collect();
                if HostTrust::capture_rotation(&config, store).await? != trust {
                    return Err(HostTrust::error(
                        &config,
                        "trust changed during rotation probe; verify again",
                    ));
                }
                let _lifecycle = self.lifecycle.read().await;
                permit.ensure_current()?;
                let challenge = self.host_challenges.present(
                    &config,
                    &attempt_id,
                    trust,
                    key,
                    HostKeyPurpose::Rotation,
                )?;
                Ok(SshHostKeyRotationPreview {
                    challenge,
                    previous_keys,
                })
            })
            .await;
        if result.is_err() {
            self.host_challenges.discard(id, &attempt_id);
        }
        result
    }

    pub async fn confirm_host_key_rotation(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<SshHostKeyRotationResult> {
        self.ensure_loaded().await;
        let permit = self.connection_operations.acquire(id).await?;
        let config = self.operation_config(id, &permit).await?;
        let pending = {
            let _lifecycle = self.lifecycle.read().await;
            permit.ensure_current()?;
            self.host_challenges.take(id, attempt_id, challenge_id)?
        };
        let result = async {
            if pending.challenge.kind != "key_rotation" {
                return Err(SshError::InvalidConfig(
                    "not an SSH rotation management challenge".into(),
                ));
            }
            let store = self.application_trust_store()?;
            // Both observations reject the key without authenticating.
            permit
                .run(async {
                    if config_proof(&config)? != pending.config_proof
                        || HostTrust::capture_rotation(&config, store.clone()).await?
                            != pending.trust
                    {
                        return Err(HostTrust::error(
                            &config,
                            "configuration or trust changed since rotation preview",
                        ));
                    }
                    if probe(&config, store, pending.deadline).await? != pending.key {
                        return Err(HostTrust::error(
                            &config,
                            "server key changed before rotation commit",
                        ));
                    }
                    Ok(())
                })
                .await?;
            self.commit_host_key_rotation(&config, pending, &permit)
                .await
        }
        .await;
        self.host_challenges.discard(id, attempt_id);
        result
    }

    async fn commit_host_key_rotation(
        &self,
        config: &SshConnectionConfig,
        pending: PendingHostKey,
        permit: &ConnectionPermit,
    ) -> SshResult<SshHostKeyRotationResult> {
        // No network I/O owns this writer. Once the local CAS starts, retain
        // its receipt and finish alias cleanup even if disconnect is requested.
        let _lifecycle = permit
            .run(async { Ok(self.lifecycle.write().await) })
            .await?;
        permit.ensure_current()?;
        self.host_challenges
            .ensure_current(&config.id, &pending.challenge.attempt_id)?;
        let store = self.application_trust_store()?;
        let current = self.config_for(&config.id).await?;
        if config_proof(&current)? != pending.config_proof
            || HostTrust::capture_rotation(&current, store.clone()).await? != pending.trust
        {
            return Err(HostTrust::error(
                config,
                "configuration or trust changed before rotation commit",
            ));
        }
        permit.ensure_current()?;
        self.host_challenges
            .ensure_current(&config.id, &pending.challenge.attempt_id)?;
        let previous = pending
            .trust
            .rotation_previous(config, &pending.key)?
            .into_iter()
            .map(|(_, key)| key)
            .collect::<Vec<_>>();
        let snapshot = pending.trust.store_snapshot()?;
        let identity = SshHostIdentity::for_config(config)?;
        let audit = tokio::task::spawn_blocking(move || {
            store.rotate_verified(snapshot, &previous, &pending.key)
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH rotation commit task: {error}")))??;
        let mut result = SshHostKeyRotationResult {
            audit,
            disconnect_failed_ids: Vec::new(),
        };
        // Trust identity is shared by aliases. Close their PTY/SFTP clients too.
        for connection in self.list_connections().await {
            if self
                .config_for(&connection.id)
                .await
                .ok()
                .and_then(|config| SshHostIdentity::for_config(&config).ok())
                .as_ref()
                == Some(&identity)
            {
                if let Err(error) = self.disconnect_locked(&connection.id).await {
                    tracing::warn!(target: "deepagent_ssh", connection_id = connection.id, %error, "SSH rotation committed but cleanup failed");
                    result.disconnect_failed_ids.push(connection.id);
                }
            }
        }
        Ok(result)
    }
}
