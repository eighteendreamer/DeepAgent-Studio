use super::*;
use deepagent_persistence::Database;

fn fixture() -> (tempfile::TempDir, Arc<Database>, Arc<CanvasArtifactUploads>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let artifacts = Arc::new(CanvasArtifactService::new(dir.path(), db.clone()).unwrap());
    (dir, db, Arc::new(CanvasArtifactUploads::new(artifacts)))
}

fn begin(
    uploads: &CanvasArtifactUploads,
    scope: Option<&str>,
    caller: &str,
    kind: ArtifactKind,
    streaming: bool,
    size: u64,
) -> String {
    let mut action = json!({
        "action": if streaming { "begin_streaming" } else { "begin" },
        "kind": kind.as_str(), "mime": format!("{}/synthetic", kind.as_str()),
        "name": "synthetic-media", "size": size,
    });
    if streaming {
        action.as_object_mut().unwrap().remove("size");
    }
    uploads
        .apply(serde_json::from_value(action).unwrap(), scope, caller)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn unavailable(result: Result<Value>, id: &str) {
    match result.unwrap_err() {
        CoreError::ArtifactUploadUnavailable { upload_id } => assert_eq!(upload_id, id),
        other => panic!("expected opaque upload scope error, got {other}"),
    }
}

#[test]
fn h253_import_and_both_upload_modes_share_the_same_namespace_and_identity() {
    for scope in [None, Some("ws-a"), Some("ws-b")] {
        for kind in [
            ArtifactKind::Image,
            ArtifactKind::Audio,
            ArtifactKind::Video,
        ] {
            for streaming in [false, true] {
                if kind == ArtifactKind::Image && streaming {
                    continue; // The existing streaming API supports audio/video only.
                }
                let (_dir, _db, uploads) = fixture();
                let direct = uploads
                    .artifacts
                    .import_bytes(kind, None, b"media", scope)
                    .unwrap();
                let id = begin(&uploads, scope, "window-a", kind, streaming, 5);
                assert_eq!(uploads.artifacts.list(scope, None).unwrap().len(), 1);
                for (offset, bytes) in [(0, b"me".to_vec()), (2, b"dia".to_vec())] {
                    uploads
                        .apply(
                            ArtifactUploadAction::Write {
                                id: id.clone(),
                                offset,
                                bytes,
                            },
                            scope,
                            "window-a",
                        )
                        .unwrap();
                }
                let dto = uploads
                    .apply(
                        ArtifactUploadAction::Finish { id: id.clone() },
                        scope,
                        "window-a",
                    )
                    .unwrap();
                assert_eq!(dto["workspaceId"], json!(scope));
                let artifact_id = dto["id"].as_str().unwrap();
                let record = uploads
                    .artifacts
                    .record(artifact_id, scope)
                    .unwrap()
                    .unwrap();
                let imported = uploads
                    .artifacts
                    .record(&direct.id, scope)
                    .unwrap()
                    .unwrap();
                assert_eq!(record.workspace_id, imported.workspace_id);
                assert_eq!(record.digest, imported.digest);
                assert_eq!(record.byte_size, 5);
                assert_eq!(
                    uploads
                        .artifacts
                        .read_bytes(artifact_id, scope)
                        .unwrap()
                        .unwrap(),
                    b"media"
                );
                uploads
                    .artifacts
                    .display_target(dto["uri"].as_str().unwrap(), scope)
                    .unwrap();
                uploads
                    .artifacts
                    .resolve_for_provider(dto["uri"].as_str().unwrap(), scope)
                    .unwrap();
                for foreign in [None, Some("ws-a"), Some("ws-b")] {
                    if foreign == scope {
                        continue;
                    }
                    assert!(uploads.artifacts.list(foreign, None).unwrap().is_empty());
                    assert!(uploads
                        .artifacts
                        .read_bytes(artifact_id, foreign)
                        .unwrap()
                        .is_none());
                    assert!(uploads
                        .artifacts
                        .resolve_for_provider(dto["uri"].as_str().unwrap(), foreign)
                        .is_err());
                    assert!(uploads
                        .artifacts
                        .display_target(dto["uri"].as_str().unwrap(), foreign)
                        .is_err());
                    assert!(!uploads.artifacts.delete(artifact_id, foreign).unwrap());
                }
                unavailable(
                    uploads.apply(
                        ArtifactUploadAction::Finish { id: id.clone() },
                        scope,
                        "window-a",
                    ),
                    &id,
                );
                assert_eq!(uploads.artifacts.list(scope, None).unwrap().len(), 2);
                assert!(uploads.artifacts.delete(artifact_id, scope).unwrap());
            }
        }
    }
}

#[test]
fn h253_foreign_actions_cannot_consume_modify_or_refresh_an_upload() {
    let (_dir, _db, uploads) = fixture();
    for scope in [None, Some("ws-a"), Some("ws-b")] {
        let id = begin(&uploads, scope, "window-a", ArtifactKind::Video, false, 3);
        let touched = uploads.uploads.lock().unwrap()[&id].touched;
        for foreign in [None, Some("ws-a"), Some("ws-b")] {
            for caller in ["window-a", "window-b"] {
                if foreign == scope && caller == "window-a" {
                    continue;
                }
                for action in [
                    ArtifactUploadAction::Write {
                        id: id.clone(),
                        offset: 99,
                        bytes: vec![],
                    },
                    ArtifactUploadAction::Finish { id: id.clone() },
                    ArtifactUploadAction::Abort { id: id.clone() },
                ] {
                    unavailable(uploads.apply(action, foreign, caller), &id);
                    let map = uploads.uploads.lock().unwrap();
                    assert_eq!(map[&id].written, 0);
                    assert_eq!(map[&id].touched, touched);
                }
            }
        }
        let normalized = scope.map(|s| format!(" {s} "));
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 0,
                    bytes: b"own".to_vec(),
                },
                normalized.as_deref(),
                "window-a",
            )
            .unwrap();
        let dto = uploads
            .apply(ArtifactUploadAction::Finish { id }, scope, "window-a")
            .unwrap();
        assert_eq!(dto["workspaceId"], json!(scope));
    }
}

#[test]
fn h253_concurrent_foreign_finish_and_abort_do_not_disrupt_owner_chunks() {
    let (_dir, _db, uploads) = fixture();
    let id = begin(
        &uploads,
        Some("ws-a"),
        "window-a",
        ArtifactKind::Video,
        false,
        32,
    );
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        let foreign = threads.spawn(|| {
            barrier.wait();
            for _ in 0..32 {
                unavailable(
                    uploads.apply(
                        ArtifactUploadAction::Finish { id: id.clone() },
                        Some("ws-b"),
                        "window-a",
                    ),
                    &id,
                );
                unavailable(
                    uploads.apply(
                        ArtifactUploadAction::Abort { id: id.clone() },
                        Some("ws-a"),
                        "window-b",
                    ),
                    &id,
                );
            }
        });
        barrier.wait();
        for offset in 0..32 {
            uploads
                .apply(
                    ArtifactUploadAction::Write {
                        id: id.clone(),
                        offset,
                        bytes: vec![offset as u8],
                    },
                    Some("ws-a"),
                    "window-a",
                )
                .unwrap();
        }
        foreign.join().unwrap();
    });
    let dto = uploads
        .apply(
            ArtifactUploadAction::Finish { id },
            Some("ws-a"),
            "window-a",
        )
        .unwrap();
    assert_eq!(
        uploads
            .artifacts
            .read_bytes(dto["id"].as_str().unwrap(), Some("ws-a"))
            .unwrap()
            .unwrap(),
        (0u8..32).collect::<Vec<_>>()
    );
}

#[test]
fn h253_invalid_context_never_falls_back_or_consumes_the_stream() {
    let (dir, _db, uploads) = fixture();
    for scope in [Some(""), Some(" "), Some("../ws"), Some("工作区")] {
        let action = || ArtifactUploadAction::BeginStreaming {
            kind: "video".into(),
            mime: "video/mp4".into(),
            name: "synthetic.mp4".into(),
        };
        assert!(uploads.apply(action(), scope, "window-a").is_err());
        assert!(uploads.uploads.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    assert!(uploads
        .apply(
            ArtifactUploadAction::BeginStreaming {
                kind: "video".into(),
                mime: "video/mp4".into(),
                name: "synthetic.mp4".into()
            },
            None,
            ""
        )
        .is_err());
    let id = begin(
        &uploads,
        Some("ws-a"),
        "window-a",
        ArtifactKind::Video,
        true,
        0,
    );
    assert!(uploads
        .apply(
            ArtifactUploadAction::Abort { id: id.clone() },
            Some(""),
            "window-a"
        )
        .is_err());
    assert!(uploads.uploads.lock().unwrap().contains_key(&id));
    uploads
        .apply(
            ArtifactUploadAction::Abort { id: id.clone() },
            Some("ws-a"),
            "window-a",
        )
        .unwrap();
    unavailable(
        uploads.apply(
            ArtifactUploadAction::Abort { id: id.clone() },
            Some("ws-a"),
            "window-a",
        ),
        &id,
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn h253_expiration_incomplete_chunks_and_drop_clean_only_unpublished_streams() {
    let (dir, _db, uploads) = fixture();
    let id = begin(
        &uploads,
        Some("ws-a"),
        "window-a",
        ArtifactKind::Video,
        true,
        0,
    );
    uploads
        .uploads
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .touched = Instant::now() - IDLE_TIMEOUT - Duration::from_secs(1);
    unavailable(
        uploads.apply(
            ArtifactUploadAction::Finish { id: id.clone() },
            Some("ws-a"),
            "window-a",
        ),
        &id,
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let id = begin(
        &uploads,
        Some("ws-b"),
        "window-b",
        ArtifactKind::Audio,
        false,
        5,
    );
    uploads
        .apply(
            ArtifactUploadAction::Write {
                id: id.clone(),
                offset: 0,
                bytes: vec![1, 2],
            },
            Some("ws-b"),
            "window-b",
        )
        .unwrap();
    assert!(uploads
        .apply(
            ArtifactUploadAction::Finish { id },
            Some("ws-b"),
            "window-b"
        )
        .is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let id = begin(&uploads, None, "window-a", ArtifactKind::Video, true, 0);
    assert!(uploads
        .apply(
            ArtifactUploadAction::Write {
                id,
                offset: 0,
                bytes: vec![1; MAX_CHUNK + 1]
            },
            None,
            "window-a"
        )
        .is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    begin(
        &uploads,
        Some("ws-a"),
        "window-a",
        ArtifactKind::Video,
        true,
        0,
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let artifacts = uploads.artifacts.clone();
    drop(uploads);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(artifacts.list(Some("ws-a"), None).unwrap().is_empty());
}

#[test]
fn h253_index_failure_rolls_back_bytes_without_reassigning_owner() {
    let (dir, db, uploads) = fixture();
    let id = begin(
        &uploads,
        Some("ws-a"),
        "window-a",
        ArtifactKind::Video,
        false,
        3,
    );
    uploads
        .apply(
            ArtifactUploadAction::Write {
                id: id.clone(),
                offset: 0,
                bytes: vec![1, 2, 3],
            },
            Some("ws-a"),
            "window-a",
        )
        .unwrap();
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER h253_fail_insert BEFORE INSERT ON artifacts BEGIN SELECT RAISE(ABORT, 'synthetic publication failure'); END;")
            .map_err(|error| CoreError::other(error.to_string()))?;
        Ok(())
    }).unwrap();
    let error = uploads
        .apply(
            ArtifactUploadAction::Finish { id: id.clone() },
            Some("ws-a"),
            "window-a",
        )
        .unwrap_err();
    assert!(error.to_string().contains("synthetic publication failure"));
    assert!(uploads.uploads.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    for scope in [None, Some("ws-a"), Some("ws-b")] {
        assert!(uploads.artifacts.list(scope, None).unwrap().is_empty());
    }
    unavailable(
        uploads.apply(
            ArtifactUploadAction::Finish { id: id.clone() },
            Some("ws-b"),
            "window-b",
        ),
        &id,
    );
}
