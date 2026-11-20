use super::tests::{noop_progress, raw_entry, sha256_of, BytesDownloader};
use super::*;

fn service(root: &Path, bytes: &[u8], entry: RuntimeEntry) -> RuntimeService {
    RuntimeService::with_registry(
        &[root.join("runtimes")],
        Arc::new(BytesDownloader {
            bytes: bytes.to_vec(),
        }),
        vec![entry],
    )
    .with_state_root(root)
}

fn ledger(root: &Path) -> LedgerStore {
    LedgerStore::at(root, root.join(crate::install_ledger::LEDGER_FILE))
}

#[tokio::test]
async fn unpinned_download_records_actual_hash_after_successful_install() {
    let root = tempfile::tempdir().unwrap();
    let svc = service(root.path(), b"abc", raw_entry(None));
    let installed = svc.install("test-model", noop_progress()).await.unwrap();
    assert!(installed.installed);
    assert!(
        !installed.checksum_pinned,
        "a local observation is not an upstream pin"
    );
    let records = ledger(root.path()).entries().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].sha256.as_deref(),
        Some(sha256_of(b"abc").as_str())
    );
    assert_eq!(records[0].version.as_deref(), Some("v1"));
    assert_eq!(records[0].source.spec, "https://example.com/model.bin");
}

#[tokio::test]
async fn same_version_and_source_reuses_persisted_hash_after_restart() {
    let root = tempfile::tempdir().unwrap();
    service(root.path(), b"abc", raw_entry(None))
        .install("test-model", noop_progress())
        .await
        .unwrap();
    let svc = service(root.path(), b"changed", raw_entry(None));
    let error = svc
        .install("test-model", noop_progress())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("checksum mismatch"));
    assert_eq!(
        std::fs::read(root.path().join("runtimes/speech/models/model.bin")).unwrap(),
        b"abc"
    );
    assert_eq!(
        ledger(root.path()).entries().unwrap()[0].sha256,
        Some(sha256_of(b"abc"))
    );
    assert!(!std::fs::read_dir(root.path().join("runtimes"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".download-")));
}

#[tokio::test]
async fn unchanged_download_matches_persisted_hash() {
    let root = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        service(root.path(), b"abc", raw_entry(None))
            .install("test-model", noop_progress())
            .await
            .unwrap();
    }
    assert_eq!(ledger(root.path()).entries().unwrap().len(), 1);
}

#[tokio::test]
async fn version_or_source_change_does_not_reuse_old_hash() {
    for change_version in [true, false] {
        let root = tempfile::tempdir().unwrap();
        service(root.path(), b"abc", raw_entry(None))
            .install("test-model", noop_progress())
            .await
            .unwrap();
        let mut entry = raw_entry(None);
        if change_version {
            entry.version = "v2".to_string();
        } else {
            entry.artifacts.get_mut(&Platform::current()).unwrap().url =
                "https://example.com/new-model.bin".to_string();
        }
        let installed = service(root.path(), b"new version", entry)
            .install("test-model", noop_progress())
            .await
            .unwrap();
        assert!(installed.installed);
        assert_eq!(
            ledger(root.path()).entries().unwrap()[0].sha256,
            Some(sha256_of(b"new version"))
        );
    }
}

#[tokio::test]
async fn upstream_pin_takes_precedence_over_local_observation() {
    let root = tempfile::tempdir().unwrap();
    service(root.path(), b"abc", raw_entry(None))
        .install("test-model", noop_progress())
        .await
        .unwrap();
    let svc = service(
        root.path(),
        b"trusted update",
        raw_entry(Some(sha256_of(b"trusted update"))),
    );
    assert!(
        svc.install("test-model", noop_progress())
            .await
            .unwrap()
            .installed
    );
    assert_eq!(
        ledger(root.path()).entries().unwrap()[0].sha256,
        Some(sha256_of(b"trusted update"))
    );
}

#[tokio::test]
async fn failed_extraction_does_not_record_a_hash() {
    let root = tempfile::tempdir().unwrap();
    let mut entry = raw_entry(None);
    entry
        .artifacts
        .get_mut(&Platform::current())
        .unwrap()
        .archive = ArchiveKind::Zip;
    let svc = service(root.path(), b"not a zip", entry);
    assert!(svc.install("test-model", noop_progress()).await.is_err());
    assert!(ledger(root.path()).entries().unwrap().is_empty());
}

#[tokio::test]
async fn corrupt_ledger_does_not_silently_reset_the_baseline() {
    let root = tempfile::tempdir().unwrap();
    let svc = service(root.path(), b"abc", raw_entry(None));
    std::fs::write(ledger(root.path()).path(), b"invalid json").unwrap();
    let error = svc
        .install("test-model", noop_progress())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("parse ledger"));
    assert!(!svc.is_installed("test-model"));
}

#[tokio::test]
async fn fallback_records_the_url_that_supplied_the_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut entry = raw_entry(None);
    entry
        .artifacts
        .get_mut(&Platform::current())
        .unwrap()
        .mirror_urls = vec!["https://mirror.example.com/model.bin".to_string()];
    let svc = RuntimeService::with_registry(
        &[root.path().join("runtimes")],
        Arc::new(super::tests::MirrorFallbackDownloader {
            bytes: b"abc".to_vec(),
            seen_urls: Mutex::new(Vec::new()),
        }),
        vec![entry.clone()],
    )
    .with_state_root(root.path());
    svc.install("test-model", noop_progress()).await.unwrap();
    let record = ledger(root.path()).entries().unwrap().remove(0);
    assert_eq!(record.source.spec, "https://mirror.example.com/model.bin");
    assert_eq!(record.sha256, Some(sha256_of(b"abc")));
    let error = service(root.path(), b"changed", entry)
        .install("test-model", noop_progress())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("checksum mismatch"));
}
