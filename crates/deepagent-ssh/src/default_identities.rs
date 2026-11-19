//! Metadata-only discovery; selecting a candidate uses the existing KeyFile flow.
use crate::{SshError, SshResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEFAULT_IDENTITY_NAMES: [&str; 3] = ["id_ed25519", "id_rsa", "id_ecdsa"];
const MAX_IDENTITY_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshDefaultIdentityState {
    Available,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDefaultIdentity {
    pub name: String,
    pub path: String,
    pub state: SshDefaultIdentityState,
    pub reason: Option<String>,
}

/// Check the supported default names only. Availability is metadata, not a key-validity verdict.
/// No directory enumeration, file reads, key parsing, signing, or remote connection occurs here.
pub fn discover_default_ssh_identities_in(home: &Path) -> SshResult<Vec<SshDefaultIdentity>> {
    if !home.is_absolute() {
        return Err(SshError::InvalidConfig(
            "SSH identity discovery requires an absolute home directory".into(),
        ));
    }
    DEFAULT_IDENTITY_NAMES
        .into_iter()
        .map(|name| {
            let path = home.join(".ssh").join(name);
            let display_path = path.to_str().ok_or_else(|| {
                SshError::InvalidConfig("SSH identity path is not valid Unicode".into())
            })?;
            let (state, reason) = match std::fs::metadata(&path) {
                Ok(metadata) => {
                    let reason = if !metadata.is_file() {
                        Some("candidate is not a regular file")
                    } else if metadata.len() == 0 {
                        Some("candidate file is empty")
                    } else if metadata.len() > MAX_IDENTITY_BYTES {
                        Some("candidate file exceeds the 1 MiB discovery limit")
                    } else {
                        None
                    };
                    (
                        if reason.is_some() {
                            SshDefaultIdentityState::Unavailable
                        } else {
                            SshDefaultIdentityState::Available
                        },
                        reason.map(str::to_owned),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    (SshDefaultIdentityState::Missing, None)
                }
                Err(error) => (
                    SshDefaultIdentityState::Unavailable,
                    Some(error.to_string()),
                ),
            };
            Ok(SshDefaultIdentity {
                name: name.to_owned(),
                path: display_path.to_owned(),
                state,
                reason,
            })
        })
        .collect()
}

/// Resolve the actual user profile on each request; never cache a prior discovery result.
pub async fn discover_default_ssh_identities() -> SshResult<Vec<SshDefaultIdentity>> {
    tokio::task::spawn_blocking(|| {
        let home = dirs::home_dir()
            .ok_or_else(|| SshError::InvalidConfig("SSH home directory is unavailable".into()))?;
        discover_default_ssh_identities_in(&home)
    })
    .await
    .map_err(|error| SshError::Internal(format!("discover default SSH identities: {error}")))?
}
