//! Follow one real imported image from the live turn into retained history.

use super::*;
use deepagent_app_core::{AttachmentIngestDto, AttachmentService, VisionMode, VisionSettings};
use deepagent_models::ModelRole;
use deepagent_runtime::InputAttachment;

struct ImageAnswer {
    images_per_request: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl HttpTransport for ImageAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
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
        let item = serde_json::json!({
            "type": "message", "id": "audit-image-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline image preparation audit."}],
        });
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{
                    "status":"completed","output":[item]
                }})
                .to_string(),
            ],
        )
    }
}

async fn imported_image_across_turns() -> Vec<usize> {
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
    let settings = Arc::new(SettingsService::new(
        db.clone(),
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
    let counts = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(ImageAnswer {
            images_per_request: counts.clone(),
        }),
        root.path(),
    );
    let session_id = chat
        .run_in_session_with_attachments(
            "Inspect the attached image for this offline audit.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-image-first"),
            vec![InputAttachment::Image {
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
    chat.run_in_session(
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
    .await
    .unwrap();
    assert!(path.is_file());
    for run in ["audit-image-first", "audit-image-continued"] {
        let events = chat.run_events(run, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| event.event_type == "run_terminal" && event.data["kind"] == "succeeded"));
    }
    let counts = counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2);
    eprintln!("image-preparation imported_ready=true bytes={} file_still_exists=true first={} continued={} both_runs_succeeded=true", imported.size_bytes, counts[0], counts[1]);
    counts
}

#[tokio::test]
async fn retained_imported_image_reaches_continuation_request() {
    let counts = imported_image_across_turns().await;
    assert_eq!(
        counts[1], 1,
        "the same file/resolver must work through retained native history"
    );
}

#[tokio::test]
#[ignore = "audit probe: current turn attachments must reach the first model request"]
async fn audit_new_image_reaches_first_model_request() {
    assert_eq!(imported_image_across_turns().await, vec![1, 1]);
}
