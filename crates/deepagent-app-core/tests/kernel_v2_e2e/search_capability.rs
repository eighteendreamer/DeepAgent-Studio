//! Audit native search selection against the selected model's capabilities.

use super::*;
use deepagent_app_core::{WebSearchProvider, WebSearchSettings};
use deepagent_models::ModelRole;

struct CaptureAnswer {
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[async_trait]
impl HttpTransport for CaptureAnswer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        emit_responses_events(sink, &response_text("Offline audit response."))
    }
}

async fn search_on_wire(model: &str, provider: WebSearchProvider, enabled: bool) -> (bool, bool) {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, model).unwrap();
    }
    settings
        .set_tool_search_mode(deepagent_builtins::ToolSearchMode::Disabled)
        .unwrap();
    settings
        .set_web_search_settings(WebSearchSettings {
            enabled,
            provider,
            ..WebSearchSettings::default()
        })
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let chat = ChatService::new(
        db,
        settings,
        Arc::new(CaptureAnswer {
            requests: requests.clone(),
        }),
        root.path(),
    );
    let run_id = "audit-search-model-capability";
    chat.run_in_session(
        "Reply briefly for this offline capability audit.",
        None,
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
    .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], model);
    let tools = requests[0]["tools"].as_array().unwrap();
    let native = tools.iter().any(|tool| tool["type"] == "web_search");
    let function = tools
        .iter()
        .any(|tool| tool["type"] == "function" && tool["name"] == "web_search");
    let events = chat.run_events(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1,
    );
    assert!(events
        .iter()
        .any(|event| event.event_type == "run_terminal" && event.data["kind"] == "succeeded"));
    // No web tool is invoked: this probe inspects the actual outbound schema,
    // without issuing search requests or pretending to test search quality.
    eprintln!(
        "search-capability model={model} provider={provider:?} enabled={enabled} \
         native={native} callable_function={function} run_succeeded=true",
    );
    (native, function)
}

async fn assert_search_controls() {
    assert_eq!(
        search_on_wire("deepseek-v4-pro", WebSearchProvider::DeepSeekFirst, true).await,
        (true, false),
    );
    assert_eq!(
        search_on_wire("deepseek-flash", WebSearchProvider::DuckDuckGo, true).await,
        (false, true),
    );
    assert_eq!(
        search_on_wire("deepseek-flash", WebSearchProvider::DeepSeekFirst, false).await,
        (false, false),
    );
}

#[tokio::test]
async fn supported_native_and_external_search_keep_their_wire_contracts() {
    assert_search_controls().await;
}

#[tokio::test]
#[ignore = "audit probe: Flash must keep callable search when native search is unsupported"]
async fn audit_flash_keeps_callable_search_when_native_is_unsupported() {
    assert_search_controls().await;
    assert_eq!(
        search_on_wire("deepseek-flash", WebSearchProvider::DeepSeekFirst, true).await,
        (false, true),
        "Flash ignores native web_search: preserve the callable provider fallback",
    );
}
