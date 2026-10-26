//! Native move commit boundary. Old targets are never deleted before publication.
//!
//! A replacement keeps the old entry in a private sibling staging directory until
//! the source is published. Failed publication restores it without overwriting a
//! concurrent target. Failed rollback retains the backup, including on unwinding.
//! This is not a crash-atomic multi-rename transaction or a cross-volume copy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use deepagent_persistence::fs_commit::{canonicalize_with_missing_suffix, rename_no_replace};
use deepagent_tools::ToolOutput;

use crate::fs_guard::{WorkspaceRoot, WriteTarget};

pub(super) fn move_path(
    root: &WorkspaceRoot,
    source: &Path,
    destination: &Path,
    overwrite: bool,
) -> ToolOutput {
    // Reject ancestry before creating any missing destination parents. The
    // same rule is checked again by the existing commit boundary after pins.
    if let Err(error) = validate_move(root.path(), source, destination) {
        return ToolOutput::failure(format!("invalid move: {error}"));
    }
    let source_target = match root.entry_target(source, false) {
        Ok(target) => target,
        Err(error) => return ToolOutput::failure(format!("source authorization failed: {error}")),
    };
    let destination_target = match root.entry_target(destination, true) {
        Ok(target) => target,
        Err(error) => {
            return ToolOutput::failure(format!("destination authorization failed: {error}"))
        }
    };
    let physical_root = match root.read_identity(root.path()) {
        Ok(path) => WorkspaceRoot::new(path).with_access(root.access()),
        Err(error) => return ToolOutput::failure(format!("workspace identity failed: {error}")),
    };
    let mut output = move_with(
        &physical_root,
        source_target.path(),
        destination_target.path(),
        overwrite,
        rename_no_replace,
        |path| fs::remove_dir_all(path),
    );
    if output.ok {
        output.value["source"] = root.relativize(source).into();
        output.value["destination"] = root.relativize(destination).into();
    }
    output
}

fn move_with(
    root: &WorkspaceRoot,
    source: &Path,
    destination: &Path,
    overwrite: bool,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    mut cleanup: impl FnMut(&Path) -> io::Result<()>,
) -> ToolOutput {
    if let Err(error) = validate_move(root.path(), source, destination) {
        return ToolOutput::failure(format!("invalid move: {error}"));
    }
    let destination_exists = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return ToolOutput::failure(format!("destination metadata failed: {error}")),
    };
    if destination_exists && !overwrite {
        return ToolOutput::failure(
            "destination already exists; pass overwrite=true to replace it",
        );
    }
    let Some(parent) = destination.parent() else {
        return ToolOutput::failure("destination has no parent directory");
    };
    if let Err(error) = fs::create_dir_all(parent) {
        return ToolOutput::failure(format!("mkdir failed: {error}"));
    }
    let backup = if destination_exists {
        match stage_destination(root, destination, parent, &mut rename) {
            Ok(backup) => Some(backup),
            Err(error) => return ToolOutput::failure(format!("cannot stage destination: {error}")),
        }
    } else {
        None
    };
    if let Err(error) = rename(source, destination) {
        return failed_publication(root, destination, backup, error, &mut rename);
    }

    let source_absent = matches!(
        fs::symlink_metadata(source),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    );
    if !source_absent || fs::symlink_metadata(destination).is_err() {
        let mut output = ToolOutput::failure(
            "move publication returned success but verification failed; inspect both paths before retrying",
        );
        output.value["outcome_unknown"] = true.into();
        if let Some(backup) = &backup {
            output.value["recovery_path"] = root.relativize(&backup.join("previous")).into();
        }
        return output;
    }

    let mut output = ToolOutput::success(serde_json::json!({
        "source": root.relativize(source),
        "destination": root.relativize(destination),
        "moved": true,
        "verified": true,
    }));
    if let Some(backup) = backup {
        let backup = backup.release_guard();
        if let Err(error) = cleanup(&backup) {
            // Publication has committed. Do not misreport it as a failed move,
            // or remove the new target to hide a secondary cleanup failure.
            output.value["cleanup_warning"] = error.to_string().into();
            output.value["recovery_path"] = root.relativize(&backup.join("previous")).into();
            tracing::warn!(error = %error, "move committed; previous target cleanup failed");
        }
    }
    output
}

fn validate_move(workspace: &Path, source: &Path, destination: &Path) -> io::Result<()> {
    // A metadata failure is not equivalent to absence or permission to replace.
    fs::symlink_metadata(source)?;
    let workspace = fs::canonicalize(workspace)?;
    let source = entry_identity(source)?;
    let destination = entry_identity(destination)?;
    if source == workspace || destination == workspace {
        return Err(io::Error::other(
            "moving or replacing the workspace root is forbidden",
        ));
    }
    if source.starts_with(&destination) || destination.starts_with(&source) {
        return Err(io::Error::other(
            "source and destination must be distinct and neither may contain the other",
        ));
    }
    Ok(())
}

fn entry_identity(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() => fs::canonicalize(path),
        Ok(_) => {
            // Rename acts on the link itself, not its target. Resolve the parent
            // so aliases cannot hide ancestry, but preserve the final entry.
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("path has no parent"))?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("path has no name"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            canonicalize_with_missing_suffix(path)
        }
        Err(error) => Err(error),
    }
}

struct StagedDestination {
    path: PathBuf,
    _guard: WriteTarget,
}

impl std::ops::Deref for StagedDestination {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl StagedDestination {
    fn release_guard(self) -> PathBuf {
        let Self { path, _guard } = self;
        drop(_guard);
        path
    }
}

fn stage_destination(
    root: &WorkspaceRoot,
    destination: &Path,
    parent: &Path,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<StagedDestination> {
    // Disable automatic recursive cleanup BEFORE any business data enters the
    // directory. Errors/panics must not drop the only remaining old target.
    let staging = tempfile::Builder::new()
        .prefix(".deepagent-move-")
        .tempdir_in(parent)?
        .keep();
    // Pin the new private namespace before any previous target enters it.
    // Rollback and publication keep the pin; cleanup drops it only when the
    // standard library's no-follow recursive removal takes over.
    let guard = match root.entry_target(&staging.join("previous"), false) {
        Ok(guard) => guard,
        Err(error) => {
            if let Err(cleanup_error) = fs::remove_dir(&staging) {
                tracing::warn!(error = %cleanup_error, "empty move staging cleanup failed");
            }
            return Err(error);
        }
    };
    // A link may have replaced the fresh name before the pin was acquired.
    // Subsequent I/O uses the pinned physical parent, never that mutable alias.
    let staging = guard
        .path()
        .parent()
        .expect("validated staging parent")
        .to_path_buf();
    if let Err(error) = rename(destination, &staging.join("previous")) {
        drop(guard);
        if let Err(cleanup_error) = fs::remove_dir(&staging) {
            tracing::warn!(error = %cleanup_error, "empty move staging cleanup failed");
        }
        return Err(error);
    }
    Ok(StagedDestination {
        path: staging,
        _guard: guard,
    })
}

fn failed_publication(
    root: &WorkspaceRoot,
    destination: &Path,
    backup: Option<StagedDestination>,
    error: io::Error,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> ToolOutput {
    let mut output = ToolOutput::failure(format!("move failed: {error}"));
    if let Some(backup) = backup {
        match rename(&backup.join("previous"), destination) {
            Ok(()) => {
                output.value["destination_restored"] = true.into();
                let backup = backup.release_guard();
                if let Err(error) = fs::remove_dir(backup) {
                    tracing::warn!(error = %error, "empty move staging cleanup failed");
                }
            }
            Err(restore_error) => {
                output.value["destination_restored"] = false.into();
                output.value["restore_error"] = restore_error.to_string().into();
                output.value["recovery_path"] = root.relativize(&backup.join("previous")).into();
                tracing::error!(error = %restore_error, "move rollback failed; previous target retained");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "source bytes";
    const PREVIOUS: &str = "previous target bytes";

    fn fixture() -> (tempfile::TempDir, WorkspaceRoot, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::new(dir.path());
        let source = dir.path().join("source");
        let destination = dir.path().join("target");
        (dir, root, source, destination)
    }

    fn write_entry(path: &Path, directory: bool, bytes: &str) {
        if directory {
            fs::create_dir(path).unwrap();
            fs::write(path.join("child"), bytes).unwrap();
        } else {
            fs::write(path, bytes).unwrap();
        }
    }

    fn read_entry(path: &Path, directory: bool) -> String {
        fs::read_to_string(if directory {
            path.join("child")
        } else {
            path.to_owned()
        })
        .unwrap()
    }

    fn staging_dirs(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".deepagent-move-")
            })
            .collect()
    }

    #[test]
    fn overwrite_supports_files_and_nonempty_directories_without_staging_leaks() {
        for source_dir in [false, true] {
            for destination_dir in [false, true] {
                let (_dir, root, source, destination) = fixture();
                write_entry(&source, source_dir, SOURCE);
                write_entry(&destination, destination_dir, PREVIOUS);
                fs::write(root.path().join("unrelated"), "untouched").unwrap();
                let output = move_path(&root, &source, &destination, true);
                assert!(output.ok, "{output:?}");
                assert!(!source.exists());
                assert_eq!(read_entry(&destination, source_dir), SOURCE);
                assert_eq!(
                    fs::read_to_string(root.path().join("unrelated")).unwrap(),
                    "untouched"
                );
                assert!(staging_dirs(root.path()).is_empty());
            }
        }
    }

    #[test]
    fn rejection_preserves_source_target_and_unrelated_entries() {
        for overwrite in [false, true] {
            let (_dir, root, source, destination) = fixture();
            write_entry(&source, true, SOURCE);
            write_entry(&destination, true, PREVIOUS);
            for (from, to) in [
                (source.clone(), source.clone()),
                (source.join("child"), source.clone()),
                (source.clone(), source.join("new/nested")),
                (source.clone(), root.path().join(".")),
                (root.path().join("."), destination.clone()),
            ] {
                let output = move_path(&root, &from, &to, overwrite);
                assert!(!output.ok, "{from:?} -> {to:?}: {output:?}");
                assert_eq!(read_entry(&source, true), SOURCE);
                assert_eq!(read_entry(&destination, true), PREVIOUS);
                assert!(!source.join("new").exists());
                assert!(staging_dirs(root.path()).is_empty());
            }
        }
    }

    #[test]
    fn missing_source_and_unapproved_overwrite_preserve_target() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&destination, false, PREVIOUS);
        assert!(!move_path(&root, &source, &destination, true).ok);
        write_entry(&source, false, SOURCE);
        assert!(!move_path(&root, &source, &destination, false).ok);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert_eq!(read_entry(&destination, false), PREVIOUS);
        assert!(staging_dirs(root.path()).is_empty());
    }

    #[test]
    fn failure_to_stage_destination_preserves_both_entries() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let output = move_with(
            &root,
            &source,
            &destination,
            true,
            |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected stage refusal",
                ))
            },
            |_| panic!("no business data may be cleaned on stage failure"),
        );
        assert!(!output.ok);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert_eq!(read_entry(&destination, false), PREVIOUS);
        assert!(staging_dirs(root.path()).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn staging_parent_stays_pinned_through_publication_failure_and_rollback() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let mut pin_checks = 0;
        let output = move_with(
            &root,
            &source,
            &destination,
            true,
            |from, to| {
                if to.file_name().is_some_and(|name| name == "previous")
                    || from.file_name().is_some_and(|name| name == "previous")
                {
                    let parent = if to.file_name().is_some_and(|name| name == "previous") {
                        to.parent()
                    } else {
                        from.parent()
                    }
                    .unwrap();
                    let error = fs::rename(parent, root.path().join("parked-staging")).unwrap_err();
                    assert_eq!(error.raw_os_error(), Some(32));
                    pin_checks += 1;
                }
                if from == source {
                    Err(io::Error::other("injected publication failure"))
                } else {
                    rename_no_replace(from, to)
                }
            },
            |_| panic!("failed publication must restore previous data"),
        );
        assert!(!output.ok);
        assert_eq!(output.value["destination_restored"], true);
        assert_eq!(pin_checks, 2);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert_eq!(read_entry(&destination, false), PREVIOUS);
        assert!(staging_dirs(root.path()).is_empty());
    }

    #[test]
    fn publication_failure_rolls_back_files_and_directories() {
        for directory in [false, true] {
            let (_dir, root, source, destination) = fixture();
            write_entry(&source, directory, SOURCE);
            write_entry(&destination, directory, PREVIOUS);
            let output = move_with(
                &root,
                &source,
                &destination,
                true,
                |from, to| {
                    if from == source {
                        assert!(
                            !destination.exists(),
                            "old target must be staged, not deleted"
                        );
                        let stages = staging_dirs(root.path());
                        assert_eq!(stages.len(), 1);
                        assert_eq!(read_entry(&stages[0].join("previous"), directory), PREVIOUS);
                        Err(io::Error::other(
                            "injected publish failure (e.g. cross-volume)",
                        ))
                    } else {
                        rename_no_replace(from, to)
                    }
                },
                |_| panic!("rollback must never recursively delete previous data"),
            );
            assert!(!output.ok);
            assert_eq!(output.value["destination_restored"], true);
            assert_eq!(read_entry(&source, directory), SOURCE);
            assert_eq!(read_entry(&destination, directory), PREVIOUS);
            assert!(staging_dirs(root.path()).is_empty());
        }
    }

    #[test]
    fn failed_rollback_retains_previous_target_and_exposes_recovery_path() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let output = move_with(
            &root,
            &source,
            &destination,
            true,
            |from, to| {
                if from == destination {
                    rename_no_replace(from, to)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected refusal",
                    ))
                }
            },
            |_| panic!("the only previous target copy cannot be cleaned"),
        );
        assert!(!output.ok);
        assert_eq!(output.value["destination_restored"], false);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert!(!destination.exists());
        let retained = root
            .path()
            .join(output.value["recovery_path"].as_str().unwrap());
        assert_eq!(read_entry(&retained, false), PREVIOUS);
        assert!(output.value["restore_error"]
            .as_str()
            .unwrap()
            .contains("injected refusal"));
    }

    #[test]
    fn concurrent_target_is_not_overwritten_by_publication_or_rollback() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let output = move_with(
            &root,
            &source,
            &destination,
            true,
            |from, to| {
                if from == source {
                    fs::write(to, "concurrent target").unwrap();
                }
                rename_no_replace(from, to)
            },
            |_| panic!("failed rollback retains previous data"),
        );
        assert!(!output.ok);
        assert_eq!(output.value["destination_restored"], false);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert_eq!(read_entry(&destination, false), "concurrent target");
        let retained = root
            .path()
            .join(output.value["recovery_path"].as_str().unwrap());
        assert_eq!(read_entry(&retained, false), PREVIOUS);
    }

    #[test]
    fn no_replace_preserves_existing_file_and_directory() {
        for directory in [false, true] {
            let (_dir, _root, source, destination) = fixture();
            write_entry(&source, directory, SOURCE);
            write_entry(&destination, directory, PREVIOUS);
            assert!(rename_no_replace(&source, &destination).is_err());
            assert_eq!(read_entry(&source, directory), SOURCE);
            assert_eq!(read_entry(&destination, directory), PREVIOUS);
        }
    }

    #[test]
    fn target_created_after_absence_check_is_preserved_without_overwrite() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        let output = move_with(
            &root,
            &source,
            &destination,
            false,
            |from, to| {
                fs::write(to, "concurrent target").unwrap();
                rename_no_replace(from, to)
            },
            |_| panic!("there was no previous target to stage or clean"),
        );
        assert!(!output.ok);
        assert_eq!(read_entry(&source, false), SOURCE);
        assert_eq!(read_entry(&destination, false), "concurrent target");
        assert!(staging_dirs(root.path()).is_empty());
    }

    #[test]
    fn post_commit_cleanup_failure_is_not_reported_as_failed_move() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let output = move_with(
            &root,
            &source,
            &destination,
            true,
            rename_no_replace,
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected cleanup refusal",
                ))
            },
        );
        assert!(output.ok, "publication has already committed: {output:?}");
        assert_eq!(output.value["moved"], true);
        assert!(output.value["cleanup_warning"].is_string());
        assert!(!source.exists());
        assert_eq!(read_entry(&destination, false), SOURCE);
        let retained = root
            .path()
            .join(output.value["recovery_path"].as_str().unwrap());
        assert_eq!(read_entry(&retained, false), PREVIOUS);
    }

    #[test]
    fn unwinding_does_not_drop_the_only_previous_target_copy() {
        let (_dir, root, source, destination) = fixture();
        write_entry(&source, false, SOURCE);
        write_entry(&destination, false, PREVIOUS);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            move_with(
                &root,
                &source,
                &destination,
                true,
                |from, to| {
                    if from == source {
                        panic!("injected worker panic after staging");
                    }
                    rename_no_replace(from, to)
                },
                |_| panic!("no cleanup during unwinding"),
            )
        }));
        assert!(panic.is_err());
        assert_eq!(read_entry(&source, false), SOURCE);
        let stages = staging_dirs(root.path());
        assert_eq!(stages.len(), 1);
        assert_eq!(read_entry(&stages[0].join("previous"), false), PREVIOUS);
    }

    #[cfg(windows)]
    #[test]
    fn case_alias_cannot_hide_self_or_ancestor_replacement() {
        let (_dir, root, source, _destination) = fixture();
        write_entry(&source, true, SOURCE);
        for (from, to) in [
            (source.clone(), root.path().join("SOURCE")),
            (source.join("child"), root.path().join("SOURCE")),
        ] {
            assert!(!move_path(&root, &from, &to, true).ok);
            assert_eq!(read_entry(&source, true), SOURCE);
            assert!(staging_dirs(root.path()).is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_cannot_hide_descendant_destination() {
        let (_dir, root, source, _destination) = fixture();
        write_entry(&source, true, SOURCE);
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let output = move_path(&root, &source, &alias.join("new/nested"), true);
        assert!(!output.ok);
        assert_eq!(read_entry(&source, true), SOURCE);
        assert!(!source.join("new").exists());
    }
}
