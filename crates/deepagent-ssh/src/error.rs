//! SSH error types.

use thiserror::Error;

pub type SshResult<T> = Result<T, SshError>;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH jump {stage}: {source}")]
    Jump {
        stage: &'static str,
        source: Box<SshError>,
    },
    #[error("SSH proxy {stage}: {reason}")]
    Proxy { stage: &'static str, reason: String },
    #[error("ssh host key verification failed for [{host}]:{port}: {reason}; known_hosts={known_hosts_path}. Verify the host fingerprint with a trusted administrator before adding or replacing its public key; never accept a changed key blindly.")]
    HostKeyVerification {
        host: String,
        port: u16,
        known_hosts_path: String,
        reason: String,
    },
    #[error("connection not found: {0}")]
    NotFound(String),
    #[error("connection already exists with id {0}")]
    AlreadyExists(String),
    #[error("ssh process error: {0}")]
    Process(#[from] std::io::Error),
    #[error("ssh command failed (exit {exit_code}): {stderr}")]
    CommandFailed { exit_code: i32, stderr: String },
    #[error("connection lost: {0}")]
    ConnectionLost(String),
    #[error("SSH service is shutting down and no longer accepts work")]
    ServiceStopped,
    #[error("authentication failed for {0}: {1}")]
    AuthFailed(String, String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("persistence error: {0}")]
    Persistence(String),
    #[error("pty error: {0}")]
    Pty(String),
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
    #[error("integrity verification failed: {0}")]
    IntegrityMismatch(String),
    #[error("internal error: {0}")]
    Internal(String),
}
