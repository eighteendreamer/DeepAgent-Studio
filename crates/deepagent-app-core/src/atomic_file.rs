//! Write a whole file so a reader never sees a half-written version.
//!
//! Plugin state, the install ledger and any other JSON the host persists have the
//! same requirement: crash-safe replacement on Windows and POSIX alike. One
//! implementation, because a second copy of this logic is how a half-written
//! ledger gets shipped.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use deepagent_core::error::{CoreError, Result};

/// Write `contents` to `path` via a temp file in the same directory plus rename.
pub(crate) fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::invalid(format!("target path has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        CoreError::Persistence(format!("create parent {}: {error}", parent.display()))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("state.json");
    let pid = std::process::id();
    for attempt in 0..100u32 {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let tmp = parent.join(format!(".{file_name}.tmp-{pid}-{stamp}-{attempt}"));
        let mut file = match open_exclusive(&tmp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CoreError::Persistence(format!(
                    "create temp file {}: {error}",
                    tmp.display()
                )));
            }
        };
        let result = file
            .write_all(contents)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                CoreError::Persistence(format!("write temp file {}: {error}", tmp.display()))
            });
        drop(file);
        if let Err(error) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        return replace_file(&tmp, path);
    }

    Err(CoreError::other(format!(
        "failed to allocate a temp file for {}",
        path.display()
    )))
}

fn open_exclusive(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Rename `source` onto `target`, clearing an existing target when the platform
/// refuses to overwrite through a rename.
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(rename_error) if target.exists() => {
            std::fs::remove_file(target).map_err(|remove_error| {
                CoreError::Persistence(format!(
                    "replace {} failed after rename error ({rename_error}); \
                     removing the target failed: {remove_error}",
                    target.display()
                ))
            })?;
            std::fs::rename(source, target).map_err(|second_error| {
                CoreError::Persistence(format!(
                    "replace {} with {} failed: {second_error}",
                    target.display(),
                    source.display()
                ))
            })
        }
        Err(error) => {
            let _ = std::fs::remove_file(source);
            Err(CoreError::Persistence(format!(
                "replace {} with {} failed: {error}",
                target.display(),
                source.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::write_file_atomically;
    use std::path::Path;

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn writes_a_new_file_and_leaves_no_temp_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("state.json");
        write_file_atomically(&path, b"{\"version\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"version\":1}");
        assert!(temp_leftovers(&tmp.path().join("nested")).is_empty());
    }

    #[test]
    fn replaces_an_existing_file_without_duplicates_or_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ledger.json");
        write_file_atomically(&path, b"first").unwrap();
        write_file_atomically(&path, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(
            std::fs::read_dir(tmp.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().is_file())
                .count(),
            1,
            "only the target file remains"
        );
        assert!(temp_leftovers(tmp.path()).is_empty());
    }

    #[test]
    fn refuses_a_path_with_no_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Path::new(tmp.path().components().next().unwrap().as_os_str());
        let error = write_file_atomically(root, b"x").unwrap_err();
        assert!(error.to_string().contains("no parent"), "{error}");
    }
}
