//! H-71: real admitted images, persisted discovery and the effective model.
use super::*;
use deepagent_app_core::attachment_service::FileAttachmentResolver;
use deepagent_core::message::MessageAttachment;
use deepagent_models::{ModelClient, ModelConfig, WireMode};
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::{Agent, AttachmentDataResolver, ModelAgent};
use serde_json::{json, Value};

const CATALOG: &str = r#"{"data":[
    {"id":"h71-vision","object":"model","owned_by":"deepseek","input_modalities":["text","image"]},
    {"id":"h71-text","object":"model","owned_by":"deepseek","input_modalities":["text"]}
]}"#;

#[derive(Default)]
struct Model {
    requests: Mutex<Vec<Value>>,
    failure_status: Option<u16>,
    corrupt_after_first: Option<PathBuf>,
}

#[async_trait]
impl HttpTransport for Model {
    async fn get_json(&self, url: &str, _: &str) -> Result<String> {
        assert_eq!(url, "https://api.deepseek.com/models");
        Ok(CATALOG.into())
    }
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len()
        };
        if index == 1 {
            if let Some(status) = self.failure_status {
                if let Some(path) = &self.corrupt_after_first {
                    std::fs::write(path, b"changed synthetic image").unwrap();
                }
                return Err(CoreError::Provider {
                    status: Some(status),
                    code: None,
                    message: "synthetic unavailable".into(),
                    retry_after_ms: None,
                    request_id: None,
                });
            }
        }
        if request.url.ends_with("/responses") {
            let item = json!({"type":"message","id":"h71-answer","role":"assistant",
                "content":[{"type":"output_text","text":"synthetic answer"}]});
            emit_responses_events(
                sink,
                &[
                    json!({"type":"response.output_item.done","item":item}).to_string(),
                    json!({"type":"response.completed","response":{"id":"h71","object":"response",
                    "created_at":1,"model":"fixture","status":"completed","output":[item]}})
                    .to_string(),
                ],
            )
        } else {
            for event in [
                r#"{"choices":[{"delta":{"content":"synthetic answer"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ] {
                if sink.on_event(event)? {
                    break;
                }
            }
            Ok(())
        }
    }
}

fn imported(root: &std::path::Path) -> deepagent_app_core::AttachmentDto {
    use base64::Engine;
    #[cfg(feature = "runtimes")]
    let bytes = {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    };
    #[cfg(not(feature="runtimes"))]
    let bytes=base64::engine::general_purpose::STANDARD.decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").unwrap();
    let dto = AttachmentService::new(root.join("attachments"))
        .ingest(AttachmentIngestDto {
            id: Some("h71-image".into()),
            session_id: None,
            kind: "image".into(),
            name: "pixel.png".into(),
            mime: "image/png".into(),
            source: "picker".into(),
            local_path: None,
            text: None,
            data_url: Some(format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
        })
        .unwrap();
    assert_eq!(dto.status, "ready");
    #[cfg(feature = "runtimes")]
    {
        let decoded = image::open(dto.original_path.as_ref().unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1, 1));
    }
    dto
}

fn reference(dto: &deepagent_app_core::AttachmentDto) -> MessageAttachment {
    serde_json::from_value(json!({"id":dto.id,"kind":"image","path":dto.original_path,
        "media_type":dto.mime,"sha256":dto.sha256}))
    .unwrap()
}

fn terminal(db: &Database, run: &str, kind: &str) {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(run).unwrap().unwrap().terminal_kind.as_deref(),
        Some(kind)
    );
    let events = store.events_after(run, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], kind);
}

async fn run(
    chat: &ChatService,
    session: Option<&str>,
    id: &str,
    image: Option<&deepagent_app_core::AttachmentDto>,
) -> Result<String> {
    let attachments = image
        .into_iter()
        .map(|dto| InputAttachment::Image {
            id: dto.id.clone(),
            path: dto.original_path.as_ref().unwrap().into(),
            media_type: dto.mime.clone(),
            sha256: dto.sha256.clone(),
        })
        .collect();
    chat.run_in_session_with_attachments(
        "Inspect the synthetic image.",
        session,
        None,
        None,
        vec![],
        None,
        false,
        Some(id),
        attachments,
        |_| {},
        |_| {},
    )
    .await
}

#[tokio::test]
async fn sqlite_reopen_model_switch_rejects_before_read_and_vision_recovers() {
    let root = tempfile::tempdir().unwrap();
    let dto = imported(root.path());
    let path = PathBuf::from(dto.original_path.as_ref().unwrap());
    let original = std::fs::read(&path).unwrap();
    let database = root.path().join("history.sqlite");
    let model = Arc::new(Model::default());
    let secrets = Arc::new(MemorySecretStore::new());
    let session;
    let admitted;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            model.clone(),
            secrets.clone(),
        ));
        let view = settings.initialize("synthetic-placeholder").await.unwrap();
        let encoded = serde_json::to_value(view).unwrap();
        let cap = encoded["model_capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|cap| cap["model_id"] == "h71-text")
            .unwrap();
        assert_eq!(cap["input_modalities"], json!(["text"]));
        settings
            .set_vision_settings(VisionSettings {
                mode: VisionMode::Off,
                ..Default::default()
            })
            .unwrap();
        settings.set_model(ModelRole::Chat, "h71-vision").unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
        session = run(&chat, None, "h71-first", Some(&dto)).await.unwrap();
        terminal(&db, "h71-first", "succeeded");
        admitted = read_failure::image_events(&db, &session);
    }
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            model.clone(),
            secrets.clone(),
        ));
        assert_eq!(
            serde_json::to_value(settings.load().unwrap().unwrap().catalog).unwrap()["available"]
                [0]["input_modalities"],
            json!(["text", "image"])
        );
        settings.set_model(ModelRole::Chat, "h71-text").unwrap();
        // A rejected model must not read even an unreadable retained image.
        std::fs::write(&path, []).unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
        let error = run(&chat, Some(&session), "h71-text-image", None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("image_input_unsupported"),
            "{error}"
        );
        terminal(&db, "h71-text-image", "failed");
        assert_eq!(read_failure::image_events(&db, &session), admitted);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }
    std::fs::write(&path, &original).unwrap();
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), model.clone(), secrets));
        settings.set_model(ModelRole::Chat, "h71-vision").unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
        run(&chat, Some(&session), "h71-recovered", None)
            .await
            .unwrap();
        terminal(&db, "h71-recovered", "succeeded");
        assert_eq!(read_failure::image_events(&db, &session), admitted);
    }
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for body in requests.iter() {
        assert_eq!(body["model"], "h71-vision");
        let images: Vec<_> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .collect();
        assert_eq!(images.len(), 1);
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(
                    images[0]["image_url"]
                        .as_str()
                        .unwrap()
                        .split_once(',')
                        .unwrap()
                        .1
                )
                .unwrap(),
            original
        );
    }
}

struct CountingResolver(AtomicUsize);
impl AttachmentDataResolver for CountingResolver {
    fn data_url(&self, attachment: &MessageAttachment) -> Option<String> {
        self.try_data_url(attachment).ok().flatten()
    }
    fn try_data_url(&self, attachment: &MessageAttachment) -> Result<Option<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        FileAttachmentResolver.try_data_url(attachment)
    }
    fn is_resolvable(&self, attachment: &MessageAttachment) -> bool {
        FileAttachmentResolver.is_resolvable(attachment)
    }
}

#[tokio::test]
async fn effective_model_gate_precedes_resolver_and_survives_retry_fallback_cancel() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for scenario in [
            "text",
            "unknown",
            "cancel",
            "retry",
            "fallback-text",
            "fallback-vision",
        ] {
            let root = tempfile::tempdir().unwrap();
            let dto = imported(root.path());
            let model = Arc::new(Model {
                failure_status: match scenario {
                    "retry" => Some(503),
                    "fallback-text" | "fallback-vision" => Some(529),
                    _ => None,
                },
                corrupt_after_first: (scenario == "fallback-text")
                    .then(|| dto.original_path.as_ref().unwrap().into()),
                ..Default::default()
            });
            let catalog = deepagent_models::ModelDiscovery::deepseek(model.clone())
                .discover("synthetic")
                .await
                .unwrap();
            let client = Arc::new(ModelClient::new(
                model.clone(),
                ModelConfig::from_catalog("synthetic", &catalog, ModelRole::Chat)
                    .with_wire_mode(mode),
            ));
            let resolver = Arc::new(CountingResolver(AtomicUsize::new(0)));
            let selected = match scenario {
                "text" => "h71-text",
                "unknown" | "cancel" => "h71-unknown",
                _ => "h71-vision",
            };
            let mut agent = ModelAgent::new_with_attachments(
                client,
                selected,
                "rules",
                "synthetic image",
                vec![reference(&dto)],
                vec![],
            )
            .with_attachment_resolver(resolver.clone())
            .with_max_model_attempts(3);
            if scenario == "fallback-text" {
                agent = agent.with_fallback_model(Some("h71-text".into()));
            }
            if scenario == "fallback-vision" {
                agent = agent.with_fallback_model(Some("deepseek-flash".into()));
            }
            let result = agent
                .think_cancelled(
                    0,
                    &[],
                    Some(Arc::new(AtomicBool::new(scenario == "cancel"))),
                )
                .await;
            match scenario {
                "retry" | "fallback-vision" => {
                    result.unwrap();
                    assert_eq!(model.requests.lock().unwrap().len(), 2);
                    // Existing runtime materializes its initial projection,
                    // then rebuilds the canonical Surface for each attempt.
                    assert_eq!(resolver.0.load(Ordering::SeqCst), 3);
                }
                "fallback-text" => {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("image_input_unsupported"));
                    assert_eq!(model.requests.lock().unwrap().len(), 1);
                    assert_eq!(resolver.0.load(Ordering::SeqCst), 2);
                }
                _ => {
                    let message = result.unwrap_err().to_string();
                    assert!(
                        message.contains(if scenario == "cancel" {
                            "cancelled"
                        } else {
                            "image_input_"
                        }),
                        "{message}"
                    );
                    assert!(model.requests.lock().unwrap().is_empty());
                    assert_eq!(resolver.0.load(Ordering::SeqCst), 0);
                }
            }
        }
    }
}
