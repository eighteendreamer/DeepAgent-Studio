//! Native no-clobber publication shared by file tools and checkpoint restore.
//!
//! No-clobber moves never fall back to copy/delete or replacement. The path
//! operation gate coordinates cooperating read/modify/write implementations;
//! it is advisory and cannot exclude arbitrary external filesystem writers.

use std::io;
use std::path::Path;

pub mod access;

/// Stable advisory ownership for one path, independent of the inode replaced
/// by an atomic write. Lock files must not be unlinked while owners are live.
#[must_use]
pub struct PathOperationGuard {
    _file: std::fs::File,
}

/// Serialize cooperating file operations across caches and host processes.
/// The deepest existing parent determines ownership even before directories
/// are created. This is advisory coordination, not path authorization.
pub fn lock_path_operation(path: &Path) -> io::Result<PathOperationGuard> {
    use sha2::{Digest, Sha256};

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("file path has no name"))?;
    let identity = canonicalize_with_missing_suffix(parent)?.join(name);
    #[cfg(windows)]
    let identity = std::path::PathBuf::from(identity.to_string_lossy().to_lowercase());
    let digest = Sha256::digest(identity.as_os_str().as_encoded_bytes());
    let directory = std::env::temp_dir().join("deepagent-file-operation-locks-v1");
    std::fs::create_dir_all(&directory)?;
    lock_operation_file(&directory.join(format!("{digest:x}.lock")))
}

/// The same bounded operation gate at a caller-owned, stable lock-file path.
/// Use this when cooperating hosts can have different temporary directories.
/// The parent must exist; never unlink the lock file while hosts can use it.
pub fn lock_operation_file(path: &Path) -> io::Result<PathOperationGuard> {
    use std::time::{Duration, Instant};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(PathOperationGuard { _file: file }),
            Err(error)
                if error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
                    || error.kind() == io::ErrorKind::WouldBlock =>
            {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "file operation is busy",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Resolve through the deepest existing ancestor without creating anything.
/// A dangling link is an error, not a missing directory to be manufactured.
/// Callers still need their own authorization and operation-time I/O boundary.
pub fn canonicalize_with_missing_suffix(path: &Path) -> io::Result<std::path::PathBuf> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut physical) => {
                for name in suffix.into_iter().rev() {
                    physical.push(name);
                }
                return Ok(physical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(ancestor) {
                    Ok(_) => return Err(error),
                    Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {}
                    Err(metadata_error) => return Err(metadata_error),
                }
                let name = ancestor.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "path has no final component")
                })?;
                suffix.push(name.to_os_string());
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
pub fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // No REPLACE_EXISTING or COPY_ALLOWED: neither a racing target nor a
    // cross-volume failure may be hidden by an implicit delete/copy operation.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
pub fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos"
)))]
pub fn rename_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native no-replace move is unavailable on this platform",
    ))
}

#[cfg(test)]
mod path_identity_tests {
    use super::*;

    #[test]
    fn explicit_gate_uses_the_stable_file_and_releases_with_its_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared.lock");
        let guard = lock_operation_file(&path).unwrap();
        let observer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let error = fs2::FileExt::try_lock_exclusive(&observer).unwrap_err();
        assert!(
            error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
        );
        drop(guard);
        fs2::FileExt::try_lock_exclusive(&observer).unwrap();
        drop(observer);
        assert!(
            path.is_file(),
            "ownership must not unlink a shared lock path"
        );
        let missing = directory.path().join("not-created/lock");
        assert!(lock_operation_file(&missing).is_err());
        assert!(!missing.parent().unwrap().exists());
    }

    #[test]
    fn missing_parent_identity_is_stable_after_creation_and_does_not_create_paths() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("new").join("nested");
        let before = canonicalize_with_missing_suffix(&parent).unwrap();
        let guard = lock_path_operation(&parent.join("file.txt")).unwrap();
        assert!(!parent.exists());
        std::fs::create_dir_all(&parent).unwrap();
        assert_eq!(before, std::fs::canonicalize(&parent).unwrap());
        drop(guard);
    }
}
