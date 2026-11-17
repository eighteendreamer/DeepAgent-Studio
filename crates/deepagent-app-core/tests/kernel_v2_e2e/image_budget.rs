//! H-69: genuine imported PNGs, durable history and the shared sampling seam.
use super::*;
use deepagent_app_core::attachment_service::FileAttachmentResolver;
use deepagent_models::{ModelClient, ModelConfig, WireMode};
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::{Agent, ModelAgent};

const LIMIT: usize = 48 * 1024 * 1024;

fn large_png(root: &std::path::Path) -> PathBuf {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let source = root.join("large.png");
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
    source
}

fn import(root: &std::path::Path, source: &std::path::Path, index: usize) -> InputAttachment {
    let dto = AttachmentService::new(root.join("attachments"))
        .ingest(AttachmentIngestDto {
            id: Some(format!("h69-large-{index}")),
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
    assert_eq!(dto.status, "ready");
    assert!((20 * 1024 * 1024..32 * 1024 * 1024).contains(&dto.size_bytes));
    InputAttachment::Image {
        id: dto.id,
        path: dto.original_path.unwrap().into(),
        media_type: dto.mime,
        sha256: dto.sha256,
    }
}

async fn run(
    chat: &ChatService,
    session: Option<&str>,
    id: &str,
    images: Vec<InputAttachment>,
) -> Result<String> {
    chat.run_in_session_with_attachments(
        "Inspect and retain the synthetic images.",
        session,
        None,
        None,
        vec![],
        None,
        false,
        Some(id),
        images,
        |_| {},
        |_| {},
    )
    .await
}

fn terminal(db: &Database, id: &str, kind: &str) -> String {
    let store = RunStore::new(db);
    let record = store.get(id).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some(kind));
    let events = store.events_after(id, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], kind);
    record.session_id
}

fn too_large(error: CoreError) {
    assert!(
        error.to_string().contains("request_body_too_large"),
        "{error}"
    );
    assert!(!error.to_string().contains("data:image"));
}

pub(super) async fn verify_original_case() {
    let root = tempfile::tempdir().unwrap();
    let source = large_png(root.path());
    for count in [1, 2] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let counts = Arc::new(Mutex::new(Vec::new()));
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            Arc::new(ImageAnswer {
                images_per_request: counts.clone(),
                request_bytes: Some(sizes.clone()),
                image_hashes: None,
            }),
            root.path(),
        );
        let images = (0..count)
            .map(|i| import(root.path(), &source, count * 10 + i))
            .collect();
        let first = run(&chat, None, "h69-first", images).await;
        if count == 1 {
            let session = first.unwrap();
            run(&chat, Some(&session), "h69-continued", vec![])
                .await
                .unwrap();
            terminal(&db, "h69-first", "succeeded");
            terminal(&db, "h69-continued", "succeeded");
            assert_eq!(*counts.lock().unwrap(), vec![1, 1]);
            assert_eq!(sizes.lock().unwrap().len(), 2);
        } else {
            too_large(first.unwrap_err());
            terminal(&db, "h69-first", "failed");
            assert!(counts.lock().unwrap().is_empty());
        }
        let sizes = sizes.lock().unwrap();
        eprintln!("h69 images={count} actual_transport_bytes={sizes:?}");
        assert!(sizes.iter().all(|n| *n <= LIMIT));
    }
}

#[tokio::test]
async fn sqlite_reopen_rejects_accumulated_images_without_rewriting_their_references() {
    let root = tempfile::tempdir().unwrap();
    let source = large_png(root.path());
    let first = import(root.path(), &source, 1);
    let second = import(root.path(), &source, 2);
    let database = root.path().join("history.sqlite");
    let counts = Arc::new(Mutex::new(Vec::new()));
    let hashes = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(ImageAnswer {
        images_per_request: counts.clone(),
        request_bytes: None,
        image_hashes: Some(hashes.clone()),
    });
    let session;
    let original;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            transport.clone(),
            root.path(),
        );
        session = run(&chat, None, "h69-original", vec![first.clone()])
            .await
            .unwrap();
        original = read_failure::image_events(&db, &session);
        terminal(&db, "h69-original", "succeeded");
    }
    let admitted;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        assert_eq!(read_failure::image_events(&db, &session), original);
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            transport.clone(),
            root.path(),
        );
        too_large(
            run(
                &chat,
                Some(&session),
                "h69-accumulated",
                vec![second.clone()],
            )
            .await
            .unwrap_err(),
        );
        terminal(&db, "h69-accumulated", "failed");
        admitted = read_failure::image_events(&db, &session);
        assert_eq!(
            admitted.len(),
            4,
            "both UI and native references for both images remain durable"
        );
        assert_eq!(&admitted[..2], original.as_slice());
    }
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            transport,
            root.path(),
        );
        too_large(
            run(&chat, Some(&session), "h69-reopened", vec![])
                .await
                .unwrap_err(),
        );
        terminal(&db, "h69-reopened", "failed");
        assert_eq!(read_failure::image_events(&db, &session), admitted);
    }
    assert_eq!(
        *counts.lock().unwrap(),
        vec![1],
        "no retry/fallback/redispatch of the oversized body"
    );
    for input in [first, second] {
        let InputAttachment::Image { path, sha256, .. } = input else {
            unreachable!()
        };
        use sha2::{Digest, Sha256};
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
            sha256.unwrap()
        );
    }
}

struct RetryTransport {
    sizes: Mutex<Vec<usize>>,
    failure_status: u16,
}

#[async_trait]
impl HttpTransport for RetryTransport {
    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        let mut sizes = self.sizes.lock().unwrap();
        sizes.push(request.body.len());
        assert!(request.body.len() <= LIMIT);
        if sizes.len() == 1 {
            Err(CoreError::Provider {
                status: Some(self.failure_status),
                code: None,
                message: "synthetic failure".into(),
                retry_after_ms: None,
                request_id: None,
            })
        } else {
            Err(CoreError::invalid("h69 valid retry reached transport"))
        }
    }
}

#[tokio::test]
async fn retry_overload_and_cancel_share_the_same_final_wire_gate() {
    let root = tempfile::tempdir().unwrap();
    let source = large_png(root.path());
    let input = import(root.path(), &source, 1);
    let InputAttachment::Image {
        id,
        path,
        media_type,
        sha256,
    } = input
    else {
        unreachable!()
    };
    let reference = deepagent_core::message::MessageAttachment {
        id,
        path: Some(path.to_string_lossy().into_owned()),
        kind: "image".into(),
        media_type: Some(media_type),
        sha256,
    };
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for status in [503, 529] {
            let transport = Arc::new(RetryTransport {
                sizes: Mutex::new(vec![]),
                failure_status: status,
            });
            let make = |images| {
                let config = ModelConfig::deepseek("synthetic").with_wire_mode(mode);
                ModelAgent::new_with_attachments(
                    Arc::new(ModelClient::new(transport.clone(), config)),
                    "deepseek-flash",
                    "rules",
                    "Inspect synthetic images",
                    images,
                    vec![],
                )
                .with_attachment_resolver(Arc::new(FileAttachmentResolver))
                .with_max_model_attempts(3)
            };
            let mut oversized = make(vec![reference.clone(), reference.clone()]);
            too_large(oversized.think(0, &[]).await.unwrap_err());
            assert!(transport.sizes.lock().unwrap().is_empty());
            let mut cancelled = make(vec![reference.clone(), reference.clone()]);
            assert!(cancelled
                .think_cancelled(0, &[], Some(Arc::new(AtomicBool::new(true))))
                .await
                .unwrap_err()
                .to_string()
                .contains("cancelled"));
            assert!(transport.sizes.lock().unwrap().is_empty());
            let mut valid = make(vec![reference.clone()]);
            if status == 529 {
                assert_eq!(
                    deepagent_models::classify_model_error(&valid.think(0, &[]).await.unwrap_err()),
                    deepagent_models::ModelFailureKind::Overloaded
                );
                assert_eq!(transport.sizes.lock().unwrap().len(), 1);
            } else {
                let error = valid.think(0, &[]).await.unwrap_err();
                assert!(error
                    .to_string()
                    .contains("h69 valid retry reached transport"));
                assert_eq!(transport.sizes.lock().unwrap().len(), 2);
            }
        }
    }
}
