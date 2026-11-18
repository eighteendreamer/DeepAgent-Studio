//! Registry selections must reach the real admission -> bind -> model wire.
use async_trait::async_trait;
use deepagent_app_core::{
    ChatService, HarnessRunOverrides, MemorySecretStore, SecretStore, SettingsService,
};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider};
use deepagent_persistence::Database;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[path = "../../deepagent-models/tests/support/sequence.rs"]
pub mod sequence;

fn emit_responses_events(sink: &mut dyn EventSink, events: &[String]) -> Result<()> {
    for event in sequence::numbered(events.iter().cloned()) {
        if sink.on_event(&event)? {
            break;
        }
    }
    Ok(())
}

fn response_text(text: &str) -> Vec<String> {
    vec![
        serde_json::json!({"type":"response.output_text.delta","delta":text}).to_string(),
        serde_json::json!({"type":"response.completed","response":{
            "id":"registry-response","object":"response","created_at":1,"model":MODEL,
            "output":[],"status":"completed"}})
        .to_string(),
    ]
}

fn response_function_call(call_id: &str, name: &str, arguments: serde_json::Value) -> Vec<String> {
    let item = serde_json::json!({"type":"function_call","id":"registry-tool-item",
        "call_id":call_id,"name":name,"arguments":arguments.to_string()});
    vec![
        serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
        serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
        serde_json::json!({"type":"response.completed","response":{
            "id":"registry-response","object":"response","created_at":1,"model":MODEL,
            "output":[item],"status":"completed"}})
        .to_string(),
    ]
}

const INPUT: &str = "Read fixture.txt and acknowledge it.";
const MODEL: &str = "registry-model";

async fn slash(f: &Fixture, prompt: &str) -> Result<String> {
    let output = Arc::new(Mutex::new(String::new()));
    let captured = output.clone();
    f.chat
        .run_in_session(
            prompt,
            None,
            None,
            None,
            vec![],
            None,
            false,
            None,
            move |event| {
                if let deepagent_runtime::RuntimeEvent::RunCompleted { message, .. } = event {
                    *captured.lock().unwrap() = message;
                }
            },
            |_| {},
        )
        .await?;
    let message = output.lock().unwrap().clone();
    Ok(message)
}

#[tokio::test]
async fn slash_model_uses_activated_registry_and_routes_the_next_turn() {
    let f = Fixture::new("responses", None).await;
    let mut inactive = model(None);
    inactive.model_name = "inactive-model".into();
    inactive.active = false;
    f.db.save_harness_models("alpha", &[inactive]).unwrap();
    let listing = slash(&f, "/model").await.unwrap();
    assert!(listing.contains("Alpha"));
    assert!(listing.contains(MODEL));
    assert!(!listing.contains("inactive-model"));
    let receipt = slash(&f, &format!("/model {MODEL}")).await.unwrap();
    assert!(receipt.contains("Alpha"));
    assert_eq!(
        f.db.harness_chat_selection().unwrap(),
        Some(("alpha".into(), MODEL.into()))
    );
    assert!(
        f.wire.requests.lock().unwrap().is_empty(),
        "slash never calls inference"
    );
    f.run("after-slash-model", HarnessRunOverrides::default())
        .await
        .unwrap();
    let requests = f.wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "https://alpha.test/v1/responses",
        "synthetic-alpha-key",
        MODEL,
    );
}

#[tokio::test]
async fn slash_model_rejects_inactive_and_ambiguous_names_without_changing_selection() {
    let f = Fixture::new("responses", None).await;
    let mut other = provider("responses");
    other.id = "beta".into();
    other.channel = "Beta".into();
    f.db.save_harness_provider(&other).unwrap();
    let mut duplicate = model(None);
    duplicate.provider_id = "beta".into();
    f.db.save_harness_models("beta", &[duplicate]).unwrap();
    let before = f.db.harness_chat_selection().unwrap();
    assert!(slash(&f, &format!("/model {MODEL}"))
        .await
        .unwrap_err()
        .to_string()
        .contains("multiple providers"));
    assert_eq!(f.db.harness_chat_selection().unwrap(), before);
    slash(&f, &format!("/model beta/{MODEL}")).await.unwrap();
    f.db.set_harness_model_active("alpha", MODEL, false)
        .unwrap();
    assert!(slash(&f, &format!("/model alpha/{MODEL}"))
        .await
        .unwrap_err()
        .to_string()
        .contains("not activated"));
    assert_eq!(
        f.db.harness_chat_selection().unwrap(),
        Some(("beta".into(), MODEL.into()))
    );
    assert!(f.wire.requests.lock().unwrap().is_empty());
}

fn selection(provider: &str, model: &str) -> HarnessRunOverrides {
    HarnessRunOverrides {
        provider: Some(provider.into()),
        model: Some(model.into()),
        permission_profile: Some("read_only".into()),
        sandbox_backend: Some("direct".into()),
        ..Default::default()
    }
}

fn provider(protocol: &str) -> HarnessProvider {
    HarnessProvider {
        id: "alpha".into(),
        channel: "Alpha".into(),
        logo: String::new(),
        website_url: String::new(),
        model_url: "https://alpha.test/v1".into(),
        protocol: protocol.into(),
        api_key_ref: "registry-alpha-key".into(),
        balance_kind: "none".into(),
        enabled: true,
        revision: 0,
    }
}

fn model(protocol: Option<&str>) -> HarnessModel {
    HarnessModel {
        provider_id: "alpha".into(),
        model_name: MODEL.into(),
        display_name: MODEL.into(),
        protocol: protocol.map(str::to_owned),
        context_window: 64000,
        output_limit: 4096,
        capabilities: r#"{"input_modalities":["text"],"supports_tools":true}"#.into(),
        active: true,
    }
}

#[derive(Default)]
struct Wire {
    requests: Mutex<Vec<TransportRequest>>,
    pause: AtomicBool,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl HttpTransport for Wire {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let chat = request.url.ends_with("/chat/completions");
        self.requests.lock().unwrap().push(request);
        if self.pause.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(15), self.release.notified())
                .await
                .map_err(|_| CoreError::other("registry fixture timed out"))?;
            return emit_responses_events(
                sink,
                &response_function_call(
                    "registry-read",
                    "read_file",
                    serde_json::json!({"path":"fixture.txt"}),
                ),
            );
        }
        if chat {
            sink.on_event(r#"{"id":"registry-response","choices":[{"index":0,"delta":{"content":"Acknowledged."},"finish_reason":null}]}"#)?;
            sink.on_event(r#"{"id":"registry-response","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#)?;
            sink.on_event("[DONE]")?;
            Ok(())
        } else {
            emit_responses_events(sink, &response_text("Acknowledged."))
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    db: Arc<Database>,
    secrets: Arc<MemorySecretStore>,
    settings: Arc<SettingsService>,
    wire: Arc<Wire>,
    chat: Arc<ChatService>,
    contexts: Arc<Mutex<Vec<(usize, usize)>>>,
}

impl Fixture {
    async fn new(protocol: &str, override_protocol: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fixture.txt"), "registry fixture").unwrap();
        let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash"}]}"#,
            )),
            secrets.clone(),
        ));
        settings.initialize("synthetic-legacy-key").await.unwrap();
        secrets
            .set("registry-alpha-key", "synthetic-alpha-key")
            .unwrap();
        db.save_harness_provider(&provider(protocol)).unwrap();
        db.save_harness_models("alpha", &[model(override_protocol)])
            .unwrap();
        let wire = Arc::new(Wire::default());
        let chat = Arc::new(ChatService::new(
            db.clone(),
            settings.clone(),
            wire.clone(),
            root.path(),
        ));
        Self {
            root,
            db,
            secrets,
            settings,
            wire,
            chat,
            contexts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn run(&self, id: &str, selected: HarnessRunOverrides) -> Result<()> {
        let contexts = self.contexts.clone();
        self.chat
            .run_in_session_with_overrides(
                INPUT,
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(id),
                selected,
                move |event| {
                    if let deepagent_runtime::RuntimeEvent::ContextUsage { snapshot } = event {
                        contexts
                            .lock()
                            .unwrap()
                            .push((snapshot.context_window, snapshot.reserved_output_tokens));
                    }
                },
                |_| {},
            )
            .await
            .map(|_| ())
    }
}

fn assert_request(request: &TransportRequest, url: &str, expected_key: &str, expected_model: &str) {
    assert_eq!(request.url, url);
    assert!(
        request.api_key == expected_key,
        "wire must bind only the selected provider credential"
    );
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["model"], expected_model);
    let output = body
        .get("max_output_tokens")
        .or_else(|| body.get("max_tokens"));
    assert_eq!(output, Some(&serde_json::json!(4096)));
}

#[tokio::test]
async fn registry_route_drives_provider_protocol_override_capacities_and_key() {
    for (protocol, override_protocol, suffix) in [
        ("responses", None, "responses"),
        ("responses", Some("chat_completions"), "chat/completions"),
        ("chat_completions", None, "chat/completions"),
    ] {
        let f = Fixture::new(protocol, override_protocol).await;
        let config = f
            .chat
            .read_execution_config(&selection("alpha", MODEL))
            .unwrap();
        assert_eq!(config["provider"], "alpha");
        assert_eq!(config["model"], MODEL);
        f.run("registry-route", selection("alpha", MODEL))
            .await
            .unwrap();
        let requests = f.wire.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_request(
            &requests[0],
            &format!("https://alpha.test/v1/{suffix}"),
            "synthetic-alpha-key",
            MODEL,
        );
        let events = f.chat.run_events("registry-route", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        let public = serde_json::to_string(&events).unwrap();
        assert!(!public.contains("synthetic-alpha-key"));
        assert!(!public.contains("registry-alpha-key"));
        let run = deepagent_persistence::run_store::RunStore::new(&f.db)
            .get("registry-route")
            .unwrap()
            .unwrap();
        let history = deepagent_persistence::event_store::EventStore::new(&f.db)
            .load_session(run.session_id.parse().unwrap())
            .unwrap();
        let answer = history
            .iter()
            .find(|event| {
                let payload = serde_json::to_value(&event.payload).unwrap();
                payload["item"]["role"] == "assistant"
                    && payload["type"] == "response_item_appended"
            })
            .expect("real model answer must be persisted");
        assert_eq!(
            serde_json::to_value(&answer.payload).unwrap()["origin"],
            serde_json::json!({"provider_id":"alpha","model_name":MODEL,
                "protocol":override_protocol.unwrap_or(protocol)})
        );
        assert_eq!(f.contexts.lock().unwrap().last(), Some(&(64000, 4096)));
    }
}

#[tokio::test]
async fn registry_auxiliary_consumers_share_the_selected_provider_route() {
    let f = Fixture::new("responses", None).await;
    let mut default_provider = provider("chat_completions");
    default_provider.id = "deepseek".into();
    f.db.save_harness_provider(&default_provider).unwrap();
    let session = f.chat.create_session(None).unwrap();
    let prepared = f
        .chat
        .admit_prepared_input(
            "registry-auxiliary",
            &session,
            INPUT,
            HarnessRunOverrides::default(),
        )
        .unwrap();
    f.chat
        .run_prepared(prepared, INPUT, |_| {}, |_| {})
        .await
        .unwrap();
    f.chat
        .run_review_streaming(
            "Classify briefly.",
            "Review this synthetic example.",
            deepagent_models::ReasoningEffort::None,
            64,
            |_| {},
        )
        .await
        .unwrap();
    assert!(f
        .chat
        .maybe_generate_session_title_for_run(&session, "registry-auxiliary")
        .await
        .unwrap()
        .is_some());
    let requests = f.wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request.url, "https://alpha.test/v1/chat/completions");
        assert!(
            request.api_key == "synthetic-alpha-key",
            "auxiliary calls must use the selected credential"
        );
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], "deepseek-flash");
    }
}

#[tokio::test]
async fn registry_inactive_legacy_model_and_disabled_provider_fail_without_wire_calls() {
    for disabled_provider in [false, true] {
        let f = Fixture::new("responses", None).await;
        if disabled_provider {
            let mut row = provider("responses");
            row.enabled = false;
            f.db.save_harness_provider(&row).unwrap();
        } else {
            f.db.set_harness_model_active("deepseek", "deepseek-flash", false)
                .unwrap();
        }
        let selected = if disabled_provider {
            selection("alpha", MODEL)
        } else {
            HarnessRunOverrides::default()
        };
        let error = f.run("registry-disabled", selected).await.unwrap_err();
        assert!(
            error.to_string().contains(if disabled_provider {
                "is disabled"
            } else {
                "not activated"
            }),
            "{error}"
        );
        assert!(f.wire.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn registry_missing_key_cannot_use_the_onboarding_key() {
    let f = Fixture::new("responses", None).await;
    f.secrets.delete("registry-alpha-key").unwrap();
    let error = f
        .run("registry-missing-key", selection("alpha", MODEL))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("has no API key"), "{error}");
    assert!(f.wire.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn registry_legacy_key_retains_the_failure_closed_credential_lifecycle() {
    let f = Fixture::new("responses", None).await;
    f.db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_settings BEFORE UPDATE ON documents
         WHEN NEW.collection = 'settings' AND NEW.id = 'app'
         BEGIN SELECT RAISE(ABORT, 'synthetic settings failure'); END;",
        )
        .map_err(|error| CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    assert!(f
        .settings
        .initialize("synthetic-incomplete-key")
        .await
        .is_err());
    let error = f
        .run("registry-credential", HarnessRunOverrides::default())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("credential update incomplete"),
        "{error}"
    );
    assert!(f.wire.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn registry_admission_freezes_route_and_binds_its_original_key_reference() {
    let f = Fixture::new("responses", None).await;
    let session = f.chat.create_session(None).unwrap();
    let prepared = f
        .chat
        .admit_prepared_input(
            "registry-frozen",
            &session,
            INPUT,
            selection("alpha", MODEL),
        )
        .unwrap();
    let snapshot = prepared.capability_snapshot().clone();
    assert_eq!(snapshot["context"]["executionConfig"]["provider"], "alpha");
    let mut changed = provider("chat_completions");
    changed.model_url = "https://changed.test/v2".into();
    changed.api_key_ref = "changed-alpha-key".into();
    f.secrets
        .set("changed-alpha-key", "synthetic-changed-key")
        .unwrap();
    f.db.save_harness_provider(&changed).unwrap();
    f.chat
        .run_prepared(prepared, INPUT, |_| {}, |_| {})
        .await
        .unwrap();
    assert_request(
        &f.wire.requests.lock().unwrap()[0],
        "https://alpha.test/v1/responses",
        "synthetic-alpha-key",
        MODEL,
    );
    assert_eq!(
        f.chat
            .run_events("registry-frozen", None)
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "capability.snapshot")
            .unwrap()
            .data,
        snapshot
    );
}

#[tokio::test]
async fn registry_changes_during_tool_round_trip_apply_only_to_next_user_turn() {
    let f = Fixture::new("responses", None).await;
    f.wire.pause.store(true, Ordering::SeqCst);
    let session = f.chat.create_session(None).unwrap();
    let prepared = f
        .chat
        .admit_prepared_input("registry-tools", &session, INPUT, selection("alpha", MODEL))
        .unwrap();
    let chat = f.chat.clone();
    let pending =
        tokio::spawn(async move { chat.run_prepared(prepared, INPUT, |_| {}, |_| {}).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        f.wire.started.notified(),
    )
    .await
    .unwrap();
    let mut changed = provider("chat_completions");
    changed.model_url = "https://changed.test/v2".into();
    f.db.save_harness_provider(&changed).unwrap();
    f.secrets
        .set("registry-alpha-key", "synthetic-rotated-key")
        .unwrap();
    let mut next_model = model(None);
    next_model.model_name = "registry-next-model".into();
    f.db.save_harness_models("alpha", &[next_model]).unwrap();
    f.wire.release.notify_one();
    pending.await.unwrap().unwrap();
    let next = f
        .chat
        .admit_prepared_input(
            "registry-next",
            &session,
            "Continue.",
            selection("alpha", "registry-next-model"),
        )
        .unwrap();
    f.chat
        .run_prepared(next, "Continue.", |_| {}, |_| {})
        .await
        .unwrap();
    let requests = f.wire.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "one tool call, one continuation, one next user turn"
    );
    for request in &requests[..2] {
        assert_request(
            request,
            "https://alpha.test/v1/responses",
            "synthetic-alpha-key",
            MODEL,
        );
    }
    assert_request(
        &requests[2],
        "https://changed.test/v2/chat/completions",
        "synthetic-rotated-key",
        "registry-next-model",
    );
    let second: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert!(second["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "function_call_output" && item["call_id"] == "registry-read"));
    let cold = Database::open(&f.root.path().join("state.sqlite")).unwrap();
    let history = deepagent_persistence::event_store::EventStore::new(&cold)
        .load_session(session.parse().unwrap())
        .unwrap();
    let produced: Vec<_> = history
        .iter()
        .filter_map(|event| {
            let payload = serde_json::to_value(&event.payload).unwrap();
            (payload["type"] == "response_item_appended" && !payload["origin"].is_null())
                .then(|| (payload["item"]["type"].clone(), payload["origin"].clone()))
        })
        .collect();
    assert_eq!(
        produced.len(),
        3,
        "one tool call and two answers retain their own origin"
    );
    assert_eq!(produced[0].0, "function_call");
    for (_, origin) in &produced[..2] {
        assert_eq!(
            *origin,
            serde_json::json!({
            "provider_id":"alpha","model_name":MODEL,"protocol":"responses"})
        );
    }
    assert_eq!(
        produced[2].1,
        serde_json::json!({"provider_id":"alpha",
        "model_name":"registry-next-model","protocol":"chat_completions"})
    );
    assert!(f.root.path().join("fixture.txt").exists());
}
