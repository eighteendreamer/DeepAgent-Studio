//! A raw branch sequence can fall between a source event and its native mirror.

use super::*;
use deepagent_app_core::AppService;
use deepagent_core::{
    clock::{Clock, FixedClock},
    event::EventPayload,
    message::Message,
};
use deepagent_persistence::event_store::EventStore;
use deepagent_session::Session;

const RETAINED: &str = "audit-retained-user-branch-anchor-7f80";

#[derive(Debug, Clone, Copy)]
enum BranchAction {
    Fork,
    Rewind,
}

struct BranchAnswer {
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

fn user_texts(body: &serde_json::Value) -> Vec<String> {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["role"] == "user")
        .map(|item| match &item["content"] {
            serde_json::Value::String(text) => text.clone(),
            content => content
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .collect()
}

#[async_trait]
impl HttpTransport for BranchAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        self.requests.lock().unwrap().push(body);
        let item = serde_json::json!({
            "type": "message", "id": "audit-branch-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline branch answer."}],
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

async fn observe_branch_request(action: BranchAction, keep_native_mirror: bool) -> bool {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let mut session = Session::create(&db, &clock, Some("branch boundary audit")).unwrap();
    for message in [
        Message::user("audit-earlier-user"),
        Message::assistant("audit-earlier-assistant"),
    ] {
        session
            .append(EventPayload::MessageAppended { message })
            .unwrap();
    }
    let anchor = session
        .append(EventPayload::MessageAppended {
            message: Message::user(RETAINED),
        })
        .unwrap()
        .sequence;
    let mirrored = EventStore::new(&db)
        .load_session(session.id())
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    assert_eq!(mirrored, anchor + 1);
    session
        .append(EventPayload::MessageAppended {
            message: Message::assistant("audit-discarded-assistant"),
        })
        .unwrap();
    let source_id = session.id().to_string();
    drop(session);
    let app = AppService::from_shared(db.clone());
    let keep = if keep_native_mirror { mirrored } else { anchor };
    let (target, effective) = match action {
        BranchAction::Fork => {
            let result = app.fork_session(&source_id, keep).unwrap();
            (result.new_session_id, result.forked_at)
        }
        BranchAction::Rewind => {
            let result = app.rewind_session(&source_id, keep).unwrap();
            (source_id, result.kept_through)
        }
    };
    // Raw source/interior cursors now resolve to the complete projection group.
    // The DTO and durable tail must agree about the effective boundary.
    assert_eq!(effective, mirrored);
    let kept = EventStore::new(&db)
        .load_session(target.parse().unwrap())
        .unwrap();
    assert_eq!(kept.last().unwrap().sequence, effective);
    assert!(kept.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageAppended { message } if message.content == RETAINED
    )));
    assert!(!kept.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageAppended { message }
            if message.content == "audit-discarded-assistant"
    )));
    let body = branch_continuation(db, &target).await.unwrap();
    let texts = user_texts(&body);
    assert!(texts.iter().any(|text| text.contains("audit-earlier-user")));
    assert!(texts
        .iter()
        .any(|text| text.contains("audit-continue-current-branch")));
    let retained = texts.iter().any(|text| text.contains(RETAINED));
    eprintln!("branch request audit: action={action:?}, keep={keep}, mirror={keep_native_mirror}, retained_user_sent={retained}");
    retained
}

async fn branch_continuation(db: Arc<Database>, target: &str) -> Result<serde_json::Value> {
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek","input_modalities":["text","image"]}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", "deepseek-flash")
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let run_id = format!("audit-branch-{}", deepagent_core::id::EventId::new());
    let workspace = tempfile::tempdir().unwrap();
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(BranchAnswer {
            requests: requests.clone(),
        }),
        workspace.path(),
    );
    let outcome = chat
        .run_in_session(
            "audit-continue-current-branch",
            Some(target),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&run_id),
            |_| {},
            |_| {},
        )
        .await;
    if let Err(error) = outcome {
        assert!(
            requests.lock().unwrap().is_empty(),
            "malformed history reached the model"
        );
        return Err(error);
    }
    let events = chat.run_events(&run_id, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    Ok(requests[0].clone())
}

#[tokio::test]
async fn complete_native_branch_boundaries_keep_user_input() {
    for action in [BranchAction::Fork, BranchAction::Rewind] {
        assert!(observe_branch_request(action, true).await);
    }
}

#[tokio::test]
async fn audit_branch_at_source_message_preserves_that_message_in_request() {
    let mut results = Vec::new();
    for action in [BranchAction::Fork, BranchAction::Rewind] {
        results.push((action, observe_branch_request(action, false).await));
    }
    assert!(results.iter().all(|(_, retained)| *retained), "{results:?}");
}

fn branch_at(db: Arc<Database>, source: &str, cursor: u64, action: BranchAction) -> (String, u64) {
    let app = AppService::from_shared(db);
    match action {
        BranchAction::Fork => {
            let result = app.fork_session(source, cursor).unwrap();
            (result.new_session_id, result.forked_at)
        }
        BranchAction::Rewind => {
            let result = app.rewind_session(source, cursor).unwrap();
            (source.into(), result.kept_through)
        }
    }
}

#[tokio::test]
async fn rich_projection_and_compaction_reach_wire_at_every_cursor_after_disk_reopen() {
    use deepagent_core::message::ToolCall;

    for action in [BranchAction::Fork, BranchAction::Rewind] {
        for offset in 0..=4 {
            let temporary = tempfile::tempdir().unwrap();
            let database_path = temporary.path().join("branch.db");
            let db = Arc::new(Database::open(&database_path).unwrap());
            let clock = FixedClock::new(1000);
            let mut session = Session::create(&db, &clock, None).unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("compacted old input"),
                })
                .unwrap();
            let retained = session
                .append(EventPayload::MessageAppended {
                    message: Message::user(RETAINED),
                })
                .unwrap()
                .sequence;
            session
                .append(EventPayload::ContextCompacted {
                    tokens_before: 5000,
                    tokens_after: 1000,
                    strategy: "session_memory".into(),
                    summary: Some("retained branch summary".into()),
                    first_kept_sequence: Some(retained),
                })
                .unwrap();
            let message = Message::assistant("retained rich answer")
                .with_reasoning("retained branch reasoning")
                .with_tool_calls(vec![
                    ToolCall {
                        id: "branch-read".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path":"fixture.txt"}),
                    },
                    ToolCall {
                        id: "branch-list".into(),
                        name: "list_dir".into(),
                        arguments: serde_json::json!({}),
                    },
                ]);
            let anchor = session
                .append(EventPayload::MessageAppended { message })
                .unwrap()
                .sequence;
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("discarded rich tail"),
                })
                .unwrap();
            let source = session.id().to_string();
            drop(session);
            drop(db);
            let db = Arc::new(Database::open(&database_path).unwrap());
            let (target, effective) = branch_at(db.clone(), &source, anchor + offset, action);
            assert_eq!(effective, anchor + 4);
            assert_eq!(
                EventStore::new(&db)
                    .load_session(target.parse().unwrap())
                    .unwrap()
                    .last()
                    .unwrap()
                    .sequence,
                effective
            );
            drop(db);
            let db = Arc::new(Database::open(&database_path).unwrap());
            let wire = branch_continuation(db, &target).await.unwrap();
            let input = wire["input"].as_array().unwrap();
            let texts = user_texts(&wire);
            assert!(texts
                .iter()
                .any(|text| text.contains("retained branch summary")));
            assert!(texts.iter().any(|text| text.contains(RETAINED)));
            assert!(!texts
                .iter()
                .any(|text| text.contains("compacted old input")
                    || text.contains("discarded rich tail")));
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "reasoning"
                        && item.to_string().contains("retained branch reasoning"))
                    .count(),
                1
            );
            for id in ["branch-read", "branch-list"] {
                assert_eq!(
                    input
                        .iter()
                        .filter(|item| item["type"] == "function_call" && item["call_id"] == id)
                        .count(),
                    1
                );
                // The cut retains calls without results. Existing repair supplies
                // interrupted outputs, without executing those old tools again.
                assert_eq!(
                    input
                        .iter()
                        .filter(
                            |item| item["type"] == "function_call_output" && item["call_id"] == id
                        )
                        .count(),
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn repeated_user_inputs_are_not_deduplicated_at_a_source_boundary() {
    for action in [BranchAction::Fork, BranchAction::Rewind] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1000);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let message = || EventPayload::MessageAppended {
            message: Message::user(RETAINED),
        };
        session.append(message()).unwrap();
        let anchor = session.append(message()).unwrap().sequence;
        let source = session.id().to_string();
        drop(session);
        let (target, _) = branch_at(db.clone(), &source, anchor, action);
        let wire = branch_continuation(db, &target).await.unwrap();
        assert_eq!(
            user_texts(&wire)
                .iter()
                .filter(|text| text.contains(RETAINED))
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn native_attachment_is_kept_once_when_branching_at_its_ui_source() {
    use deepagent_app_core::{AttachmentIngestDto, AttachmentService};
    use deepagent_core::message::MessageAttachment;
    use deepagent_core::response_item::ResponseItem;

    for action in [BranchAction::Fork, BranchAction::Rewind] {
        let temporary = tempfile::tempdir().unwrap();
        let imported = AttachmentService::new(temporary.path().join("attachments")).ingest(AttachmentIngestDto {
            id: Some("branch-image".into()), session_id: None, kind: "image".into(), name: "pixel.png".into(), mime: "image/png".into(), source: "picker".into(), local_path: None,
            data_url: Some(concat!("data:image/png;base64,", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").into()), text: None,
        }).unwrap();
        assert_eq!(imported.status, "ready");
        let attachment = MessageAttachment {
            sha256: imported.sha256,
            id: imported.id,
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: imported.original_path,
        };
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1000);
        let mut session = Session::create(&db, &clock, None).unwrap();
        session
            .append_response_item(ResponseItem::InputMessage {
                role: "user".into(),
                content: RETAINED.into(),
                attachments: vec![attachment.clone()],
            })
            .unwrap();
        let anchor = session
            .append_without_response_projection(EventPayload::MessageAppended {
                message: Message::user(RETAINED).with_attachments(vec![attachment]),
            })
            .unwrap()
            .sequence;
        let source = session.id().to_string();
        drop(session);
        let (target, effective) = branch_at(db.clone(), &source, anchor, action);
        assert_eq!(effective, anchor);
        let wire = branch_continuation(db, &target).await.unwrap();
        assert_eq!(
            user_texts(&wire)
                .iter()
                .filter(|text| text.contains(RETAINED))
                .count(),
            1
        );
        let images: Vec<_> = wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .collect();
        assert_eq!(images.len(), 1);
        assert!(images[0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }
}

#[tokio::test]
async fn malformed_legacy_tail_is_rejected_before_wire_but_earlier_rewind_recovers() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1000);
    let mut session = Session::create(&db, &clock, None).unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: Message::user("valid earlier input"),
        })
        .unwrap();
    let id = session.id().to_string();
    EventStore::new(&db)
        .append(
            session.id(),
            EventPayload::MessageAppended {
                message: Message::user("orphaned legacy source"),
            },
            clock.now(),
        )
        .unwrap();
    drop(session);
    let error = branch_continuation(db.clone(), &id).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("incomplete or ambiguous legacy response projection"),
        "unexpected recovery failure: {error}"
    );
    let result = AppService::from_shared(db.clone())
        .rewind_session(&id, 0)
        .unwrap();
    assert_eq!(result.kept_through, 0);
    let wire = branch_continuation(db, &id).await.unwrap();
    assert!(!user_texts(&wire)
        .iter()
        .any(|text| text.contains("orphaned legacy source")));
}
