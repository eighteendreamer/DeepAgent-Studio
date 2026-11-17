//! Real command hooks and native history; no callback/log-based context injection.
use super::*;
use deepagent_core::response_item::ResponseItem;
use serde_json::{json, Value};
use std::path::Path;

const FIRST: &str = "H140_FIRST_CONTEXT_6E20";
const SECOND: &str = "H140_SECOND_CONTEXT_A19F";
const HIDDEN: &str = "H140_NOT_ADMITTED_CONTEXT_432A";

fn command(output: &str, delay_ms: u64, code: i32) -> Value {
    json!({"type":"command","shell":"powershell","timeout":5,
        "command":format!("Start-Sleep -Milliseconds {delay_ms}; [Console]::Out.Write('{}'); exit {code}",output.replace('\'',"''"))})
}

fn structured(event: &str, text: &str) -> String {
    json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":text}}).to_string()
}

async fn open(
    path: &Path,
    root: &Path,
    model: Arc<RecordingModel>,
    actions: Vec<Value>,
) -> (Arc<Database>, Arc<SettingsService>, ChatService) {
    let db = Arc::new(Database::open(path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-h140-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
        .set_hooks_json(&json!({"hooks":{"UserPromptSubmit":[{"hooks":actions}]}}).to_string())
        .unwrap();
    let chat = ChatService::new(db.clone(), settings.clone(), model, root);
    (db, settings, chat)
}

async fn run(chat: &ChatService, session: Option<&str>, run: &str) -> (String, Vec<RuntimeEvent>) {
    let live = Arc::new(Mutex::new(Vec::new()));
    let capture = live.clone();
    let id = tokio::time::timeout(
        Duration::from_secs(12),
        chat.run_in_session(
            PROMPT,
            session,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            move |event| capture.lock().unwrap().push(event),
            |_| {},
        ),
    )
    .await
    .expect("finite local command hooks and offline model")
    .unwrap();
    let events = live.lock().unwrap().clone();
    (id, events)
}

fn context_items(request: &Value) -> Vec<Value> {
    request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| {
            let text = item["content"].as_str()?;
            let text = text.strip_prefix(deepagent_core::response_item::HOOK_CONTEXT_PREFIX)?;
            let value: Value = serde_json::from_str(text).ok()?;
            if value["kind"] != "hook_additional_context" {
                return None;
            }
            assert_eq!(
                item["role"], "user",
                "hook output cannot become system/developer authority"
            );
            assert_eq!(value["version"], 1);
            Some(value)
        })
        .collect()
}

fn native_contexts(events: &[deepagent_core::event::Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ResponseItemAppended {
                item: ResponseItem::Message { role, content },
                ..
            } => {
                let content =
                    content.strip_prefix(deepagent_core::response_item::HOOK_CONTEXT_PREFIX)?;
                let value: Value = serde_json::from_str(content).ok()?;
                if value["kind"] != "hook_additional_context" {
                    return None;
                }
                assert_eq!(role, "user");
                Some(value)
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn h140_ordered_contexts_survive_service_reopen_without_rewriting_or_reexecution() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let model = Arc::new(RecordingModel::default());
    let first = format!("{FIRST}\n{}", "synthetic context line\n".repeat(100));
    let second = structured("UserPromptSubmit", SECOND);
    let (session, persisted) = {
        // The first configured handler completes last. Delivery order must not
        // depend on command completion, nor on which decision won reduction.
        let (db, settings, chat) = open(
            &path,
            temp.path(),
            model.clone(),
            vec![command(&first, 200, 0), command(&second, 0, 0)],
        )
        .await;
        let (session, live) = run(&chat, None, "h140-first").await;
        assert_eq!(live.iter().filter(|event| matches!(event,RuntimeEvent::HookCompleted {event,..} if event=="UserPromptSubmit")).count(),2);
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let contexts = context_items(&requests[0]);
        assert_eq!(
            contexts.len(),
            1,
            "one source-qualified native batch must reach the wire"
        );
        let entries = contexts[0]["contexts"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0]["text"].as_str().unwrap().starts_with(FIRST));
        assert_eq!(entries[1]["text"], SECOND);
        assert_ne!(entries[0]["ordinal"], entries[1]["ordinal"]);
        assert!(entries
            .iter()
            .all(|entry| entry["hook"] == "external_command_hook"));
        let wire_items: Vec<ResponseItem> =
            serde_json::from_value(requests[0]["input"].clone()).unwrap();
        let wire = &requests[0];
        let mut priced = deepagent_models::ResponseRequest::from_response_items(
            wire["model"].as_str().unwrap(),
            wire["instructions"].as_str().map(str::to_owned),
            wire_items,
        );
        priced.stream = true;
        priced.reasoning_effort = wire["reasoning"]["effort"]
            .as_str()
            .map(|effort| deepagent_models::ReasoningEffort::parse(effort).unwrap());
        priced.tools = wire.get("tools").map_or_else(Vec::new, |tools| {
            tools
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| {
                    let mut schema = deepagent_models::ToolSchema::function(
                        tool["name"].as_str().unwrap(),
                        tool["description"].as_str().unwrap_or_default(),
                        tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
                    );
                    schema.kind = tool["type"].as_str().unwrap().into();
                    schema
                })
                .collect()
        });
        priced.max_output_tokens = wire
            .get("max_output_tokens")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap();
        priced.temperature = wire
            .get("temperature")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap();
        priced.top_p = wire
            .get("top_p")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap();
        priced.top_logprobs = wire
            .get("top_logprobs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap();
        priced.tool_choice = wire.get("tool_choice").cloned();
        priced.text = wire.get("text").cloned();
        priced.user = wire
            .get("user")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&priced).unwrap(),
            *wire,
            "pricing oracle must reconstruct the complete dispatched request"
        );
        let client = deepagent_models::ModelClient::new(
            Arc::new(MockTransport::new([])),
            deepagent_models::ModelConfig::deepseek("offline-h140-pricing"),
        );
        let expected_surface = client.count_response_prompt(&priced).unwrap().unwrap();
        priced.input.retain(|item| {
            !matches!(item, ResponseItem::Message { content, .. }
            if content.starts_with(deepagent_core::response_item::HOOK_CONTEXT_PREFIX))
        });
        assert!(
            expected_surface > client.count_response_prompt(&priced).unwrap().unwrap(),
            "admitted hook context must increase the full prompt measurement"
        );
        // The DTO keeps schemas separate from surface_tokens. Per-block costs
        // are apportioned estimates, but their sum is the full prompt count.
        let metered = live.iter().any(|event| {
            matches!(event,
            RuntimeEvent::ContextUsage { snapshot }
                if snapshot.estimated_prompt_tokens == expected_surface
                    && snapshot.surface_tokens + snapshot.tool_schema_tokens == expected_surface)
        });
        assert!(
            metered,
            "the full dispatched prompt, including admitted hook context and schemas, must be metered: expected {expected_surface}"
        );
        drop(requests);
        let events = EventStore::new(&db)
            .load_session(session.parse().unwrap())
            .unwrap();
        assert_eq!(native_contexts(&events), contexts);
        let original_inputs: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::MessageAppended { message }
                    if message.role == deepagent_core::message::Role::User =>
                {
                    Some(message.content.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            original_inputs,
            vec![PROMPT],
            "the source input must not be rewritten to carry hook text"
        );
        settings.set_hooks_json(r#"{"hooks":{}}"#).unwrap();
        (session, contexts)
    };
    // Every original ChatService/Settings/Database handle has been dropped.
    let (db, _settings, chat) = open(&path, temp.path(), model.clone(), Vec::new()).await;
    let (_, live) = run(&chat, Some(&session), "h140-after-reopen").await;
    assert!(!live.iter().any(
        |event| matches!(event,RuntimeEvent::HookCompleted {event,..} if event=="UserPromptSubmit")
    ));
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        context_items(&requests[1]),
        persisted,
        "native replay must not duplicate or forget context"
    );
    let events = EventStore::new(&db)
        .load_session(session.parse().unwrap())
        .unwrap();
    assert_eq!(native_contexts(&events), persisted);
}

#[tokio::test]
async fn h140_wrong_event_unknown_json_nonzero_and_async_outputs_are_not_context() {
    for (index,output,code) in [
        structured("SessionStart",HIDDEN),
        json!({"debug":HIDDEN}).to_string(),
        format!("{{malformed-json-{HIDDEN}"),
        json!({"async":true,"asyncTimeout":5,"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":HIDDEN}}).to_string(),
        HIDDEN.to_string(),
    ].into_iter().enumerate().map(|(index,output)| (index,output,if index==4{7}else{0})) {
        let temp=tempfile::tempdir().unwrap();
        let model=Arc::new(RecordingModel::default());
        let (db,_settings,chat)=open(&temp.path().join("state.sqlite"),temp.path(),model.clone(),vec![command(&output,0,code)]).await;
        let (session,_)=run(&chat,None,&format!("h140-negative-{index}")).await;
        let requests=model.0.lock().unwrap();
        assert_eq!(requests.len(),1);
        assert!(!requests[0].to_string().contains(HIDDEN),"unadmitted output: case {index}");
        assert!(context_items(&requests[0]).is_empty());
        let events=EventStore::new(&db).load_session(session.parse().unwrap()).unwrap();
        assert!(native_contexts(&events).is_empty());
    }
}

#[tokio::test]
async fn h140_block_or_ask_cannot_commit_another_handlers_context_or_call_the_model() {
    for decision in [
        json!({"decision":"block","reason":"synthetic block"}),
        json!({"permissionDecision":"ask","permissionDecisionReason":"synthetic ask"}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let model = Arc::new(RecordingModel::default());
        let (db, _settings, chat) = open(
            &temp.path().join("state.sqlite"),
            temp.path(),
            model.clone(),
            vec![
                command(HIDDEN, 0, 0),
                command(&decision.to_string(), 100, 0),
            ],
        )
        .await;
        let (session, _) = run(&chat, None, "h140-veto").await;
        assert!(model.0.lock().unwrap().is_empty());
        let events = EventStore::new(&db)
            .load_session(session.parse().unwrap())
            .unwrap();
        assert!(native_contexts(&events).is_empty());
        assert!(!events.iter().any(|event| match &event.payload {
            EventPayload::MessageAppended { message } => message.content.contains(HIDDEN),
            EventPayload::ResponseItemAppended { item, .. } =>
                serde_json::to_string(item).unwrap().contains(HIDDEN),
            _ => false,
        }));
    }
}

#[tokio::test]
async fn h140_long_unicode_context_is_bounded_and_marks_truncation() {
    let temp = tempfile::tempdir().unwrap();
    let model = Arc::new(RecordingModel::default());
    let text = format!("{FIRST}{}H140_OMITTED_TAIL", "界🦀".repeat(10_000));
    // Avoid shell command-line limits: the owned fixture file is read by the
    // real hook. This is not injection from a test callback.
    let file = temp.path().join("output.txt");
    std::fs::write(&file, &text).unwrap();
    let action = json!({"type":"command","shell":"powershell","timeout":5,
        "command":format!("[Console]::Out.Write([IO.File]::ReadAllText('{}'))",file.to_string_lossy().replace('\'',"''"))});
    let (_db, _settings, chat) = open(
        &temp.path().join("state.sqlite"),
        temp.path(),
        model.clone(),
        vec![action],
    )
    .await;
    run(&chat, None, "h140-long").await;
    let requests = model.0.lock().unwrap();
    let batch = context_items(&requests[0]);
    assert_eq!(batch.len(), 1);
    let context = &batch[0]["contexts"][0];
    let delivered = context["text"].as_str().unwrap();
    assert!(delivered.starts_with(FIRST));
    assert!(
        delivered.len() <= 16 * 1024,
        "per-handler handoff bound is UTF-8 bytes"
    );
    assert_eq!(context["truncated"], true);
    assert!(!requests[0].to_string().contains("H140_OMITTED_TAIL"));
}
