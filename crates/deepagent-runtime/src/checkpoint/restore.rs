//! Prepared, compensating restore for an ordered set of existing checkpoints.
//!
//! All replacement trees are built before workspace publication. Overlapping
//! targets share one staged root, preserving reverse-entry/checkpoint ordering.
//! Previous roots remain reachable until the caller's history commit succeeds.
//! This is an in-process compensation boundary, not a crash-atomic transaction
//! or a lock against unrelated workspace writers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use deepagent_core::{CoreError, Result};
use deepagent_persistence::checkpoint_store::CheckpointRecord;
use deepagent_persistence::fs_commit::rename_no_replace;

use super::{normalize_path, CheckpointManifest};

struct RestoreEntry {
    target: PathBuf,
    reported: PathBuf,
    backup: Option<PathBuf>,
    #[cfg(windows)]
    access: Option<super::access::AccessTree>,
}

struct StagedRoot {
    target: PathBuf,
    area: PathBuf,
    previous_moved: bool,
    published: bool,
    #[cfg(windows)]
    access: Vec<super::access::AccessLease>,
}

#[derive(Default)]
pub(super) struct RestoreTransaction {
    stages: Vec<StagedRoot>,
    created_parents: Vec<PathBuf>,
    // Once business data is moved, Drop must never delete its only old copy.
    dirty: bool,
}

impl RestoreTransaction {
    pub(super) fn run<T>(
        records: &[CheckpointRecord],
        finalize: impl FnOnce(Vec<PathBuf>) -> Result<T>,
    ) -> Result<T> {
        let entries = parse_entries(records)?;
        if entries.is_empty() {
            return finalize(Vec::new());
        }
        let paths = entries.iter().map(|entry| entry.reported.clone()).collect();
        let mut transaction = Self::prepare(&entries)?;
        transaction.dirty = true;
        let result = transaction.publish().and_then(|()| finalize(paths));
        match result {
            Ok(value) => {
                transaction.dirty = false;
                Ok(value)
            }
            Err(error) => Err(transaction.compensate(error)),
        }
    }

    fn compensate(&mut self, error: CoreError) -> CoreError {
        if let Err(rollback) = self.rollback() {
            let recovery = self
                .stages
                .iter()
                .map(|stage| &stage.area)
                .collect::<Vec<_>>();
            return CoreError::other(format!("checkpoint restore failed: {error}; compensation incomplete: {rollback}; recovery directories retained: {recovery:?}"));
        }
        self.dirty = false;
        tracing::warn!(%error, workspace_restored = true, "checkpoint restore rejected; compensated workspace publication");
        error
    }

    fn prepare(entries: &[RestoreEntry]) -> Result<Self> {
        let mut roots: Vec<PathBuf> = entries.iter().map(|entry| entry.target.clone()).collect();
        roots.sort_by_key(|path| path.components().count());
        let mut transaction = Self::default();
        for target in roots {
            if transaction
                .stages
                .iter()
                .any(|stage| target.starts_with(&stage.target))
            {
                continue;
            }
            let mut parent = target
                .parent()
                .ok_or_else(|| CoreError::invalid("restore target has no parent"))?;
            while !parent.try_exists().map_err(fs_error)? {
                parent = parent
                    .parent()
                    .ok_or_else(|| CoreError::invalid("restore parent unavailable"))?;
            }
            // Sibling/ancestor location stays on the target filesystem. No
            // cross-volume copy/delete fallback is used for publication.
            let area = tempfile::Builder::new()
                .prefix(".deepagent-restore-")
                .tempdir_in(parent)
                .map_err(fs_error)?
                .keep();
            transaction.stages.push(StagedRoot {
                target: target.clone(),
                area: area.clone(),
                previous_moved: false,
                published: false,
                #[cfg(windows)]
                access: Vec::new(),
            });
        }
        for stage in &mut transaction.stages {
            // Every minimal root is itself restored by at least one entry.
            // Its last complete snapshot supersedes all earlier descendants;
            // only later descendant operations can affect its final contents.
            let start = entries
                .iter()
                .rposition(|entry| entry.target == stage.target)
                .expect("root is a restore target");
            for entry in entries[start..]
                .iter()
                .filter(|entry| entry.target.starts_with(&stage.target))
            {
                let suffix = entry
                    .target
                    .strip_prefix(&stage.target)
                    .expect("matching root");
                let desired = if suffix.as_os_str().is_empty() {
                    stage.area.join("desired")
                } else {
                    stage.area.join("desired").join(suffix)
                };
                #[cfg(windows)]
                stage
                    .access
                    .retain(|lease| !lease.staged.starts_with(&desired));
                remove_staged(&desired).map_err(|error| {
                    CoreError::other(format!(
                        "checkpoint superseded staging cleanup failed: {error}"
                    ))
                })?;
                if let Some(backup) = &entry.backup {
                    if let Some(parent) = desired.parent() {
                        fs::create_dir_all(parent).map_err(fs_error)?;
                    }
                    #[cfg(windows)]
                    stage.access.extend(super::access::prepare_copy(
                        backup,
                        &desired,
                        entry.access.as_ref().expect("validated Windows DACL tree"),
                    )?);
                    #[cfg(not(windows))]
                    copy_entry(backup, &desired)?;
                }
            }
        }
        Ok(transaction)
    }

    fn publish(&mut self) -> Result<()> {
        for stage in &mut self.stages {
            create_parents(&stage.target, &mut self.created_parents)?;
            #[cfg(windows)]
            super::access::prepare_publication(&mut stage.access, &stage.area.join("desired"))?;
            if exists(&stage.target)? {
                rename_no_replace(&stage.target, &stage.area.join("previous")).map_err(
                    |error| {
                        CoreError::other(format!(
                            "checkpoint retaining previous root failed: {error}"
                        ))
                    },
                )?;
                stage.previous_moved = true;
            }
            let desired = stage.area.join("desired");
            if exists(&desired)? {
                rename_no_replace(&desired, &stage.target).map_err(|error| {
                    CoreError::other(format!(
                        "checkpoint publishing desired root failed: {error}"
                    ))
                })?;
                stage.published = true;
                #[cfg(windows)]
                {
                    for lease in &stage.access {
                        let relative = lease
                            .staged
                            .strip_prefix(&desired)
                            .expect("owned staged entry");
                        let published = if relative.as_os_str().is_empty() {
                            stage.target.clone()
                        } else {
                            stage.target.join(relative)
                        };
                        lease.publish_policy(&published)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        for stage in self.stages.iter_mut().rev() {
            let result = (|| -> Result<()> {
                if stage.published {
                    // Retain the replacement as well if restoring the previous
                    // target fails. Never recursively delete a live target.
                    rename_no_replace(&stage.target, &stage.area.join("desired"))
                        .map_err(fs_error)?;
                    stage.published = false;
                }
                if stage.previous_moved {
                    rename_no_replace(&stage.area.join("previous"), &stage.target)
                        .map_err(fs_error)?;
                    stage.previous_moved = false;
                }
                Ok(())
            })();
            if let Err(error) = result {
                failures.push(format!("{}: {error}", stage.target.display()));
            }
        }
        for parent in self.created_parents.iter().rev() {
            if let Err(error) = fs::remove_dir(parent) {
                failures.push(format!(
                    "remove newly created parent {}: {error}",
                    parent.display()
                ));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(CoreError::other(failures.join("; ")))
        }
    }
}

impl Drop for RestoreTransaction {
    fn drop(&mut self) {
        for stage in &self.stages {
            if self.dirty {
                tracing::error!(target = %stage.target.display(), recovery_path = %stage.area.display(), "checkpoint recovery directory retained; restore did not finish");
            } else if let Err(error) = fs::remove_dir_all(&stage.area) {
                // Final publication/history commit already succeeded, or the
                // pre-call state was restored. Cleanup is not a failed restore.
                tracing::warn!(%error, recovery_path = %stage.area.display(), "checkpoint staging cleanup failed");
            }
        }
    }
}

fn parse_entries(records: &[CheckpointRecord]) -> Result<Vec<RestoreEntry>> {
    let mut entries = Vec::new();
    let mut storage_roots = Vec::new();
    for record in records {
        let root = fs::canonicalize(&record.workspace_root).map_err(fs_error)?;
        let lexical_root = absolute(Path::new(&record.workspace_root))?;
        let manifest: CheckpointManifest = serde_json::from_str(&record.manifest)
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        let storage = fs::canonicalize(&manifest.storage_dir).map_err(fs_error)?;
        storage_roots.push(storage.clone());
        for entry in manifest.entries.iter().rev() {
            let lexical_target = absolute(Path::new(&entry.path))?;
            let suffix = lexical_target
                .strip_prefix(&lexical_root)
                .map_err(|_| CoreError::invalid("checkpoint path escapes workspace"))?;
            let target = resolve_target(&root.join(suffix))?;
            if target == root || !target.starts_with(&root) {
                return Err(CoreError::invalid(
                    "checkpoint path escapes workspace or replaces its root",
                ));
            }
            let backup = if entry.existed {
                let source = entry
                    .backup
                    .as_deref()
                    .ok_or_else(|| CoreError::Persistence("missing checkpoint backup".into()))?;
                let source = fs::canonicalize(source).map_err(fs_error)?;
                if source == storage || !source.starts_with(&storage) {
                    return Err(CoreError::invalid("checkpoint backup escapes storage"));
                }
                let metadata = fs::symlink_metadata(&source).map_err(fs_error)?;
                if metadata.is_dir() != entry.was_dir || !(metadata.is_dir() || metadata.is_file())
                {
                    return Err(CoreError::invalid(
                        "checkpoint backup type differs from manifest",
                    ));
                }
                Some(source)
            } else {
                if entry.backup.is_some() {
                    return Err(CoreError::invalid(
                        "absent checkpoint entry unexpectedly has a backup",
                    ));
                }
                None
            };
            if let Some(source) = &backup {
                #[cfg(windows)]
                {
                    let tree = entry.windows_dacl.as_ref().ok_or_else(|| CoreError::invalid("legacy checkpoint lacks Windows DACL metadata; exact access-policy restoration is unavailable"))?;
                    super::access::validate_backup(source, tree)?;
                }
                #[cfg(not(windows))]
                if entry.windows_dacl.is_some() {
                    return Err(CoreError::invalid(
                        "Windows checkpoint DACL metadata cannot be restored on this platform",
                    ));
                }
            } else if entry.windows_dacl.is_some() {
                return Err(CoreError::invalid(
                    "absent checkpoint entry unexpectedly has DACL metadata",
                ));
            }
            entries.push(RestoreEntry {
                target,
                reported: lexical_target,
                backup,
                #[cfg(windows)]
                access: entry.windows_dacl.clone(),
            });
        }
    }
    for entry in &entries {
        if storage_roots
            .iter()
            .any(|storage| storage.starts_with(&entry.target) || entry.target.starts_with(storage))
        {
            return Err(CoreError::invalid(
                "checkpoint target overlaps checkpoint storage",
            ));
        }
    }
    Ok(entries)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(normalize_path(&if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(fs_error)?.join(path)
    }))
}

fn resolve_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(CoreError::invalid("checkpoint target is a symbolic link"));
            }
            fs::canonicalize(path).map_err(fs_error)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| CoreError::invalid("checkpoint target has no parent"))?;
            Ok(resolve_target(parent)?.join(
                path.file_name()
                    .ok_or_else(|| CoreError::invalid("checkpoint target has no name"))?,
            ))
        }
        Err(error) => Err(fs_error(error)),
    }
}

#[cfg(not(windows))]
fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source).map_err(fs_error)?;
    if metadata.file_type().is_symlink() {
        return Err(CoreError::invalid(format!(
            "checkpoint restore does not follow symbolic links: {}",
            source.display()
        )));
    }
    if metadata.is_dir() {
        fs::create_dir(destination).map_err(fs_error)?;
        for entry in fs::read_dir(source).map_err(fs_error)? {
            let entry = entry.map_err(fs_error)?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions()).map_err(fs_error)?;
    } else if metadata.is_file() {
        fs::copy(source, destination).map_err(fs_error)?;
    } else {
        return Err(CoreError::invalid(
            "checkpoint restore requires regular files or directories",
        ));
    }
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fs_error(error)),
    }
}

fn remove_staged(path: &Path) -> Result<()> {
    if exists(path)? {
        if fs::symlink_metadata(path).map_err(fs_error)?.is_dir() {
            fs::remove_dir_all(path).map_err(fs_error)?;
        } else {
            fs::remove_file(path).map_err(fs_error)?;
        }
    }
    Ok(())
}

fn create_parents(target: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("restore target has no parent"))?;
    if !exists(parent)? {
        create_parents(parent, created)?;
        fs::create_dir(parent).map_err(fs_error)?;
        created.push(parent.to_owned());
    }
    Ok(())
}

fn fs_error(error: io::Error) -> CoreError {
    CoreError::other(format!("checkpoint restore filesystem error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::super::CheckpointEntry;
    use super::*;

    struct Fixture {
        workspace: tempfile::TempDir,
        storage: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                workspace: tempfile::tempdir().unwrap(),
                storage: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.workspace.path().join(name)
        }

        fn snapshot(&self, names: &[&str]) -> CheckpointRecord {
            let id = deepagent_core::id::EventId::new().to_string();
            let storage_dir = self.storage.path().join(&id);
            fs::create_dir(&storage_dir).unwrap();
            let entries = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let path = self.path(name);
                    let existed = exists(&path).unwrap();
                    let was_dir = path.is_dir();
                    let (backup, windows_dacl) = if existed {
                        let backup = storage_dir.join(index.to_string());
                        let access = super::super::backup_entry(&path, &backup).unwrap();
                        (Some(backup.to_string_lossy().to_string()), access)
                    } else {
                        (None, None)
                    };
                    CheckpointEntry {
                        path: path.to_string_lossy().to_string(),
                        existed,
                        was_dir,
                        backup,
                        windows_dacl,
                    }
                })
                .collect();
            CheckpointRecord {
                id,
                run_id: "fixture".into(),
                session_sequence: 1,
                workspace_root: self.workspace.path().to_string_lossy().to_string(),
                manifest: serde_json::to_string(&CheckpointManifest {
                    branch_anchor: None,
                    storage_dir: storage_dir.to_string_lossy().to_string(),
                    entries,
                    external_evidence: vec![],
                })
                .unwrap(),
                created_at: 1,
            }
        }

        fn assert_no_stages(&self) {
            assert!(fs::read_dir(self.workspace.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".deepagent-restore-")));
        }
    }

    #[test]
    fn missing_backup_in_older_checkpoint_preserves_all_current_files() {
        let fixture = Fixture::new();
        fs::write(fixture.path("first"), "first-before").unwrap();
        let older = fixture.snapshot(&["first"]);
        fs::write(fixture.path("second"), "second-before").unwrap();
        let newer = fixture.snapshot(&["second"]);
        fs::write(fixture.path("first"), "first-current").unwrap();
        fs::write(fixture.path("second"), "second-current").unwrap();
        let manifest: CheckpointManifest = serde_json::from_str(&older.manifest).unwrap();
        fs::remove_file(manifest.entries[0].backup.as_ref().unwrap()).unwrap();
        assert!(RestoreTransaction::run(&[newer, older], |_| -> Result<()> {
            panic!("cannot commit incomplete plan")
        })
        .is_err());
        assert_eq!(
            fs::read_to_string(fixture.path("first")).unwrap(),
            "first-current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("second")).unwrap(),
            "second-current"
        );
        fixture.assert_no_stages();
    }

    #[cfg(windows)]
    #[test]
    fn missing_or_corrupt_dacl_metadata_rejects_restore_before_publication() {
        for invalid in [
            "legacy",
            "tree-version",
            "policy-version",
            "invalid-sddl",
            "traversal",
            "omitted",
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.path("file"), "before").unwrap();
            let mut record = fixture.snapshot(&["file"]);
            let mut manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
            let entry = &mut manifest["entries"][0];
            match invalid {
                "legacy" => {
                    entry.as_object_mut().unwrap().remove("windows_dacl");
                }
                "tree-version" => entry["windows_dacl"]["version"] = 2.into(),
                "policy-version" => {
                    entry["windows_dacl"]["entries"][0]["policy"]["version"] = 2.into()
                }
                "invalid-sddl" => {
                    entry["windows_dacl"]["entries"][0]["policy"]["sddl"] = "D:\0bad".into()
                }
                "traversal" => {
                    entry["windows_dacl"]["entries"][0]["relative"] = "../outside".into()
                }
                "omitted" => entry["windows_dacl"]["entries"] = serde_json::json!([]),
                _ => unreachable!(),
            }
            record.manifest = manifest.to_string();
            fs::write(fixture.path("file"), "current").unwrap();
            assert!(
                RestoreTransaction::run(&[record], |_| -> Result<()> {
                    panic!("invalid access metadata must not reach history commit")
                })
                .is_err(),
                "{invalid}"
            );
            assert_eq!(fs::read_to_string(fixture.path("file")).unwrap(), "current");
            fixture.assert_no_stages();
        }
    }

    #[cfg(windows)]
    #[test]
    fn changed_final_parent_policy_compensates_content_and_dacl() {
        use deepagent_persistence::fs_commit::access::{
            create_snapshot_target, open_snapshot_source, WindowsDacl,
        };
        let fixture = Fixture::new();
        let parent = create_snapshot_target(&fixture.path("tree"), true).unwrap();
        let target = fixture.path("tree/file");
        fs::write(&target, "before").unwrap();
        let original = WindowsDacl::capture(&open_snapshot_source(&target).unwrap()).unwrap();
        let record = fixture.snapshot(&["tree/file"]);
        let changed: WindowsDacl = serde_json::from_value(
            serde_json::json!({"version":1,"protected":true,"sddl":"D:P(A;OICI;FA;;;WD)"}),
        )
        .unwrap();
        changed.apply(&parent, false).unwrap();
        fs::write(&target, "current").unwrap();
        let before_restore = WindowsDacl::capture(&open_snapshot_source(&target).unwrap()).unwrap();
        assert!(
            original != before_restore,
            "fixture must change inherited policy"
        );
        let error = RestoreTransaction::run(&[record], |_| -> Result<()> {
            panic!("mismatched final DACL must not reach history commit")
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("DACL differs after publication"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "current");
        assert!(
            WindowsDacl::capture(&open_snapshot_source(&target).unwrap()).unwrap()
                == before_restore,
            "compensation changed original DACL"
        );
        fixture.assert_no_stages();
    }

    #[cfg(windows)]
    #[test]
    fn directory_restore_and_history_compensation_preserve_every_original_policy() {
        use deepagent_persistence::fs_commit::access::{
            create_snapshot_target, open_policy_source, WindowsDacl,
        };
        let fixture = Fixture::new();
        let tree = fixture.path("tree");
        let parent = create_snapshot_target(&tree, true).unwrap();
        let restricted: WindowsDacl = serde_json::from_value(serde_json::json!({"version":1,"protected":true,"sddl":"D:P(D;OICI;0x20;;;WD)(A;OICI;FA;;;WD)"})).unwrap();
        restricted.apply(&parent, false).unwrap();
        fs::create_dir(tree.join("nested")).unwrap();
        fs::write(tree.join("nested/file"), "before").unwrap();
        let paths = [tree.clone(), tree.join("nested"), tree.join("nested/file")];
        let capture = || {
            paths
                .iter()
                .map(|path| WindowsDacl::capture(&open_policy_source(path).unwrap()).unwrap())
                .collect::<Vec<_>>()
        };
        let original = capture();
        let record = fixture.snapshot(&["tree"]);
        let changed: WindowsDacl = serde_json::from_value(
            serde_json::json!({"version":1,"protected":true,"sddl":"D:P(A;OICI;FA;;;WD)"}),
        )
        .unwrap();
        changed.apply(&parent, false).unwrap();
        fs::write(tree.join("nested/file"), "current").unwrap();
        let current = capture();
        assert!(
            current != original,
            "fixture must change protected and inherited policies"
        );
        let result = RestoreTransaction::run(std::slice::from_ref(&record), |_| -> Result<()> {
            assert_eq!(
                fs::read_to_string(tree.join("nested/file")).unwrap(),
                "before"
            );
            assert!(
                capture() == original,
                "restore changed original tree policy"
            );
            Err(CoreError::Persistence("injected history failure".into()))
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(tree.join("nested/file")).unwrap(),
            "current"
        );
        assert!(
            capture() == current,
            "compensation changed current tree policy"
        );
        RestoreTransaction::run(&[record], |_| Ok(())).unwrap();
        assert_eq!(
            fs::read_to_string(tree.join("nested/file")).unwrap(),
            "before"
        );
        assert!(
            capture() == original,
            "successful restore changed original tree policy"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn malformed_late_entry_is_rejected_before_any_publication() {
        for invalid in [
            "missing-field",
            "outside-backup",
            "outside-target",
            "root-target",
            "wrong-type",
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.path("first"), "first-before").unwrap();
            fs::write(fixture.path("second"), "second-before").unwrap();
            let mut record = fixture.snapshot(&["first", "second"]);
            let mut manifest: CheckpointManifest = serde_json::from_str(&record.manifest).unwrap();
            match invalid {
                "missing-field" => manifest.entries[0].backup = None,
                "outside-backup" => {
                    manifest.entries[0].backup =
                        Some(fixture.path("first").to_string_lossy().to_string())
                }
                "outside-target" => {
                    manifest.entries[0].path = fixture
                        .storage
                        .path()
                        .join("escape")
                        .to_string_lossy()
                        .to_string()
                }
                "root-target" => {
                    manifest.entries[0].path =
                        fixture.workspace.path().to_string_lossy().to_string()
                }
                "wrong-type" => manifest.entries[0].was_dir = true,
                _ => unreachable!(),
            }
            record.manifest = serde_json::to_string(&manifest).unwrap();
            fs::write(fixture.path("first"), "first-current").unwrap();
            fs::write(fixture.path("second"), "second-current").unwrap();
            assert!(
                RestoreTransaction::run(&[record], |_| Ok(())).is_err(),
                "{invalid}"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("first")).unwrap(),
                "first-current"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("second")).unwrap(),
                "second-current"
            );
            fixture.assert_no_stages();
        }
    }

    #[test]
    fn failed_history_commit_compensates_modified_deleted_and_created_targets() {
        let fixture = Fixture::new();
        fs::write(fixture.path("modified"), "before").unwrap();
        fs::write(fixture.path("deleted"), "deleted-before").unwrap();
        let record = fixture.snapshot(&["modified", "deleted", "created"]);
        fs::write(fixture.path("modified"), "current").unwrap();
        fs::remove_file(fixture.path("deleted")).unwrap();
        fs::write(fixture.path("created"), "created-current").unwrap();
        let result = RestoreTransaction::run(&[record], |_| -> Result<()> {
            assert_eq!(
                fs::read_to_string(fixture.path("modified")).unwrap(),
                "before"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("deleted")).unwrap(),
                "deleted-before"
            );
            assert!(!fixture.path("created").exists());
            Err(CoreError::Persistence("injected pre-commit failure".into()))
        });
        assert!(
            matches!(result, Err(CoreError::Persistence(message)) if message == "injected pre-commit failure")
        );
        assert_eq!(
            fs::read_to_string(fixture.path("modified")).unwrap(),
            "current"
        );
        assert!(!fixture.path("deleted").exists());
        assert_eq!(
            fs::read_to_string(fixture.path("created")).unwrap(),
            "created-current"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn nested_snapshots_and_multiple_checkpoints_keep_existing_restore_order() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("tree")).unwrap();
        fs::write(fixture.path("tree/child"), "oldest").unwrap();
        let older = fixture.snapshot(&["tree"]);
        fs::write(fixture.path("tree/child"), "middle").unwrap();
        let newer = fixture.snapshot(&["tree/child", "tree"]);
        fs::write(fixture.path("tree/child"), "current").unwrap();
        RestoreTransaction::run(&[newer, older], |paths| {
            assert_eq!(paths.len(), 3);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(fixture.path("tree/child")).unwrap(),
            "oldest"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn failed_rollback_retains_current_copy_and_reports_its_location() {
        let fixture = Fixture::new();
        fs::write(fixture.path("file"), "before").unwrap();
        let record = fixture.snapshot(&["file"]);
        fs::write(fixture.path("file"), "current").unwrap();
        let result = RestoreTransaction::run(&[record], |_| -> Result<()> {
            // Deterministically obstruct the no-replace rollback destination
            // inside the fixture's own stage, without deleting either version.
            let area = fs::read_dir(fixture.workspace.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".deepagent-restore-")
                })
                .unwrap();
            fs::write(area.join("desired"), "rollback obstruction").unwrap();
            Err(CoreError::Persistence("injected finalization error".into()))
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("compensation incomplete"));
        assert!(error.contains("recovery directories retained"));
        assert_eq!(fs::read_to_string(fixture.path("file")).unwrap(), "before");
        let area = fs::read_dir(fixture.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            fs::read_to_string(area.join("previous")).unwrap(),
            "current"
        );
    }

    #[test]
    fn panic_never_drops_the_only_pre_restore_copy() {
        let fixture = Fixture::new();
        fs::write(fixture.path("file"), "before").unwrap();
        let record = fixture.snapshot(&["file"]);
        fs::write(fixture.path("file"), "current").unwrap();
        assert!(std::panic::catch_unwind(|| RestoreTransaction::run(
            &[record],
            |_| -> Result<()> { panic!("injected commit panic") }
        ))
        .is_err());
        let area = fs::read_dir(fixture.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            fs::read_to_string(area.join("previous")).unwrap(),
            "current"
        );
        assert_eq!(fs::read_to_string(fixture.path("file")).unwrap(), "before");
    }

    #[cfg(windows)]
    #[test]
    fn native_publication_failure_compensates_earlier_roots() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        fs::write(fixture.path("locked"), "locked-before").unwrap();
        fs::write(fixture.path("healthy"), "healthy-before").unwrap();
        let record = fixture.snapshot(&["locked", "healthy"]);
        fs::write(fixture.path("locked"), "locked-current").unwrap();
        fs::write(fixture.path("healthy"), "healthy-current").unwrap();
        // share_mode(0) is an exclusive handle; the real OS refuses rename.
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(fixture.path("locked"))
            .unwrap();
        assert!(RestoreTransaction::run(&[record], |_| -> Result<()> {
            panic!("publication must fail")
        })
        .is_err());
        drop(handle);
        assert_eq!(
            fs::read_to_string(fixture.path("locked")).unwrap(),
            "locked-current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("healthy")).unwrap(),
            "healthy-current"
        );
        fixture.assert_no_stages();
    }

    #[cfg(windows)]
    #[test]
    fn failed_directory_copy_never_publishes_partial_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("tree")).unwrap();
        fs::write(fixture.path("tree/child"), "before").unwrap();
        fs::write(fixture.path("healthy"), "healthy-before").unwrap();
        let record = fixture.snapshot(&["tree", "healthy"]);
        let manifest: CheckpointManifest = serde_json::from_str(&record.manifest).unwrap();
        let backup = Path::new(manifest.entries[0].backup.as_ref().unwrap()).join("child");
        fs::write(fixture.path("tree/child"), "current").unwrap();
        fs::write(fixture.path("healthy"), "healthy-current").unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(backup)
            .unwrap();
        assert!(RestoreTransaction::run(&[record], |_| -> Result<()> {
            panic!("incomplete replacement cannot commit")
        })
        .is_err());
        drop(handle);
        assert_eq!(
            fs::read_to_string(fixture.path("tree/child")).unwrap(),
            "current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("healthy")).unwrap(),
            "healthy-current"
        );
        fixture.assert_no_stages();
    }
}
