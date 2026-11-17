//! H-67: admitted images belong to the live, durable and recovered Surface.
use super::*;
use deepagent_app_core::{AttachmentIngestDto, AttachmentService};
use deepagent_core::event::EventPayload;
use deepagent_core::response_item::ResponseItem;
use deepagent_runtime::InputAttachment;

#[derive(Default)]
struct ImageSurfaceTransport {
    requests: Mutex<Vec<serde_json::Value>>,
    tool_issued: AtomicBool,
}

fn image_count(body: &serde_json::Value) -> usize {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "input_image")
        .count()
}

#[async_trait]
impl HttpTransport for ImageSurfaceTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let images = image_count(&body);
        self.requests.lock().unwrap().push(body);
        let item = if images > 0 && !self.tool_issued.swap(true, Ordering::AcqRel) {
            serde_json::json!({"type":"function_call","call_id":"h67-read", "name":"read_file",
                "arguments":serde_json::json!({"path":"fixture.txt"}).to_string()})
        } else {
            serde_json::json!({"type":"message","role":"assistant","id":"h67-answer",
                "content":[{"type":"output_text","text":"Offline image surface answer."}]})
        };
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash",
                "status":"completed","output":[item]}})
                .to_string(),
            ],
        )
    }
}

#[tokio::test]
async fn multi_image_tool_loop_and_disk_restart_keep_one_admitted_turn() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture.txt"), "h67 tool result").unwrap();
    let service = AttachmentService::new(root.path().join("attachments"));
    let mut inputs = Vec::new();
    for id in ["h67-left", "h67-right"] {
        let imported = service.ingest(AttachmentIngestDto {
            id: Some(id.into()), session_id: None, kind: "image".into(),
            name: format!("{id}.png"), mime: "image/png".into(), source: "picker".into(),
            local_path: None, text: None,
            data_url: Some(concat!("data:image/png;base64,",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").into()),
        }).unwrap();
        assert_eq!(imported.status, "ready");
        inputs.push(InputAttachment::Image {
            sha256: imported.sha256.clone(),
            id: imported.id,
            path: imported.original_path.unwrap().into(),
            media_type: "image/png".into(),
        });
    }
    let transport = Arc::new(ImageSurfaceTransport::default());
    let database = root.path().join("h67.sqlite");
    let session_id;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = image_preparation::image_settings(db.clone()).await;
        let chat = ChatService::new(db.clone(), settings, transport.clone(), root.path());
        session_id = chat
            .run_in_session(
                "h67-text-seed",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h67-seed"),
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        chat.run_in_session_with_attachments(
            "h67-image-turn",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h67-images"),
            inputs,
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session_id.parse().unwrap())
            .unwrap();
        let image_items: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended {
                    item:
                        ResponseItem::InputMessage {
                            content,
                            attachments,
                            ..
                        },
                    ..
                } if content.contains("h67-image-turn") => Some(attachments),
                _ => None,
            })
            .collect();
        assert_eq!(
            image_items.len(),
            1,
            "admission must persist one image item"
        );
        assert_eq!(
            image_items[0]
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            ["h67-left", "h67-right"]
        );
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("data:image"),
            "durable events must remain reference-only"
        );
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ToolCallCompleted { call_id, ok: true, .. } if call_id == "h67-read")));
        assert_one_terminal(&chat, "h67-seed");
        assert_one_terminal(&chat, "h67-images");
    }
    // Reopen the actual database and session log, not an in-memory conversation.
    let db = Arc::new(Database::open(&database).unwrap());
    let settings = image_preparation::image_settings(db.clone()).await;
    let chat = ChatService::new(db.clone(), settings, transport.clone(), root.path());
    chat.run_in_session(
        "h67-after-restart",
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("h67-restart"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_one_terminal(&chat, "h67-restart");
    // Emulate an old source-first text mirror in this test's own durable log.
    // Only its model-view projection may change; the raw records stay intact.
    let store = deepagent_persistence::event_store::EventStore::new(&db);
    let sid = session_id.parse().unwrap();
    let events = store.load_session(sid).unwrap();
    let refs = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ResponseItemAppended {
                item: ResponseItem::InputMessage { attachments, .. },
                ..
            } => Some(attachments.clone()),
            _ => None,
        })
        .unwrap();
    let time = deepagent_core::clock::Timestamp::from_millis(1000);
    store
        .append(
            sid,
            EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("h67-legacy-turn")
                    .with_attachments(refs),
            },
            time,
        )
        .unwrap();
    let old_mirror = store
        .append(
            sid,
            EventPayload::ResponseItemAppended {
                origin: None,
                item: ResponseItem::Message {
                    role: "user".into(),
                    content: "h67-legacy-turn".into(),
                },
            },
            time,
        )
        .unwrap();
    chat.run_in_session(
        "h67-after-legacy",
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("h67-legacy"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_one_terminal(&chat, "h67-legacy");
    assert_eq!(
        store
            .load_session(sid)
            .unwrap()
            .iter()
            .find(|e| e.id == old_mirror.id)
            .unwrap(),
        &old_mirror
    );
    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests.iter().map(image_count).collect::<Vec<_>>(),
        [0, 2, 2, 2, 4]
    );
    for request in &requests[1..] {
        let image_turns: Vec<_> = request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item.to_string().contains("h67-image-turn"))
            .collect();
        assert_eq!(
            image_turns.len(),
            1,
            "do not replay the admitted turn twice"
        );
        assert!(
            image_turns[0].to_string().contains("[id:u2]"),
            "keep snip tags"
        );
    }
    assert!(requests[2]["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "function_call_output" && item["call_id"] == "h67-read"));
}

#[tokio::test]
async fn system_vision_text_projection_does_not_invent_native_images() {
    use deepagent_app_core::{VisionMode, VisionSettings};
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = image_preparation::image_settings(db.clone()).await;
    settings
        .set_vision_settings(VisionSettings {
            mode: VisionMode::System,
            ..VisionSettings::default()
        })
        .unwrap();
    let transport = Arc::new(ImageSurfaceTransport::default());
    let chat = ChatService::new(db, settings, transport.clone(), root.path());
    // App.tsx sends recognized text and an empty structured image list in this
    // mode. Recognition itself is outside this local backend regression.
    let prompt = "Inspect this.\n<attachments><attachment kind=\"image\">\nOffline recognized diagram.\n</attachment></attachments>";
    chat.run_in_session(
        prompt,
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("h67-vision-text"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_one_terminal(&chat, "h67-vision-text");
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(image_count(&requests[0]), 0);
    assert!(requests[0]["input"]
        .to_string()
        .contains("Offline recognized diagram."));
}

fn assert_one_terminal(chat: &ChatService, run: &str) {
    let events = chat.run_events(run, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}
