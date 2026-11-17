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
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
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
async fn h171_native_image_roles_survive_database_reopen_without_silent_relabeling() {
    use deepagent_core::response_item::ResponseInputItem;
    use deepagent_persistence::event_store::EventStore;
    use deepagent_session::Session;
    for role in ["user", "developer", "system", "assistant"] {
        let root = tempfile::tempdir().unwrap();
        let dto = imported(root.path());
        let original_bytes = std::fs::read(dto.original_path.as_ref().unwrap()).unwrap();
        let native = ResponseInputItem::InputMessage {
            role: role.into(),
            content: "H171_IMAGE_REFERENCE".into(),
            attachments: vec![reference(&dto)],
        };
        let database = root.path().join("h171.sqlite");
        let model = Arc::new(Model::default());
        let secrets = Arc::new(MemorySecretStore::new());
        let mut session_id = None;
        for phase in 0..2 {
            let db = Arc::new(Database::open(&database).unwrap());
            let settings = Arc::new(SettingsService::new(
                db.clone(),
                model.clone(),
                secrets.clone(),
            ));
            if phase == 0 {
                settings.initialize("synthetic-h171").await.unwrap();
                settings
                    .set_vision_settings(VisionSettings {
                        mode: VisionMode::Off,
                        ..Default::default()
                    })
                    .unwrap();
                settings.set_model(ModelRole::Chat, "h71-vision").unwrap();
            }
            let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
            if phase == 0 {
                let id = chat.create_session(None).unwrap();
                let clock = deepagent_core::clock::SystemClock;
                Session::recover(&db, &clock, id.parse().unwrap())
                    .unwrap()
                    .append_response_item(native.clone())
                    .unwrap();
                session_id = Some(id);
                if matches!(role, "system" | "assistant") {
                    // A role error must precede attachment decoding as well as sampling.
                    std::fs::write(dto.original_path.as_ref().unwrap(), []).unwrap();
                }
            }
            let session = session_id.as_deref().unwrap();
            let run_id = format!("h171-{role}-{phase}");
            let result = run(&chat, Some(session), &run_id, None).await;
            if matches!(role, "user" | "developer") {
                result.unwrap();
                terminal(&db, &run_id, "succeeded");
                let requests = model.requests.lock().unwrap();
                assert_eq!(requests.len(), phase + 1);
                let images: Vec<_> = requests[phase]["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| {
                        item["content"].as_array().is_some_and(|parts| {
                            parts.iter().any(|part| part["type"] == "input_image")
                        })
                    })
                    .collect();
                assert_eq!(images.len(), 1);
                assert_eq!(images[0]["role"], role);
                assert!(images[0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("H171_IMAGE_REFERENCE"));
                assert!(images[0]["content"][1]["image_url"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,"));
                use base64::Engine;
                let url = images[0]["content"][1]["image_url"].as_str().unwrap();
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(url.split_once(',').unwrap().1)
                        .unwrap(),
                    original_bytes
                );
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("image_input_role_unsupported"));
                terminal(&db, &run_id, "failed");
                assert!(model.requests.lock().unwrap().is_empty());
            }
            let retained: Vec<_> = EventStore::new(&db)
                .load_session(session.parse().unwrap())
                .unwrap()
                .into_iter()
                .filter_map(|event| match event.payload {
                    deepagent_core::event::EventPayload::ResponseItemAppended { item }
                        if !item.image_attachments().is_empty() =>
                    {
                        Some(item)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                retained.as_slice(),
                std::slice::from_ref(&native),
                "the authoritative imported role/references must not be rewritten"
            );
        }
    }
}

#[tokio::test]
async fn h171_model_agent_role_policy_precedes_reads_retries_and_overload() {
    use deepagent_core::response_item::ResponseInputItem;
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for role in ["user", "developer", "system", "assistant"] {
            for status in [503, 529] {
                let root = tempfile::tempdir().unwrap();
                let dto = imported(root.path());
                let model = Arc::new(Model {
                    failure_status: Some(status),
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
                let native = ResponseInputItem::InputMessage {
                    role: role.into(),
                    content: "H171_IMAGE_REFERENCE".into(),
                    attachments: vec![reference(&dto)],
                };
                let mut agent = ModelAgent::new(client, "h71-vision", "rules", "continue", vec![])
                    .with_response_history(vec![native])
                    .with_attachment_resolver(resolver.clone())
                    .with_max_model_attempts(3);
                let result = agent.think(0, &[]).await;
                if role == "user" || mode == WireMode::Responses && role == "developer" {
                    let expected_calls = if status == 529 {
                        assert_eq!(
                            deepagent_models::classify_model_error(&result.unwrap_err()),
                            deepagent_models::ModelFailureKind::Overloaded
                        );
                        1
                    } else {
                        result.unwrap();
                        2
                    };
                    let requests = model.requests.lock().unwrap();
                    assert_eq!(requests.len(), expected_calls);
                    assert_eq!(resolver.0.load(Ordering::SeqCst), expected_calls + 1);
                    for request in requests.iter() {
                        let messages = request[if mode == WireMode::Responses {
                            "input"
                        } else {
                            "messages"
                        }]
                        .as_array()
                        .unwrap();
                        let visual = messages
                            .iter()
                            .find(|message| message["content"].is_array())
                            .unwrap();
                        assert_eq!(visual["role"], role);
                    }
                } else {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("image_input_role_unsupported"));
                    assert!(model.requests.lock().unwrap().is_empty());
                    assert_eq!(resolver.0.load(Ordering::SeqCst), 0);
                }
            }
        }
    }
}

#[tokio::test]
async fn effective_model_gate_precedes_resolver_and_survives_retry_overload_cancel() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for scenario in ["text", "unknown", "cancel", "retry", "overload"] {
            let root = tempfile::tempdir().unwrap();
            let dto = imported(root.path());
            let model = Arc::new(Model {
                failure_status: match scenario {
                    "retry" => Some(503),
                    "overload" => Some(529),
                    _ => None,
                },
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
            let result = agent
                .think_cancelled(
                    0,
                    &[],
                    Some(Arc::new(AtomicBool::new(scenario == "cancel"))),
                )
                .await;
            match scenario {
                "retry" => {
                    result.unwrap();
                    assert_eq!(model.requests.lock().unwrap().len(), 2);
                    // Existing runtime materializes its initial projection,
                    // then rebuilds the canonical Surface for each attempt.
                    assert_eq!(resolver.0.load(Ordering::SeqCst), 3);
                }
                "overload" => {
                    assert_eq!(
                        deepagent_models::classify_model_error(&result.unwrap_err()),
                        deepagent_models::ModelFailureKind::Overloaded
                    );
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
