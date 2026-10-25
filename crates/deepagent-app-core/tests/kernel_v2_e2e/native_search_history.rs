//! Native search is provider-owned history, not an executable local tool call.

use super::*;
use deepagent_app_core::{WebSearchProvider, WebSearchSettings};
use deepagent_core::{event::EventPayload, response_item::ResponseItem};
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use serde_json::{json, Value};

const SEARCH_ID: &str = "ws-offline-history-audit";
const FIRST_ANSWER: &str = "AUDIT_NATIVE_SEARCH_ANSWER";

fn search_item() -> Value {
    json!({"type":"web_search_call","id":SEARCH_ID,"status":"completed",
        "action":{"type":"search","queries":["AUDIT_SYNTHETIC_QUERY","第二条合成查询"]}})
}

fn search_response() -> Vec<String> {
    let item = search_item();
    let message = super::responses_fixture::text("search-answer-item", FIRST_ANSWER);
    let mut added = item.clone();
    added["status"] = json!("in_progress");
    added.as_object_mut().unwrap().remove("action");
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"native-search-response",
            "object":"response","created_at":1,"model":"deepseek-v4-pro",
            "status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":added}),
    ];
    for status in ["in_progress", "searching", "completed"] {
        events.push(json!({"type":format!("response.web_search_call.{status}"),
            "item_id":SEARCH_ID,"output_index":0}));
    }
    events.extend([
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.output_item.added","output_index":1,
            "item":{"type":"message","id":"search-answer-item","role":"assistant",
                "status":"in_progress","content":[]}}),
        json!({"type":"response.output_item.done","output_index":1,"item":message}),
        json!({"type":"response.completed","response":{"id":"native-search-response",
            "object":"response","created_at":1,"model":"deepseek-v4-pro","status":"completed",
            "output":[item,message],"usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14},
            "error":null,"incomplete_details":null}}),
    ]);
    events
        .into_iter()
        .enumerate()
        .map(|(index, mut event)| {
            event["sequence_number"] = index.into();
            event.to_string()
        })
        .collect()
}

struct NativeSearchModel {
    first_search: bool,
    requests: Mutex<Vec<Value>>,
}

impl NativeSearchModel {
    fn new(first_search: bool) -> Self {
        Self {
            first_search,
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl HttpTransport for NativeSearchModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request: Value = serde_json::from_str(&request.body)?;
        assert_eq!(request["model"], "deepseek-v4-pro");
        assert!(request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["type"] == "web_search"));
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        let events = if self.first_search && attempt == 1 {
            search_response()
        } else {
            super::responses_fixture::completed(
                "search-followup-response",
                "deepseek-v4-pro",
                super::responses_fixture::text(
                    "search-followup-item",
                    "AUDIT_NATIVE_SEARCH_CONTINUED",
                ),
                json!({"input_tokens":13,"output_tokens":2,"total_tokens":15}),
            )
        };
        emit_responses_events(sink, &events)
    }
}

async fn settings(
    db: Arc<Database>,
    secrets: Arc<MemorySecretStore>,
    initialize: bool,
) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        secrets,
    ));
    assert_eq!(settings.load().unwrap().is_none(), initialize);
    if initialize {
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
        for role in [ModelRole::Chat, ModelRole::Reasoner] {
            settings.set_model(role, "deepseek-v4-pro").unwrap();
        }
        settings
            .set_tool_search_mode(deepagent_builtins::ToolSearchMode::Disabled)
            .unwrap();
        settings
            .set_web_search_settings(WebSearchSettings {
                enabled: true,
                provider: WebSearchProvider::DeepSeekFirst,
                ..Default::default()
            })
            .unwrap();
    }
    settings
}

async fn run(chat: &ChatService, session: Option<&str>, run_id: &str) -> String {
    chat.run_in_session(
        "Continue this offline native-search audit.",
        session,
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
    .unwrap()
}

fn assert_stored_search(db: &Database, session: &str) {
    let events = EventStore::new(db)
        .load_session(session.parse().unwrap())
        .unwrap();
    let searches: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ResponseItemAppended {
                item: ResponseItem::WebSearchCall { id, status, action },
            } => Some(json!({"type":"web_search_call","id":id,"status":status,"action":action})),
            _ => None,
        })
        .collect();
    assert_eq!(searches, [search_item()]);
}

fn assert_lifecycle(chat: &ChatService) {
    let events = chat.run_events("native-search-first", None).unwrap();
    let searches: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "responses_web_search_call")
        .collect();
    for status in ["in_progress", "searching", "completed"] {
        assert!(searches
            .iter()
            .any(|event| event.data["call_id"] == SEARCH_ID && event.data["status"] == status));
    }
    assert!(searches
        .iter()
        .any(|event| event.data["action_type"] == "search" && event.data["queries_count"] == 2));
    assert!(searches
        .iter()
        .all(|event| !event.data.to_string().contains("AUDIT_SYNTHETIC_QUERY")));
    assert!(!events
        .iter()
        .any(|event| event.event_type == "tool_started"));
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "succeeded");
}

async fn observe(cold: bool) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let db = Arc::new(Database::open(&path).unwrap());
    let config = settings(db.clone(), secrets.clone(), true).await;
    let model = Arc::new(NativeSearchModel::new(true));
    let chat = ChatService::new(db.clone(), config, model.clone(), &workspace);
    let session = run(&chat, None, "native-search-first").await;
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_stored_search(&db, &session);
    assert_lifecycle(&chat);
    let (chat, model, db) = if cold {
        drop(chat);
        drop(model);
        drop(db);
        let db = Arc::new(Database::open(&path).unwrap());
        assert_stored_search(&db, &session);
        let config = settings(db.clone(), secrets, false).await;
        let model = Arc::new(NativeSearchModel::new(false));
        let chat = ChatService::new(db.clone(), config, model.clone(), &workspace);
        assert_lifecycle(&chat);
        (chat, model, db)
    } else {
        (chat, model, db)
    };
    run(&chat, Some(&session), "native-search-next").await;
    assert_stored_search(&db, &session);
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), if cold { 1 } else { 2 });
    let input = requests.last().unwrap()["input"].as_array().unwrap();
    let searches: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "web_search_call")
        .cloned()
        .collect();
    assert_eq!(searches, [search_item()]);
    let search_index = input
        .iter()
        .position(|item| item["type"] == "web_search_call")
        .unwrap();
    assert!(input[search_index + 1..]
        .iter()
        .any(|item| item["role"] == "assistant" && item["content"] == FIRST_ANSWER));
    assert!(!input
        .iter()
        .any(|item| item["type"] == "function_call_output" && item["call_id"] == SEARCH_ID));
    eprintln!("native search history: cold={cold}, completed_search_items={}, input_items={}, local_tool_outputs=0", searches.len(), input.len());
}

#[tokio::test]
async fn native_search_item_and_lifecycle_survive_warm_continuation() {
    observe(false).await;
}

#[tokio::test]
async fn native_search_item_and_lifecycle_survive_clean_reconstruction() {
    observe(true).await;
}
