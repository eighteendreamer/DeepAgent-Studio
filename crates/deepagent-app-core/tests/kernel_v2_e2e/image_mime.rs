//! H169: cold legacy images must not bypass admission or reach sampling.
use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::message::{Message, MessageAttachment};
use deepagent_persistence::run_store::RunStore;
use sha2::{Digest, Sha256};

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1" fill="red"/></svg>"#;

pub(super) async fn invalid_reference_is_rejected(
    bytes: &[u8],
    declared_mime: &str,
    run_id: &str,
    retained: bool,
) {
    let root = tempfile::tempdir().unwrap();
    // Seed an old, digest-bearing native history without calling the new
    // admission API. Updating upload validation alone cannot fix this path.
    let path = root.path().join("legacy-image.bin");
    std::fs::write(&path, bytes).unwrap();
    let database = root.path().join("legacy.sqlite");
    let session_id = {
        let db = Database::open(&database).unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::create(&db, &clock, None).unwrap();
        let id = session.id().to_string();
        if retained {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("legacy selected image").with_attachments(vec![
                        MessageAttachment {
                            id: "h169-legacy".into(),
                            kind: "image".into(),
                            media_type: Some(declared_mime.into()),
                            path: Some(path.to_string_lossy().into_owned()),
                            sha256: Some(format!("{:x}", Sha256::digest(bytes))),
                        },
                    ]),
                })
                .unwrap();
        }
        id
    };
    let db = Arc::new(Database::open(&database).unwrap());
    let urls = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db.clone(),
        image_settings(db.clone()).await,
        Arc::new(ImageUrlAnswer {
            urls_per_request: urls.clone(),
        }),
        root.path(),
    );
    let result = chat
        .run_in_session_with_attachments(
            "Continue with the retained image.",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run_id),
            if retained {
                Vec::new()
            } else {
                vec![InputAttachment::Image {
                    id: "h169-current".into(),
                    path: path.clone(),
                    media_type: declared_mime.into(),
                    sha256: Some(format!("{:x}", Sha256::digest(bytes))),
                }]
            },
            |_| {},
            |_| {},
        )
        .await;
    eprintln!(
        "H169 cold reference: declared={declared_mime}, api_ok={}, requests={}",
        result.is_ok(),
        urls.lock().unwrap().len()
    );
    assert!(result.is_err(), "invalid legacy image must fail explicitly");
    assert!(
        urls.lock().unwrap().is_empty(),
        "invalid image reached transport"
    );
    assert_eq!(
        RunStore::new(&db)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
}

#[tokio::test]
async fn h169_cold_svg_with_an_admitted_digest_is_rejected_before_transport() {
    invalid_reference_is_rejected(SVG, "image/svg+xml", "h169-legacy-svg", true).await;
}

#[tokio::test]
async fn h169_cold_png_mime_cannot_disguise_svg_bytes() {
    invalid_reference_is_rejected(SVG, "image/png", "h169-legacy-spoof", true).await;
}

#[tokio::test]
async fn h169_current_raw_image_cannot_bypass_ingestion_with_spoofed_mime() {
    invalid_reference_is_rejected(SVG, "image/png", "h169-current-spoof", false).await;
}

#[tokio::test]
async fn h169_four_decodable_formats_keep_exact_bytes_in_current_and_cold_history() {
    use base64::Engine;
    use std::io::Cursor;
    for (format, mime) in [
        (image::ImageFormat::Jpeg, "image/jpeg"),
        (image::ImageFormat::Png, "image/png"),
        (image::ImageFormat::Gif, "image/gif"),
        (image::ImageFormat::WebP, "image/webp"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("rasters.sqlite");
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut encoded, format)
            .unwrap();
        let bytes = encoded.into_inner();
        let url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
        let imported = AttachmentService::new(root.path().join("attachments"))
            .ingest(AttachmentIngestDto {
                id: Some("valid-raster".into()),
                session_id: None,
                kind: "image".into(),
                name: "not-an-image-extension.txt".into(),
                mime: mime.into(),
                source: "picker".into(),
                data_url: Some(url.clone()),
                local_path: None,
                text: None,
            })
            .unwrap();
        assert_eq!(imported.mime, mime);
        assert_eq!(
            std::fs::read(imported.original_path.as_ref().unwrap()).unwrap(),
            bytes
        );
        let urls = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(ImageUrlAnswer {
            urls_per_request: urls.clone(),
        });
        let session = {
            let db = Arc::new(Database::open(&database).unwrap());
            let chat = ChatService::new(
                db.clone(),
                image_settings(db).await,
                transport.clone(),
                root.path(),
            );
            chat.run_in_session_with_attachments(
                "Read this original raster.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h169-valid-current"),
                vec![InputAttachment::Image {
                    id: imported.id,
                    path: PathBuf::from(imported.original_path.unwrap()),
                    media_type: imported.mime,
                    sha256: imported.sha256,
                }],
                |_| {},
                |_| {},
            )
            .await
            .unwrap()
        };
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(db.clone(), image_settings(db).await, transport, root.path());
        chat.run_in_session(
            "Use the retained original.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h169-valid-reopen"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            *urls.lock().unwrap(),
            vec![vec![url.clone()], vec![url]],
            "{mime}"
        );
    }
}
