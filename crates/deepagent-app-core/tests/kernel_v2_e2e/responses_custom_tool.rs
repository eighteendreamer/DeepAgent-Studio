use super::*;

struct ObservedModel {
    replay: ReplayTransport,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for ObservedModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        self.replay.stream(request, sink).await
    }
}

#[tokio::test]
async fn default_harness_request_does_not_advertise_apply_patch() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "sk-audit-placeholder")
        .await
        .unwrap();
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([response_text("AUDIT_CUSTOM_TOOL_CAPTURED")]),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    chat.run_in_session(
        "Reply briefly without using tools.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-custom-tool-schema"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let tools = requests[0]["tools"].as_array().unwrap();
    assert!(
        !tools.is_empty(),
        "the request must carry the normal tool set"
    );
    assert!(
        !tools.iter().any(|tool| tool["name"] == "apply_patch"),
        "the default AppCore registry has no first-party apply_patch tool"
    );
}
