//! One provider-video byte policy shared by the broker and compatibility path.
use deepagent_core::error::{CoreError, Result};

pub(super) struct VideoDownloadLimit {
    max_bytes: u64,
    received: u64,
}

impl VideoDownloadLimit {
    pub(super) fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            received: 0,
        }
    }

    pub(super) fn check_declared(&self, length: Option<u64>) -> Result<()> {
        if let Some(length) = length {
            self.check(length, "content_length")?;
        }
        Ok(())
    }

    /// Called before writing each chunk; a refused chunk never reaches staging.
    pub(super) fn accept(&mut self, size: usize) -> Result<()> {
        let total = self.received.checked_add(size as u64).ok_or_else(|| {
            CoreError::invalid("VideoArtifactTooLarge: received byte count overflow")
        })?;
        self.check(total, "chunks")?;
        self.received = total;
        Ok(())
    }

    fn check(&self, size: u64, source: &'static str) -> Result<()> {
        if size > self.max_bytes {
            tracing::warn!(
                limit_bytes = self.max_bytes,
                observed_bytes = size,
                source,
                "provider video download limit exceeded"
            );
            return Err(CoreError::invalid(format!(
                "VideoArtifactTooLarge: provider video exceeds the {}-byte download limit",
                self.max_bytes
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_and_actual_lengths_share_the_same_inclusive_limit() {
        let mut limit = VideoDownloadLimit::new(64);
        limit.check_declared(None).unwrap();
        limit.check_declared(Some(64)).unwrap();
        assert!(limit.check_declared(Some(65)).is_err());
        assert_eq!(limit.received, 0);
        limit.accept(63).unwrap();
        limit.accept(1).unwrap();
        assert!(limit.accept(1).is_err());
        assert_eq!(limit.received, 64);
    }

    #[test]
    fn counter_overflow_is_rejected_without_mutation() {
        let mut limit = VideoDownloadLimit::new(u64::MAX);
        limit.received = u64::MAX;
        assert!(limit.accept(1).is_err());
        assert_eq!(limit.received, u64::MAX);
    }
}
