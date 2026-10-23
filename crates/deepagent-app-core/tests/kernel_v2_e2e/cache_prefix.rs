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
async fn consecutive_session_requests_preserve_responses_prefix_on_wire() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([
            response_text("AUDIT_CACHE_FIRST_ANSWER"),
            response_text("AUDIT_CACHE_SECOND_ANSWER"),
        ]),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path());

    let session = chat
        .run_in_session(
            "AUDIT_CACHE_FIRST_INPUT",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-cache-first"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    chat.run_in_session(
        "AUDIT_CACHE_SECOND_INPUT",
        Some(&session),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-cache-second"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let instructions = requests
        .iter()
        .map(|request| request["instructions"].as_str().unwrap())
        .collect::<Vec<_>>();
    let dynamic_boundary = instructions[0]
        .find("<<<DYNAMIC>>>")
        .expect("production system prompt has its cache boundary");
    assert_eq!(
        &instructions[0][..dynamic_boundary],
        &instructions[1][..dynamic_boundary],
        "same-session requests must preserve the production stable prompt prefix"
    );

    assert_eq!(
        requests[0]["tools"], requests[1]["tools"],
        "an unchanged tool registry must serialize in the same order"
    );
    let first_input = requests[0]["input"].as_array().unwrap();
    let second_input = requests[1]["input"].as_array().unwrap();
    assert!(
        second_input.starts_with(first_input),
        "the next stateless Responses request must retain the prior input prefix"
    );
    assert!(second_input.iter().any(|item| {
        item["content"]
            .as_str()
            .is_some_and(|text| text.contains("AUDIT_CACHE_FIRST_ANSWER"))
    }));
    assert!(second_input.iter().any(|item| {
        item["content"]
            .as_str()
            .is_some_and(|text| text.contains("AUDIT_CACHE_SECOND_INPUT"))
    }));
}
