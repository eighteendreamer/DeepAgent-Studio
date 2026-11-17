//! H-70: content identity is durable evidence, not mutable sidecar metadata.
use super::*;
use deepagent_app_core::attachment_service::FileAttachmentResolver;
use deepagent_core::event::EventPayload;
use deepagent_core::message::Message;
use deepagent_core::response_item::ResponseItem;
use deepagent_models::{ModelClient, ModelConfig, WireMode};
use deepagent_persistence::event_store::EventStore;
use deepagent_runtime::{Agent, ModelAgent};

fn import(service: &AttachmentService, bucket: &str) -> deepagent_app_core::AttachmentDto {
    service.ingest(AttachmentIngestDto {
        id: Some("h70-image".into()), session_id: Some(bucket.into()), kind: "image".into(),
        name: "pixel.png".into(), mime: "image/png".into(), source: "picker".into(),
        local_path: None, text: None,
        data_url: Some(concat!("data:image/png;base64,",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").into()),
    }).unwrap()
}

fn input(dto: &deepagent_app_core::AttachmentDto, digest: Option<&str>) -> InputAttachment {
    serde_json::from_value(serde_json::json!({
        "type":"image", "id":dto.id, "path":dto.original_path,
        "media_type":dto.mime, "sha256":digest,
    }))
    .unwrap()
}

async fn run_image(
    chat: &ChatService,
    session: Option<&str>,
    run: &str,
    attachments: Vec<InputAttachment>,
) -> Result<String> {
    chat.run_in_session_with_attachments(
        "Inspect the synthetic image.",
        session,
        None,
        None,
        vec![],
        None,
        false,
        Some(run),
        attachments,
        |_| {},
        |_| {},
    )
    .await
}

fn failed_once(chat: &ChatService, run: &str) {
    let events = chat.run_events(run, None).unwrap();
    let terminal: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "failed");
}

#[tokio::test]
async fn ingress_digest_is_required_and_checked_before_transport() {
    for digest in [
        None,
        Some("invalid"),
        Some("0000000000000000000000000000000000000000000000000000000000000000"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let dto = import(
            &AttachmentService::new(root.path().join("attachments")),
            "first",
        );
        let db = Arc::new(Database::open_in_memory().unwrap());
        let counts = Arc::new(Mutex::new(Vec::new()));
        let chat = ChatService::new(
            db.clone(),
            image_settings(db).await,
            Arc::new(ImageAnswer {
                images_per_request: counts.clone(),
                request_bytes: None,
                image_hashes: None,
            }),
            root.path(),
        );
        let result = run_image(&chat, None, "h70-invalid-input", vec![input(&dto, digest)]).await;
        assert!(
            result.is_err(),
            "missing/invalid/mismatched digest must not be trusted"
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("attachment_integrity"));
        assert!(counts.lock().unwrap().is_empty());
        failed_once(&chat, "h70-invalid-input");
    }
}

#[tokio::test]
async fn sqlite_restart_preserves_digest_and_does_not_trust_rewritten_metadata() {
    for mutate in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let dto = import(
            &AttachmentService::new(root.path().join("attachments")),
            "first",
        );
        let database = root.path().join("history.sqlite");
        let counts = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(ImageAnswer {
            images_per_request: counts.clone(),
            request_bytes: None,
            image_hashes: None,
        });
        let session;
        {
            let db = Arc::new(Database::open(&database).unwrap());
            let chat = ChatService::new(
                db.clone(),
                image_settings(db.clone()).await,
                transport.clone(),
                root.path(),
            );
            session = run_image(
                &chat,
                None,
                "h70-first",
                vec![input(&dto, dto.sha256.as_deref())],
            )
            .await
            .unwrap();
            let events = EventStore::new(&db)
                .load_session(session.parse().unwrap())
                .unwrap();
            for native in [false, true] {
                let refs: Vec<_> = events
                    .iter()
                    .filter_map(|event| match &event.payload {
                        EventPayload::MessageAppended { message }
                            if !native && !message.attachments.is_empty() =>
                        {
                            Some(&message.attachments)
                        }
                        EventPayload::ResponseItemAppended {
                            item: ResponseItem::InputMessage { attachments, .. },
                            ..
                        } if native && !attachments.is_empty() => Some(attachments),
                        _ => None,
                    })
                    .collect();
                assert_eq!(refs.len(), 1);
                assert_eq!(
                    serde_json::to_value(&refs[0][0]).unwrap()["sha256"],
                    dto.sha256.as_deref().unwrap()
                );
            }
        }
        if mutate {
            let path = PathBuf::from(dto.original_path.as_ref().unwrap());
            std::fs::write(&path, b"different synthetic bytes").unwrap();
            let mut metadata: serde_json::Value = serde_json::from_slice(
                &std::fs::read(path.parent().unwrap().join("metadata.json")).unwrap(),
            )
            .unwrap();
            use sha2::{Digest, Sha256};
            metadata["sha256"] =
                format!("{:x}", Sha256::digest(b"different synthetic bytes")).into();
            std::fs::write(
                path.parent().unwrap().join("metadata.json"),
                serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
        }
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(db.clone(), image_settings(db).await, transport, root.path());
        let result = run_image(&chat, Some(&session), "h70-reopen", vec![]).await;
        if mutate {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("attachment_integrity"));
            assert_eq!(*counts.lock().unwrap(), vec![1]);
            failed_once(&chat, "h70-reopen");
        } else {
            result.unwrap();
            assert_eq!(*counts.lock().unwrap(), vec![1, 1]);
        }
    }
}

#[tokio::test]
async fn legacy_history_is_readable_but_not_resigned_from_current_files() {
    let root = tempfile::tempdir().unwrap();
    let dto = import(
        &AttachmentService::new(root.path().join("attachments")),
        "first",
    );
    let database = root.path().join("legacy.sqlite");
    let sid;
    {
        let db = Database::open(&database).unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::create(&db, &clock, None).unwrap();
        sid = session.id().to_string();
        let attachment = serde_json::from_value(serde_json::json!({"id":dto.id,"kind":"image","path":dto.original_path,"media_type":"image/png"})).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("legacy image").with_attachments(vec![attachment]),
            })
            .unwrap();
    }
    let db = Arc::new(Database::open(&database).unwrap());
    let counts = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db.clone(),
        image_settings(db).await,
        Arc::new(ImageAnswer {
            images_per_request: counts.clone(),
            request_bytes: None,
            image_hashes: None,
        }),
        root.path(),
    );
    let result = run_image(&chat, Some(&sid), "h70-legacy", vec![]).await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("attachment_integrity"));
    assert!(counts.lock().unwrap().is_empty());
    failed_once(&chat, "h70-legacy");
}

#[derive(Default)]
struct BoundaryTransport {
    requests: Mutex<Vec<serde_json::Value>>,
    replace_after_first: Option<PathBuf>,
}

#[async_trait]
impl HttpTransport for BoundaryTransport {
    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        if let Some(path) = &self.replace_after_first {
            std::fs::write(path, b"replaced after a retryable provider failure").unwrap();
            return Err(CoreError::Provider {
                status: Some(503),
                code: None,
                message: "synthetic unavailable".into(),
                retry_after_ms: None,
                request_id: None,
            });
        }
        Err(CoreError::invalid("h70 synthetic transport reached"))
    }
}

fn reference(
    dto: &deepagent_app_core::AttachmentDto,
) -> deepagent_core::message::MessageAttachment {
    serde_json::from_value(serde_json::json!({"id":dto.id,"kind":"image","path":dto.original_path,"media_type":dto.mime,"sha256":dto.sha256})).unwrap()
}

fn agent(
    transport: Arc<BoundaryTransport>,
    mode: WireMode,
    attachments: Vec<deepagent_core::message::MessageAttachment>,
) -> ModelAgent {
    ModelAgent::new_with_attachments(
        Arc::new(ModelClient::new(
            transport,
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        )),
        "deepseek-flash",
        "rules",
        "Inspect synthetic image",
        attachments,
        vec![],
    )
    .with_attachment_resolver(Arc::new(FileAttachmentResolver))
    .with_max_model_attempts(2)
}

#[tokio::test]
async fn both_provider_projections_keep_verified_bytes_and_reject_id_collisions() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let root = tempfile::tempdir().unwrap();
        let service = AttachmentService::new(root.path().join("attachments"));
        let first = import(&service, "first");
        let mut second = import(&service, "second");
        std::fs::write(
            second.original_path.as_ref().unwrap(),
            b"different content identity",
        )
        .unwrap();
        second.sha256 = Some(format!(
            "{:x}",
            Sha256::digest(b"different content identity")
        ));
        let transport = Arc::new(BoundaryTransport::default());
        // Identical repeated references remain valid and both project the same
        // verified bytes; the public id-indexed URL contract is unchanged.
        let mut control = agent(
            transport.clone(),
            mode,
            vec![reference(&first), reference(&first)],
        );
        assert!(control
            .think(0, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("h70 synthetic transport reached"));
        {
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let key = if mode == WireMode::Responses {
                "input"
            } else {
                "messages"
            };
            let urls: Vec<_> = requests[0][key]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item["content"].as_array())
                .flatten()
                .filter_map(|part| {
                    if mode == WireMode::Responses {
                        part["image_url"].as_str()
                    } else {
                        part["image_url"]["url"].as_str()
                    }
                })
                .collect();
            assert_eq!(urls.len(), 2);
            for url in urls {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(url.split_once(',').unwrap().1)
                    .unwrap();
                assert_eq!(
                    format!("{:x}", Sha256::digest(bytes)),
                    first.sha256.as_deref().unwrap()
                );
            }
        }
        let mut conflict = agent(
            transport.clone(),
            mode,
            vec![reference(&first), reference(&second)],
        );
        assert!(conflict
            .think(0, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("conflicting image references"));
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn retry_revalidates_the_actual_file_and_pre_cancel_does_not_materialize() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let root = tempfile::tempdir().unwrap();
        let dto = import(
            &AttachmentService::new(root.path().join("attachments")),
            "first",
        );
        let transport = Arc::new(BoundaryTransport {
            replace_after_first: Some(dto.original_path.as_ref().unwrap().into()),
            ..Default::default()
        });
        let mut retry = agent(transport.clone(), mode, vec![reference(&dto)]);
        assert!(retry
            .think(0, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("attachment_integrity"));
        assert_eq!(
            transport.requests.lock().unwrap().len(),
            1,
            "replacement must not reach the retry transport"
        );
        let mut cancelled = agent(transport.clone(), mode, vec![reference(&dto)]);
        let error = cancelled
            .think_cancelled(0, &[], Some(Arc::new(AtomicBool::new(true))))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(!error.to_string().contains("attachment_integrity"));
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }
}
