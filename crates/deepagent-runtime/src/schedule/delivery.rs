//! Stable occurrence identity and synchronous durable admission receipt.
use super::store::ScheduledTask;
use deepagent_core::error::{CoreError, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use time::PrimitiveDateTime;

pub struct ScheduledOccurrence {
    pub run_id: String,
    pub scheduled_for: String,
    pub schedule_anchor_ms: i64,
    pub polled_at_ms: i64,
}

impl ScheduledOccurrence {
    pub(super) fn new(
        path: &Path,
        task: &ScheduledTask,
        scheduled_for: PrimitiveDateTime,
        polled_at_ms: i64,
    ) -> Result<Self> {
        let identity = std::fs::canonicalize(path).map_err(|error| {
            CoreError::Persistence(format!("resolve schedule identity: {error}"))
        })?;
        #[cfg(windows)]
        let identity = std::path::PathBuf::from(identity.to_string_lossy().to_lowercase());
        let mut hash = Sha256::new();
        hash.update(b"deepagent-scheduled-occurrence-v1\0");
        let path = identity.as_os_str().as_encoded_bytes();
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path);
        // This is transient hashing only. Never persist a prompt in origin
        // metadata or logs. The persisted anchor, not retry wall time, owns
        // identity; explicit edits create a different revision/occurrence.
        hash.update(serde_json::to_vec(task)?);
        Ok(Self {
            run_id: format!("cron-v1-{:x}", hash.finalize()),
            scheduled_for: scheduled_for.to_string(),
            schedule_anchor_ms: task.schedule_anchor_ms(),
            polled_at_ms,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DispatchReceipt {
    /// Timestamp of the original admission, not this bookkeeping retry.
    pub accepted_at_ms: i64,
}

#[derive(Debug)]
pub enum SchedulePollError {
    Read(CoreError),
    Admission(CoreError),
    Bookkeeping(CoreError),
}

impl std::fmt::Display for SchedulePollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (phase, source) = match self {
            Self::Read(error) => ("read schedule before dispatch", error),
            Self::Admission(error) => ("scheduled admission failed; no receipt", error),
            Self::Bookkeeping(error) => ("schedule bookkeeping failed after admission", error),
        };
        write!(f, "{phase}: {source}")
    }
}

impl std::error::Error for SchedulePollError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Read(error) | Self::Admission(error) | Self::Bookkeeping(error) => error,
        })
    }
}

impl From<SchedulePollError> for CoreError {
    fn from(error: SchedulePollError) -> Self {
        CoreError::other(error.to_string())
    }
}
