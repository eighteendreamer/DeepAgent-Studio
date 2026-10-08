//! Primary-host election for one data root.
//!
//! [`crate::AppService::recover_unfinished_runs`] closes out every non-terminal
//! run it finds in the database. That is only correct while exactly one host owns
//! the data root — and the desktop shell, the TUI and a headless `run` all open the
//! same SQLite file, so a naive second host would mark the first host's live run
//! failed at startup.
//!
//! A process therefore takes an OS advisory lock on `<data root>/host.lock` and
//! keeps it for its whole lifetime. The kernel drops it when the process exits or
//! crashes, so there is no TTL to renew, no heartbeat thread, and no stale "I am
//! still primary" record left behind by a killed host.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;

pub const LOCK_FILE: &str = "host.lock";
/// The holder's name lives in its own file: on Windows an exclusive byte-range
/// lock also blocks other processes from *reading* the locked file, so the lock
/// handle and the diagnostic text cannot share one file.
pub const OWNER_FILE: &str = "host.owner";

/// Which role this process holds for a data root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRole {
    /// This process owns the data root and may run startup recovery.
    Primary,
    /// Another live process owns it. Recovery is skipped; normal reads and writes
    /// continue (SQLite WAL + busy_timeout serialize the writers).
    Standby { holder: String },
}

impl HostRole {
    pub fn is_primary(&self) -> bool {
        matches!(self, Self::Primary)
    }
}

/// The held advisory lock. Releasing it (drop, or process exit) hands primary host
/// status to whichever process retries the claim next.
pub struct HostLock {
    file: File,
    path: PathBuf,
    owner: String,
}

/// One lock per data root per process. Without this, a second `AppService` in the
/// same process would contend with its own lock, because Windows locks are per
/// file handle rather than per process.
fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<HostLock>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<HostLock>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Claim `data_root` for this process, or report the live holder.
pub fn claim(data_root: &Path) -> Result<HostRole> {
    std::fs::create_dir_all(data_root).map_err(|error| {
        CoreError::Persistence(format!("create {}: {error}", data_root.display()))
    })?;
    let key = canonical_key(data_root);
    if held_by_this_process(&key) {
        return Ok(HostRole::Primary);
    }
    let path = key.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| CoreError::Persistence(format!("open {}: {error}", path.display())))?;
    let owner = process_owner();
    match file.try_lock_exclusive() {
        Ok(()) => {
            if let Err(error) = write_holder(&key, &owner) {
                // The owner file is diagnostic only; losing it must not make this
                // process forget that it really holds the lock.
                tracing::warn!(%error, "claimed the host lock but could not record the holder");
            }
            let shown = key.display().to_string();
            let lock = Arc::new(HostLock { file, path, owner });
            registry()
                .lock()
                .map_err(|_| CoreError::Persistence("host lock registry poisoned".to_string()))?
                .insert(key, Arc::clone(&lock));
            tracing::info!(data_root = %shown, owner = %lock.owner, "claimed primary host lock");
            Ok(HostRole::Primary)
        }
        Err(error) => {
            let holder = read_holder(&key)
                .unwrap_or_else(|| format!("unknown (lock error: {error}, no holder record yet)"));
            tracing::warn!(
                data_root = %key.display(),
                %holder,
                "another host already owns this data root; starting as standby"
            );
            Ok(HostRole::Standby { holder })
        }
    }
}

/// Give up this process's claim. Used by hosts that shut down deliberately and by
/// tests that need a clean slate.
pub fn release(data_root: &Path) {
    let key = canonical_key(data_root);
    let removed = registry()
        .lock()
        .map(|mut held| held.remove(&key))
        .unwrap_or_else(|_| {
            tracing::warn!("host lock registry poisoned during release");
            None
        });
    if let Some(lock) = removed {
        tracing::info!(path = %lock.path.display(), owner = %lock.owner, "released primary host lock");
    }
}

fn held_by_this_process(key: &Path) -> bool {
    registry()
        .lock()
        .map(|held| held.contains_key(key))
        .unwrap_or_else(|_| {
            tracing::warn!("host lock registry poisoned; assuming no local claim");
            false
        })
}

/// Locks are keyed by the resolved path so one data root spelled two ways still
/// maps to one claim inside a process.
fn canonical_key(data_root: &Path) -> PathBuf {
    std::fs::canonicalize(data_root).unwrap_or_else(|_| data_root.to_path_buf())
}

fn process_owner() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "deepagent".to_string());
    format!("{exe}#{}", std::process::id())
}

/// Record the current holder so a standby host can name it in logs and UI.
fn write_holder(data_root: &Path, owner: &str) -> Result<()> {
    let path = data_root.join(OWNER_FILE);
    std::fs::write(&path, format!("{owner}\n"))
        .map_err(|error| CoreError::Persistence(format!("record host owner: {error}")))
}

/// Best-effort name of the process that last claimed the root.
fn read_holder(data_root: &Path) -> Option<String> {
    std::fs::read_to_string(data_root.join(OWNER_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

impl Drop for HostLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(path = %self.path.display(), %error, "failed to unlock host lock");
        }
    }
}
