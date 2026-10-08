//! Reverse of an install: delete what this program put on disk, by the record.
//!
//! The install ledger is the only input here. Nothing is discovered by scanning
//! directories or by re-deriving paths from a registry, because both of those
//! have been wrong before (a changed `dest_subdir` left a runtime undeletable).
//! A record whose path escapes the data root is refused, not cleaned up.
//!
//! Two scopes, because the user's data and the program's assets are different
//! things:
//! - [`uninstall_installed`] removes every recorded asset. Sessions, uploaded
//!   attachments, recordings and the knowledge vault survive.
//! - [`purge`] additionally drops the database and the managed tree. The OS
//!   keychain entry is left alone on purpose: removing it needs the same unlock
//!   path that stored it, and losing the wrapper key would leave the encrypted
//!   records unreadable rather than merely uninstalled.

use std::path::{Path, PathBuf};

use deepagent_core::error::Result;

use crate::atomic_file::{remove_dir_all_with_retry, remove_file_with_retry};
use crate::data_root::{UserPaths, DATABASE_FILE};
use crate::install_ledger::{InstallKind, LedgerEntry, LedgerStore};

/// One thing the uninstaller removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedItem {
    pub id: String,
    pub kind: InstallKind,
    pub path: PathBuf,
}

/// What the uninstaller did, including what it could not do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallOutcome {
    pub removed: Vec<RemovedItem>,
    /// The scope-only deletions of [`purge`]: the database and the managed tree.
    /// They are not ledger assets, so they are reported apart from `removed`.
    pub purged: Vec<PathBuf>,
    /// `(id, reason)` for every asset still on disk. Non-empty means the run did
    /// not finish and the caller must report failure rather than claim success.
    pub failed: Vec<(String, String)>,
    /// Records that named a path outside the data root, or user data. Never
    /// deleted, and always reported.
    pub refused: Vec<(String, String)>,
}

impl UninstallOutcome {
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty() && self.refused.is_empty()
    }
}

fn store(target: &UserPaths) -> LedgerStore {
    LedgerStore::at(target.root.clone(), target.install_ledger())
}

/// Everything the ledger says this program installed, in deletion order.
pub fn plan(target: &UserPaths) -> Result<Vec<LedgerEntry>> {
    Ok(store(target)
        .read()?
        .uninstall_order()
        .into_iter()
        .cloned()
        .collect())
}

/// Delete every recorded asset. The database and the user's own files stay.
pub fn uninstall_installed(target: &UserPaths) -> Result<UninstallOutcome> {
    let ledger = store(target);
    let entries = ledger
        .read()?
        .uninstall_order()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut outcome = UninstallOutcome::default();
    for entry in entries {
        remove_entry(target, &ledger, entry, &mut outcome);
    }
    Ok(outcome)
}

fn remove_entry(
    target: &UserPaths,
    ledger: &LedgerStore,
    entry: LedgerEntry,
    outcome: &mut UninstallOutcome,
) {
    let path = match entry.absolute_path(&target.root) {
        Ok(path) => path,
        Err(error) => {
            outcome.refused.push((entry.id.clone(), error.to_string()));
            return;
        }
    };
    if let Some(reason) = protected_reason(target, &path) {
        outcome.refused.push((entry.id.clone(), reason));
        return;
    }
    let removal = if path.is_dir() {
        remove_dir_all_with_retry(&path).map_err(|error| error.to_string())
    } else if path.is_file() {
        remove_file_with_retry(&path).map_err(|error| error.to_string())
    } else {
        // Already gone. The record is what is left to clean, and nothing is
        // reported as removed — that would claim a deletion that did not happen.
        if let Err(error) = ledger.forget(entry.kind, &entry.id) {
            outcome.failed.push((entry.id, format!("record: {error}")));
        }
        return;
    };
    match removal {
        Ok(()) => {
            // The asset is gone, so its record goes with it. Losing the record is
            // reported but does not undo the deletion.
            if let Err(error) = ledger.forget(entry.kind, &entry.id) {
                outcome
                    .failed
                    .push((entry.id.clone(), format!("record: {error}")));
            }
            outcome.removed.push(RemovedItem {
                id: entry.id,
                kind: entry.kind,
                path,
            });
        }
        Err(error) => outcome
            .failed
            .push((entry.id, format!("{}: {error}", path.display()))),
    }
}

/// Why a path must never be deleted by an uninstall, even if a record claims it.
fn protected_reason(target: &UserPaths, path: &Path) -> Option<String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path == target.database
        || name == DATABASE_FILE
        || name.starts_with(&format!("{DATABASE_FILE}-"))
    {
        return Some(format!("{} holds the session history", path.display()));
    }
    if path.starts_with(&target.attachments) || path.starts_with(&target.recordings) {
        return Some(format!("{} is the user's own files", path.display()));
    }
    None
}

/// Drop the recorded assets, then the database and the managed tree.
pub fn purge(target: &UserPaths) -> Result<UninstallOutcome> {
    let mut outcome = uninstall_installed(target)?;
    let ledger = store(target);
    let ledger_path = ledger.path().to_path_buf();
    let mut targets = vec![
        target.files.clone(),
        target.database.clone(),
        target
            .database
            .with_file_name(format!("{DATABASE_FILE}-wal")),
        target
            .database
            .with_file_name(format!("{DATABASE_FILE}-shm")),
        target.runtime_logs.clone(),
        ledger_path.clone(),
        ledger.lock_path().to_path_buf(),
        ledger.owner_path().to_path_buf(),
    ];
    targets.retain(|path| path != &target.root);
    for path in targets {
        // Report only a real deletion: claiming a file that was never there was
        // purged would make the summary lie about what happened.
        if !path.exists() {
            continue;
        }
        let removal = if path.is_dir() {
            remove_dir_all_with_retry(&path).map_err(|error| error.to_string())
        } else {
            remove_file_with_retry(&path).map_err(|error| error.to_string())
        };
        match removal {
            Ok(()) => outcome.purged.push(path),
            Err(error) => outcome
                .failed
                .push(("purge".to_string(), format!("{}: {error}", path.display()))),
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{plan, protected_reason, purge, uninstall_installed};
    use crate::data_root::UserPaths;
    use crate::install_ledger::{
        InstallKind, InstallSource, InstallSourceKind, LedgerEntry, LedgerStore,
    };
    use std::path::Path;

    fn entry(id: &str, kind: InstallKind, relative: &str) -> LedgerEntry {
        LedgerEntry {
            id: id.to_string(),
            kind,
            version: Some("1.0.0".to_string()),
            source: InstallSource {
                kind: InstallSourceKind::Dir,
                spec: format!("https://example.com/{id}"),
            },
            path: relative.to_string(),
            sidecars: Vec::new(),
            sha256: None,
            installed_at: String::new(),
            last_updated: None,
        }
    }

    /// A data root holding one plugin, one runtime file, one cache directory, a
    /// session database and the user's attachments.
    fn fixture(dir: &Path) -> (UserPaths, LedgerStore) {
        let paths = UserPaths::under(dir);
        paths.ensure_dirs().unwrap();
        std::fs::create_dir_all(paths.plugins.join("demo")).unwrap();
        std::fs::write(paths.plugins.join("demo").join("plugin.json"), b"{}").unwrap();
        std::fs::write(paths.runtimes.join("pandoc.exe"), b"bin").unwrap();
        std::fs::write(&paths.database, b"not really sqlite").unwrap();
        std::fs::create_dir_all(paths.attachments.join("keep")).unwrap();
        std::fs::write(paths.attachments.join("keep").join("photo.png"), b"png").unwrap();

        let ledger = LedgerStore::at(paths.root.clone(), paths.install_ledger());
        ledger
            .record(entry("demo", InstallKind::Plugin, "files/plugins/demo"))
            .unwrap();
        ledger
            .record(entry(
                "tool",
                InstallKind::Runtime,
                "files/runtimes/pandoc.exe",
            ))
            .unwrap();
        ledger
            .record(entry("cache", InstallKind::Cache, "files/cache"))
            .unwrap();
        (paths, ledger)
    }

    #[test]
    fn uninstall_removes_recorded_assets_and_keeps_user_data() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ledger) = fixture(tmp.path());

        let outcome = uninstall_installed(&paths).unwrap();
        assert!(outcome.is_clean(), "{:?}", outcome.refused);
        assert!(!paths.plugins.join("demo").exists());
        assert!(!paths.runtimes.join("pandoc.exe").exists());
        assert!(!paths.cache.exists());
        assert!(paths.database.exists(), "sessions survive an uninstall");
        assert!(paths.attachments.join("keep").join("photo.png").is_file());
        assert_eq!(outcome.removed.len(), 3);
        assert!(
            ledger.entries().unwrap().is_empty(),
            "the records went with the assets"
        );
    }

    #[test]
    fn caches_are_deleted_before_installed_assets() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, _) = fixture(tmp.path());
        let order: Vec<InstallKind> = plan(&paths)
            .unwrap()
            .iter()
            .map(|entry| entry.kind)
            .collect();
        let cache_at = order
            .iter()
            .position(|kind| *kind == InstallKind::Cache)
            .unwrap();
        let plugin_at = order
            .iter()
            .position(|kind| *kind == InstallKind::Plugin)
            .unwrap();
        assert!(
            cache_at < plugin_at,
            "regenerable stuff goes first: {order:?}"
        );
    }

    #[test]
    fn purge_also_drops_the_database_and_managed_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ledger) = fixture(tmp.path());
        std::fs::write(paths.runtime_logs.clone(), b"logs").unwrap();

        let outcome = purge(&paths).unwrap();
        assert!(outcome.is_clean(), "{:?}", outcome.failed);
        assert!(!paths.database.exists());
        assert!(!paths.files.exists());
        assert!(!paths.runtime_logs.exists());
        assert!(!paths.install_ledger().exists());
        assert!(
            outcome.purged.len() >= 4,
            "only real deletions are reported: {:?}",
            outcome.purged
        );
        assert!(
            !ledger.lock_path().exists(),
            "the lock artifacts go with the ledger"
        );
        assert!(paths.root.exists(), "the root itself is never deleted");
    }

    #[test]
    fn an_asset_already_deleted_by_hand_loses_its_record_without_a_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ledger) = fixture(tmp.path());
        std::fs::remove_dir_all(paths.plugins.join("demo")).unwrap();

        let outcome = uninstall_installed(&paths).unwrap();
        assert!(outcome.is_clean());
        assert!(
            !outcome
                .removed
                .iter()
                .any(|item| item.id == "demo@personal"),
            "a file that was already gone is not reported as removed"
        );
        assert!(!ledger
            .entries()
            .unwrap()
            .iter()
            .any(|e| e.id == "demo@personal"));
    }

    #[test]
    fn a_fresh_root_has_nothing_to_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = UserPaths::under(tmp.path());
        paths.ensure_dirs().unwrap();
        let outcome = uninstall_installed(&paths).unwrap();
        assert!(outcome.is_clean());
        assert!(outcome.removed.is_empty());
        assert!(!paths.database.exists());
    }

    #[test]
    fn a_recorded_path_outside_the_root_is_never_touched() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("deepagent");
        let outside = outer.path().join("escape");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"do not delete").unwrap();

        let paths = UserPaths::under(root.clone());
        paths.ensure_dirs().unwrap();
        let ledger = LedgerStore::at(root.clone(), paths.install_ledger());
        // Written straight to disk: `record` rejects an escaping path by design,
        // so this simulates a ledger that was edited by hand.
        std::fs::write(
            ledger.path(),
            r#"{"version":1,"entries":[{"id":"bad","kind":"plugin","source":{"kind":"dir","spec":"x"},"installed_at":"0","path":"../escape"}]}"#,
        )
        .unwrap();

        let outcome = uninstall_installed(&paths).unwrap();
        assert_eq!(outcome.refused.len(), 1, "{:?}", outcome.refused);
        assert!(
            outcome.refused[0].1.contains("not rooted"),
            "{:?}",
            outcome.refused
        );
        assert!(
            outside.join("precious.txt").is_file(),
            "nothing outside the root moves"
        );
    }

    #[test]
    fn the_database_is_protected_even_if_a_record_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ledger) = fixture(tmp.path());
        let relative = paths
            .database
            .strip_prefix(&paths.root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(
            ledger.path(),
            format!(
                r#"{{"version":1,"entries":[{{"id":"db","kind":"plugin","source":{{"kind":"dir","spec":"x"}},"installed_at":"0","path":"{relative}"}}]}}"#
            ),
        )
        .unwrap();

        let outcome = uninstall_installed(&paths).unwrap();
        assert_eq!(outcome.refused.len(), 1, "{:?}", outcome.refused);
        assert!(
            paths.database.exists(),
            "the session history is never an install asset"
        );
    }

    #[test]
    fn protection_covers_the_wal_and_shared_memory_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = UserPaths::under(tmp.path());
        for suffix in ["", "-wal", "-shm"] {
            let path = paths.root.join(format!("deepagent.db{suffix}"));
            assert!(
                protected_reason(&paths, &path).is_some(),
                "{path:?} must be protected"
            );
        }
        assert!(protected_reason(&paths, &paths.attachments).is_some());
        assert!(protected_reason(&paths, &paths.plugins).is_none());
    }
}
