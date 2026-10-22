//! A raw branch sequence can fall between a source event and its native mirror.

use super::*;
use deepagent_app_core::AppService;
use deepagent_core::{clock::FixedClock, event::EventPayload, message::Message};
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_session::Session;

const RETAINED: &str = "audit-retained-user-branch-anchor-7f80";

#[derive(Debug, Clone, Copy)]
enum BranchAction {
    Fork,
    Rewind,
}

struct BranchAnswer {
    user_texts: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl HttpTransport for BranchAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let texts = body["input"]
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
            .collect();
        self.user_texts.lock().unwrap().push(texts);
        let item = serde_json::json!({
            "type": "message", "id": "audit-branch-answer", "role": "assistant",
            "content": [{"type": "output_text", "text": "Offline branch answer."}],
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
    let target = match action {
        BranchAction::Fork => app.fork_session(&source_id, keep).unwrap().new_session_id,
        BranchAction::Rewind => {
            app.rewind_session(&source_id, keep).unwrap();
            source_id
        }
    };
    let kept = EventStore::new(&db)
        .load_session(target.parse().unwrap())
        .unwrap();
    assert_eq!(kept.last().unwrap().sequence, keep);
    assert!(kept.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageAppended { message } if message.content == RETAINED
    )));
    assert!(!kept.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageAppended { message }
            if message.content == "audit-discarded-assistant"
    )));
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
    let user_texts = Arc::new(Mutex::new(Vec::new()));
    let workspace = tempfile::tempdir().unwrap();
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(BranchAnswer {
            user_texts: user_texts.clone(),
        }),
        workspace.path(),
    );
    chat.run_in_session(
        "audit-continue-current-branch",
        Some(&target),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-branch-continuation"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let events = chat.run_events("audit-branch-continuation", None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let requests = user_texts.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let texts = &requests[0];
    assert!(texts.iter().any(|text| text.contains("audit-earlier-user")));
    assert!(texts
        .iter()
        .any(|text| text.contains("audit-continue-current-branch")));
    let retained = texts.iter().any(|text| text.contains(RETAINED));
    eprintln!("branch request audit: action={action:?}, keep={keep}, mirror={keep_native_mirror}, retained_user_sent={retained}");
    retained
}

#[tokio::test]
async fn complete_native_branch_boundaries_keep_user_input() {
    for action in [BranchAction::Fork, BranchAction::Rewind] {
        assert!(observe_branch_request(action, true).await);
    }
}

#[tokio::test]
#[ignore = "audit: a kept source message must survive the next native model request"]
async fn audit_branch_at_source_message_preserves_that_message_in_request() {
    let mut results = Vec::new();
    for action in [BranchAction::Fork, BranchAction::Rewind] {
        results.push((action, observe_branch_request(action, false).await));
    }
    assert!(results.iter().all(|(_, retained)| *retained), "{results:?}");
}
