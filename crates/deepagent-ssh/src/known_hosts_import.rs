//! Bounded, reviewed known_hosts enrollment. Never used as a connection verifier.
use std::{collections::BTreeMap, io::Read};

use russh::keys::{
    ssh_key::known_hosts::{HostPatterns, KnownHosts, Marker},
    HashAlg, PublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    service::SshServiceImpl, SshConnectionConfig, SshError, SshHostIdentity, SshHostKeyStore,
    SshResult,
};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshKnownHostsImportStatus {
    Ready,
    Duplicate,
    ExistingTrust,
    Malformed,
    HashedHost,
    UnsupportedPattern,
    CertAuthority,
    RevokedMarker,
    RevokedSourceKey,
    SourceConflict,
    InvalidEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshKnownHostsImportRow {
    pub line_number: usize,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub algorithm: Option<String>,
    pub fingerprint: Option<String>,
    pub status: SshKnownHostsImportStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshKnownHostsImportPreview {
    pub connection_id: String,
    pub host: String,
    pub port: u16,
    pub known_hosts_path: String,
    pub source_sha256: String,
    pub rows: Vec<SshKnownHostsImportRow>,
    pub importable: usize,
    pub skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct SshKnownHostsImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub preview: SshKnownHostsImportPreview,
    pub disconnect_failed_ids: Vec<String>,
}

struct Candidate {
    row: SshKnownHostsImportRow,
    identity: Option<SshHostIdentity>,
    key: Option<PublicKey>,
}

impl Candidate {
    fn skipped(line_number: usize, status: SshKnownHostsImportStatus) -> Self {
        Self {
            row: SshKnownHostsImportRow {
                line_number,
                host: None,
                port: None,
                algorithm: None,
                fingerprint: None,
                status,
            },
            identity: None,
            key: None,
        }
    }
}

struct ImportPlan {
    preview: SshKnownHostsImportPreview,
    groups: Vec<(SshHostIdentity, Vec<PublicKey>)>,
}

fn endpoint(pattern: &str) -> Option<SshHostIdentity> {
    let (host, port) = if let Some(rest) = pattern.strip_prefix('[') {
        let (host, port) = rest.rsplit_once("]:")?;
        (host, port.parse::<u16>().ok()?)
    } else {
        if pattern.contains(':') && pattern.parse::<std::net::Ipv6Addr>().is_err() {
            return None;
        }
        (pattern, 22)
    };
    SshHostIdentity::direct(host, port).ok()
}

fn push(rows: &mut Vec<Candidate>, row: Candidate) -> SshResult<()> {
    if rows.len() == MAX_ROWS {
        return Err(SshError::InvalidConfig(
            "known_hosts import exceeds 4096 expanded rows".into(),
        ));
    }
    rows.push(row);
    Ok(())
}

fn parse(bytes: &[u8]) -> SshResult<(Vec<Candidate>, Vec<PublicKey>)> {
    use SshKnownHostsImportStatus as Status;
    let mut rows = Vec::new();
    let mut revoked = Vec::new();
    for (index, bytes) in bytes.split(|byte| *byte == b'\n').enumerate() {
        let line_number = index + 1;
        let line = match std::str::from_utf8(bytes) {
            Ok(line) => line.trim(),
            Err(_) => {
                let start = bytes
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .unwrap_or(bytes.len());
                if bytes[start..].starts_with(b"@revoked") {
                    return Err(SshError::InvalidConfig(
                        "invalid revoked known_hosts entry; repair the source before import".into(),
                    ));
                }
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::Malformed),
                )?;
                continue;
            }
        };
        let entry = match KnownHosts::new(line).next() {
            None => continue,
            Some(Ok(entry)) => entry,
            Some(Err(_)) if line.starts_with("@revoked") => {
                return Err(SshError::InvalidConfig(
                    "invalid revoked known_hosts entry; repair the source before import".into(),
                ))
            }
            Some(Err(_)) => {
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::Malformed),
                )?;
                continue;
            }
        };
        let mut public = entry.public_key().clone();
        public.set_comment("");
        match entry.marker() {
            Some(Marker::Revoked) => {
                revoked.push(public);
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::RevokedMarker),
                )?;
                continue;
            }
            Some(Marker::CertAuthority) => {
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::CertAuthority),
                )?;
                continue;
            }
            None => {}
        }
        let HostPatterns::Patterns(patterns) = entry.host_patterns() else {
            push(
                &mut rows,
                Candidate::skipped(line_number, Status::HashedHost),
            )?;
            continue;
        };
        for pattern in patterns {
            if pattern.contains(['*', '?', '!', '|']) {
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::UnsupportedPattern),
                )?;
                continue;
            }
            let Some(identity) = endpoint(pattern) else {
                push(
                    &mut rows,
                    Candidate::skipped(line_number, Status::InvalidEndpoint),
                )?;
                continue;
            };
            push(
                &mut rows,
                Candidate {
                    row: SshKnownHostsImportRow {
                        line_number,
                        host: Some(identity.host.clone()),
                        port: Some(identity.port),
                        algorithm: Some(public.algorithm().as_str().into()),
                        fingerprint: Some(public.fingerprint(HashAlg::Sha256).to_string()),
                        status: Status::Ready,
                    },
                    identity: Some(identity),
                    key: Some(public.clone()),
                },
            )?;
        }
    }
    Ok((rows, revoked))
}

fn classify(
    rows: &mut [Candidate],
    revoked: &[PublicKey],
    store: &SshHostKeyStore,
) -> SshResult<Vec<(SshHostIdentity, Vec<PublicKey>)>> {
    use SshKnownHostsImportStatus as Status;
    let mut targets = BTreeMap::<(String, u16), Vec<usize>>::new();
    for (index, candidate) in rows.iter_mut().enumerate() {
        if let (Some(identity), Some(key)) = (&candidate.identity, &candidate.key) {
            // A source-revoked key cannot be laundered via a plain duplicate.
            // We conservatively block that public key across this source file.
            if revoked.contains(key) {
                candidate.row.status = Status::RevokedSourceKey;
                continue;
            }
            targets
                .entry((identity.host.clone(), identity.port))
                .or_default()
                .push(index);
        }
    }
    let mut groups = Vec::new();
    for ((host, port), indices) in targets {
        let identity = SshHostIdentity::direct(&host, port)?;
        if !store.snapshot(&identity)?.keys()?.is_empty() {
            for index in indices {
                rows[index].row.status = Status::ExistingTrust;
            }
            continue;
        }
        let conflict = indices.iter().enumerate().any(|(at, index)| {
            indices[..at].iter().any(|prior| {
                let a = rows[*index].key.as_ref().unwrap();
                let b = rows[*prior].key.as_ref().unwrap();
                a.algorithm() == b.algorithm() && a != b
            })
        });
        if conflict {
            for index in indices {
                rows[index].row.status = Status::SourceConflict;
            }
            continue;
        }
        let mut keys = Vec::new();
        for index in indices {
            let key = rows[index].key.as_ref().unwrap();
            if keys.contains(key) {
                rows[index].row.status = Status::Duplicate;
            } else {
                keys.push(key.clone());
            }
        }
        groups.push((identity, keys));
    }
    Ok(groups)
}

fn prepare(config: &SshConnectionConfig, store: &SshHostKeyStore) -> SshResult<ImportPlan> {
    let path = config
        .known_hosts_path
        .as_deref()
        .ok_or_else(|| SshError::InvalidConfig("known_hosts source is not configured".into()))?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((MAX_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| {
            SshError::Persistence(format!("known_hosts import source: {:?}", error.kind()))
        })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(SshError::InvalidConfig(
            "known_hosts import exceeds 1 MiB".into(),
        ));
    }
    let (mut rows, revoked) = parse(&bytes)?;
    let groups = classify(&mut rows, &revoked, store)?;
    let importable = rows
        .iter()
        .filter(|row| row.row.status == SshKnownHostsImportStatus::Ready)
        .count();
    let skipped = rows.len() - importable;
    Ok(ImportPlan {
        preview: SshKnownHostsImportPreview {
            connection_id: config.id.clone(),
            host: config.host.clone(),
            port: config.port,
            known_hosts_path: path.into(),
            source_sha256: format!("{:x}", Sha256::digest(&bytes)),
            rows: rows.into_iter().map(|candidate| candidate.row).collect(),
            importable,
            skipped,
        },
        groups,
    })
}

impl SshServiceImpl {
    pub async fn preview_known_hosts_import(
        &self,
        id: &str,
    ) -> SshResult<SshKnownHostsImportPreview> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        tokio::task::spawn_blocking(move || prepare(&config, &store).map(|plan| plan.preview))
            .await
            .map_err(|error| SshError::Internal(format!("SSH import preview task: {error}")))?
    }

    pub async fn import_known_hosts(
        &self,
        id: &str,
        expected: SshKnownHostsImportPreview,
    ) -> SshResult<SshKnownHostsImportResult> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.write().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        let (mut result, identities) = tokio::task::spawn_blocking(move || {
            let plan = prepare(&config, &store)?;
            if plan.preview != expected {
                return Err(SshError::InvalidConfig(
                    "known_hosts import preview is stale; inspect and verify again".into(),
                ));
            }
            store.import_unknown_batch(&plan.groups, &plan.preview.known_hosts_path)?;
            let identities = plan
                .groups
                .into_iter()
                .map(|(identity, _)| identity)
                .collect::<Vec<_>>();
            Ok((
                SshKnownHostsImportResult {
                    imported: plan.preview.importable,
                    skipped: plan.preview.skipped,
                    preview: plan.preview,
                    disconnect_failed_ids: Vec::new(),
                },
                identities,
            ))
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH import commit task: {error}")))??;
        for connection in self.list_connections().await {
            if SshHostIdentity::direct(&connection.host, connection.port)
                .ok()
                .is_some_and(|identity| identities.contains(&identity))
            {
                if let Err(error) = self.disconnect(&connection.id).await {
                    tracing::warn!(target: "deepagent_ssh", connection_id = connection.id, %error, "SSH trust committed but connection cleanup failed");
                    result.disconnect_failed_ids.push(connection.id);
                }
            }
        }
        Ok(result)
    }
}
