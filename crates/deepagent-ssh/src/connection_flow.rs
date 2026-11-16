//! User-initiated host enrollment wraps the existing connection publication path.

use tokio::time::{timeout, Duration};

use crate::connection_challenge::config_proof;
use crate::host_trust::HostTrust;
use crate::service::SshServiceImpl;
use crate::{
    SshClient, SshConnectionAttempt, SshError, SshHostIdentity, SshHostKeyDecision, SshResult,
    SshServiceHandle,
};

impl SshServiceImpl {
    pub async fn begin_connection(&self, id: &str) -> SshResult<SshConnectionAttempt> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        let cfg = self.config_for(id).await?;
        let attempt_id = self.host_challenges.reserve(id)?;
        let result = async {
            let trust = HostTrust::capture(&cfg, self.host_key_store.clone()).await?;
            if !trust.is_unknown() || self.reusable_client(&cfg).await?.is_some() {
                self.host_challenges.ensure_current(id, &attempt_id)?;
                self.connect_config(&cfg, None).await?;
                return Ok(SshConnectionAttempt::Connected);
            }
            let key = timeout(Duration::from_secs(20), SshClient::probe_host_key(&cfg, &trust)).await
                .map_err(|_| SshError::Timeout(Duration::from_secs(20)))??;
            if HostTrust::capture(&cfg, self.host_key_store.clone()).await? != trust {
                return Err(HostTrust::error(&cfg, "trust changed during host-key probe; retry after verification"));
            }
            let challenge = self.host_challenges.present(&cfg, &attempt_id, trust, key, self.host_key_store.is_some())?;
            tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, "SSH unknown host awaits explicit verification");
            Ok(SshConnectionAttempt::ConfirmationRequired { challenge: Box::new(challenge) })
        }.await;
        if !matches!(
            result,
            Ok(SshConnectionAttempt::ConfirmationRequired { .. })
        ) {
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
    ) -> SshResult<SshServiceHandle> {
        self.ensure_loaded().await;
        // No lock is held while the user is deciding. Configuration edits and
        // concurrent responses serialize only during the bounded resume phase.
        let _lifecycle = self.lifecycle.write().await;
        let pending = self.host_challenges.take(id, attempt_id, challenge_id)?;
        let cfg = self.config_for(id).await?;
        if config_proof(&cfg)? != pending.config_proof
            || HostTrust::capture(&cfg, self.host_key_store.clone()).await? != pending.trust
        {
            return Err(HostTrust::error(
                &cfg,
                "connection configuration or trust changed since the challenge",
            ));
        }
        let trust = match decision {
            SshHostKeyDecision::Once => pending.trust.confirm_key(pending.key),
            SshHostKeyDecision::Save => {
                let store = self.application_trust_store()?;
                let identity = SshHostIdentity::direct(&cfg.host, cfg.port)?;
                let key = pending.key.clone();
                tokio::task::spawn_blocking(move || {
                    store.trust_verified(&identity, &key, "first_connection_confirmed")
                })
                .await
                .map_err(|error| {
                    SshError::Internal(format!("SSH host-key confirmation task: {error}"))
                })??;
                let persisted =
                    HostTrust::load_with_store(&cfg, self.host_key_store.clone()).await?;
                if !persisted.same_file(&pending.trust) || !persisted.accepts(&pending.key) {
                    return Err(HostTrust::error(
                        &cfg,
                        "trust changed while saving the confirmed key",
                    ));
                }
                persisted.confirm_key(pending.key)
            }
        };
        tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, ?decision, "SSH host-key decision accepted for one attempt");
        self.connect_config(&cfg, Some(trust)).await
    }

    pub fn cancel_host_key(&self, id: &str, attempt_id: &str, challenge_id: &str) -> SshResult<()> {
        self.host_challenges.take(id, attempt_id, challenge_id)?;
        tracing::info!(target: "deepagent_ssh", connection_id = id, attempt_id, "SSH host-key challenge cancelled");
        Ok(())
    }
}
