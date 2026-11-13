use super::*;

fn latest_wire_snapshot(
    request: &serde_json::Value,
) -> deepagent_core::runtime_snapshot::RuntimeSnapshot {
    request["input"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find_map(|item| {
            let item = deepagent_core::response_item::ResponseItem::Message {
                role: item["role"].as_str()?.into(),
                content: item["content"].as_str()?.into(),
            };
            deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(&item)
        })
        .expect("host runtime snapshot reaches the real wire")
}

pub(super) fn wire_system_text(request: &serde_json::Value) -> String {
    format!(
        "{}\n{}",
        request["instructions"].as_str().unwrap(),
        latest_wire_snapshot(request).context
    )
}

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
async fn h168_changed_workspace_after_cold_reopen_appends_new_version_without_touching_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = temp.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let (session, before) = {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = cold_cache_settings(db.clone(), secrets.clone(), true).await;
        let model = Arc::new(ObservedModel {
            replay: ReplayTransport::new([response_text("first answer")]),
            requests: Mutex::new(vec![]),
        });
        let chat = ChatService::new(db, settings, model.clone(), &workspace);
        let session = cold_cache_run(&chat, None, "first question").await;
        let before = model.requests.lock().unwrap()[0].clone();
        (session, before)
    };
    std::fs::write(workspace.join("h168-new-after-reopen.txt"), "synthetic").unwrap();
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = cold_cache_settings(db.clone(), secrets, false).await;
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([response_text("second answer")]),
        requests: Mutex::new(vec![]),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    cold_cache_run(&chat, Some(&session), "second question").await;
    let requests = model.requests.lock().unwrap();
    let next = &requests[0];
    assert_eq!(before["instructions"], next["instructions"]);
    assert!(next["input"]
        .as_array()
        .unwrap()
        .starts_with(before["input"].as_array().unwrap()));
    assert_eq!(
        latest_wire_snapshot(next).revision,
        latest_wire_snapshot(&before).revision + 1
    );
    assert!(latest_wire_snapshot(next)
        .context
        .contains("h168-new-after-reopen.txt"));
    let sid = session.parse().unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(sid)
        .unwrap();
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| {
            let deepagent_core::event::EventPayload::ResponseItemAppended { item } = &event.payload
            else {
                return None;
            };
            deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(item)
        })
        .collect();
    assert_eq!(
        snapshots,
        vec![latest_wire_snapshot(&before), latest_wire_snapshot(next)]
    );
}

#[tokio::test]
async fn h168_live_permission_update_and_stable_style_change_use_distinct_versions() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("README.md"), "H168_CURRENT_README_CANARY").unwrap();
    let db = Arc::new(Database::open(temp.path().join("state.sqlite")).unwrap());
    let settings = cold_cache_settings(db.clone(), Arc::new(MemorySecretStore::new()), true).await;
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([
            response_text("one"),
            response_text("two"),
            response_text("three"),
        ]),
        requests: Mutex::new(vec![]),
    });
    let chat = ChatService::new(db, settings.clone(), model.clone(), &workspace);
    let session = cold_cache_run(&chat, None, "first question").await;
    settings
        .set_permission_rules(deepagent_hooks::PermissionRules::new(
            [],
            [],
            ["Read".into()],
        ))
        .unwrap();
    cold_cache_run(&chat, Some(&session), "second question").await;
    settings
        .set_output_style(deepagent_app_core::OutputStyle::Explanatory)
        .unwrap();
    cold_cache_run(&chat, Some(&session), "third question").await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let snapshots: Vec<_> = requests.iter().map(latest_wire_snapshot).collect();
    assert!(snapshots[0].context.contains("H168_CURRENT_README_CANARY"));
    assert!(!snapshots[1].context.contains("H168_CURRENT_README_CANARY"));
    assert_eq!(requests[0]["instructions"], requests[1]["instructions"]);
    assert_eq!((snapshots[0].series, snapshots[0].revision), (1, 1));
    assert_eq!((snapshots[1].series, snapshots[1].revision), (1, 2));
    assert_eq!((snapshots[2].series, snapshots[2].revision), (2, 1));
    assert_ne!(requests[1]["instructions"], requests[2]["instructions"]);
    for pair in requests.windows(2) {
        assert!(pair[1]["input"]
            .as_array()
            .unwrap()
            .starts_with(pair[0]["input"].as_array().unwrap()));
    }
}

#[tokio::test]
async fn h168_unknown_persisted_host_snapshot_blocks_actual_model_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = Arc::new(Database::open(temp.path().join("state.sqlite")).unwrap());
    let settings = cold_cache_settings(db.clone(), Arc::new(MemorySecretStore::new()), true).await;
    let model = Arc::new(ObservedModel {
        replay: ReplayTransport::new([response_text("one")]),
        requests: Mutex::new(vec![]),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let id = cold_cache_run(&chat, None, "first question").await;
    let clock = deepagent_core::clock::SystemClock;
    let mut session =
        deepagent_session::Session::recover(&db, &clock, id.parse().unwrap()).unwrap();
    session
        .append_response_item(deepagent_core::response_item::ResponseItem::Message {
            role: "system".into(),
            content: "[DeepAgent runtime context snapshot]\n{\"version\":99}".into(),
        })
        .unwrap();
    let error = chat
        .run_in_session(
            "next",
            Some(&id),
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
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("unsupported or malformed runtime context snapshot"));
    assert_eq!(
        model.requests.lock().unwrap().len(),
        1,
        "the failed recovery must make zero further provider attempts"
    );
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
    assert_eq!(
        instructions[0], instructions[1],
        "same-session requests must preserve the production stable prompt prefix"
    );
    assert!(!instructions[0].contains("<<<DYNAMIC>>>"));
    assert!(latest_wire_snapshot(&requests[0])
        .context
        .contains("# Environment"));
    assert_eq!(
        latest_wire_snapshot(&requests[0]),
        latest_wire_snapshot(&requests[1])
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
    let boundary = first_instructions.len();
    let first_difference = first_instructions
        .bytes()
        .zip(second_instructions.bytes())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| first_instructions.len().min(second_instructions.len()));
    let first_input = requests[0]["input"].as_array().unwrap();
    let second_input = requests[1]["input"].as_array().unwrap();
    assert!(second_input.starts_with(first_input));
    let before_snapshot = latest_wire_snapshot(&requests[0]);
    let after_snapshot = latest_wire_snapshot(&requests[1]);
    assert!(!before_snapshot.context.contains("new_after_first_turn.rs"));
    assert!(after_snapshot.context.contains("new_after_first_turn.rs"));
    assert_eq!(after_snapshot.series, before_snapshot.series);
    assert_eq!(after_snapshot.revision, before_snapshot.revision + 1);
    assert_eq!(first_difference, boundary);
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
