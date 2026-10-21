//! Verified, serialized activation of plugin runtime archives.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;

use crate::atomic_file::remove_dir_all_with_retry;
use crate::plugin::spec::resolve_existing_within;

// Host maintenance only. One disk-heavy job across plugins and bundled MCP.
static DISK_PREPARATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn check_cancel(cancel: &deepagent_runtime::CancellationTree) -> Result<()> {
    if cancel.is_cancelled() {
        Err(CoreError::other("plugin preparation cancelled"))
    } else {
        Ok(())
    }
}

const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A matching archive marker is only an identity hint: verify extracted bytes
/// too, so missing dependencies and same-size damage are repaired automatically.
pub fn prepare_runtime_payload(payload_root: &Path, data_dir: &Path) -> Result<()> {
    prepare_runtime_payload_with_cancel(payload_root, data_dir, &Default::default())
}

pub(crate) fn prepare_runtime_payload_with_cancel(
    payload_root: &Path,
    data_dir: &Path,
    cancel: &deepagent_runtime::CancellationTree,
) -> Result<()> {
    let archive_path = payload_root.join("runtime.zip");
    if !archive_path.is_file() {
        return Ok(());
    }
    let _disk = loop {
        check_cancel(cancel)?;
        match DISK_PREPARATION.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(25))
            }
        }
    };
    let started = Instant::now();
    std::fs::create_dir_all(data_dir).map_err(|e| io_error("create plugin data", e))?;
    let _lock = acquire_lock(data_dir, cancel)?;
    std::fs::create_dir_all(data_dir.join("workspace"))
        .map_err(|e| io_error("create payload workspace", e))?;
    let runtime = data_dir.join("runtime");
    let previous = data_dir.join(".runtime.previous");
    // Recover an interrupted activation before inspecting its marker.
    if !runtime.exists() && previous.exists() {
        std::fs::rename(&previous, &runtime)
            .map_err(|e| io_error("recover previous runtime", e))?;
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut source = File::open(&archive_path).map_err(|e| io_error("open runtime archive", e))?;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = source
            .read(&mut buffer)
            .map_err(|e| io_error("hash runtime archive", e))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = format!("{:x}", hasher.finalize());
    let file = File::open(&archive_path).map_err(|e| io_error("open runtime archive", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| io_error("read runtime archive", e))?;
    validate_archive(&mut archive, cancel)?;
    let marker_matches = std::fs::read_to_string(runtime.join(".payload-sha256"))
        .ok()
        .as_deref()
        == Some(&digest);
    if marker_matches && payload_matches(&mut archive, &runtime, cancel)? {
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "plugin payload verified"
        );
        return Ok(());
    }
    tracing::info!(path = %runtime.display(), marker_matches, "preparing verified plugin runtime payload");
    let stage = tempfile::Builder::new()
        .prefix(".runtime-stage-")
        .tempdir_in(data_dir)
        .map_err(|e| io_error("create runtime stage", e))?;
    for index in 0..archive.len() {
        check_cancel(cancel)?;
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
            loop {
                check_cancel(cancel)?;
                let count = entry
                    .read(&mut buffer)
                    .map_err(|e| io_error("read payload entry", e))?;
                if count == 0 {
                    break;
                }
                out.write_all(&buffer[..count])
                    .map_err(|e| io_error("extract payload entry", e))?;
            }
        }
    }
    // Publish the marker with the completed tree, never before extraction succeeds.
    std::fs::write(stage.path().join(".payload-sha256"), &digest)
        .map_err(|e| io_error("write runtime marker", e))?;
    check_cancel(cancel)?;
    activate(stage.path(), &runtime, &previous)?;
    Ok(())
}

fn acquire_lock(data: &Path, cancel: &deepagent_runtime::CancellationTree) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join(".runtime.lock"))
        .map_err(|e| io_error("open runtime lock", e))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        check_cancel(cancel)?;
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

fn validate_archive(
    archive: &mut zip::ZipArchive<File>,
    cancel: &deepagent_runtime::CancellationTree,
) -> Result<()> {
    if archive.len() > MAX_FILES {
        return Err(CoreError::invalid(
            "runtime payload contains too many files",
        ));
    }
    let mut bytes = 0u64;
    for index in 0..archive.len() {
        check_cancel(cancel)?;
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

fn payload_matches(
    archive: &mut zip::ZipArchive<File>,
    runtime: &Path,
    cancel: &deepagent_runtime::CancellationTree,
) -> Result<bool> {
    if std::fs::symlink_metadata(runtime).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(false);
    }
    for index in 0..archive.len() {
        check_cancel(cancel)?;
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
            check_cancel(cancel)?;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "audit integration: exclusively holds the production disk-preparation gate"]
    async fn blocked_host_payload_preparation_does_not_block_conversations() {
        use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
        use crate::{ChatService, McpService, MemorySecretStore, PluginService, SettingsService};
        use deepagent_models::transport::MockTransport;
        use deepagent_persistence::Database;
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("chat-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let roots = crate::plugin_loader::PluginRoots {
            session: Vec::new(),
            builtin: tmp.path().join("builtin"),
            workspace: None,
            personal: tmp.path().join("personal"),
        };
        let plugin = roots.builtin.join("audit-slow");
        std::fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin.join("skills/audit-slow-skill")).unwrap();
        std::fs::write(
            plugin.join(".codex-plugin/plugin.json"),
            r#"{"name":"audit-slow","version":"0.1.0","skills":"skills"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("skills/audit-slow-skill/SKILL.md"),
            "---\nname: audit-slow-skill\ndescription: Audit payload readiness\n---\nReady only after payload verification.",
        )
        .unwrap();
        archive(&plugin, b"verified payload");
        let install = tmp.path().join("app-data");
        let data = install.join("plugins/data/audit-slow-builtin");
        std::fs::create_dir_all(&data).unwrap();
        let held_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data.join(".runtime.lock"))
            .unwrap();
        FileExt::lock_exclusive(&held_lock).unwrap();

        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("sk-audit-placeholder").await.unwrap();
        let plugins = Arc::new(PluginService::new(roots, &install));
        let skills = Arc::new(Mutex::new(
            crate::SkillsService::open_v2(deepagent_skills::SkillsRoots {
                builtin: tmp.path().join("skills/builtin"),
                user: tmp.path().join("skills/user"),
                marketplace: tmp.path().join("skills/marketplace"),
                workspace: None,
            })
            .unwrap(),
        ));
        let (_, model) = scripted_client_with_transport(vec![
            ScriptedTransport::text("first"),
            ScriptedTransport::text("second"),
            ScriptedTransport::text("ready"),
        ]);
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace)
            .with_plugins(plugins.clone())
            .with_mcp(Arc::new(McpService::new(db)))
            .with_skills(skills.clone());
        chat.start_plugin_preparation(Arc::new(|| {}));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // No other preparation runs in this explicitly isolated test.
                // Holding this gate proves the host worker entered payload
                // preparation and is waiting behind our real OS file lock.
                if matches!(
                    DISK_PREPARATION.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("host worker must enter the production disk preparation path");
        assert!(plugins.ready_snapshot().0.skill_roots.is_empty());

        let began = Instant::now();
        let conversations = tokio::time::timeout(Duration::from_secs(5), async {
            let mut session = None;
            for run in ["payload-wait-first", "payload-wait-second"] {
                session = Some(
                    chat.run_in_session(
                        "Reply briefly.",
                        session.as_deref(),
                        None,
                        None,
                        Vec::new(),
                        None,
                        false,
                        Some(run),
                        |_| {},
                        |_| {},
                    )
                    .await?,
                );
                assert!(!model
                    .last_prompt()
                    .contains("- audit-slow-skill: Audit payload readiness"));
            }
            Ok::<_, CoreError>(())
        })
        .await;
        let elapsed = began.elapsed();
        let unpublished_while_locked = plugins.ready_snapshot().0.skill_roots.is_empty();
        let unextracted_while_locked = !data.join("runtime/value.txt").exists();
        // Release even if the conversation times out, so the blocking host
        // worker can exit and Tokio shutdown cannot hang behind the fixture.
        FileExt::unlock(&held_lock).unwrap();
        let published = tokio::time::timeout(Duration::from_secs(5), async {
            while !skills
                .lock()
                .unwrap()
                .manager()
                .registry()
                .contains("audit-slow-skill")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        eprintln!(
            "audit payload wait: two_turns_ms={}, unpublished_while_locked={unpublished_while_locked}, unextracted_while_locked={unextracted_while_locked}, published_after_release={}",
            elapsed.as_millis(), published.is_ok()
        );
        assert!(matches!(conversations, Ok(Ok(()))), "{conversations:?}");
        assert!(unpublished_while_locked && unextracted_while_locked);
        assert!(published.is_ok());
        assert_eq!(
            std::fs::read(data.join("runtime/value.txt")).unwrap(),
            b"verified payload"
        );
        chat.run_in_session(
            "Reply briefly.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("payload-now-ready"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        assert!(model
            .last_prompt()
            .contains("- audit-slow-skill: Audit payload readiness"));
    }
}
