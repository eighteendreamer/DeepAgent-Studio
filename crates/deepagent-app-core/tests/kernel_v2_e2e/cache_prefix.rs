use super::*;

struct ObservedModel {
    replay: ReplayTransport,
    requests: Mutex<Vec<serde_json::Value>>,
}

async fn cold_cache_settings(
    db: Arc<Database>,
    secrets: Arc<MemorySecretStore>,
    initialize: bool,
) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        secrets,
    ));
    assert_eq!(settings.load().unwrap().is_none(), initialize);
    if initialize {
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
    }
    settings
}

async fn cold_cache_run(chat: &ChatService, session: Option<&str>, prompt: &str) -> String {
    chat.run_in_session(
        prompt,
        session,
        None,
        None,
        Vec::new(),
        None,
        false,
        None,
        |_| {},
        |_| {},
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn clean_reconstruction_preserves_cache_prefix_with_a_real_tool_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(
        workspace.join("cold-prefix-visible.txt"),
        "synthetic fixture\n",
    )
    .unwrap();
    // State stays outside the workspace so DB/log writes cannot change its listing.
    let path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let usage = serde_json::json!({"input_tokens":10,"output_tokens":2,"total_tokens":12});
    let (session, prior) = {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = cold_cache_settings(db.clone(), secrets.clone(), true).await;
        let model = Arc::new(ObservedModel {
            replay: ReplayTransport::new([
                super::responses_fixture::completed(
                    "cold-cache-tool-response",
                    "deepseek-flash",
                    super::responses_fixture::call(
                        "cold-cache-list",
                        "list_dir",
                        serde_json::json!({}),
                    ),
                    usage.clone(),
                ),
                super::responses_fixture::completed(
                    "cold-cache-first-response",
                    "deepseek-flash",
                    super::responses_fixture::text(
                        "cold-cache-first-item",
                        "AUDIT_COLD_CACHE_ANSWER",
                    ),
                    usage.clone(),
                ),
            ]),
            requests: Mutex::new(Vec::new()),
        });
        let chat = ChatService::new(db, settings, model.clone(), &workspace);
        let session = cold_cache_run(&chat, None, "AUDIT_COLD_CACHE_FIRST_INPUT").await;
        let requests = model.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        let last = requests.last().unwrap().clone();
        let output = last["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "cold-cache-list"
            })
            .expect("the real list_dir result must reach the model before the first answer");
        let output = output["output"].as_str().unwrap();
        assert!(output.contains("cold-prefix-visible.txt"));
        assert!(!output.contains("\"status\":\"error\""));
        (session, last)
    }; // All first ChatService/SettingsService/Database/model instances are dropped.

    let db = Arc::new(Database::open(&path).unwrap());
    let settings = cold_cache_settings(db.clone(), secrets, false).await;
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([super::responses_fixture::completed(
            "cold-cache-second-response",
            "deepseek-flash",
            super::responses_fixture::text("cold-cache-second-item", "AUDIT_COLD_CACHE_CONTINUED"),
            usage,
        )]),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), &workspace);
    assert_eq!(
        cold_cache_run(&chat, Some(&session), "AUDIT_COLD_CACHE_NEXT_INPUT").await,
        session
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let next = &requests[0];
    for key in [
        "model",
        "instructions",
        "tools",
        "reasoning",
        "max_output_tokens",
        "tool_choice",
        "user",
    ] {
        assert_eq!(
            prior[key], next[key],
            "cold reconstruction changed request field {key}"
        );
    }
    assert!(!prior["tools"].as_array().unwrap().is_empty());
    let before = prior["input"].as_array().unwrap();
    let after = next["input"].as_array().unwrap();
    assert!(
        after.starts_with(before),
        "cold reconstruction changed the native input prefix"
    );
    assert!(after[before.len()..].iter().any(|item| item["content"]
        .to_string()
        .contains("AUDIT_COLD_CACHE_ANSWER")));
    assert!(after[before.len()..].iter().any(|item| item["content"]
        .to_string()
        .contains("AUDIT_COLD_CACHE_NEXT_INPUT")));
    assert!(next.get("previous_response_id").is_none());
    eprintln!("cold cache wire: stable tools={}, instructions_bytes={}, retained_input_items={}, next_input_items={}",
        prior["tools"].as_array().unwrap().len(), prior["instructions"].as_str().unwrap().len(), before.len(), after.len());
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
