//! Cross-process exclusive file lock — plan §6.2's one mutual-exclusion
//! mechanism for memory writes.
//!
//! The desktop app and the headless CLI can point at the same data root, so an
//! in-process mutex is not enough: the vault and the session note both take this
//! lock around their writes. It is an OS record lock on a dedicated `.lock`
//! file, which Windows byte-range locks and POSIX `flock` both release when the
//! holder's handle closes — including when the holder dies. That is why there is
//! no PID file and no stale-lock reclaim path to maintain.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;

/// How long a writer waits for whoever holds the lock before giving up. Kept
/// short because these writes sit on the request path: a stalled UI is worse
/// than a retriable "locked" error.
pub const WAIT: Duration = Duration::from_millis(500);

/// An exclusively held lock, released when dropped.
#[derive(Debug)]
pub struct FileLock {
    file: std::fs::File,
    what: &'static str,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(%error, what = self.what, "unlock failed");
        }
    }
}

/// Whether a failed `try_lock_exclusive` means "someone else is inside" rather
/// than "the lock cannot be taken at all".
///
/// POSIX reports contention as `EAGAIN`/`EACCES`, which std already classifies.
/// Windows reports a contended immediate lock as `ERROR_LOCK_FAILED` (33) with
/// an `Uncategorized` kind, so its raw code has to be checked — measured here
/// rather than assumed.
fn contention(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::PermissionDenied
    ) {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(33) | Some(36))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Take `path` exclusively, waiting at most `wait`. `what` names the guarded
/// resource in the error, so a caller can tell a stalled vault from a busy note.
pub fn acquire(path: &Path, wait: Duration, what: &'static str) -> Result<FileLock> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| CoreError::Persistence(format!("create lock directory: {error}")))?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| CoreError::Persistence(format!("open {what} lock: {error}")))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(FileLock { file, what }),
            Err(error) if contention(&error) => {
                if Instant::now() >= deadline {
                    return Err(CoreError::Persistence(format!(
                        "{what} is held by another writer: {}",
                        path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(CoreError::Persistence(format!("lock {what}: {error}")));
            }
        }
    }
}

/// The lock file guarding `target`, as a hidden sibling so a directory scan for
/// content files cannot mistake it for one.
pub fn sidecar(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "memory".to_string());
    target.with_file_name(format!(".{name}.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_blocks_the_next_writer_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".note.lock");
        let holder = acquire(&path, WAIT, "session note").unwrap();
        let shared = path.clone();
        let waiting = std::thread::spawn(move || acquire(&shared, WAIT, "session note").is_ok());
        std::thread::sleep(Duration::from_millis(50));
        drop(holder);
        assert!(
            waiting.join().unwrap(),
            "the lock stayed held after its owner dropped it"
        );
    }

    #[test]
    fn a_stuck_holder_times_out_and_names_the_resource() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".note.lock");
        let holder = acquire(&path, WAIT, "session note").unwrap();
        let error = acquire(&path, Duration::from_millis(50), "session note").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("session note is held by another writer"),
            "{error}"
        );
        drop(holder);
    }

    #[test]
    fn sidecars_are_hidden_siblings_of_the_file_they_guard() {
        let note = Path::new("/data/sessions/e0/session-memory.md");
        assert_eq!(
            sidecar(note),
            PathBuf::from("/data/sessions/e0/.session-memory.md.lock")
        );
    }
}
