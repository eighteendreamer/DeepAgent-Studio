//! Verified, serialized activation of plugin runtime archives.
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;

use crate::atomic_file::remove_dir_all_with_retry;
use crate::plugin::spec::resolve_existing_within;

const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A matching archive marker is only an identity hint: verify extracted bytes
/// too, so missing dependencies and same-size damage are repaired automatically.
pub fn prepare_runtime_payload(payload_root: &Path, data_dir: &Path) -> Result<()> {
    let archive_path = payload_root.join("runtime.zip");
    if !archive_path.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(data_dir).map_err(|e| io_error("create plugin data", e))?;
    let _lock = acquire_lock(data_dir)?;
    std::fs::create_dir_all(data_dir.join("workspace"))
        .map_err(|e| io_error("create payload workspace", e))?;
    let runtime = data_dir.join("runtime");
    let previous = data_dir.join(".runtime.previous");
    // Recover an interrupted activation before inspecting its marker.
    if !runtime.exists() && previous.exists() {
        std::fs::rename(&previous, &runtime)
            .map_err(|e| io_error("recover previous runtime", e))?;
    }
    let digest = crate::runtime_service::sha256_file(&archive_path)?;
    let file = File::open(&archive_path).map_err(|e| io_error("open runtime archive", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| io_error("read runtime archive", e))?;
    validate_archive(&mut archive)?;
    let marker_matches = std::fs::read_to_string(runtime.join(".payload-sha256"))
        .ok()
        .as_deref()
        == Some(&digest);
    if marker_matches && payload_matches(&mut archive, &runtime)? {
        return Ok(());
    }
    tracing::info!(path = %runtime.display(), marker_matches, "preparing verified plugin runtime payload");
    let stage = tempfile::Builder::new()
        .prefix(".runtime-stage-")
        .tempdir_in(data_dir)
        .map_err(|e| io_error("create runtime stage", e))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| io_error("read payload entry", e))?;
        let output = stage.path().join(safe_entry_path(&entry)?);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|e| io_error("create payload directory", e))?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| io_error("create payload parent", e))?;
            }
            let mut out = File::create(&output).map_err(|e| io_error("create payload file", e))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| io_error("extract payload entry", e))?;
        }
    }
    // Publish the marker with the completed tree, never before extraction succeeds.
    std::fs::write(stage.path().join(".payload-sha256"), &digest)
        .map_err(|e| io_error("write runtime marker", e))?;
    activate(stage.path(), &runtime, &previous)?;
    Ok(())
}

fn acquire_lock(data: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join(".runtime.lock"))
        .map_err(|e| io_error("open runtime lock", e))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file), // Closing the handle releases the OS lock.
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock || e.raw_os_error() == Some(33) =>
            {
                if Instant::now() >= deadline {
                    return Err(CoreError::other(
                        "timed out waiting for plugin runtime preparation",
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(io_error("lock runtime preparation", e)),
        }
    }
}

fn safe_entry_path(entry: &zip::read::ZipFile<'_>) -> Result<PathBuf> {
    if entry
        .unix_mode()
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err(CoreError::invalid(
            "runtime payload contains a symbolic link",
        ));
    }
    entry
        .enclosed_name()
        .ok_or_else(|| CoreError::invalid("runtime payload contains an unsafe path"))
}

fn validate_archive(archive: &mut zip::ZipArchive<File>) -> Result<()> {
    if archive.len() > MAX_FILES {
        return Err(CoreError::invalid(
            "runtime payload contains too many files",
        ));
    }
    let mut bytes = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| io_error("inspect payload entry", e))?;
        safe_entry_path(&entry)?;
        bytes = bytes.saturating_add(entry.size());
        if bytes > MAX_BYTES {
            return Err(CoreError::invalid("runtime payload exceeds 512 MiB"));
        }
    }
    Ok(())
}

fn payload_matches(archive: &mut zip::ZipArchive<File>, runtime: &Path) -> Result<bool> {
    if std::fs::symlink_metadata(runtime).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(false);
    }
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| io_error("verify payload entry", e))?;
        let expected = runtime.join(safe_entry_path(&entry)?);
        let Ok(path) = resolve_existing_within(runtime, &expected) else {
            return Ok(false);
        };
        if entry.is_dir() {
            if !path.is_dir() {
                return Ok(false);
            }
            continue;
        }
        let Ok(mut actual) = File::open(&path) else {
            return Ok(false);
        };
        if !actual
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() == entry.size())
        {
            return Ok(false);
        }
        let mut source = [0u8; 32 * 1024];
        let mut target = [0u8; 32 * 1024];
        loop {
            let count = entry
                .read(&mut source)
                .map_err(|e| io_error("verify archived bytes", e))?;
            if count == 0 {
                break;
            }
            if actual.read_exact(&mut target[..count]).is_err()
                || source[..count] != target[..count]
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn activate(stage: &Path, runtime: &Path, previous: &Path) -> Result<()> {
    remove_dir_all_with_retry(previous)
        .map_err(|e| io_error("clear previous runtime backup", e))?;
    let had_previous = runtime.exists();
    if had_previous {
        // Never delete the live tree in place: a Windows sharing violation must
        // leave its files and marker intact rather than produce a partial cache.
        std::fs::rename(runtime, previous).map_err(|e| {
            io_error(
                "back up runtime (close running plugin processes and retry)",
                e,
            )
        })?;
    }
    if let Err(activation_error) = std::fs::rename(stage, runtime) {
        if had_previous {
            std::fs::rename(previous, runtime).map_err(|rollback_error| CoreError::Persistence(format!(
                "activate runtime: {activation_error}; rollback failed: {rollback_error}; previous runtime remains at {}", previous.display())))?;
        }
        return Err(io_error("activate runtime payload", activation_error));
    }
    if had_previous {
        if let Err(error) = remove_dir_all_with_retry(previous) {
            tracing::warn!(path = %previous.display(), %error, "runtime activated; old backup is still in use");
        }
    }
    Ok(())
}

fn io_error(action: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Persistence(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(root: &Path, value: &[u8]) {
        std::fs::create_dir_all(root).unwrap();
        let mut zip = zip::ZipWriter::new(File::create(root.join("runtime.zip")).unwrap());
        zip.start_file("value.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(value).unwrap();
        zip.start_file("nested/keep.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"keep").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn concurrent_preparations_publish_one_complete_tree_and_keep_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let data = tmp.path().join("data");
        archive(&source, b"complete");
        std::fs::create_dir_all(data.join("workspace")).unwrap();
        std::fs::write(data.join("workspace/user.txt"), b"user").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        std::thread::scope(|scope| {
            for _ in 0..6 {
                let barrier = barrier.clone();
                let source = &source;
                let data = &data;
                scope.spawn(move || {
                    barrier.wait();
                    prepare_runtime_payload(source, data).unwrap();
                });
            }
        });
        assert_eq!(
            std::fs::read(data.join("runtime/value.txt")).unwrap(),
            b"complete"
        );
        assert_eq!(
            std::fs::read(data.join("runtime/nested/keep.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(
            std::fs::read(data.join("workspace/user.txt")).unwrap(),
            b"user"
        );
        assert!(!std::fs::read_dir(data).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".runtime-stage-")));
    }

    #[test]
    fn invalid_archive_preserves_live_cache_and_interrupted_activation_recovers() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let data = tmp.path().join("data");
        archive(&source, b"old");
        prepare_runtime_payload(&source, &data).unwrap();
        let marker = std::fs::read(data.join("runtime/.payload-sha256")).unwrap();
        std::fs::write(source.join("runtime.zip"), b"broken zip").unwrap();
        assert!(prepare_runtime_payload(&source, &data).is_err());
        assert_eq!(
            std::fs::read(data.join("runtime/value.txt")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(data.join("runtime/.payload-sha256")).unwrap(),
            marker
        );
        archive(&source, b"new");
        std::fs::rename(data.join("runtime"), data.join(".runtime.previous")).unwrap();
        prepare_runtime_payload(&source, &data).unwrap();
        assert_eq!(
            std::fs::read(data.join("runtime/value.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn activation_failure_rolls_back_without_deleting_live_files() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::write(runtime.join("keep.txt"), b"keep").unwrap();
        assert!(activate(
            &tmp.path().join("missing-stage"),
            &runtime,
            &tmp.path().join("previous")
        )
        .is_err());
        assert_eq!(std::fs::read(runtime.join("keep.txt")).unwrap(), b"keep");
    }
}
