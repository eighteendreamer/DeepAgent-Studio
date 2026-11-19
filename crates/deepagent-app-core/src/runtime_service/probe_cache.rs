//! Version discovery hints, shared by every SDK resolver and catalog probe.
//!
//! Resolution still visits current project/PATH/managed candidates each time.
//! Only an unchanged executable's version is reused; this is neither a cached
//! permission decision nor an executable integrity check.

use super::{probe_runtime_version, version_matches, InspectedRuntime};
use crate::atomic_file::write_file_atomically;
use deepagent_knowledge::file_lock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const SCHEMA_VERSION: u32 = 1;
const MAX_RECORD_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExecutableIdentity {
    canonical_path: PathBuf,
    length: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(unix)]
    mode: u32,
}

impl ExecutableIdentity {
    fn capture(path: &Path) -> Option<Self> {
        let canonical_path = std::fs::canonicalize(path).ok()?;
        let metadata = std::fs::metadata(&canonical_path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            canonical_path,
            length: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            #[cfg(unix)]
            mode: metadata.mode(),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ProbeRecord {
    schema_version: u32,
    executable: ExecutableIdentity,
    version: String,
}

pub(super) struct RuntimeProbeCache {
    directory: PathBuf,
    /// Also joins concurrent first probes. Never shared with run-owned objects.
    entries: Mutex<HashMap<PathBuf, ProbeRecord>>,
}

impl RuntimeProbeCache {
    pub(super) fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn inspect(
        &self,
        executable: &Path,
        requirement: &Option<String>,
    ) -> Option<InspectedRuntime> {
        let version = self.version(executable)?;
        version_matches(&version, requirement.as_deref()).then(|| InspectedRuntime {
            executable: executable.to_path_buf(),
            version,
        })
    }

    fn version(&self, executable: &Path) -> Option<String> {
        if !executable.is_file() {
            return None;
        }
        let Some(identity) = ExecutableIdentity::capture(executable) else {
            // Some filesystems cannot provide a stable timestamp. Preserve the
            // resolver's previous behavior rather than reusing an unsafe hint.
            return probe_runtime_version(executable);
        };
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(record) = entries.get(&identity.canonical_path) {
            if record.executable == identity {
                return Some(record.version.clone());
            }
        }
        let digest = Sha256::digest(identity.canonical_path.as_os_str().as_encoded_bytes());
        let path = self.directory.join(format!("{digest:x}.json"));
        if let Some(record) = read_record(&path, &identity) {
            let version = record.version.clone();
            entries.insert(identity.canonical_path, record);
            return Some(version);
        }

        // A record per executable avoids lost updates between hosts resolving
        // different projects. Recheck after the lock: another host may be done.
        let guard = match file_lock::acquire(
            &file_lock::sidecar(&path),
            file_lock::WAIT,
            "runtime version probe",
        ) {
            Ok(guard) => Some(guard),
            Err(error) => {
                tracing::warn!(%error, "runtime probe persistence unavailable; probing locally");
                None
            }
        };
        if guard.is_some() {
            if let Some(record) = read_record(&path, &identity) {
                let version = record.version.clone();
                entries.insert(identity.canonical_path, record);
                return Some(version);
            }
        }
        let started = std::time::Instant::now();
        let version = probe_runtime_version(executable)?;
        if ExecutableIdentity::capture(executable).as_ref() != Some(&identity) {
            tracing::warn!("runtime executable changed during version probe; discarding result");
            return None;
        }
        let record = ProbeRecord {
            schema_version: SCHEMA_VERSION,
            executable: identity,
            version: version.clone(),
        };
        if guard.is_some() {
            let result = serde_json::to_vec(&record)
                .map_err(deepagent_core::error::CoreError::from)
                .and_then(|bytes| write_file_atomically(&path, &bytes));
            if let Err(error) = result {
                tracing::warn!(%error, "failed to persist runtime version; retaining in-memory hint");
            }
        }
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "runtime version probed"
        );
        entries.insert(record.executable.canonical_path.clone(), record);
        Some(version)
    }
}

fn read_record(path: &Path, identity: &ExecutableIdentity) -> Option<ProbeRecord> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "failed to read runtime version record; probing again");
            return None;
        }
    };
    let record: ProbeRecord = match serde_json::from_reader(file.take(MAX_RECORD_BYTES)) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(%error, "invalid runtime version record; probing again");
            return None;
        }
    };
    (record.schema_version == SCHEMA_VERSION && record.executable == *identity).then_some(record)
}
