//! Follow one real imported image from the live turn into retained history.

use super::*;
use deepagent_app_core::{AttachmentIngestDto, AttachmentService, VisionMode, VisionSettings};
use deepagent_models::ModelRole;
use deepagent_runtime::InputAttachment;

#[path = "image_integrity.rs"]
mod integrity;

struct ImageAnswer {
    images_per_request: Arc<Mutex<Vec<usize>>>,
    request_bytes: Option<Arc<Mutex<Vec<usize>>>>,
    image_hashes: Option<Arc<Mutex<Vec<Vec<String>>>>>,
}

struct ImageUrlAnswer {
    urls_per_request: Arc<Mutex<Vec<Vec<String>>>>,
}

#[derive(Debug)]
struct ImageReferenceObservation {
    request_bytes: usize,
    inline_urls: Vec<String>,
    file_ids: Vec<String>,
}

struct ImageReferenceAnswer {
    observations: Arc<Mutex<Vec<ImageReferenceObservation>>>,
}

#[async_trait]
impl HttpTransport for ImageReferenceAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let parts: Vec<&serde_json::Value> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .collect();
        self.observations
            .lock()
            .unwrap()
            .push(ImageReferenceObservation {
                request_bytes: request.body.len(),
                inline_urls: parts
                    .iter()
                    .filter_map(|part| part["image_url"].as_str().map(str::to_string))
                    .collect(),
                file_ids: parts
                    .iter()
                    .filter_map(|part| part["file_id"].as_str().map(str::to_string))
                    .collect(),
            });
        let item = serde_json::json!({
            "type": "message", "id": "audit-image-file-reference-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline image file reference audit."}],
        });
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash",
                    "status":"completed","output":[item]
                }})
                .to_string(),
            ],
        )
    }
}

#[async_trait]
impl HttpTransport for ImageUrlAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let urls = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .filter_map(|part| part["image_url"].as_str().map(str::to_string))
            .collect();
        self.urls_per_request.lock().unwrap().push(urls);
        let item = serde_json::json!({
            "type": "message", "id": "audit-image-mime-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline image MIME audit."}],
        });
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash",
                    "status":"completed","output":[item]
                }})
                .to_string(),
            ],
        )
    }
}

#[async_trait]
impl HttpTransport for ImageAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        if let Some(sizes) = self.request_bytes.as_ref() {
            sizes.lock().unwrap().push(request.body.len());
        }
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], "deepseek-flash");
        let images = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .inspect(|part| {
                assert!(part["image_url"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,"));
            })
            .count();
        self.images_per_request.lock().unwrap().push(images);
        if let Some(hashes) = &self.image_hashes {
            use base64::Engine;
            use sha2::{Digest, Sha256};
            let current = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item["content"].as_array())
                .flatten()
                .filter(|part| part["type"] == "input_image")
                .map(|part| {
                    let encoded = part["image_url"]
                        .as_str()
                        .unwrap()
                        .split_once(',')
                        .unwrap()
                        .1;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .unwrap();
                    format!("{:x}", Sha256::digest(bytes))
                })
                .collect();
            hashes.lock().unwrap().push(current);
        }
        let item = serde_json::json!({
            "type": "message", "id": "audit-image-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline image preparation audit."}],
        });
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash",
                    "status":"completed","output":[item]
                }})
                .to_string(),
            ],
        )
    }
}

pub(super) async fn image_settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
        .set_vision_settings(VisionSettings {
            mode: VisionMode::Off,
            ..VisionSettings::default()
        })
        .unwrap();
    settings
}

async fn imported_image_across_turns(seed_plain_turn: bool) -> Vec<usize> {
    observe_imported_image(seed_plain_turn, ImageMutation::Keep)
        .await
        .0
}

enum ImageMutation {
    Keep,
    Remove,
    #[cfg(feature = "runtimes")]
    Replace(Vec<u8>),
}

async fn observe_imported_image(
    seed_plain_turn: bool,
    mutation: ImageMutation,
) -> (Vec<usize>, bool, Vec<Vec<String>>, String) {
    let remove_before_continuation = matches!(mutation, ImageMutation::Remove);
    let root = tempfile::tempdir().unwrap();
    let attachments = AttachmentService::new(root.path().join("attachments"));
    // A complete one-pixel PNG, imported through the desktop's production DTO.
    let imported = attachments
        .ingest(AttachmentIngestDto {
            id: Some("audit-image".into()),
            session_id: None,
            kind: "image".into(),
            name: "pixel.png".into(),
            mime: "image/png".into(),
            source: "picker".into(),
            local_path: None,
            data_url: Some(concat!(
                "data:image/png;base64,",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
            ).into()),
            text: None,
        })
        .unwrap();
    assert_eq!(imported.status, "ready");
    assert!(imported.sha256.is_some());
    let path = PathBuf::from(imported.original_path.unwrap());
    assert!(path.is_file());
    assert_eq!(imported.size_bytes, 68);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = image_settings(db.clone()).await;
    let counts = Arc::new(Mutex::new(Vec::new()));
    let hashes = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(ImageAnswer {
            images_per_request: counts.clone(),
            request_bytes: None,
            image_hashes: Some(hashes.clone()),
        }),
        root.path(),
    );
    let seeded_session = if seed_plain_turn {
        Some(
            chat.run_in_session(
                "Establish a text-only turn before attaching an image.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("audit-image-seed"),
                |_| {},
                |_| {},
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    let session_id = chat
        .run_in_session_with_attachments(
            "Inspect the attached image for this offline audit.",
            seeded_session.as_deref(),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-image-first"),
            vec![InputAttachment::Image {
                sha256: imported.sha256.clone(),
                id: imported.id.clone(),
                path: path.clone(),
                media_type: "image/png".into(),
            }],
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let persisted = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(session_id.parse().unwrap())
        .unwrap();
    assert!(
        persisted.iter().any(|event| matches!(
            &event.payload,
            deepagent_core::event::EventPayload::MessageAppended { message }
                if message.role == deepagent_core::message::Role::User
                && message.attachments.iter().any(|attachment|
                    attachment.id == imported.id && attachment.path.as_deref() == path.to_str())
        )),
        "the first user message must already contain the real attachment reference"
    );
    let native_images = persisted.iter().filter(|event| matches!(
        &event.payload,
        deepagent_core::event::EventPayload::ResponseItemAppended {
            item: deepagent_core::response_item::ResponseItem::InputMessage { attachments, .. }
        } if attachments.iter().any(|attachment| attachment.id == imported.id)
    )).count();
    assert_eq!(
        native_images, 1,
        "admission must persist one reference-only native image item"
    );
    match mutation {
        ImageMutation::Keep => {}
        ImageMutation::Remove => {
            // Only mutate this test's own file, after durable admission.
            std::fs::remove_file(&path).unwrap();
        }
        #[cfg(feature = "runtimes")]
        ImageMutation::Replace(bytes) => {
            std::fs::write(&path, bytes).unwrap();
        }
    }
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path.parent().unwrap().join("metadata.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["sha256"].as_str(), imported.sha256.as_deref());
    let continuation = chat
        .run_in_session(
            "Refer to the previously attached image.",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-image-continued"),
            |_| {},
            |_| {},
        )
        .await;
    let continuation_succeeded = continuation.is_ok();
    assert_eq!(path.is_file(), !remove_before_continuation);
    for run in [
        "audit-image-seed",
        "audit-image-first",
        "audit-image-continued",
    ] {
        if run == "audit-image-seed" && !seed_plain_turn {
            continue;
        }
        let events = chat.run_events(run, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        if run != "audit-image-continued" || continuation_succeeded {
            assert!(events.iter().any(
                |event| event.event_type == "run_terminal" && event.data["kind"] == "succeeded"
            ));
        }
    }
    let counts = counts.lock().unwrap().clone();
    if !remove_before_continuation && continuation_succeeded {
        assert!(continuation_succeeded);
        assert_eq!(counts.len(), if seed_plain_turn { 3 } else { 2 });
    }
    eprintln!("image-preparation seed_plain={seed_plain_turn} imported_ready=true bytes={} removed_before_continuation={remove_before_continuation} native_image_refs={native_images} wire_image_counts={counts:?} continuation_succeeded={continuation_succeeded}", imported.size_bytes);
    let hashes = hashes.lock().unwrap().clone();
    (
        counts,
        continuation_succeeded,
        hashes,
        imported.sha256.unwrap(),
    )
}

#[tokio::test]
async fn retained_imported_image_reaches_continuation_request() {
    let counts = imported_image_across_turns(false).await;
    assert_eq!(
        counts[1], 1,
        "the same file/resolver must work through retained native history"
    );
}

#[tokio::test]
async fn audit_new_image_reaches_first_model_request() {
    let initial = imported_image_across_turns(false).await;
    let later = imported_image_across_turns(true).await;
    assert_eq!((initial, later), (vec![1, 1], vec![0, 1, 1]));
}

#[tokio::test]
#[ignore = "audit probe: unsupported image MIME types must be rejected before transport"]
async fn audit_unsupported_image_mime_is_rejected_before_transport() {
    use base64::Engine as _;

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1" fill="red"/></svg>"#;
    let root = tempfile::tempdir().unwrap();
    let attachments = AttachmentService::new(root.path().join("attachments"));
    let imported = attachments
        .ingest(AttachmentIngestDto {
            id: Some("audit-svg".into()),
            session_id: None,
            kind: "image".into(),
            name: "pixel.svg".into(),
            mime: "image/svg+xml".into(),
            source: "picker".into(),
            local_path: None,
            data_url: Some(format!(
                "data:image/svg+xml;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(SVG)
            )),
            text: None,
        })
        .unwrap();
    assert_eq!(imported.status, "ready");
    let path = PathBuf::from(imported.original_path.unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), SVG);

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = image_settings(db.clone()).await;
    let urls = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(ImageUrlAnswer {
            urls_per_request: urls.clone(),
        }),
        root.path(),
    );
    let session = chat
        .run_in_session_with_attachments(
            "Retain this SVG attachment for the MIME audit.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-svg-first"),
            vec![InputAttachment::Image {
                sha256: imported.sha256.clone(),
                id: imported.id,
                path,
                media_type: "image/svg+xml".into(),
            }],
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    chat.run_in_session(
        "Use the retained attachment.",
        Some(&session),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-svg-continued"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let urls = urls.lock().unwrap().clone();
    eprintln!("image MIME audit: urls_per_request={urls:?}");
    assert_eq!(urls.len(), 2);
    assert!(urls[0].is_empty(), "H-67 control unexpectedly changed");
    assert!(
        urls[1]
            .iter()
            .all(|url| !url.starts_with("data:image/svg+xml")),
        "DeepSeek Responses only accepts JPEG, PNG, GIF, or WebP image_url payloads"
    );
}

#[tokio::test]
#[ignore = "audit probe: retained images should reuse a DeepSeek Files API identity"]
async fn audit_retained_image_reuses_deepseek_file_id() {
    let root = tempfile::tempdir().unwrap();
    let attachments = AttachmentService::new(root.path().join("attachments"));
    let imported = attachments
        .ingest(AttachmentIngestDto {
            id: Some("audit-file-reference".into()),
            session_id: None,
            kind: "image".into(),
            name: "pixel.png".into(),
            mime: "image/png".into(),
            source: "picker".into(),
            local_path: None,
            data_url: Some(concat!(
                "data:image/png;base64,",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
            ).into()),
            text: None,
        })
        .unwrap();
    assert_eq!(imported.status, "ready");
    assert_eq!(imported.size_bytes, 68);

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = image_settings(db.clone()).await;
    let observations = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(ImageReferenceAnswer {
            observations: observations.clone(),
        }),
        root.path(),
    );
    let session = chat
        .run_in_session_with_attachments(
            "Retain this PNG for the file-reference audit.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-file-reference-first"),
            vec![InputAttachment::Image {
                sha256: imported.sha256.clone(),
                id: imported.id,
                path: PathBuf::from(imported.original_path.unwrap()),
                media_type: "image/png".into(),
            }],
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    for (prompt, run_id) in [
        (
            "Use the retained PNG in the first continuation.",
            "audit-file-reference-second",
        ),
        (
            "Use the same retained PNG again.",
            "audit-file-reference-third",
        ),
    ] {
        chat.run_in_session(
            prompt,
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run_id),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    }

    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 3);
    assert!(
        observations[0].inline_urls.is_empty(),
        "H-67 control changed"
    );
    assert!(observations[0].file_ids.is_empty(), "H-67 control changed");
    let inline_counts: Vec<_> = observations
        .iter()
        .map(|item| item.inline_urls.len())
        .collect();
    let file_id_counts: Vec<_> = observations
        .iter()
        .map(|item| item.file_ids.len())
        .collect();
    let request_bytes: Vec<_> = observations.iter().map(|item| item.request_bytes).collect();
    let repeated_inline_chars = observations[1]
        .inline_urls
        .iter()
        .chain(observations[2].inline_urls.iter())
        .map(String::len)
        .sum::<usize>();
    let same_inline_payload = observations[1].inline_urls == observations[2].inline_urls;
    eprintln!(
        "image file-reference audit: inline_counts={inline_counts:?} file_id_counts={file_id_counts:?} request_bytes={request_bytes:?} repeated_inline_chars={repeated_inline_chars} same_inline_payload={same_inline_payload}"
    );
    assert!(
        same_inline_payload,
        "the control image bytes changed between turns"
    );
    assert_eq!(
        (inline_counts, file_id_counts),
        (vec![0, 0, 0], vec![0, 1, 1]),
        "retained images should be uploaded once and reuse the same provider file_id instead of resending complete data URLs"
    );
    assert_eq!(
        observations[1].file_ids, observations[2].file_ids,
        "the provider file identity must stay stable across unchanged requests"
    );
}

#[tokio::test]
#[ignore = "audit probe: a missing retained image must fail explicitly before sampling"]
async fn audit_missing_retained_image_is_not_silently_dropped() {
    let readable = observe_imported_image(false, ImageMutation::Keep).await;
    assert_eq!((readable.0, readable.1), (vec![0, 1], true));
    let missing = observe_imported_image(false, ImageMutation::Remove).await;
    assert_eq!(
        (missing.0, missing.1),
        (vec![0], false),
        "a retained image that disappears must not become a successful text-only request"
    );
}

#[cfg(feature = "runtimes")]
async fn large_image_request(image_count: usize, source: &std::path::Path) -> (Vec<usize>, bool) {
    let root = tempfile::tempdir().unwrap();
    let attachments = AttachmentService::new(root.path().join("attachments"));
    let mut inputs = Vec::new();
    for index in 0..image_count {
        let imported = attachments
            .ingest(AttachmentIngestDto {
                id: Some(format!("large-{index}")),
                session_id: None,
                kind: "image".into(),
                name: format!("large-{index}.png"),
                mime: "image/png".into(),
                source: "picker".into(),
                local_path: Some(source.to_string_lossy().into_owned()),
                data_url: None,
                text: None,
            })
            .unwrap();
        assert_eq!(imported.status, "ready");
        assert!(imported.size_bytes > 20 * 1024 * 1024);
        assert!(imported.size_bytes < 32 * 1024 * 1024);
        inputs.push(InputAttachment::Image {
            sha256: imported.sha256.clone(),
            id: imported.id,
            path: PathBuf::from(imported.original_path.unwrap()),
            media_type: "image/png".into(),
        });
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = image_settings(db.clone()).await;
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let counts = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(ImageAnswer {
            images_per_request: counts.clone(),
            request_bytes: Some(sizes.clone()),
            image_hashes: None,
        }),
        root.path(),
    );
    let session = chat
        .run_in_session_with_attachments(
            "Keep the imported images for the next turn.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("large-image-first"),
            inputs,
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let continuation = chat
        .run_in_session(
            "Compare the previously attached images.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("large-image-continued"),
            |_| {},
            |_| {},
        )
        .await;
    let sizes = sizes.lock().unwrap().clone();
    let counts = counts.lock().unwrap().clone();
    eprintln!("image-budget images={image_count} body_bytes={sizes:?} wire_image_counts={counts:?} continuation_succeeded={}", continuation.is_ok());
    if continuation.is_ok() {
        let events = chat.run_events("large-image-continued", None).unwrap();
        let terminal: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
        assert_eq!(counts, vec![0, image_count]);
    }
    (sizes, continuation.is_ok())
}

#[cfg(feature = "runtimes")]
#[tokio::test]
#[ignore = "audit probe: individually valid images must respect the total inline body budget"]
async fn audit_multi_image_request_respects_inline_body_budget() {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("large.png");
    // A real, decodable 2048x2560 RGBA PNG; the library emits valid chunks/CRCs.
    // Uncompressed pixels exercise byte limits without huge pixel dimensions.
    PngEncoder::new_with_quality(
        std::fs::File::create(&source).unwrap(),
        CompressionType::Uncompressed,
        FilterType::NoFilter,
    )
    .write_image(
        &vec![127; 2048 * 2560 * 4],
        2048,
        2560,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    let decoded = image::open(&source).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (2048, 2560));
    drop(decoded);
    let one = large_image_request(1, &source).await;
    assert!(one.1);
    assert_eq!(one.0.len(), 2);
    assert!(one.0[1] < 48 * 1024 * 1024);
    let two = large_image_request(2, &source).await;
    assert!(
        two.0.iter().all(|bytes| *bytes <= 48 * 1024 * 1024),
        "an over-budget inline body reached the model transport"
    );
}

#[cfg(feature = "runtimes")]
#[tokio::test]
async fn audit_retained_image_verifies_ingested_digest() {
    use image::ImageEncoder;
    use sha2::{Digest, Sha256};

    let readable = observe_imported_image(false, ImageMutation::Keep).await;
    assert!(readable.1);
    assert_eq!(readable.2[1], vec![readable.3]);
    let mut replacement = Vec::new();
    image::codecs::png::PngEncoder::new(&mut replacement)
        .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let decoded = image::load_from_memory(&replacement).unwrap().to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    let replacement_digest = format!("{:x}", Sha256::digest(&replacement));
    let changed = observe_imported_image(false, ImageMutation::Replace(replacement)).await;
    assert_ne!(changed.3, replacement_digest);
    let replacement_sent = changed
        .2
        .iter()
        .flatten()
        .any(|hash| hash == &replacement_digest);
    eprintln!("image-integrity metadata_still_original=true replacement_sent={replacement_sent} continuation_succeeded={}", changed.1);
    assert!(
        !replacement_sent,
        "the original persisted attachment identity was reused to send different image bytes"
    );
    assert!(
        !changed.1,
        "integrity failure must not silently become text-only success"
    );
    assert_eq!(
        changed.0,
        vec![1],
        "only the verified first request may reach transport"
    );
}
