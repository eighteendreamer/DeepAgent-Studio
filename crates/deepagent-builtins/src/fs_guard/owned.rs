//! Host-owned storage using the same root identity and native I/O as file tools.
//!
//! This is not model filesystem authorization: callers must own the root and
//! choose the allowed descendant identity. No workspace sensitive-name policy
//! is applied to uploaded filenames. Links below the root are not aliases.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::{root, WriteTarget};

#[derive(Debug, Clone)]
/// Stable host-owned root identity, shared across service clones.
pub struct OwnedDirectoryRoot {
    logical: PathBuf,
    anchor: Arc<Result<root::RootAnchor, (io::ErrorKind, String)>>,
}

/// Operation lifetime, not just a checked pathname. On Windows every ancestor
/// is pinned and native opens refuse reparse traversal. Other platforms retain
/// the existing portable preflight behavior, not an atomic no-follow claim.
pub struct OwnedDirectory {
    path: PathBuf,
    _root_pin: root::RootPin,
    #[cfg(windows)]
    _directory: super::windows::WriteDirectory,
}

impl OwnedDirectoryRoot {
    /// Capture the root (or its existing ancestor) without creating storage.
    pub fn new(logical: PathBuf) -> Self {
        let anchor =
            Arc::new(root::RootAnchor::capture(&logical).map_err(|e| (e.kind(), e.to_string())));
        Self { logical, anchor }
    }

    /// Open/create only a nonempty, ordinary-component descendant. The root
    /// itself can never be returned as a removable child.
    pub fn directory(&self, relative: &Path, create: bool) -> io::Result<OwnedDirectory> {
        validate_relative(relative)?;
        let anchor = self.anchor()?;
        let _initial_pin = anchor.pin(&self.logical)?;
        let path = anchor.physical.join(relative);
        reject_links(&anchor.physical, relative)?;
        #[cfg(windows)]
        let directory = super::windows::WriteDirectory::open(&path, create)?;
        #[cfg(not(windows))]
        if create {
            fs::create_dir_all(&path)?;
        }
        reject_links(&anchor.physical, relative)?;
        if !fs::metadata(&path)?.is_dir() || fs::canonicalize(&path)? != path {
            return Err(denied("storage directory identity changed"));
        }
        Ok(OwnedDirectory {
            path,
            _root_pin: anchor.pin(&self.logical)?,
            #[cfg(windows)]
            _directory: directory,
        })
    }

    /// Delete just the selected descendant, retaining the authorized parent's
    /// operation pins until recursive cleanup completes. Missing is idempotent;
    /// invalid identities, links, and other I/O failures are not absence.
    pub fn remove_tree(&self, relative: &Path) -> io::Result<bool> {
        self.remove_tree_after_check(relative, || {})
    }

    fn remove_tree_after_check(
        &self,
        relative: &Path,
        before_mutation: impl FnOnce(),
    ) -> io::Result<bool> {
        validate_relative(relative)?;
        let parent = relative
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| denied("removal requires an owning directory"))?;
        let directory = match self.directory(parent, false) {
            Ok(directory) => directory,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let name = relative.file_name().expect("validated descendant");
        let path = directory.path.join(name);
        match reject_entry_link(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        if !fs::metadata(&path)?.is_dir() {
            return Err(denied("attachment storage entry is not a directory"));
        }
        before_mutation();
        let target = WriteTarget::open(path, false)?;
        match target.delete_entry(true) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn anchor(&self) -> io::Result<&root::RootAnchor> {
        self.anchor.as_ref().as_ref().map_err(|(kind, message)| {
            io::Error::new(*kind, format!("storage root capture failed: {message}"))
        })
    }
}

impl OwnedDirectory {
    /// Pinned physical directory; callers may retain logical paths in DTOs.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Publish a fresh file rather than truncating an existing inode (which
    /// could be hard-linked elsewhere). Reuse the file tools' staging boundary.
    pub fn write_file(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        self.write_file_after_check(name, bytes, || {})
    }

    fn write_file_after_check(
        &self,
        name: &str,
        bytes: &[u8],
        before_mutation: impl FnOnce(),
    ) -> io::Result<PathBuf> {
        validate_storage_name(name)?;
        let path = self.path.join(name);
        match reject_entry_link(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        before_mutation();
        let target = WriteTarget::open(path.clone(), false)?;
        let mut stage = target.stage()?;
        stage.file_mut().write_all(bytes)?;
        stage.file_mut().sync_all()?;
        stage.publish(&target, true)?;
        Ok(path)
    }

    /// Read an ordinary child through the shared operation-time open boundary.
    pub fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        validate_storage_name(name)?;
        let path = self.path.join(name);
        reject_entry_link(&path)?;
        let mut file = super::open_physical_read(&path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Portable storage names deliberately exclude Windows aliases even on Unix.
/// Filenames may contain dots/Unicode; an identity layer can be stricter.
pub fn validate_storage_name(name: &str) -> io::Result<()> {
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || device
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid storage name",
        ));
    }
    Ok(())
}

fn validate_relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(denied("storage descendant is empty"));
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(denied(
                "storage descendant must contain ordinary components",
            ));
        };
        validate_storage_name(
            name.to_str()
                .ok_or_else(|| denied("non-Unicode storage name"))?,
        )?;
    }
    Ok(())
}

fn reject_links(root: &Path, relative: &Path) -> io::Result<()> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match reject_entry_link(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn reject_entry_link(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if metadata.file_type().is_symlink() || reparse {
        return Err(denied("links are not attachment storage identities"));
    }
    Ok(())
}

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_storage_rejects_invalid_descendants_before_creation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("storage");
        let root = OwnedDirectoryRoot::new(path.clone());
        for relative in [
            "",
            ".",
            "..",
            "session/..",
            "../outside",
            "session/name.",
            "session/NUL.txt",
        ] {
            assert!(
                root.directory(Path::new(relative), true).is_err(),
                "{relative}"
            );
            assert!(root.remove_tree(Path::new(relative)).is_err(), "{relative}");
            assert!(!path.exists());
        }
        assert!(root.remove_tree(Path::new("single-component")).is_err());
    }

    #[test]
    fn owned_storage_normal_roundtrip_keeps_host_uploaded_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = OwnedDirectoryRoot::new(temp.path().join("storage"));
        let relative = Path::new("session/attachment.v1");
        let directory = root.directory(relative, true).unwrap();
        directory
            .write_file("my-secret.文档.txt", b"fixture")
            .unwrap();
        assert_eq!(
            directory.read_file("my-secret.文档.txt").unwrap(),
            b"fixture"
        );
        drop(directory);
        assert!(root.remove_tree(relative).unwrap());
        assert!(!root.remove_tree(relative).unwrap());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn operation_guard_pins_root_bucket_and_empty_attachment_through_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("attachments");
        let root = OwnedDirectoryRoot::new(root_path.clone());
        let directory = root.directory(Path::new("session/first"), true).unwrap();
        let physical = directory.path().to_path_buf();
        assert!(
            fs::read_dir(&physical).unwrap().count() > 0,
            "empty directory must have a live guard"
        );
        directory
            .write_file_after_check("canary.txt", b"new", || {
                for path in [
                    &root_path,
                    &root_path.join("session"),
                    &root_path.join("session/first"),
                ] {
                    let error = fs::rename(path, temp.path().join("renamed")).unwrap_err();
                    assert_eq!(error.raw_os_error(), Some(32), "{path:?}: {error}");
                }
            })
            .unwrap();
        assert_eq!(directory.read_file("canary.txt").unwrap(), b"new");
        drop(directory);
        fs::rename(&physical, temp.path().join("renamed")).unwrap();
    }

    #[test]
    fn hardlink_installed_after_write_check_does_not_redirect_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = OwnedDirectoryRoot::new(temp.path().join("attachments"));
        let directory = root.directory(Path::new("session/first"), true).unwrap();
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        directory
            .write_file_after_check("canary.txt", b"new", || {
                fs::hard_link(&outside, directory.path().join("canary.txt")).unwrap();
            })
            .unwrap();
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
        assert_eq!(directory.read_file("canary.txt").unwrap(), b"new");
    }

    #[test]
    fn junction_installed_after_removal_check_never_deletes_target_contents() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("attachments");
        let original = root_path.join("session/first");
        let parked = root_path.join("session/parked");
        let outside = temp.path().join("outside");
        for dir in [&original, &outside] {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join("canary.txt"), "owned canary").unwrap();
        }
        let root = OwnedDirectoryRoot::new(root_path);
        let result = root.remove_tree_after_check(Path::new("session/first"), || {
            fs::rename(&original, &parked).unwrap();
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(original.to_string_lossy().replace('/', "\\"))
                .arg(outside.to_string_lossy().replace('/', "\\"))
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        });
        eprintln!("H-58 removal after final-entry junction replacement: {result:?}");
        assert_eq!(
            fs::read_to_string(outside.join("canary.txt")).unwrap(),
            "owned canary"
        );
        assert_eq!(
            fs::read_to_string(parked.join("canary.txt")).unwrap(),
            "owned canary"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        if fs::symlink_metadata(&original).is_ok() {
            // Unlink only this fixture-created junction, never recurse.
            fs::remove_dir(&original).unwrap();
        }
    }
}
