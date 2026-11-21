//! User-initiated host enrollment wraps the existing connection publication path.

use tokio::time::Duration;

use crate::connection_challenge::{config_proof, HostKeyPurpose};
use crate::host_trust::HostTrust;
use crate::service::SshServiceImpl;
use crate::{
    SshClient, SshConnectionAttempt, SshConnectionConfig, SshError, SshHostIdentity,
    SshHostKeyDecision, SshHostKeyPolicy, SshHostKeyStore, SshResult,
};
use std::sync::Arc;

/// All saved/standalone connection paths share the same policy enforcement.
pub(crate) async fn connection_trust(
    cfg: &SshConnectionConfig,
    store: Option<Arc<SshHostKeyStore>>,
) -> SshResult<HostTrust> {
    if cfg.host_key_policy == SshHostKeyPolicy::Strict {
        return HostTrust::load_with_store(cfg, store).await;
    }
    let trust = HostTrust::capture(cfg, store.clone()).await?;
    if !trust.is_unknown() {
        return Ok(trust); // The transport still must match a trusted, non-revoked key.
    }
    let store = store.ok_or_else(|| {
        SshError::InvalidConfig(
            "SSH AcceptNew requires the application trust store; no key was accepted".into(),
        )
    })?;
    // Probe rejects the presented key and closes before authentication. Enrollment
    // precedes a new, pinned handshake; never offer credentials on first sight.
    let key = SshClient::probe_host_key(cfg, &trust, Some(store.clone())).await?;
    trust.persist_unknown(cfg, store, key).await
}

impl SshServiceImpl {
    pub async fn begin_connection(&self, id: &str) -> SshResult<SshConnectionAttempt> {
        self.ensure_loaded().await;
        let permit = self.connection_operations.acquire(id).await?;
        let cfg = self.operation_config(id, &permit).await?;
        let session = self.operation_session(&cfg, &permit).await?;
        self.authentication_challenges.invalidate(id).await;
        let attempt_id = self.host_challenges.reserve(id)?;
        let deadline = self.host_challenges.deadline(id, &attempt_id)?;
        let result = permit.run(async {
            if let Some(jump) = &cfg.jump {
                let hop = jump.config(&cfg);
                let trust = HostTrust::capture(&hop, self.host_key_store.clone()).await?;
                if hop.host_key_policy == SshHostKeyPolicy::Strict && trust.is_unknown() {
                    let key = cfg.timing.connecting_until(SshClient::probe_host_key(&hop, &trust, self.host_key_store.clone()), Some(deadline.instant)).await?;
                    if HostTrust::capture(&hop, self.host_key_store.clone()).await? != trust {
                        return Err(HostTrust::error(&hop, "jump trust changed during probe"));
                    }
                    let _lifecycle = self.lock_connection_generation(&permit, &session).await?;
                    let challenge = self.host_challenges.present_for_role(&cfg, &attempt_id, trust, key, HostKeyPurpose::Enrollment {can_save:self.host_key_store.is_some()}, crate::SshConnectionRole::Jump)?;
                    return Ok(SshConnectionAttempt::ConfirmationRequired {challenge:Box::new(challenge)});
                }
            }
            // Establish the jump (including MFA) before observing an unknown target.
            // The transport retains that verified route across target confirmation.
            if cfg.jump.is_some() {
                return self.connect_config_attempt(&cfg, None, None, &attempt_id, &permit).await;
            }
            let trust = HostTrust::capture(&cfg, self.host_key_store.clone()).await?;
            if cfg.host_key_policy == SshHostKeyPolicy::AcceptNew || !trust.is_unknown() || self.reusable_client(&cfg, &permit).await?.is_some() {
                self.host_challenges.ensure_current(id, &attempt_id)?;
                return self.connect_config_attempt(&cfg, None, None, &attempt_id, &permit).await;
            }
            let key = cfg.timing.connecting_until(SshClient::probe_host_key(&cfg, &trust, self.host_key_store.clone()), Some(deadline.instant)).await?;
            if HostTrust::capture(&cfg, self.host_key_store.clone()).await? != trust {
                return Err(HostTrust::error(&cfg, "trust changed during host-key probe; retry after verification"));
            }
            let _lifecycle = self.lock_connection_generation(&permit, &session).await?;
            let challenge = self.host_challenges.present(&cfg, &attempt_id, trust, key, HostKeyPurpose::Enrollment { can_save: self.host_key_store.is_some() })?;
            tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, "SSH unknown host awaits explicit verification");
            Ok(SshConnectionAttempt::ConfirmationRequired { challenge: Box::new(challenge) })
        }).await;
        if matches!(result, Err(_) | Ok(SshConnectionAttempt::Connected)) {
            self.host_challenges.discard(id, &attempt_id);
        }
        result
    }

    pub async fn respond_host_key(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
        decision: SshHostKeyDecision,
    ) -> SshResult<SshConnectionAttempt> {
        self.ensure_loaded().await;
        // No lock is held while the user is deciding. Configuration edits and
        // concurrent responses serialize only during the bounded resume phase.
        let permit = self.connection_operations.acquire(id).await?;
        let cfg = self.operation_config(id, &permit).await?;
        let session = self.operation_session(&cfg, &permit).await?;
        let pending = self.host_challenges.take(id, attempt_id, challenge_id)?;
        let result = async {
        if pending.challenge.kind != "unknown_host" {
            return Err(SshError::InvalidConfig(
                "SSH rotation requires the separate management action".into(),
            ));
        }
        let role = if pending.challenge.role == "jump" {
            crate::SshConnectionRole::Jump
        } else {
            crate::SshConnectionRole::Target
        };
        let active = role.config(&cfg)?;
        let trust = {
        // Keep local trust validation/commit in the current configuration
        // generation, then release the lock before resuming network I/O.
        let _lifecycle = self.lock_connection_generation(&permit, &session).await?;
        if config_proof(&cfg)? != pending.config_proof
            || HostTrust::capture(&active, self.host_key_store.clone()).await? != pending.trust
        {
            return Err(HostTrust::error(
                &cfg,
                "connection configuration or trust changed since the challenge",
            ));
        }
        permit.ensure_current()?;
        match decision {
            SshHostKeyDecision::Once => pending.trust.confirm_key(pending.key),
            SshHostKeyDecision::Save => {
                let store = self.application_trust_store()?;
                let identity = SshHostIdentity::for_config(&active)?;
                let key = pending.key.clone();
                tokio::task::spawn_blocking(move || {
                    store.trust_verified(&identity, &key, "first_connection_confirmed")
                })
                .await
                .map_err(|error| {
                    SshError::Internal(format!("SSH host-key confirmation task: {error}"))
                })??;
                let persisted =
                    HostTrust::load_with_store(&active, self.host_key_store.clone()).await?;
                if !persisted.same_file(&pending.trust) || !persisted.accepts(&pending.key) {
                    return Err(HostTrust::error(
                        &cfg,
                        "trust changed while saving the confirmed key",
                    ));
                }
                persisted.confirm_key(pending.key)
            }
        }
        };
        tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, ?decision, "SSH host-key decision accepted for one attempt");
        if let Some(continuation) = pending.continuation {
            let progress = permit.run(cfg.timing.connecting_until(
                continuation.resume(&cfg, trust, self.host_key_store.clone()),
                Some(pending.deadline.instant),
            )).await?;
            return self.accept_connection_progress(&cfg, attempt_id, progress, Duration::ZERO, Some(pending.deadline), &session, &permit).await;
        }
        match role {
            crate::SshConnectionRole::Target => {
                self.connect_config_attempt(&cfg, Some(trust), None, attempt_id, &permit)
                    .await
            }
            crate::SshConnectionRole::Jump => {
                self.connect_config_attempt(&cfg, None, Some(trust), attempt_id, &permit)
                    .await
            }
        }
        }.await;
        if matches!(result, Err(_) | Ok(SshConnectionAttempt::Connected)) {
            self.host_challenges.discard(id, attempt_id);
        }
        result
    }

    pub fn cancel_host_key(&self, id: &str, attempt_id: &str, challenge_id: &str) -> SshResult<()> {
        self.host_challenges.take(id, attempt_id, challenge_id)?;
        self.host_challenges.discard(id, attempt_id);
        tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, "SSH host-key challenge cancelled");
        Ok(())
    }
}
