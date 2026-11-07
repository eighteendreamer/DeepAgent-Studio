//! One byte counter for provider media, before decoding or writing staging.
use deepagent_core::error::{CoreError, Result};

#[derive(Clone, Copy)]
pub(super) enum MediaLimitKind {
    Image,
    Video,
    AudioInput,
    SpeechOutput,
}

impl MediaLimitKind {
    fn code(self) -> &'static str {
        match self {
            Self::Image => "ImageArtifactTooLarge",
            Self::Video => "VideoArtifactTooLarge",
            Self::AudioInput => "RemoteAudioTooLarge",
            Self::SpeechOutput => "SpeechArtifactTooLarge",
        }
    }
}

pub(super) struct MediaByteLimit {
    kind: MediaLimitKind,
    max_bytes: u64,
    received: u64,
}

impl MediaByteLimit {
    pub(super) fn new(kind: MediaLimitKind, max_bytes: u64) -> Self {
        Self {
            kind,
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
            CoreError::invalid(format!(
                "{}: received byte count overflow",
                self.kind.code()
            ))
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
                kind = self.kind.code(),
                "media byte limit exceeded"
            );
            let message = match self.kind {
                MediaLimitKind::Image => format!(
                    "ImageArtifactTooLarge: provider image exceeds the {}-byte output limit",
                    self.max_bytes
                ),
                MediaLimitKind::Video => format!(
                    "VideoArtifactTooLarge: provider video exceeds the {}-byte download limit",
                    self.max_bytes
                ),
                MediaLimitKind::SpeechOutput => format!(
                    "SpeechArtifactTooLarge: provider speech exceeds the {}-byte output limit",
                    self.max_bytes
                ),
                // Preserve the existing remote-audio-input contract, not the
                // independent provider speech-synthesis output policy.
                MediaLimitKind::AudioInput => {
                    "remote audio exceeds the 32 MiB download budget".into()
                }
            };
            return Err(CoreError::invalid(message));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_and_actual_lengths_share_the_same_inclusive_limit() {
        for kind in [
            MediaLimitKind::Image,
            MediaLimitKind::Video,
            MediaLimitKind::AudioInput,
            MediaLimitKind::SpeechOutput,
        ] {
            let mut limit = MediaByteLimit::new(kind, 64);
            limit.check_declared(None).unwrap();
            limit.check_declared(Some(64)).unwrap();
            assert!(limit.check_declared(Some(65)).is_err());
            assert_eq!(limit.received, 0);
            limit.accept(63).unwrap();
            limit.accept(1).unwrap();
            assert!(limit.accept(1).is_err());
            assert_eq!(limit.received, 64);
        }
    }

    #[test]
    fn counter_overflow_is_rejected_without_mutation() {
        let mut limit = MediaByteLimit::new(MediaLimitKind::Video, u64::MAX);
        limit.received = u64::MAX;
        assert!(limit.accept(1).is_err());
        assert_eq!(limit.received, u64::MAX);
    }
}
