//! What this program put on disk, recorded so an uninstall can reverse it.
//!
//! deepseek-harness' plugin manager has no separate manifest: its "record" is the
//! set of artifacts the install actually wrote, and uninstall edits those back.
//! This repo cannot copy that exactly because a downloaded skill, runtime or
//! plugin leaves a directory with no way to recover where it came from — so the
//! origin is recorded here, once, beside the assets themselves.
//!
//! Boundaries on purpose:
//! - `path` and `sidecars` are **relative to the data root**. Moving the root (see
//!   [`crate::migrate`]) therefore never has to rewrite the ledger, and a leaked
//!   absolute path is a validation error rather than a silent surprise.
//! - The ledger records origin and location only. Enabled/disabled state stays in
//!   `plugins/state.json`; a second copy of that would be two truths.
//! - Built-in assets shipped inside the installer are not recorded: they are
//!   removed with the installer, not by this program.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::atomic_file::write_file_atomically;
use crate::data_root::UserPaths;

pub const LEDGER_SCHEMA_VERSION: u32 = 1;
pub const LEDGER_FILE: &str = "install-ledger.json";
/// Installing is slow and uninstall must not lose an entry to it. dsh gives the
/// plugin manager two minutes for the same reason.
const LOCK_WAIT: Duration = Duration::from_secs(120);
const LOCK_RETRY: Duration = Duration::from_millis(50);

/// What kind of asset an entry describes. Uninstall deletes them in reverse:
/// regenerable caches first, installed assets last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    Cache,
    McpPayload,
    Sdk,
    Runtime,
    Skill,
    Plugin,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::McpPayload => "mcp_payload",
            Self::Sdk => "sdk",
            Self::Runtime => "runtime",
            Self::Skill => "skill",
            Self::Plugin => "plugin",
        }
    }
}

/// Where the bytes came from. `dir` covers an already-downloaded folder, which is
/// what both hosts actually hand to an installer today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSourceKind {
    Dir,
    Zip,
    Registry,
    Bundled,
    /// Written by the user through the editor instead of downloaded.
    Authored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSource {
    pub kind: InstallSourceKind,
    /// Path, URL or registry name, exactly as the caller supplied it.
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    pub kind: InstallKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: InstallSource,
    /// Relative to the data root.
    pub path: String,
    /// Extra directories the same install produced (an unpacked runtime payload,
    /// a plugin's data directory).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Epoch milliseconds, the same clock `plugins/state.json` records.
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

impl LedgerEntry {
    pub fn absolute_path(&self, root: &Path) -> Result<PathBuf> {
        resolve_relative(root, &self.path, &self.id)
    }

    pub fn sidecar_paths(&self, root: &Path) -> Result<Vec<PathBuf>> {
        self.sidecars
            .iter()
            .map(|sidecar| resolve_relative(root, sidecar, &self.id))
            .collect()
    }
}

/// Join a recorded relative path under the root, refusing anything that escapes.
///
/// `has_root` rather than `is_absolute` on purpose: on Windows `/etc/passwd` is
/// not "absolute" yet still resolves off the drive root.
fn resolve_relative(root: &Path, relative: &str, id: &str) -> Result<PathBuf> {
    let candidate = Path::new(relative);
    if candidate.has_root()
        || candidate
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(CoreError::invalid(format!(
            "ledger entry {id} is not rooted in the data directory: {relative}"
        )));
    }
    Ok(root.join(candidate))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallLedger {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<LedgerEntry>,
}

impl Default for InstallLedger {
    fn default() -> Self {
        Self {
            version: LEDGER_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl InstallLedger {
    /// Entries in the order an uninstall should delete them: regenerable caches
    /// first (lowest kind rank), installed assets last, and within a kind the
    /// newest install first.
    pub fn uninstall_order(&self) -> Vec<&LedgerEntry> {
        let mut ordered = self.entries.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| right.installed_at.cmp(&left.installed_at))
        });
        ordered
    }
}

/// Read and write the ledger under an OS lock.
///
/// The lock is a real advisory lock, so a crashed holder releases it with its
/// process; there is no stale-owner takeover to invent here (same reasoning as
/// [`crate::host_lock`]). The owner file exists only so a waiting host can name
/// who it is waiting for.
pub struct LedgerStore {
    path: PathBuf,
    lock_path: PathBuf,
    owner_path: PathBuf,
    root: PathBuf,
}

impl LedgerStore {
    pub fn open(paths: &UserPaths) -> Self {
        let path = paths.install_ledger();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(LEDGER_FILE);
        Self {
            lock_path: path.with_file_name(format!("{name}.lock")),
            owner_path: path.with_file_name(format!("{name}.lock.owner")),
            root: paths.root.clone(),
            path,
        }
    }

    /// Explicit path, used by tests and by a host that moved its root.
    pub fn at(root: impl Into<PathBuf>, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(LEDGER_FILE)
            .to_string();
        Self {
            lock_path: path.with_file_name(format!("{name}.lock")),
            owner_path: path.with_file_name(format!("{name}.lock.owner")),
            root: root.into(),
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<InstallLedger> {
        if !self.path.is_file() {
            return Ok(InstallLedger::default());
        }
        self.read_at()
    }

    fn read_at(&self) -> Result<InstallLedger> {
        let text = std::fs::read_to_string(&self.path).map_err(|error| {
            CoreError::Persistence(format!("read ledger {}: {error}", self.path.display()))
        })?;
        let ledger: InstallLedger = serde_json::from_str(&text)
            .map_err(|error| CoreError::invalid(format!("parse ledger: {error}")))?;
        if ledger.version > LEDGER_SCHEMA_VERSION {
            return Err(CoreError::invalid(format!(
                "ledger {} was written by a newer version (schema {}, this build reads {LEDGER_SCHEMA_VERSION})",
                self.path.display(),
                ledger.version
            )));
        }
        Ok(ledger)
    }

    /// Upsert `entry`, keeping the original `installed_at` on re-install.
    pub fn record(&self, mut entry: LedgerEntry) -> Result<LedgerEntry> {
        entry.absolute_path(&self.root)?;
        entry.sidecar_paths(&self.root)?;
        self.with_locked(|ledger| {
            let now = now_millis();
            match ledger
                .entries
                .iter_mut()
                .find(|existing| existing.kind == entry.kind && existing.id == entry.id)
            {
                Some(existing) => {
                    entry.installed_at = existing.installed_at.clone();
                    entry.last_updated = Some(now);
                    *existing = entry.clone();
                }
                None => {
                    entry.installed_at = now.clone();
                    entry.last_updated = None;
                    ledger.entries.push(entry.clone());
                }
            }
            Ok(entry)
        })
    }

    /// Drop the entry for `(kind, id)` and return it, so the caller can delete
    /// exactly what was recorded.
    pub fn forget(&self, kind: InstallKind, id: &str) -> Result<Option<LedgerEntry>> {
        self.with_locked(|ledger| {
            let position = ledger
                .entries
                .iter()
                .position(|entry| entry.kind == kind && entry.id == id);
            Ok(position.map(|position| ledger.entries.remove(position)))
        })
    }

    pub fn entries(&self) -> Result<Vec<LedgerEntry>> {
        Ok(self.read()?.entries)
    }

    /// Drop the entries whose recorded path is gone and return them.
    ///
    /// A directory disappears between installs when a user deletes it by hand or
    /// an install half-fails. Leaving the row behind would send the uninstaller
    /// chasing paths that no longer exist and hide which installs actually landed,
    /// so existence is re-checked rather than assumed. Only the main `path` counts:
    /// a plugin's data directory is created lazily.
    pub fn prune_missing(&self) -> Result<Vec<LedgerEntry>> {
        self.with_locked(|ledger| {
            let mut removed = Vec::new();
            let mut stale = Vec::new();
            for (position, entry) in ledger.entries.iter().enumerate() {
                if !entry.absolute_path(&self.root)?.exists() {
                    stale.push(position);
                }
            }
            for position in stale.into_iter().rev() {
                removed.push(ledger.entries.remove(position));
            }
            Ok(removed)
        })
    }

    /// One read-modify-write under the exclusive lock.
    pub fn with_locked<T>(
        &self,
        change: impl FnOnce(&mut InstallLedger) -> Result<T>,
    ) -> Result<T> {
        let guard = LedgerLock::acquire(&self.lock_path, &self.owner_path)?;
        let mut ledger = if self.path.is_file() {
            self.read_at()?
        } else {
            InstallLedger::default()
        };
        let result = change(&mut ledger)?;
        ledger.version = LEDGER_SCHEMA_VERSION;
        write_file_atomically(
            &self.path,
            serde_json::to_string_pretty(&ledger)
                .map_err(CoreError::from)?
                .as_bytes(),
        )?;
        drop(guard);
        Ok(result)
    }
}

/// The held ledger lock. Unlocking happens when it drops.
struct LedgerLock {
    file: std::fs::File,
    owner_path: PathBuf,
}

impl LedgerLock {
    fn acquire(path: &Path, owner_path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CoreError::Persistence(format!("create {}: {error}", parent.display()))
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| {
                CoreError::Persistence(format!("open ledger lock {}: {error}", path.display()))
            })?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(_) if Instant::now() >= deadline => {
                    let holder = std::fs::read_to_string(owner_path)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    return Err(CoreError::other(format!(
                        "another install or uninstall is holding {} (holder: {}); \
                         waited {} seconds",
                        path.display(),
                        if holder.is_empty() {
                            "unknown"
                        } else {
                            &holder
                        },
                        LOCK_WAIT.as_secs()
                    )));
                }
                Err(error) if is_lock_contention(&error) => {
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => {
                    return Err(CoreError::Persistence(format!(
                        "lock ledger {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        // Diagnostics only: on Windows an exclusive lock also blocks readers of
        // the locked file, so the holder name lives in its own sidecar.
        let _ = std::fs::write(owner_path, format!("{}\n", process_owner()));
        Ok(Self {
            file,
            owner_path: owner_path.to_path_buf(),
        })
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(path = %self.owner_path.display(), %error, "failed to release the ledger lock");
        }
    }
}

/// fs2 reports contention as `WouldBlock` on Unix; on Windows the byte-range lock
/// failure surfaces as a raw error, so both are treated as "wait and retry".
fn is_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33)
}

fn process_owner() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "deepagent".to_string());
    format!("{exe}#{}", std::process::id())
}

fn now_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        InstallKind, InstallLedger, InstallSource, InstallSourceKind, LedgerEntry, LedgerStore,
        LEDGER_SCHEMA_VERSION,
    };
    use std::path::{Path, PathBuf};

    fn entry(id: &str, kind: InstallKind) -> LedgerEntry {
        LedgerEntry {
            id: id.to_string(),
            kind,
            version: Some("1.0.0".to_string()),
            source: InstallSource {
                kind: InstallSourceKind::Dir,
                spec: format!("/downloads/{id}"),
            },
            path: format!("files/plugins/{id}"),
            sidecars: vec![format!("files/plugins/data/{id}")],
            sha256: Some("abc".to_string()),
            installed_at: String::new(),
            last_updated: None,
        }
    }

    fn store(dir: &Path) -> LedgerStore {
        LedgerStore::at(dir, dir.join(super::LEDGER_FILE))
    }

    #[test]
    fn a_recorded_entry_reads_back_field_for_field() {
        // This file is written by the installer and read by the uninstaller, in a
        // different process. Round-trip here is the contract test.
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        let written = ledger.record(entry("helper", InstallKind::Plugin)).unwrap();

        let read = ledger.entries().unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, written.id);
        assert_eq!(read[0].kind, written.kind);
        assert_eq!(read[0].path, written.path);
        assert_eq!(read[0].sidecars, written.sidecars);
        assert_eq!(read[0].sha256.as_deref(), Some("abc"));
        assert_eq!(read[0].source.kind, InstallSourceKind::Dir);
        assert_eq!(read[0].installed_at, written.installed_at);
        assert!(!read[0].installed_at.is_empty(), "a timestamp is recorded");
    }

    #[test]
    fn reinstalling_the_same_id_keeps_one_entry_and_the_original_date() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        let first = ledger.record(entry("helper", InstallKind::Plugin)).unwrap();

        let mut upgraded = entry("helper", InstallKind::Plugin);
        upgraded.version = Some("2.0.0".to_string());
        upgraded.path = "files/plugins/helper-v2".to_string();
        let second = ledger.record(upgraded).unwrap();

        let read = ledger.entries().unwrap();
        assert_eq!(
            read.len(),
            1,
            "an update replaces the entry, it does not stack"
        );
        assert_eq!(read[0].installed_at, first.installed_at);
        assert_eq!(
            read[0].last_updated, second.last_updated,
            "the re-install stamps its own time, not the original"
        );
        assert!(
            read[0].last_updated.is_some(),
            "a re-install stamps its write"
        );
        assert_eq!(read[0].version.as_deref(), Some("2.0.0"));
        assert_eq!(read[0].path, "files/plugins/helper-v2");
    }

    #[test]
    fn forget_returns_the_recorded_paths_and_removes_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        ledger.record(entry("pdf", InstallKind::Skill)).unwrap();

        let removed = ledger.forget(InstallKind::Skill, "pdf").unwrap().unwrap();
        assert_eq!(
            removed.absolute_path(Path::new(tmp.path())).unwrap(),
            tmp.path().join("files/plugins/pdf")
        );
        assert_eq!(
            removed.sidecar_paths(Path::new(tmp.path())).unwrap().len(),
            1
        );
        assert!(ledger.entries().unwrap().is_empty());
        assert!(ledger.forget(InstallKind::Skill, "pdf").unwrap().is_none());
    }

    #[test]
    fn absolute_or_escaping_paths_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());

        let mut escaped = entry("bad", InstallKind::Plugin);
        escaped.path = "../../Windows/system32".to_string();
        assert!(ledger
            .record(escaped)
            .unwrap_err()
            .to_string()
            .contains("not rooted"));

        let mut absolute = entry("worse", InstallKind::Plugin);
        absolute.path = "/etc/passwd".to_string();
        assert!(ledger
            .record(absolute)
            .unwrap_err()
            .to_string()
            .contains("not rooted"));
        assert!(
            !tmp.path().join(super::LEDGER_FILE).exists(),
            "a refused record must not create a ledger"
        );
    }

    #[test]
    fn kind_and_id_are_the_identity_not_path() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        ledger.record(entry("shared", InstallKind::Skill)).unwrap();
        ledger.record(entry("shared", InstallKind::Plugin)).unwrap();
        assert_eq!(ledger.entries().unwrap().len(), 2);
        ledger.forget(InstallKind::Skill, "shared").unwrap();
        let rest = ledger.entries().unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].kind, InstallKind::Plugin);
    }

    #[test]
    fn uninstall_order_clears_caches_before_installed_assets() {
        let mut ledger = InstallLedger::default();
        ledger.entries.push(entry("plugin", InstallKind::Plugin));
        ledger.entries.push(entry("cache", InstallKind::Cache));
        ledger.entries.push(entry("runtime", InstallKind::Runtime));
        let order: Vec<InstallKind> = ledger.uninstall_order().iter().map(|e| e.kind).collect();
        assert_eq!(
            order,
            vec![
                InstallKind::Cache,
                InstallKind::Runtime,
                InstallKind::Plugin,
            ]
        );
    }

    #[test]
    fn a_ledger_from_a_newer_build_is_not_guessed_at() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(super::LEDGER_FILE);
        std::fs::write(
            &path,
            format!(
                r#"{{"version":{},"entries":[]}}"#,
                LEDGER_SCHEMA_VERSION + 1
            ),
        )
        .unwrap();
        let error = store(tmp.path()).read().unwrap_err();
        assert!(error.to_string().contains("newer version"), "{error}");
    }

    #[test]
    fn a_recorded_asset_that_vanished_is_pruned_not_chased() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        for id in ["kept", "gone"] {
            std::fs::create_dir_all(tmp.path().join("files/plugins").join(id)).unwrap();
            ledger.record(entry(id, InstallKind::Plugin)).unwrap();
        }
        std::fs::remove_dir_all(tmp.path().join("files/plugins/gone")).unwrap();

        let removed = ledger.prune_missing().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "gone");
        let rest = ledger.entries().unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, "kept");
    }

    #[test]
    fn a_missing_ledger_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = store(tmp.path());
        assert!(ledger.read().unwrap().entries.is_empty());
        assert!(ledger.entries().unwrap().is_empty());
        assert!(!path_of(tmp.path()).exists());
    }

    fn path_of(dir: &Path) -> PathBuf {
        dir.join(super::LEDGER_FILE)
    }
}
