//! Native no-clobber publication shared by file tools and checkpoint restore.
//!
//! No-clobber moves never fall back to copy/delete or replacement. The path
//! operation gate coordinates cooperating read/modify/write implementations;
//! it is advisory and cannot exclude arbitrary external filesystem writers.

use std::io;
use std::path::Path;

/// Stable advisory ownership for one path, independent of the inode replaced
/// by an atomic write. Lock files must not be unlinked while owners are live.
#[must_use]
pub struct PathOperationGuard {
    _file: std::fs::File,
}

/// Serialize cooperating file operations across caches and host processes.
/// The parent must already exist. Canonicalizing it also shares ownership for
/// ordinary parent aliases; this is not a path-authorization or sandbox check.
pub fn lock_path_operation(path: &Path) -> io::Result<PathOperationGuard> {
    use sha2::{Digest, Sha256};
    use std::time::{Duration, Instant};

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("file path has no name"))?;
    let identity = std::fs::canonicalize(parent)?.join(name);
    #[cfg(windows)]
    let identity = std::path::PathBuf::from(identity.to_string_lossy().to_lowercase());
    let digest = Sha256::digest(identity.as_os_str().as_encoded_bytes());
    let directory = std::env::temp_dir().join("deepagent-file-operation-locks-v1");
    std::fs::create_dir_all(&directory)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{digest:x}.lock")))?;
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
