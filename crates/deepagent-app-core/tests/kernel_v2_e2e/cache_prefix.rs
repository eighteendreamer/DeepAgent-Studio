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

#[tokio::test]
#[ignore = "audit: volatile system context must not precede reusable conversation history"]
async fn audit_dynamic_instructions_do_not_cut_off_conversation_cache_prefix() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"cache-audit\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([
            response_text("AUDIT_DYNAMIC_CACHE_FIRST_ANSWER"),
            response_text("AUDIT_DYNAMIC_CACHE_SECOND_ANSWER"),
        ]),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path());

    let session = chat
        .run_in_session(
            "AUDIT_DYNAMIC_CACHE_FIRST_INPUT",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-dynamic-cache-first"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();

    // Change only volatile workspace context between turns. The next request
    // still retains the complete prior input history, but that history follows
    // `instructions` in the provider's semantic prompt order.
    std::fs::write(
        root.path().join("src/new_after_first_turn.rs"),
        "pub fn marker() {}\n",
    )
    .unwrap();
    chat.run_in_session(
        "AUDIT_DYNAMIC_CACHE_SECOND_INPUT",
        Some(&session),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-dynamic-cache-second"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_instructions = requests[0]["instructions"].as_str().unwrap();
    let second_instructions = requests[1]["instructions"].as_str().unwrap();
    let boundary = first_instructions.find("<<<DYNAMIC>>>").unwrap();
    let first_difference = first_instructions
        .bytes()
        .zip(second_instructions.bytes())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| first_instructions.len().min(second_instructions.len()));
    let first_input = requests[0]["input"].as_array().unwrap();
    let second_input = requests[1]["input"].as_array().unwrap();
    assert!(second_input.starts_with(first_input));
    assert!(second_instructions.contains("new_after_first_turn.rs"));
    assert!(first_difference > boundary);
    eprintln!(
        "dynamic cache audit: stable_instruction_bytes={boundary}, first_difference={first_difference}, first_instructions_bytes={}, second_instructions_bytes={}, retained_first_input_items={}, second_input_items={}",
        first_instructions.len(),
        second_instructions.len(),
        first_input.len(),
        second_input.len()
    );

    assert_eq!(
        first_instructions, second_instructions,
        "DeepSeek treats instructions as the leading system message; changing volatile instructions before the retained input history cuts the reusable prefix off before that history"
    );
}
