use super::*;

fn fixture() -> (tempfile::TempDir, CanvasArtifactService) {
    let root = tempfile::tempdir().unwrap();
    let service = CanvasArtifactService::new(
        root.path().join("media"),
        Arc::new(Database::open_in_memory().unwrap()),
    )
    .unwrap();
    (root, service)
}

fn assert_integrity_error(error: CoreError, record: &ArtifactRecord) {
    let message = error.to_string();
    assert!(message.contains("artifact_integrity_mismatch"), "{message}");
    assert!(message.contains(&record.id), "{message}");
    if let Some(digest) = &record.digest {
        assert!(message.contains(digest), "{message}");
    }
}

#[test]
fn h250_all_materializations_verify_bytes_and_preserve_the_original_index() {
    let (_root, service) = fixture();
    let original = b"original synthetic media";
    for owner in [None, Some("ws-a"), Some("ws-b")] {
        for kind in [
            ArtifactKind::Image,
            ArtifactKind::Audio,
            ArtifactKind::Video,
            ArtifactKind::Document,
        ] {
            for change in ["same-size", "truncate", "append", "empty", "missing"] {
                let item = service.import_bytes(kind, None, original, owner).unwrap();
                let record = service.record(&item.id, owner).unwrap().unwrap();
                assert_eq!(
                    service.read_bytes(&item.id, owner).unwrap().unwrap(),
                    original
                );
                assert!(service
                    .resolve_for_provider(&item.uri, owner)
                    .unwrap()
                    .is_some());
                assert_eq!(
                    service
                        .display_target(&item.uri, owner)
                        .unwrap()
                        .unwrap()
                        .path,
                    item.path
                );
                if kind == ArtifactKind::Video {
                    service.validate_video_reference(&item.uri, owner).unwrap();
                }
                match change {
                    "same-size" => std::fs::write(&item.path, vec![b'x'; original.len()]).unwrap(),
                    "truncate" => std::fs::write(&item.path, &original[..3]).unwrap(),
                    "append" => {
                        std::fs::write(&item.path, [original.as_slice(), b"suffix"].concat())
                            .unwrap()
                    }
                    "empty" => std::fs::write(&item.path, []).unwrap(),
                    "missing" => std::fs::remove_file(&item.path).unwrap(),
                    _ => unreachable!(),
                }
                assert_integrity_error(service.read_bytes(&item.id, owner).unwrap_err(), &record);
                assert_integrity_error(
                    service.resolve_for_provider(&item.uri, owner).unwrap_err(),
                    &record,
                );
                assert_integrity_error(
                    service.display_target(&item.uri, owner).unwrap_err(),
                    &record,
                );
                if kind == ArtifactKind::Video {
                    assert_integrity_error(
                        service.video_file_path(&item.uri, owner).unwrap_err(),
                        &record,
                    );
                }
                assert_eq!(service.record(&item.id, owner).unwrap(), Some(record));
                let foreign = if owner == Some("ws-b") {
                    Some("ws-a")
                } else {
                    Some("ws-b")
                };
                assert!(service.read_bytes(&item.id, foreign).unwrap().is_none());
                let error = service
                    .display_target(&item.uri, foreign)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("unknown artifact"));
                assert!(!error.contains("artifact_integrity_mismatch"));
            }
        }
    }
}

#[test]
fn h250_unverifiable_legacy_metadata_is_not_silently_blessed() {
    let (_root, service) = fixture();
    let item = service
        .import_bytes(ArtifactKind::Video, Some("video/mp4"), b"video", None)
        .unwrap();
    let original = service.record(&item.id, None).unwrap().unwrap();
    for digest in [
        None,
        Some(String::new()),
        Some("md5:untrusted".into()),
        Some(format!("sha256:{}", "g".repeat(64))),
    ] {
        let mut changed = original.clone();
        changed.digest = digest;
        ArtifactStore::new(&service.db).put(&changed).unwrap();
        assert_integrity_error(service.read_bytes(&item.id, None).unwrap_err(), &changed);
        assert_eq!(
            ArtifactStore::new(&service.db).get(&item.id).unwrap(),
            Some(changed)
        );
    }
    let mut changed = original.clone();
    changed.byte_size = -1;
    ArtifactStore::new(&service.db).put(&changed).unwrap();
    assert_integrity_error(service.read_bytes(&item.id, None).unwrap_err(), &changed);
    ArtifactStore::new(&service.db).put(&original).unwrap();
    assert_eq!(
        service.read_bytes(&item.id, None).unwrap().unwrap(),
        b"video"
    );
    let image = service
        .import_bytes(ArtifactKind::Image, None, b"image", None)
        .unwrap();
    assert!(service
        .video_file_path(&image.uri, None)
        .unwrap_err()
        .to_string()
        .contains("expected video"));
}

#[test]
fn h250_concurrent_external_rewrites_never_return_unverified_buffers() {
    let (_root, service) = fixture();
    let original = vec![b'a'; COPY_BUFFER_BYTES * 3];
    let changed = vec![b'b'; original.len()];
    let item = service
        .import_bytes(ArtifactKind::Image, None, &original, None)
        .unwrap();
    let record = service.record(&item.id, None).unwrap().unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        let writer = threads.spawn(|| {
            barrier.wait();
            for _ in 0..32 {
                std::fs::write(&item.path, &changed).unwrap();
                std::fs::write(&item.path, &original).unwrap();
            }
        });
        barrier.wait();
        for _ in 0..32 {
            match service.read_bytes(&item.id, None) {
                Ok(Some(bytes)) => assert!(
                    bytes == original,
                    "unverified buffer returned: {} bytes",
                    bytes.len()
                ),
                Err(error) => assert_integrity_error(error, &record),
                Ok(None) => panic!("the immutable index must not disappear"),
            }
        }
        writer.join().unwrap();
    });
    std::fs::write(&item.path, &original).unwrap();
    assert_eq!(
        service.read_bytes(&item.id, None).unwrap().unwrap(),
        original
    );
}
