//! Timing policy shared by connection attempts and the native SSH protocol engine.

use crate::{SshError, SshResult};
use deepagent_core::clock::{Clock, SystemClock};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

/// Keep one wall-clock projection for the complete monotonic challenge lifetime.
#[derive(Clone, Copy)]
pub(crate) struct ChallengeDeadline {
    pub instant: tokio::time::Instant,
    pub expires_at_ms: i64,
}

impl ChallengeDeadline {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            instant: tokio::time::Instant::now() + ttl,
            expires_at_ms: SystemClock.now().as_millis() + ttl.as_millis() as i64,
        }
    }

    pub(crate) fn capped_at(self, instant: tokio::time::Instant) -> Self {
        let instant = instant.min(self.instant);
        let reduction_ms = self
            .instant
            .duration_since(instant)
            .as_nanos()
            .div_ceil(1_000_000) as i64;
        Self {
            instant,
            expires_at_ms: self.expires_at_ms.saturating_sub(reduction_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshConnectionTiming {
    pub connect_timeout_secs: u32,
    pub inactivity_timeout_secs: Option<u32>,
    pub keepalive_interval_secs: Option<u32>,
    pub keepalive_max: u32,
}

impl Default for SshConnectionTiming {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 20,
            inactivity_timeout_secs: None,
            keepalive_interval_secs: None,
            keepalive_max: 3,
        }
    }
}

impl SshConnectionTiming {
    pub(crate) fn validate(&self) -> SshResult<()> {
        for (name, value) in [
            ("connect_timeout_secs", Some(self.connect_timeout_secs)),
            ("inactivity_timeout_secs", self.inactivity_timeout_secs),
            ("keepalive_interval_secs", self.keepalive_interval_secs),
        ] {
            if value.is_some_and(|value| !(1..=86_400).contains(&value)) {
                return Err(SshError::InvalidConfig(format!(
                    "{name} must be an integer between 1 and 86400 seconds"
                )));
            }
        }
        if !(1..=100).contains(&self.keepalive_max) {
            return Err(SshError::InvalidConfig(
                "keepalive_max must be an integer between 1 and 100".into(),
            ));
        }
        Ok(())
    }

    /// Bound active network work. Human challenge waiting has a separate deadline.
    pub(crate) fn connecting<T>(
        &self,
        future: impl Future<Output = SshResult<T>>,
    ) -> impl Future<Output = SshResult<T>> {
        self.connecting_until(future, None)
    }

    pub(crate) fn connecting_until<T>(
        &self,
        future: impl Future<Output = SshResult<T>>,
        challenge_deadline: Option<tokio::time::Instant>,
    ) -> impl Future<Output = SshResult<T>> {
        let timing = *self;
        // Allocate before creating the async wrapper: connection futures contain
        // the complete route/authentication state and must not grow every caller.
        let future = Box::pin(future);
        async move {
            timing.validate()?;
            let duration = Duration::from_secs(u64::from(timing.connect_timeout_secs));
            let network_deadline = tokio::time::Instant::now() + duration;
            let deadline = challenge_deadline
                .map(|deadline| deadline.min(network_deadline))
                .unwrap_or(network_deadline);
            tokio::time::timeout_at(deadline, future)
                .await
                .map_err(|_| {
                    if challenge_deadline.is_some_and(|deadline| deadline <= network_deadline) {
                        SshError::InvalidConfig("SSH connection attempt expired".into())
                    } else {
                        SshError::Timeout(duration)
                    }
                })?
        }
    }

    pub(crate) fn protocol_config(&self) -> russh::client::Config {
        russh::client::Config {
            inactivity_timeout: self
                .inactivity_timeout_secs
                .map(|s| Duration::from_secs(u64::from(s))),
            keepalive_interval: self
                .keepalive_interval_secs
                .map(|s| Duration::from_secs(u64::from(s))),
            keepalive_max: self.keepalive_max as usize,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_network_operation() -> impl Future<Output = SshResult<()>> {
        let payload = [0_u8; 32 * 1024];
        async move {
            tokio::task::yield_now().await;
            std::hint::black_box(payload);
            Ok(())
        }
    }

    #[test]
    fn network_deadline_does_not_embed_the_operations_storage() {
        let timing = SshConnectionTiming::default();
        let operation = large_network_operation();
        assert!(std::mem::size_of_val(&operation) >= 32 * 1024);
        let bounded = timing.connecting(operation);
        let size = std::mem::size_of_val(&bounded);
        assert!(size < 4096, "network deadline future occupies {size} bytes");
    }

    #[test]
    fn challenge_deadline_does_not_embed_the_operations_storage() {
        let timing = SshConnectionTiming::default();
        let bounded = timing.connecting_until(
            large_network_operation(),
            Some(tokio::time::Instant::now() + Duration::from_secs(120)),
        );
        let size = std::mem::size_of_val(&bounded);
        assert!(
            size < 4096,
            "challenge deadline future occupies {size} bytes"
        );
    }
}
